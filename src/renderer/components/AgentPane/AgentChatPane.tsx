import { useState, useEffect, useRef, useCallback, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { terminalExists } from '@/lib/terminal-daemon'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { TerminalPane } from '@/terminal-v2/TerminalPane'
import { agentChatId } from '@/lib/terminal-id'
import { getDaemonWs, daemonHttpBase } from '@/kessel/daemon-ws'
import { daemonCliGet } from '@/lib/daemon-cli'
import { agentDisplayName, resumeChatArgs } from '@/lib/workspace-agent'
import { useActiveAgentsStore } from '@/stores/active-agents'

interface AgentChatPaneProps {
  agentName: string
  projectPath: string
  /** 0.37.12 — Claude session id restored from the serialized layout.
   *  When present, AgentChatPane skips the
   *  `k2so_agents_resume_chat_args` daemon roundtrip and builds the
   *  launch config directly so the same session resumes immediately.
   *  Renderer holds the canonical record; the daemon's auto-stamp
   *  hook reconciles DB state on the next spawn. See
   *  `.k2so/prds/canonical-lane-restore.md`. */
  restoredSessionId?: string
}

/**
 * Chat pinned tab — runs the workspace agent's persistent Claude session.
 *
 * Replaces the "Chat" sub-tab from the pre-0.36.0 single AgentPane.
 * Sibling tab is `AgentInboxPane`; both are pinned by `tabs.ts`.
 *
 * Terminal id is project-namespaced (`agent-chat:<project_id>:<agent>`)
 * so two workspaces sharing an agent name don't collide on a single
 * PTY — see `.k2so/prds/heartbeats-sidebar-audit.md` Phase 1.
 */
export function AgentChatPane({ agentName, projectPath, restoredSessionId }: AgentChatPaneProps): React.JSX.Element {
  // Resolve project id synchronously from the projects store; the chat tab
  // will not render until a real id is available so the legacy collision
  // bug can never reappear via this surface.
  const projectId = useProjectsStore((s) => {
    return s.projects.find((p) => p.path === projectPath)?.id ?? null
  })

  if (!projectId) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        Loading workspace…
      </div>
    )
  }

  // `key={projectId}` forces a clean remount when the workspace
  // switches. Without it, React reuses the same AgentChatTerminal
  // instance and `terminalIdRef` (initialized from
  // `useRef(agentChatId(projectId, agentName))`) keeps the stale
  // workspace's terminal id — defense-in-depth against the
  // cross-workspace pinned-chat collision fixed in 0.36.14.
  //
  // 0.37.5: project_id alone is the canonical workspace identity
  // (post-unification, one agent per workspace, agent name is
  // metadata not address). The agent name doesn't need to be in
  // the React key.
  return (
    <AgentChatTerminal
      key={projectId}
      agentName={agentName}
      projectId={projectId}
      projectPath={projectPath}
      restoredSessionId={restoredSessionId}
    />
  )
}

interface AgentChatTerminalProps {
  agentName: string
  projectId: string
  projectPath: string
  restoredSessionId?: string
}

