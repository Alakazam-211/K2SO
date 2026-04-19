import { create } from 'zustand'

export interface WorkspacePreviewEntry {
  path: string
  action: 'archive_and_import' | 'refresh' | 'create' | 'marker_injected'
  size_bytes: number | null
  note: string
}

interface AddWorkspaceDialogState {
  isOpen: boolean
  isPending: boolean
  path: string | null
  preview: WorkspacePreviewEntry[]
  error: string | null
  /** Fires when the user confirms. The caller passes in a handler that
   * completes the add (projects_add_from_path + k2so_agents_run_workspace_ingest). */
  onConfirm: (() => Promise<void>) | null

  open: (args: { path: string; preview: WorkspacePreviewEntry[]; onConfirm: () => Promise<void> }) => void
  close: () => void
  setIsPending: (pending: boolean) => void
  setError: (error: string) => void
}

export const useAddWorkspaceDialogStore = create<AddWorkspaceDialogState>((set) => ({
  isOpen: false,
  isPending: false,
  path: null,
  preview: [],
  error: null,
  onConfirm: null,

  open: ({ path, preview, onConfirm }) =>
    set({ isOpen: true, path, preview, onConfirm, error: null, isPending: false }),

  close: () =>
    set({ isOpen: false, isPending: false, path: null, preview: [], onConfirm: null, error: null }),

  setIsPending: (isPending: boolean) => set({ isPending }),

  setError: (error: string) => set({ error, isPending: false })
}))
