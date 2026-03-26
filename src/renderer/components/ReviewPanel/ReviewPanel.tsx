import { useState, useEffect, useCallback, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { useTabsStore } from '@/stores/tabs'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'

// ── Types ───────────────────────────────────────────────────────────

interface ReviewDiffFile {
  path: string
  status: string
  additions: number
  deletions: number
}

interface WorkItem {
  filename: string
  title: string
  priority: string
  assignedBy: string
  itemType: string
  folder: string
}

interface ReviewItem {
  agentName: string
  branch: string
  worktreePath: string | null
  workItems: WorkItem[]
  diffSummary: ReviewDiffFile[]
}

interface ChatSession {
  sessionId: string
  project: string
  title: string
  timestamp: number
  provider: string
  messageCount: number
  originBranch: string | null
}

interface ChecklistItem {
  text: string
  checked: boolean
}

// ── Acceptance Criteria Parser ──────────────────────────────────────

function extractAcceptanceCriteria(workItems: WorkItem[], projectPath: string): Promise<ChecklistItem[]> {
  // Read the actual work item files to extract acceptance criteria
  return Promise.all(
    workItems.map(async (item) => {
      try {
        // Try reading the done file content via the filesystem
        const folders = ['done', 'active', 'inbox']
        for (const folder of folders) {
          try {
            const content = await invoke<string>('fs_read_file', {
              path: `${projectPath}/.k2so/agents/${item.assignedBy !== 'user' ? item.assignedBy : 'default'}/work/${folder}/${item.filename}`,
            })
            return parseCriteria(content)
          } catch {
            continue
          }
        }
        return []
      } catch {
        return []
      }
    })
  ).then((arrays) => arrays.flat())
}

function parseCriteria(content: string): ChecklistItem[] {
  const items: ChecklistItem[] = []
  // Find ## Acceptance Criteria section
  const match = content.match(/##\s*Acceptance Criteria\s*\n([\s\S]*?)(?=\n##|\n---|\Z|$)/)
  if (!match) return items

  const section = match[1]
  // Parse bullet points as criteria
  for (const line of section.split('\n')) {
    const trimmed = line.trim()
    if (trimmed.startsWith('- [x]') || trimmed.startsWith('- [X]')) {
      items.push({ text: trimmed.slice(5).trim(), checked: true })
    } else if (trimmed.startsWith('- [ ]')) {
      items.push({ text: trimmed.slice(5).trim(), checked: false })
    } else if (trimmed.startsWith('- ') || trimmed.startsWith('* ')) {
      items.push({ text: trimmed.slice(2).trim(), checked: false })
    }
  }
  return items
}

// ── Main Component ──────────────────────────────────────────────────

export default function ReviewPanel(): React.JSX.Element {
  const [reviews, setReviews] = useState<ReviewItem[]>([])
  const [loading, setLoading] = useState(true)
  const [feedbackAgent, setFeedbackAgent] = useState<string | null>(null)
  const [feedbackText, setFeedbackText] = useState('')
  const [acting, setActing] = useState(false)
  const [chats, setChats] = useState<ChatSession[]>([])
  const [criteria, setCriteria] = useState<Map<string, ChecklistItem[]>>(new Map())
  const [previewRunning, setPreviewRunning] = useState<Set<string>>(new Set())

  const projects = useProjectsStore((s) => s.projects)
  const activeProjectId = useProjectsStore((s) => s.activeProjectId)
  const activeWorkspaceId = useProjectsStore((s) => s.activeWorkspaceId)

  const activeProject = projects.find((p) => p.id === activeProjectId)
  const activeWorkspace = activeProject?.workspaces.find((w) => w.id === activeWorkspaceId)
  const workspacePath = activeWorkspace?.worktreePath ?? activeProject?.path
  const currentBranch = activeWorkspace?.branch

  // Scope reviews to the active worktree branch
  const scopedReviews = useMemo(() => {
    if (!currentBranch || currentBranch === 'main' || currentBranch === 'master') {
      // On main — show all reviews
      return reviews
    }
    // In a worktree — show only reviews matching this branch
    return reviews.filter(
      (r) => r.branch === currentBranch || r.worktreePath === workspacePath
    )
  }, [reviews, currentBranch, workspacePath])

  const fetchReviews = useCallback(async () => {
    if (!activeProject) {
      setReviews([])
      setLoading(false)
      return
    }
    try {
      const result = await invoke<ReviewItem[]>('k2so_agents_review_queue', {
        projectPath: activeProject.path,
      })
      setReviews(result)

      // Extract acceptance criteria for each review
      const criteriaMap = new Map<string, ChecklistItem[]>()
      for (const review of result) {
        const items = await extractAcceptanceCriteria(review.workItems, activeProject.path)
        if (items.length > 0) {
          criteriaMap.set(review.agentName, items)
        }
      }
      setCriteria(criteriaMap)
    } catch {
      setReviews([])
    } finally {
      setLoading(false)
    }
  }, [activeProject])

  // Fetch associated chat sessions
  const fetchChats = useCallback(async () => {
    if (!workspacePath) return
    try {
      const sessions = await invoke<ChatSession[]>('chat_history_list_for_project', {
        projectPath: workspacePath,
      })
      setChats(sessions)
    } catch {
      setChats([])
    }
  }, [workspacePath])

  useEffect(() => {
    fetchReviews()
    fetchChats()
    const interval = setInterval(() => { fetchReviews(); fetchChats() }, 15_000)
    return () => clearInterval(interval)
  }, [fetchReviews, fetchChats])

  // Match chat sessions to review items by branch
  const getChatsForReview = useCallback((review: ReviewItem): ChatSession[] => {
    if (!review.branch) return []
    return chats.filter(
      (c) => c.originBranch === review.branch || c.project.includes(review.branch)
    ).slice(0, 5) // max 5 recent chats
  }, [chats])

  const handleResumeChat = useCallback((session: ChatSession) => {
    if (!workspacePath) return
    const tabsStore = useTabsStore.getState()
    const command = session.provider === 'cursor' ? 'cursor-agent' : 'claude'
    tabsStore.addTab(workspacePath, {
      title: `Resume: ${session.title.slice(0, 30)}`,
      command,
      args: ['--resume', session.sessionId],
    })
  }, [workspacePath])

  const handleStartPreview = useCallback(async (review: ReviewItem) => {
    const path = review.worktreePath ?? workspacePath
    if (!path) return
    const tabsStore = useTabsStore.getState()
    tabsStore.addTab(path, {
      title: `Preview: ${review.branch || review.agentName}`,
      command: 'npm',
      args: ['run', 'dev'],
    })
    setPreviewRunning((prev) => new Set([...prev, review.agentName]))
  }, [workspacePath])

  const handleApprove = useCallback(async (review: ReviewItem) => {
    if (!activeProject || !review.branch) return
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Approve & Merge "${review.branch}"?`,
      message: `This will merge ${review.diffSummary.length} changed files from agent "${review.agentName}" into main.`,
      confirmLabel: 'Merge',
    })
    if (!confirmed) return

    setActing(true)
    try {
      await invoke('k2so_agents_review_approve', {
        projectPath: activeProject.path,
        branch: review.branch,
        agentName: review.agentName,
      })
      await fetchReviews()
    } catch (e) {
      console.error('[review] Approve failed:', e)
    } finally {
      setActing(false)
    }
  }, [activeProject, fetchReviews])

  const handleReject = useCallback(async (review: ReviewItem) => {
    if (!activeProject) return
    const confirmed = await useConfirmDialogStore.getState().confirm({
      title: `Reject "${review.agentName}"'s work?`,
      message: 'Done items will be moved back to the agent\'s inbox.',
      confirmLabel: 'Reject',
      destructive: true,
    })
    if (!confirmed) return

    setActing(true)
    try {
      await invoke('k2so_agents_review_reject', {
        projectPath: activeProject.path,
        agentName: review.agentName,
        reason: 'Work rejected by reviewer.',
      })
      await fetchReviews()
    } catch (e) {
      console.error('[review] Reject failed:', e)
    } finally {
      setActing(false)
    }
  }, [activeProject, fetchReviews])

  const handleRequestChanges = useCallback(async () => {
    if (!activeProject || !feedbackAgent || !feedbackText.trim()) return
    setActing(true)
    try {
      await invoke('k2so_agents_review_request_changes', {
        projectPath: activeProject.path,
        agentName: feedbackAgent,
        feedback: feedbackText.trim(),
      })
      setFeedbackAgent(null)
      setFeedbackText('')
      await fetchReviews()
    } catch (e) {
      console.error('[review] Request changes failed:', e)
    } finally {
      setActing(false)
    }
  }, [activeProject, feedbackAgent, feedbackText, fetchReviews])

  const toggleCriteria = useCallback((agentName: string, index: number) => {
    setCriteria((prev) => {
      const next = new Map(prev)
      const items = [...(next.get(agentName) ?? [])]
      if (items[index]) {
        items[index] = { ...items[index], checked: !items[index].checked }
        next.set(agentName, items)
      }
      return next
    })
  }, [])

  // ── Render ──────────────────────────────────────────────────────────

  if (!activeProject) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">No workspace selected</p>
      </div>
    )
  }

  if (loading) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">Loading reviews...</p>
      </div>
    )
  }

  if (scopedReviews.length === 0) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">
          {currentBranch && currentBranch !== 'main'
            ? `No reviews for branch "${currentBranch}"`
            : 'No agent work awaiting review'}
        </p>
      </div>
    )
  }

  return (
    <div className="h-full overflow-y-auto">
      {/* Scope indicator */}
      {currentBranch && currentBranch !== 'main' && currentBranch !== 'master' && (
        <div className="px-3 py-1.5 border-b border-[var(--color-border)] bg-[var(--color-bg-elevated)]">
          <span className="text-[10px] text-[var(--color-text-muted)]">
            Showing reviews for <span className="text-[var(--color-accent)]">{currentBranch}</span>
          </span>
        </div>
      )}

      {scopedReviews.map((review) => {
        const reviewChats = getChatsForReview(review)
        const reviewCriteria = criteria.get(review.agentName) ?? []
        const isPreviewUp = previewRunning.has(review.agentName)

        return (
          <div key={review.agentName} className="border-b border-[var(--color-border)] p-3 space-y-2">
            {/* Header */}
            <div className="flex items-center justify-between">
              <div>
                <span className="text-xs font-medium text-[var(--color-text-primary)]">
                  {review.agentName}
                </span>
                {review.branch && (
                  <span className="text-[10px] text-[var(--color-text-muted)] ml-2">
                    {review.branch}
                  </span>
                )}
              </div>
              <span className="text-[10px] text-green-400">
                {review.workItems.length} completed
              </span>
            </div>

            {/* Work items */}
            <div className="space-y-0.5">
              {review.workItems.map((item) => (
                <div key={item.filename} className="text-[10px] text-[var(--color-text-secondary)] flex items-center gap-1.5">
                  <span className={`w-1 h-1 rounded-full flex-shrink-0 ${
                    item.priority === 'high' ? 'bg-red-400' :
                    item.priority === 'critical' ? 'bg-red-600' : 'bg-green-400'
                  }`} />
                  {item.title || item.filename}
                </div>
              ))}
            </div>

            {/* Diff summary */}
            {review.diffSummary.length > 0 && (
              <div className="text-[10px] text-[var(--color-text-muted)]">
                {review.diffSummary.length} files changed
                <span className="text-green-400 ml-1.5">
                  +{review.diffSummary.reduce((sum, f) => sum + f.additions, 0)}
                </span>
                <span className="text-red-400 ml-1">
                  -{review.diffSummary.reduce((sum, f) => sum + f.deletions, 0)}
                </span>
              </div>
            )}

            {/* Acceptance Criteria Checklist */}
            {reviewCriteria.length > 0 && (
              <div className="border border-[var(--color-border)] p-2">
                <div className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
                  Acceptance Criteria
                </div>
                {reviewCriteria.map((item, i) => (
                  <label
                    key={i}
                    className="flex items-start gap-1.5 py-0.5 text-[10px] text-[var(--color-text-secondary)] cursor-pointer hover:text-[var(--color-text-primary)]"
                  >
                    <input
                      type="checkbox"
                      checked={item.checked}
                      onChange={() => toggleCriteria(review.agentName, i)}
                      className="mt-0.5 accent-[var(--color-accent)]"
                    />
                    <span className={item.checked ? 'line-through opacity-50' : ''}>
                      {item.text}
                    </span>
                  </label>
                ))}
              </div>
            )}

            {/* Preview Server */}
            <div className="flex items-center gap-1.5">
              {!isPreviewUp ? (
                <button
                  onClick={() => handleStartPreview(review)}
                  className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer"
                >
                  Start Preview
                </button>
              ) : (
                <span className="text-[10px] text-green-400 flex items-center gap-1">
                  <span className="w-1.5 h-1.5 rounded-full bg-green-400 animate-pulse" />
                  Preview running
                </span>
              )}
            </div>

            {/* Associated Chat Sessions */}
            {reviewChats.length > 0 && (
              <div>
                <div className="text-[10px] font-medium text-[var(--color-text-muted)] uppercase tracking-wider mb-1">
                  Agent Sessions
                </div>
                <div className="space-y-0.5">
                  {reviewChats.map((chat) => (
                    <div key={chat.sessionId} className="flex items-center justify-between">
                      <span className="text-[10px] text-[var(--color-text-secondary)] truncate flex-1 mr-2">
                        {chat.title}
                      </span>
                      <button
                        onClick={() => handleResumeChat(chat)}
                        className="px-1.5 py-0.5 text-[10px] text-[var(--color-accent)] hover:underline no-drag cursor-pointer flex-shrink-0"
                      >
                        Resume
                      </button>
                    </div>
                  ))}
                </div>
              </div>
            )}

            {/* Feedback form */}
            {feedbackAgent === review.agentName && (
              <div className="space-y-1">
                <textarea
                  className="w-full bg-[var(--color-bg-elevated)] border border-[var(--color-border)] text-xs text-[var(--color-text-primary)] px-2 py-1.5 resize-none outline-none focus:border-[var(--color-accent)] placeholder:text-[var(--color-text-muted)]"
                  placeholder="Describe what needs to change..."
                  rows={3}
                  value={feedbackText}
                  onChange={(e) => setFeedbackText(e.target.value)}
                  autoFocus
                />
                <div className="flex gap-1">
                  <button
                    onClick={handleRequestChanges}
                    disabled={acting || !feedbackText.trim()}
                    className="px-2 py-0.5 text-[10px] font-medium bg-yellow-600 text-white hover:bg-yellow-500 transition-colors no-drag cursor-pointer disabled:opacity-50"
                  >
                    Send Feedback
                  </button>
                  <button
                    onClick={() => { setFeedbackAgent(null); setFeedbackText('') }}
                    className="px-2 py-0.5 text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
                  >
                    Cancel
                  </button>
                </div>
              </div>
            )}

            {/* Action buttons */}
            <div className="flex gap-1.5">
              <button
                onClick={() => handleApprove(review)}
                disabled={acting || !review.branch}
                className="px-2.5 py-1 text-[10px] font-medium bg-green-600 text-white hover:bg-green-500 transition-colors no-drag cursor-pointer disabled:opacity-50"
              >
                Approve & Merge
              </button>
              <button
                onClick={() => setFeedbackAgent(review.agentName)}
                disabled={acting}
                className="px-2.5 py-1 text-[10px] font-medium bg-yellow-600/80 text-white hover:bg-yellow-500 transition-colors no-drag cursor-pointer disabled:opacity-50"
              >
                Request Changes
              </button>
              <button
                onClick={() => handleReject(review)}
                disabled={acting}
                className="px-2.5 py-1 text-[10px] text-red-400 border border-red-500/30 hover:bg-red-500/10 transition-colors no-drag cursor-pointer disabled:opacity-50"
              >
                Reject
              </button>
            </div>
          </div>
        )
      })}
    </div>
  )
}