function AgentChatTerminal({ agentName, projectId, projectPath, restoredSessionId }: AgentChatTerminalProps): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalIdRef = useRef(agentChatId(projectId, agentName))

  // P1.A — bind this pinned-Chat pane to ITS OWN project UPFRONT, before
  // any terminal-title braille tick can race. The pinned Chat has no agent
  // lifecycle hook, so the title-activity path is the only binder; without
  // this pre-registration its working state could latch onto whatever
  // workspace the user was viewing when the first tick fired (mis-bound
  // spinner). `bindPaneProject` is idempotent and never clobbers a
  // lifecycle-bound entry. Keyed on the canonical terminalId so it matches
  // the paneId carried by the title/lifecycle signals.
  useEffect(() => {
    useActiveAgentsStore.getState().bindPaneProject(terminalIdRef.current, projectId)
  }, [projectId])
  const [launchConfig, setLaunchConfig] = useState<{
    command: string
    args: string[]
    cwd: string
  } | null>(null)
  const [ready, setReady] = useState(false)
  // 0.37.4: friendly label from AGENT.md `display_name:` (falls back
  // to the technical agent name on first paint, then upgrades).
  const [displayName, setDisplayName] = useState<string>(agentName)

  // 0.37.12 — chat-history dropdown state. Lets the user switch the
  // pinned chat to a different past session (escape hatch when a
  // chat was deleted, when restoration landed on the wrong session,
  // or just to revisit an earlier conversation). See
  // `.k2so/prds/canonical-lane-restore.md`.
  const [historySessions, setHistorySessions] = useState<Array<{
    sessionId: string
    title: string
    timestamp: number
    messageCount: number
  }>>([])
  const [historyOpen, setHistoryOpen] = useState(false)
  // The Claude session id currently driving the live PTY — derived
  // from launchConfig.args (`--resume <X>` or `--session-id <X>`).
  // Used to highlight the matching dropdown entry and look up the
  // display title.
  const currentSessionId = useMemo<string | null>(() => {
    const args = launchConfig?.args
    if (!args) return null
    for (let i = 0; i + 1 < args.length; i++) {
      if (args[i] === '--resume' || args[i] === '--session-id') {
        return args[i + 1]
      }
    }
    return null
  }, [launchConfig])

  useEffect(() => {
    let cancelled = false
    agentDisplayName(projectPath)
      .then((n) => { if (!cancelled && n) setDisplayName(n) })
      .catch(() => { /* keep agentName as fallback */ })
    return () => { cancelled = true }
  }, [projectPath, agentName])

  useEffect(() => {
    let unlisten: (() => void) | null = null
    let cancelled = false
    listen('sync:projects', () => {
      agentDisplayName(projectPath)
        .then((n) => { if (n) setDisplayName(n) })
        .catch(() => {})
    }).then((u) => { if (cancelled) u(); else unlisten = u })
    return () => { cancelled = true; unlisten?.() }
  }, [projectPath])
  // Bumped on every refresh-button click to force a clean remount of
  // TerminalPane (key={refreshNonce}) and a re-run of the resolve
  // effect. Used when the user typed `exit` and the Claude process
  // ended — without a remount the dead PTY stays on screen.
  //
  // 0.38.0 commit 7 — Both the originating window AND every other
  // viewer bump this via the `chat:refreshed` Tauri broadcast
  // emitted by `k2so_chat_refresh_broadcast`. Keeps the pinned chat
  // tab in sync after one window kills the daemon PTY.
  const [refreshNonce, setRefreshNonce] = useState(0)
  const [refreshing, setRefreshing] = useState(false)

  // Listen for chat:refreshed broadcasts. Every window's
  // AgentChatPane mounts this listener; the payload's `projectPath`
  // filters to the workspace the pane is rendering. Idempotent —
  // a remount via refreshNonce++ is safe to repeat.
  useEffect(() => {
    let cancelled = false
    let unlisten: (() => void) | null = null
    listen<{ projectPath: string }>('chat:refreshed', (event) => {
      if (event.payload.projectPath !== projectPath) return
      setLaunchConfig(null)
      setReady(false)
      setRefreshNonce((n) => n + 1)
    }).then((u) => { if (cancelled) u(); else unlisten = u })
    return () => { cancelled = true; unlisten?.() }
  }, [projectPath])

  const handleRefresh = useCallback(async (): Promise<void> => {
    if (refreshing) return
    setRefreshing(true)
    // Kill the daemon-owned PTY (best-effort). The unregister hook in
    // v2_session_map clears agent_sessions.active_terminal_id and the
    // child-exit observer fires for any still-alive process. If the
    // session was already dead (user typed `exit`), the daemon's
    // find-or-spawn on the next mount just spawns fresh.
    //
    // 0.37.5: pass the canonical workspace key — bare project_id —
    // so we close THIS workspace's session. Pre-0.37.5 the key was
    // `<projectId>:<agentName>`; the suffix was vestigial
    // post-unification (one agent per workspace) and caused the
    // renderer to compute the wrong key when its mode→name mapping
    // disagreed with AGENT.md's `name:` field (C3PO 5c80bef1).
    try {
      const creds = await getDaemonWs()
      await fetch(
        `${daemonHttpBase(creds)}/cli/sessions/v2/close?token=${creds.token}`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ agent_name: projectId }),
        },
      ).catch(() => {})
    } catch { /* ignore — refresh proceeds either way */ }

    // 0.38.0 commit 7 — broadcast cross-window. The `chat:refreshed`
    // listener below also fires in THIS window and bumps the same
    // refreshNonce. That double-bump is harmless (idempotent
    // remount); routing both paths through the listener keeps the
    // originating window and every other viewer on identical state
    // machines.
    invoke('k2so_chat_refresh_broadcast', { projectPath })
      .catch((e) => console.warn('[chat-refresh] broadcast failed:', e))

    setLaunchConfig(null)
    setReady(false)
    setRefreshing(false)
  }, [projectId, projectPath, agentName, refreshing])

  // 0.37.12 — fetch chat history for the dropdown title display
  // and the popover list. Runs on mount, when the current session
  // changes (title converges after the user sends the first
  // message — claude assigns titles dynamically based on
  // conversation content), and when the popover opens (cheap
  // re-fetch to catch any new sessions created via heartbeat fires
  // or other paths).
  //
  // Phase 2.5 fix (finding #548): switched from the deleted Tauri
  // `chat_history_list_for_project` command to the daemon's
  // `/cli/chat/list` route. The Tauri shim was retired in Phase 2
  // Unit 6 (`src-tauri/src/commands/mod.rs`); this pane was the
  // last caller still routing through `invoke()` and was logging
  // "Command chat_history_list_for_project not found" on every
  // mount. Sibling pickers (ChatHistory, ReviewPanel) already
  // call `daemonCliGet('chat/list', ...)` — this matches.
  useEffect(() => {
    let cancelled = false
    void daemonCliGet<Array<{
      sessionId: string
      title: string
      timestamp: number
      messageCount: number
      provider: string
    }>>('chat/list', { project_path: projectPath })
      .then((rows) => {
        if (cancelled) return
        // Sort by recency desc; show claude sessions only — other
        // providers (codex, gemini, pi) have their own tabs/lanes.
        const claudeOnly = rows
          .filter((r) => r.provider === 'claude' || !r.provider)
          .sort((a, b) => b.timestamp - a.timestamp)
        setHistorySessions(claudeOnly)
      })
      .catch((err) => {
        console.warn('[AgentChatPane] chat/list failed:', err)
      })
    return () => { cancelled = true }
  }, [projectPath, currentSessionId, historyOpen])

  // Title display for the dropdown trigger: matched against historySessions.
  // Falls back to a placeholder when the current session has no entry
  // yet (brand-new chat — claude hasn't written its first turn).
  const currentChatTitle = useMemo<string>(() => {
    if (!currentSessionId) return 'New chat'
    const found = historySessions.find((s) => s.sessionId === currentSessionId)
    if (found?.title) return found.title
    return 'New chat'
  }, [historySessions, currentSessionId])

  // Switch the pinned chat tab to a different past session. Updates
  // `workspace_sessions.session_id` via the daemon, stamps the new
  // id on the local AgentItemData (so the next serialize captures
  // it), then triggers a refresh so the live PTY swaps too.
  const switchToSession = useCallback(async (newSessionId: string): Promise<void> => {
    if (!newSessionId || newSessionId === currentSessionId) {
      setHistoryOpen(false)
      return
    }
    setHistoryOpen(false)
    try {
      // HOST-AWARE: write the pinned session to the ACTIVE host's
      // workspace_sessions DB (local or remote) via the daemon, not the
      // local Tauri command — `invoke('workspace_session_set_session_id')`
      // only ever hit the LOCAL daemon, so on a remote the dropdown silently
      // failed to switch (the project path isn't registered locally). The
      // `/cli/workspace/set-chat-session` route reads QUERY params and isn't
      // POST-allowlisted, so it's a GET (same pattern as
      // `workspace/set-agent-display-name`).
      await daemonCliGet('workspace/set-chat-session', {
        project: projectPath,
        session_id: newSessionId,
      })
    } catch (err) {
      console.error('[AgentChatPane] switchToSession DB update failed:', err)
      return
    }
    try {
      useTabsStore.getState().stampAgentSessionId(agentName, projectPath, newSessionId, projectId)
    } catch (err) {
      console.warn('[AgentChatPane] stampAgentSessionId failed:', err)
    }
    // Trigger the same kill-and-remount path the refresh button uses
    // so the live PTY drops and AgentChatPane re-resolves with the
    // new restoredSessionId (passed in as a prop from the layout
    // store, which we just updated via stampAgentSessionId).
    void handleRefresh()
  }, [currentSessionId, projectPath, agentName, handleRefresh])

  useEffect(() => {
    let cancelled = false
    const stampSessionId = (sid: string | null | undefined): void => {
      if (!sid) return
      try {
        useTabsStore.getState().stampAgentSessionId(agentName, projectPath, sid, projectId)
      } catch (err) {
        console.warn('[AgentChatPane] stampAgentSessionId failed:', err)
      }
    }
    const resolve = async (): Promise<void> => {
      const myTerminalId = terminalIdRef.current

      // 0.37.12 — Step 0: if the serialized layout restored a
      // sessionId hint, use it directly. Skips the
      // k2so_agents_resume_chat_args daemon roundtrip that can race
      // or land on the wrong workspace_sessions row post-crash.
      // The TerminalPane spawn that follows registers under the
      // canonical project_id agent_name; the auto-stamp hook then
      // updates workspace_sessions to match. Renderer is canonical.
      if (restoredSessionId && !cancelled) {
        // `claude --resume <X>` reloads an existing conversation;
        // matches what k2so_agents_resume_chat_args would have
        // returned for a workspace whose chat session has fired
        // before. Skipping permissions matches workspace agent
        // defaults across the rest of K2SO.
        setLaunchConfig({
          command: 'claude',
          args: ['--dangerously-skip-permissions', '--resume', restoredSessionId],
          cwd: projectPath,
        })
        daemonCliGet('agents/lock', {
          project: projectPath,
          agent: agentName,
          terminal_id: myTerminalId,
          owner: 'user',
        }).catch(() => {})
        stampSessionId(restoredSessionId)
        setReady(true)
        return
      }

      // Step 1: Reattach if PTY already alive in this Tauri session
      try {
        const exists = await terminalExists(myTerminalId)
        if (!cancelled && exists) {
          setLaunchConfig(null)
          setReady(true)
          return
        }
      } catch { /* fall through */ }

      // Step 1b: Check the daemon for an existing session under this
      // workspace's canonical key. When the user has closed Tauri,
      // the daemon can keep the workspace agent's PTY alive
      // (heartbeat fires, k2so msg injections, etc.). On Tauri
      // reopen we want to attach to that existing PTY rather than
      // spawn a fresh `claude --resume`.
      //
      // 0.37.5: lookup on the bare project_id canonical key.
      // Pre-0.37.5 the key was `<projectId>:<agentName>`; that
      // suffix was vestigial post-unification and caused renderer-
      // side mismatches (C3PO 5c80bef1).
      try {
        const json = await invoke<string>('k2so_session_lookup_by_agent', {
          agent: projectId,
        })
        const data = JSON.parse(json) as {
          sessionAlive?: boolean
          sessionId?: string | null
          isV2?: boolean
        }
        if (!cancelled && data.sessionAlive) {
          console.info(
            '[AgentChatPane] daemon has live session for',
            projectId,
            'session:',
            data.sessionId,
            'isV2:',
            data.isV2,
          )
        }
      } catch { /* informational only — fall through */ }

      // Step 2: Build a *bare resume* command for the chat tab.
      //
      // We deliberately do NOT use `k2so_agents_build_launch` here.
      // build_launch is the wake-with-full-context path: it injects the
      // agent's WAKEUP.md as the positional first user message and
      // sometimes prefixes `/compact`. That's correct for an explicit
      // "Launch agent" click or a scheduled heartbeat fire — the agent
      // is supposed to wake up and triage. It is NOT correct for the
      // Chat tab re-mounting on app relaunch (the daemon's PTY dies on
      // K2SO upgrade → tab re-mounts → was firing a fresh wake every
      // time, surprising users by auto-triaging without their consent).
      //
      // `k2so_agents_resume_chat_args` returns just
      // `claude --resume <saved-session-id>` (or fresh `claude` if no
      // saved session) — no system prompt, no WAKEUP body, no
      // `/compact`.
      try {
        // agentName is unused by the route (keyed purely on projectPath,
        // matching the old proxy command which ignored it).
        const result = await resumeChatArgs(projectPath)
        if (!cancelled && result) {
          setLaunchConfig({
            command: result.command,
            args: result.args,
            cwd: result.cwd,
          })
          daemonCliGet('agents/lock', {
            project: projectPath,
            agent: agentName,
            terminal_id: myTerminalId,
            owner: 'user',
          }).catch(() => {})
          // 0.37.12 — stamp the resolved session id back onto the
          // agent item so the next serializeTab captures it. Next
          // close/reopen takes the fast path above.
          stampSessionId(result.resumeSession)
          setReady(true)
          return
        }
      } catch (err) {
        console.warn('[AgentChatPane] resume_chat_args failed, falling back:', err)
      }

      // Step 3: Last-resort fallback — fresh session
      if (!cancelled) {
        setLaunchConfig({
          command: 'claude',
          args: ['--dangerously-skip-permissions'],
          cwd: projectPath,
        })
        daemonCliGet('agents/lock', {
          project: projectPath,
          agent: agentName,
          terminal_id: myTerminalId,
          owner: 'user',
        }).catch(() => {})
        setReady(true)
      }
    }
    resolve()
    return () => { cancelled = true }
  }, [agentName, projectPath, refreshNonce, restoredSessionId])

  // Session id detection used to live here — a 12×5s polling loop that
  // called `chat_history_detect_active_session` to find the
  // most-recently-modified .jsonl in the workspace's chat history dir
  // and persist it as workspace_sessions.session_id.
  //
  // Removed in 0.37.0 because it conflated *every* JSONL in the
  // workspace's history dir (including heartbeat fires) and would
  // overwrite the pinned tab's session_id with whatever fired last.
  // Symptom: clicking Launch on fast-test would couple the pinned
  // tab to the heartbeat's session within ~5s of the next poll.
  //
  // Replacement: `k2so_agents_resume_chat_args` pre-allocates a UUID
  // and persists it via `workspace_sessions.session_id` BEFORE claude
  // starts, then passes `--session-id <UUID>` so claude uses it.
  // v2_spawn's auto-stamp hook then writes `active_terminal_id` when
  // the PTY registers. Daemon owns the truth; renderer doesn't poll.

  if (!ready) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        Loading session…
      </div>
    )
  }

  return (
    <div ref={containerRef} className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3 relative">
        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate flex-shrink-0">
          {displayName}
        </span>
        {/* spacer pushes the dropdown + refresh to the right. */}
        <div className="flex-1" />
        {/* 0.37.12 — chat-history dropdown. Lets the user switch the
            pinned chat to a different past session (escape hatch for
            orphaned/deleted sessions or just to revisit). Title
            display also serves as confirmation that the restored
            session id is what's actually running. Sits to the LEFT
            of the refresh button. */}
        <button
          type="button"
          onClick={() => setHistoryOpen((v) => !v)}
          title="Switch pinned chat to a different past Claude session"
          aria-label="Switch pinned chat session"
          aria-haspopup="listbox"
          aria-expanded={historyOpen}
          className="inline-flex items-center gap-1 px-2 py-0.5 rounded text-[10px] font-medium text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] transition-colors no-drag cursor-pointer min-w-0"
        >
          <span className="truncate max-w-[28ch]">{currentChatTitle}</span>
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0">
            <path d={historyOpen ? 'M18 15l-6-6-6 6' : 'M6 9l6 6 6-6'} />
          </svg>
        </button>
        {historyOpen && (
          <div
            role="listbox"
            className="absolute right-12 top-full mt-1 z-20 w-[36ch] max-h-[60vh] overflow-y-auto bg-[var(--color-bg-elevated)] border border-[var(--color-border)] shadow-2xl py-1"
          >
            {historySessions.length === 0 ? (
              <div className="px-3 py-2 text-[10px] text-[var(--color-text-muted)]">
                No past sessions yet.
              </div>
            ) : (
              historySessions.map((s) => {
                const isCurrent = s.sessionId === currentSessionId
                return (
                  <button
                    key={s.sessionId}
                    type="button"
                    role="option"
                    aria-selected={isCurrent}
                    onClick={() => void switchToSession(s.sessionId)}
                    className={`w-full text-left px-3 py-1.5 text-[11px] flex items-center gap-2 transition-colors no-drag cursor-pointer ${
                      isCurrent
                        ? 'bg-[var(--color-accent)]/15 text-[var(--color-text-primary)]'
                        : 'text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-hover)]'
                    }`}
                  >
                    <span className="flex-1 truncate">{s.title || 'Untitled chat'}</span>
                    <span className="flex-shrink-0 text-[9px] text-[var(--color-text-muted)] opacity-70">
                      {s.messageCount}
                    </span>
                    {isCurrent && (
                      <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="3" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true" className="flex-shrink-0 text-[var(--color-accent)]">
                        <path d="M5 12l5 5 9-11" />
                      </svg>
                    )}
                  </button>
                )
              })
            )}
          </div>
        )}
        <button
          type="button"
          onClick={handleRefresh}
          disabled={refreshing}
          title="Restart chat session — kills the current Claude process and spawns a fresh resume. Use after typing `exit` or when the session is unresponsive."
          aria-label="Refresh chat session"
          className="inline-flex items-center justify-center h-5 w-5 rounded text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 disabled:cursor-not-allowed transition-colors flex-shrink-0"
        >
          {/* Inline SVG keeps this self-contained (no icon-lib dep). */}
          <svg
            xmlns="http://www.w3.org/2000/svg"
            width="14"
            height="14"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            strokeWidth="2"
            strokeLinecap="round"
            strokeLinejoin="round"
            className={refreshing ? 'animate-spin' : ''}
            aria-hidden="true"
          >
            <path d="M21 12a9 9 0 0 0-9-9 9.75 9.75 0 0 0-6.74 2.74L3 8" />
            <path d="M3 3v5h5" />
            <path d="M3 12a9 9 0 0 0 9 9 9.75 9.75 0 0 0 6.74-2.74L21 16" />
            <path d="M16 16h5v5" />
          </svg>
        </button>
      </div>
      <div className="flex-1 min-h-0">
        <TerminalPane
          key={refreshNonce}
          terminalId={terminalIdRef.current}
          cwd={launchConfig?.cwd ?? projectPath}
          command={launchConfig?.command}
          args={launchConfig?.args}
          // Register this v2 session under the workspace's canonical
          // key — bare `projectId` (post-0.37.5).
          //
          // **0.37.5:** the canonical key dropped its `<agent_name>`
          // suffix because post-unification there's at most one
          // agent per workspace, and the suffix only created
          // opportunities for the renderer to compute the wrong
          // name (mode→legacy-sentinel hardcoding when AGENT.md said
          // scout — see C3PO 5c80bef1, the SMS Bridge bug). The
          // daemon's `canonical_session::canonical_key_for(pid)`
          // helper is the single source of truth for the shape.
          //
          // What this still gets us:
          //   1. Two workspaces with the same agent name don't
          //      collide — they have distinct project_ids.
          //   2. `k2so msg <workspace>` finds the right session
          //      via the same project_id resolution.
          //   3. Closing Tauri leaves the daemon-owned PTY alive;
          //      reopening Tauri re-attaches via project_id.
          //   4. The daemon's auto-launch (heartbeat headless
          //      wake, awareness inject, ensure_canonical_session)
          //      registers under the SAME bare-pid key, converging
          //      every path on one PTY per workspace.
          // Without this override, TerminalPane defaults to
          // `tab-${terminalId}` — a renderer-only key the daemon
          // never sees on system-driven spawns.
          attachAgentName={projectId}
          // 0.37.4 Phase B — seed the label with the agent's
          // display name and LOCK it so PTY title events (e.g.
          // claude --resume's "Claude Code" emission) cannot
          // overwrite. The daemon spawn helper for the canonical
          // workspace+agent session also seeds from the
          // display-name helper; this prop is the renderer-side
          // mirror of the same intent for the tab-driven spawn
          // path (when the renderer beats the daemon to spawning
          // the canonical session).
          seedLabel={displayName}
          lockLabel={true}
        />
      </div>
    </div>
  )
}
