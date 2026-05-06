import { useState, useEffect, useRef, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { TerminalPane } from '@/terminal-v2/TerminalPane'
import { agentChatId } from '@/lib/terminal-id'
import { getDaemonWs } from '@/kessel/daemon-ws'

interface AgentChatPaneProps {
  agentName: string
  projectPath: string
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
export function AgentChatPane({ agentName, projectPath }: AgentChatPaneProps): React.JSX.Element {
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

  // `key={projectId}:${agentName}` forces a clean remount when the
  // workspace switches. Without it, React reuses the same
  // AgentChatTerminal instance and `terminalIdRef` (initialized from
  // `useRef(agentChatId(projectId, agentName))`) keeps the stale
  // workspace's terminal id — defense-in-depth against the
  // cross-workspace pinned-chat collision fixed in 0.36.14.
  return (
    <AgentChatTerminal
      key={`${projectId}:${agentName}`}
      agentName={agentName}
      projectId={projectId}
      projectPath={projectPath}
    />
  )
}

interface AgentChatTerminalProps {
  agentName: string
  projectId: string
  projectPath: string
}

function AgentChatTerminal({ agentName, projectId, projectPath }: AgentChatTerminalProps): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const terminalIdRef = useRef(agentChatId(projectId, agentName))
  const [launchConfig, setLaunchConfig] = useState<{
    command: string
    args: string[]
    cwd: string
  } | null>(null)
  const [ready, setReady] = useState(false)
  // Bumped on every refresh-button click to force a clean remount of
  // TerminalPane (key={refreshNonce}) and a re-run of the resolve
  // effect. Used when the user typed `exit` and the Claude process
  // ended — without a remount the dead PTY stays on screen.
  const [refreshNonce, setRefreshNonce] = useState(0)
  const [refreshing, setRefreshing] = useState(false)

  const handleRefresh = useCallback(async (): Promise<void> => {
    if (refreshing) return
    setRefreshing(true)
    // Kill the daemon-owned PTY (best-effort). The unregister hook in
    // v2_session_map clears agent_sessions.active_terminal_id and the
    // child-exit observer fires for any still-alive process. If the
    // session was already dead (user typed `exit`), the daemon's
    // find-or-spawn on the next mount just spawns fresh.
    //
    // Pass the project-namespaced key (0.36.14+) so we close THIS
    // workspace's session, not whichever bare-name session happens to
    // be registered globally.
    try {
      const { port, token } = await getDaemonWs()
      await fetch(
        `http://127.0.0.1:${port}/cli/sessions/v2/close?token=${token}`,
        {
          method: 'POST',
          headers: { 'Content-Type': 'application/json' },
          body: JSON.stringify({ agent_name: `${projectId}:${agentName}` }),
        },
      ).catch(() => {})
    } catch { /* ignore — refresh proceeds either way */ }

    setLaunchConfig(null)
    setReady(false)
    setRefreshNonce((n) => n + 1)
    setRefreshing(false)
  }, [projectId, agentName, refreshing])

  useEffect(() => {
    let cancelled = false
    const resolve = async (): Promise<void> => {
      const myTerminalId = terminalIdRef.current

      // Step 1: Reattach if PTY already alive in this Tauri session
      try {
        const exists = await invoke<boolean>('terminal_exists', { id: myTerminalId })
        if (!cancelled && exists) {
          setLaunchConfig(null)
          setReady(true)
          return
        }
      } catch { /* fall through */ }

      // Step 1b: Check the daemon for an existing session under this
      // workspace-namespaced agent key. When the user has closed Tauri,
      // the daemon can keep the workspace agent's PTY alive (heartbeat
      // fires, k2so msg injections, etc.). On Tauri reopen we want to
      // attach to that existing PTY rather than spawn a fresh
      // `claude --resume`. TerminalPane will use the attachAgentName
      // prop (always set below) to reach the daemon's canonical key in
      // v2_session_map; this lookup is informational — purely for
      // log-trail/diagnostic visibility — but proves the daemon-side
      // bookkeeping is sane before we hand off to the spawn path.
      //
      // Lookup is on the prefixed `<projectId>:<agentName>` key so
      // workspaces sharing an agent name (e.g., two Workspace Manager
      // workspaces both running `manager`) don't resolve to each
      // other's session.
      try {
        const json = await invoke<string>('k2so_session_lookup_by_agent', {
          agent: `${projectId}:${agentName}`,
        })
        const data = JSON.parse(json) as {
          sessionAlive?: boolean
          sessionId?: string | null
          isV2?: boolean
        }
        if (!cancelled && data.sessionAlive) {
          console.info(
            '[AgentChatPane] daemon has live session for',
            `${projectId}:${agentName}`,
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
        const result = await invoke<{
          command: string
          args: string[]
          cwd: string
          resumeSession?: string
        }>('k2so_agents_resume_chat_args', {
          projectPath,
          agentName,
        })
        if (!cancelled && result) {
          setLaunchConfig({
            command: result.command,
            args: result.args,
            cwd: result.cwd,
          })
          invoke('k2so_agents_lock', {
            projectPath,
            agentName,
            terminalId: myTerminalId,
            owner: 'user',
          }).catch(() => {})
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
        invoke('k2so_agents_lock', {
          projectPath,
          agentName,
          terminalId: myTerminalId,
          owner: 'user',
        }).catch(() => {})
        setReady(true)
      }
    }
    resolve()
    return () => { cancelled = true }
  }, [agentName, projectPath, refreshNonce])

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
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3">
        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
          {agentName}
        </span>
        <button
          type="button"
          onClick={handleRefresh}
          disabled={refreshing}
          title="Restart chat session — kills the current Claude process and spawns a fresh resume. Use after typing `exit` or when the session is unresponsive."
          aria-label="Refresh chat session"
          className="ml-auto inline-flex items-center justify-center h-5 w-5 rounded text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-hover)] disabled:opacity-50 disabled:cursor-not-allowed transition-colors"
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
          // Register this v2 session under a project-namespaced agent
          // key (`<projectId>:<agentName>`, 0.36.14+) so:
          //   1. Two workspaces with the same agent name (e.g., both
          //      in Workspace Manager mode → both `manager`) don't
          //      collide on a single daemon-side slot. Pre-0.36.14,
          //      opening a second workspace replaced the first
          //      workspace's entry in v2_session_map and both chat
          //      tabs ended up cross-wired to the same PTY.
          //   2. `k2so msg <workspace>` finds the right session via
          //      session_lookup's bare-name mirror (registered
          //      alongside the prefixed key by v2_session_map::register
          //      for back-compat with bare-keyed callers).
          //   3. Closing Tauri leaves the daemon-owned PTY alive
          //      under both keys; reopening Tauri re-attaches via the
          //      prefixed key for unambiguous workspace identification.
          //   4. The daemon's auto-launch (heartbeat headless wake,
          //      awareness inject) registers under the same key,
          //      converging both paths on one PTY per workspace
          //      agent.
          // Without this override, TerminalPane defaults to
          // `tab-${terminalId}` — a renderer-only key the daemon
          // never sees on system-driven spawns.
          attachAgentName={`${projectId}:${agentName}`}
        />
      </div>
    </div>
  )
}
