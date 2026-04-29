import { useState, useEffect, useCallback, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useTabsStore } from '@/stores/tabs'
import { useProjectsStore } from '@/stores/projects'
import { addNavWorktree } from '@/components/Sidebar/Sidebar'
import { TerminalPane } from '@/terminal-v2/TerminalPane'
import { agentChatId } from '@/lib/terminal-id'
import { AgentInboxPane } from './AgentInboxPane'
import { AgentChatPane } from './AgentChatPane'
import Markdown from '@/components/Markdown/Markdown'
import remarkGfm from 'remark-gfm'

interface AgentPaneProps {
  agentName: string
  projectPath: string
  /** Which surface to render. Pinned tabs created post-0.36.0 set this
   *  explicitly. `undefined` falls back to 'inbox' for backwards compat
   *  with rows serialized before the split. */
  section?: 'inbox' | 'chat'
  onClose?: () => void
}

interface WorkItem {
  filename: string
  title: string
  priority: string
  assignedBy: string
  created: string
  itemType: string
  folder: string
  bodyPreview: string
}

/**
 * Thin dispatcher. Replaces the pre-0.36.0 4-sub-tab AgentPane.
 *
 * - `__wt:<id>` → WorktreeDetailPane (Task / Chat / Review for a worktree)
 * - `section === 'chat'` → AgentChatPane (persistent Claude chat)
 * - default ('inbox' or unset) → AgentInboxPane (work-queue kanban)
 *
 * CLAUDE.md and AGENT.md edit surfaces moved out — they live in
 * Workspace Settings now (the existing "Edit AGENT.md" / "Edit CLAUDE.md"
 * buttons there cover what the deleted sub-tabs did).
 */
export function AgentPane({ agentName, projectPath, section }: AgentPaneProps): React.JSX.Element {
  if (agentName.startsWith('__wt:')) {
    return <WorktreeDetailPane worktreeId={agentName.slice(5)} projectPath={projectPath} />
  }

  if (section === 'chat') {
    return <AgentChatPane agentName={agentName} projectPath={projectPath} />
  }

  // Default to inbox for legacy serialized rows that have no section field.
  return <AgentInboxPane agentName={agentName} projectPath={projectPath} />
}

// ── Worktree Detail Pane ────────────────────────────────────────────────
//
// Worktrees keep their own three-tab UI (Task / Chat / Review) — they
// were never pinned tabs and the workflow benefits from the bundled
// surface. The chat sub-tab uses the project-namespaced terminal id
// (`agent-chat:<project_id>:wt-<worktree_id>`) to avoid colliding with
// other worktrees' chat sessions.

const worktreeLastTab = new Map<string, 'task' | 'chat' | 'review'>()

