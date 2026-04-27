import { useState, useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { TerminalPane } from '@/terminal-v2/TerminalPane'
import { agentChatId } from '@/lib/terminal-id'

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

  return (
    <AgentChatTerminal
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

  useEffect(() => {
    let cancelled = false
    const resolve = async (): Promise<void> => {
      const myTerminalId = terminalIdRef.current

      // Step 1: Reattach if PTY already alive
      try {
        const exists = await invoke<boolean>('terminal_exists', { id: myTerminalId })
        if (!cancelled && exists) {
          setLaunchConfig(null)
          setReady(true)
          return
        }
      } catch { /* fall through */ }

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
  }, [agentName, projectPath])

  // Detect Claude session id from the running PTY and persist it for resume.
  useEffect(() => {
    if (!ready) return
    const interval = setInterval(async () => {
      try {
        const sessionId = await invoke<string | null>('chat_history_detect_active_session', {
          provider: 'claude',
          projectPath,
        })
        if (sessionId) {
          invoke('k2so_agents_save_session_id', {
            projectPath,
            agentName,
            sessionId,
          }).catch(() => {})
          clearInterval(interval)
        }
      } catch { /* ignore */ }
    }, 5000)
    return () => clearInterval(interval)
  }, [ready, projectPath, agentName])

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
      </div>
      <div className="flex-1 min-h-0">
        <TerminalPane
          terminalId={terminalIdRef.current}
          cwd={launchConfig?.cwd ?? projectPath}
          command={launchConfig?.command}
          args={launchConfig?.args}
        />
      </div>
    </div>
  )
}
