import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { useProjectsStore } from './projects'

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

export interface GlobalReviewItem extends ReviewItem {
  projectId: string
  projectName: string
  projectColor: string
  projectPath: string
  workspaceId: string | null
}

interface ReviewQueueState {
  isOpen: boolean
  reviews: GlobalReviewItem[]
  pendingCount: number
  loading: boolean

  open: () => void
  close: () => void
  toggle: () => void
  fetchAll: () => Promise<void>
}

export const useReviewQueueStore = create<ReviewQueueState>((set) => ({
  isOpen: false,
  reviews: [],
  pendingCount: 0,
  loading: false,

  open: () => {
    set({ isOpen: true })
    useReviewQueueStore.getState().fetchAll()
  },
  close: () => set({ isOpen: false }),
  toggle: () => {
    const { isOpen } = useReviewQueueStore.getState()
    if (!isOpen) {
      useReviewQueueStore.getState().open()
    } else {
      set({ isOpen: false })
    }
  },

  fetchAll: async () => {
    set({ loading: true })
    const projects = useProjectsStore.getState().projects
    const allReviews: GlobalReviewItem[] = []

    for (const project of projects) {
      if (!project.agentEnabled && (!project.agentMode || project.agentMode === 'off')) continue
      try {
        const reviews = await invoke<ReviewItem[]>('k2so_agents_review_queue', {
          projectPath: project.path,
        })
        for (const review of reviews) {
          // Find matching workspace for jump-to — try branch match, worktree path, or partial branch match
          const ws = project.workspaces.find(
            (w) =>
              w.branch === review.branch ||
              w.worktreePath === review.worktreePath ||
              (w.branch && review.branch && review.branch.includes(w.branch)) ||
              (w.branch && review.branch && w.branch.includes(review.branch))
          )
          allReviews.push({
            ...review,
            projectId: project.id,
            projectName: project.name,
            projectColor: project.color,
            projectPath: project.path,
            workspaceId: ws?.id ?? null,
          })
        }
      } catch {
        // Skip projects with errors
      }
    }

    set({
      reviews: allReviews,
      pendingCount: allReviews.length,
      loading: false,
    })
  },
}))

// Poll for pending count every 30 seconds
let pollInterval: ReturnType<typeof setInterval> | null = null

export function startReviewQueuePolling(): void {
  if (pollInterval) return
  useReviewQueueStore.getState().fetchAll()
  pollInterval = setInterval(() => {
    useReviewQueueStore.getState().fetchAll()
  }, 30_000)
}

export function stopReviewQueuePolling(): void {
  if (pollInterval) {
    clearInterval(pollInterval)
    pollInterval = null
  }
}