function WorktreeDetailPane({ worktreeId, projectPath }: { worktreeId: string; projectPath: string }): React.JSX.Element {
  const [activeTab, setActiveTab] = useState<'task' | 'chat' | 'review'>(
    worktreeLastTab.get(worktreeId) ?? 'chat'
  )
  const [taskContent, setTaskContent] = useState<string>('')
  const [reviewContent, setReviewContent] = useState<string>('')
  const [chatMounted, setChatMounted] = useState(activeTab === 'chat')
  const [reviewAvailable, setReviewAvailable] = useState(false)

  const workspace = useProjectsStore(useCallback((s) => {
    for (const p of s.projects) {
      const ws = p.workspaces.find((w) => w.id === worktreeId)
      if (ws) return ws
    }
    return null
  }, [worktreeId]))
  const projectId = useProjectsStore(useCallback((s) => {
    for (const p of s.projects) {
      if (p.workspaces.some((w) => w.id === worktreeId)) return p.id
    }
    return null
  }, [worktreeId]))
  const setActiveWorkspace = useProjectsStore((s) => s.setActiveWorkspace)

  const displayName = workspace?.name?.replace(/^agent\/[^/]+\//, '') || workspace?.branch || 'Worktree'
  const agentMatch = workspace?.name?.match(/^agent\/([^/]+)\//)
  const agentTemplate = agentMatch?.[1]
  const worktreePath = workspace?.worktreePath || projectPath

  useEffect(() => {
    if (!agentTemplate) {
      setTaskContent('')
      return
    }
    const loadTask = async (): Promise<void> => {
      try {
        const items = await invoke<WorkItem[]>('k2so_agents_work_list', {
          projectPath,
          agentName: agentTemplate,
          folder: 'active',
        })
        if (items.length > 0) {
          const taskPath = `${projectPath}/.k2so/agents/${agentTemplate}/work/active/${items[0].filename}`
          const result = await invoke<{ content: string }>('fs_read_file', { path: taskPath })
          setTaskContent(result.content)
        } else {
          setTaskContent('')
        }
      } catch { setTaskContent('') }
    }
    loadTask()
  }, [projectPath, agentTemplate])

  useEffect(() => {
    if (!agentTemplate) {
      setReviewAvailable(false)
      return
    }
    const loadReview = async (): Promise<void> => {
      try {
        const items = await invoke<WorkItem[]>('k2so_agents_work_list', {
          projectPath,
          agentName: agentTemplate,
          folder: 'done',
        })
        if (items.length > 0) {
          setReviewAvailable(true)
          const reviewPath = `${projectPath}/.k2so/agents/${agentTemplate}/work/done/${items[0].filename}`
          const result = await invoke<{ content: string }>('fs_read_file', { path: reviewPath })
          setReviewContent(result.content)
        } else {
          setReviewAvailable(false)
          setReviewContent('')
        }
      } catch {
        setReviewAvailable(false)
        setReviewContent('')
      }
    }
    loadReview()
  }, [projectPath, agentTemplate])

  useEffect(() => {
    if (activeTab === 'chat') setChatMounted(true)
  }, [activeTab])

  const tabs: Array<{ key: 'task' | 'chat' | 'review'; label: string; disabled: boolean }> = [
    { key: 'task', label: 'Task', disabled: !agentTemplate || !taskContent },
    { key: 'chat', label: 'Chat', disabled: false },
    { key: 'review', label: 'Review', disabled: !reviewAvailable },
  ]

  return (
    <div className="h-full flex flex-col bg-[var(--color-bg)] overflow-hidden">
      <div className="px-3 py-2 border-b border-[var(--color-border)] flex-shrink-0 flex items-center gap-3">
        <div className="flex gap-0.5 flex-shrink-0">
          {tabs.map(({ key, label, disabled }) => (
            <button
              key={key}
              onClick={() => {
                if (!disabled) {
                  setActiveTab(key)
                  worktreeLastTab.set(worktreeId, key)
                }
              }}
              disabled={disabled}
              className={`px-3 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                activeTab === key
                  ? 'bg-[var(--color-accent)] text-white'
                  : disabled
                    ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)]/40 cursor-not-allowed'
                    : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
              }`}
            >
              {label}
            </button>
          ))}
        </div>

        <span className="text-xs font-semibold text-[var(--color-text-primary)] truncate">
          {displayName}
        </span>
        {agentTemplate && (
          <span className="text-[9px] font-medium text-[var(--color-text-muted)] bg-white/5 px-1.5 py-0.5 flex-shrink-0">
            {agentTemplate}
          </span>
        )}

        <div className="flex-1" />

        {projectId && (
          <button
            onClick={() => {
              const currentTabId = useTabsStore.getState().activeTabId
              if (currentTabId) {
                useTabsStore.getState().removeTab(currentTabId)
              }
              addNavWorktree(worktreeId)
              setTimeout(() => setActiveWorkspace(projectId, worktreeId), 50)
            }}
            className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors no-drag cursor-pointer flex-shrink-0 flex items-center gap-1"
            title="Open this worktree as a full workspace with file tree and changes"
          >
            <svg className="w-3 h-3" viewBox="0 0 16 16" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
              <path d="M7 2H3a1 1 0 0 0-1 1v10a1 1 0 0 0 1 1h10a1 1 0 0 0 1-1V9" />
              <path d="M14 2l-7 7" />
              <path d="M10 2h4v4" />
            </svg>
            Open Full Workspace
          </button>
        )}
      </div>

      <div className="flex-1 overflow-hidden relative">
        {activeTab === 'task' && (
          <div className="h-full overflow-y-auto p-4">
            {taskContent ? (
              <div className="prose prose-sm prose-invert max-w-none">
                <Markdown remarkPlugins={[remarkGfm]}>{taskContent}</Markdown>
              </div>
            ) : (
              <div className="flex items-center justify-center h-full">
                <p className="text-xs text-[var(--color-text-muted)]">
                  {agentTemplate ? 'No active task assigned.' : 'This worktree was not created by the agent system.'}
                </p>
              </div>
            )}
          </div>
        )}

        {chatMounted && projectId && (
          <div
            className="absolute inset-0"
            style={{ zIndex: activeTab === 'chat' ? 1 : 0, visibility: activeTab === 'chat' ? 'visible' : 'hidden' }}
          >
            <WorktreeChatTerminal
              worktreeId={worktreeId}
              projectId={projectId}
              cwd={worktreePath}
              projectPath={projectPath}
              autoFocus={activeTab === 'chat'}
            />
          </div>
        )}

        {activeTab === 'review' && (
          <div className="h-full overflow-y-auto p-4">
            {reviewAvailable ? (
              <div className="space-y-4">
                <div className="prose prose-sm prose-invert max-w-none">
                  <Markdown remarkPlugins={[remarkGfm]}>{reviewContent}</Markdown>
                </div>

                <div className="border-t border-[var(--color-border)] pt-4 space-y-3">
                  <button
                    className="w-full px-4 py-2 text-xs font-medium bg-[var(--color-accent)] text-white hover:bg-[var(--color-accent)]/90 transition-colors no-drag cursor-pointer"
                    onClick={async () => {
                      try {
                        await invoke('git_merge_worktree', { projectPath, workspaceId: worktreeId })
                      } catch (e) {
                        console.error('[worktree] Merge failed:', e)
                      }
                    }}
                  >
                    AI Merge Worktree/Branch
                  </button>
                  <p className="text-[10px] text-[var(--color-text-muted)] leading-relaxed">
                    If the work is not right, go to the Chat tab to address the issue with the agent before merging.
                  </p>
                </div>
              </div>
            ) : (
              <div className="flex items-center justify-center h-full">
                <p className="text-xs text-[var(--color-text-muted)]">
                  No completed work to review yet.
                </p>
              </div>
            )}
          </div>
        )}
      </div>
    </div>
  )
}

// Internal: worktree chat uses a separate id (`wt-<worktreeId>` agent name)
// so it doesn't share state with the main agent chat tab. Project-namespaced
// per the same scheme.
function WorktreeChatTerminal({
  worktreeId,
  projectId,
  cwd,
  projectPath,
  autoFocus,
}: {
  worktreeId: string
  projectId: string
  cwd: string
  projectPath: string
  autoFocus: boolean
}): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const agentName = `wt-${worktreeId}`
  const terminalIdRef = useRef(agentChatId(projectId, agentName))
  const [launchConfig, setLaunchConfig] = useState<{ command: string; args: string[]; cwd: string } | null>(null)
  const [ready, setReady] = useState(false)

  useEffect(() => {
    let cancelled = false
    const resolve = async (): Promise<void> => {
      const myTerminalId = terminalIdRef.current
      try {
        const exists = await invoke<boolean>('terminal_exists', { id: myTerminalId })
        if (!cancelled && exists) {
          setLaunchConfig(null)
          setReady(true)
          return
        }
      } catch { /* fall through */ }
      try {
        const result = await invoke<{
          command: string
          args: string[]
          cwd: string
        }>('k2so_agents_build_launch', { projectPath, agentName })
        if (!cancelled && result) {
          setLaunchConfig({ command: result.command, args: result.args, cwd: result.cwd })
          invoke('k2so_agents_lock', { projectPath, agentName, terminalId: myTerminalId, owner: 'user' }).catch(() => {})
          setReady(true)
          return
        }
      } catch (err) {
        console.warn('[WorktreeChat] build_launch failed, falling back:', err)
      }
      if (!cancelled) {
        setLaunchConfig({ command: 'claude', args: ['--dangerously-skip-permissions'], cwd })
        invoke('k2so_agents_lock', { projectPath, agentName, terminalId: myTerminalId, owner: 'user' }).catch(() => {})
        setReady(true)
      }
    }
    resolve()
    return () => { cancelled = true }
  }, [agentName, projectPath, cwd])

  useEffect(() => {
    if (autoFocus && ready) {
      requestAnimationFrame(() => {
        const el = containerRef.current?.querySelector('[tabindex]') as HTMLElement | null
        el?.focus()
      })
    }
  }, [autoFocus, ready])

  if (!ready) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-[var(--color-text-muted)]">
        Loading session…
      </div>
    )
  }

  return (
    <div ref={containerRef} className="h-full">
      <TerminalPane
        terminalId={terminalIdRef.current}
        cwd={launchConfig?.cwd ?? cwd}
        command={launchConfig?.command}
        args={launchConfig?.args}
      />
    </div>
  )
}
