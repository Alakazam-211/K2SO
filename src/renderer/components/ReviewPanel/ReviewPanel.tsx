import { useState, useEffect, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from '@/stores/projects'
import { useConfirmDialogStore } from '@/stores/confirm-dialog'

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

export default function ReviewPanel(): React.JSX.Element {
  const [reviews, setReviews] = useState<ReviewItem[]>([])
  const [loading, setLoading] = useState(true)
  const [feedbackAgent, setFeedbackAgent] = useState<string | null>(null)
  const [feedbackText, setFeedbackText] = useState('')
  const [acting, setActing] = useState(false)

  const activeProject = useProjectsStore((s) => {
    const id = s.activeProjectId
    return id ? s.projects.find((p) => p.id === id) : null
  })

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
    } catch {
      setReviews([])
    } finally {
      setLoading(false)
    }
  }, [activeProject])

  useEffect(() => {
    fetchReviews()
    const interval = setInterval(fetchReviews, 15_000)
    return () => clearInterval(interval)
  }, [fetchReviews])

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

  if (reviews.length === 0) {
    return (
      <div className="h-full flex items-center justify-center p-4">
        <p className="text-xs text-[var(--color-text-muted)]">No agent work awaiting review</p>
      </div>
    )
  }

  return (
    <div className="h-full overflow-y-auto">
      {reviews.map((review) => (
        <div
          key={review.agentName}
          className="border-b border-[var(--color-border)] p-3"
        >
          {/* Header */}
          <div className="flex items-center justify-between mb-2">
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
          <div className="mb-2 space-y-0.5">
            {review.workItems.map((item) => (
              <div key={item.filename} className="text-[10px] text-[var(--color-text-secondary)] flex items-center gap-1.5">
                <span className={`w-1 h-1 rounded-full flex-shrink-0 ${
                  item.priority === 'high' ? 'bg-red-400' :
                  item.priority === 'critical' ? 'bg-red-600' :
                  'bg-green-400'
                }`} />
                {item.title || item.filename}
              </div>
            ))}
          </div>

          {/* Diff summary */}
          {review.diffSummary.length > 0 && (
            <div className="mb-2 text-[10px] text-[var(--color-text-muted)]">
              {review.diffSummary.length} files changed
              <span className="text-green-400 ml-1.5">
                +{review.diffSummary.reduce((sum, f) => sum + f.additions, 0)}
              </span>
              <span className="text-red-400 ml-1">
                -{review.diffSummary.reduce((sum, f) => sum + f.deletions, 0)}
              </span>
            </div>
          )}

          {/* Feedback form */}
          {feedbackAgent === review.agentName && (
            <div className="mb-2 space-y-1">
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
      ))}
    </div>
  )
}
