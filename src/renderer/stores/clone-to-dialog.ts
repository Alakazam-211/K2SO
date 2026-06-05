import { create } from 'zustand'

import type {
  CloneStage,
  CloneManifestSummary,
  CloneUnpackResult,
} from '@/lib/clone-to'
import type { ConnectHost } from '@/stores/connect-host'

// Store for the "Clone to" progress modal. Mirrors the open/close idiom of
// add-workspace-dialog / remote-folder-picker. The modal subscribes to this
// and renders the current stage; the orchestration (`cloneWorkspaceTo`)
// pushes updates here via its hooks.
//
// Lifecycle: the context-menu handler calls `start({ projectPath, host })`,
// then drives `cloneWorkspaceTo` whose hooks call setStage / setSummary /
// setDone / setError on this store. The modal stays open through 'done' /
// 'error' so the user sees the result + the re-supply checklist; they close
// it manually.

interface CloneToDialogState {
  isOpen: boolean
  /** Source workspace path (LOCAL). */
  projectPath: string | null
  /** Source workspace display name (for the header). */
  projectName: string | null
  /** Destination K2 Connect host. */
  host: ConnectHost | null
  stage: CloneStage
  /** Manifest summary, set once the bundle is built. */
  summary: CloneManifestSummary | null
  /** Unpack result, set on success. */
  result: CloneUnpackResult | null
  /** Error message, set on failure. */
  error: string | null

  /** Open the modal at the 'bundling' stage for a given source + host. */
  start: (args: { projectPath: string; projectName: string; host: ConnectHost }) => void
  setStage: (stage: CloneStage) => void
  setSummary: (summary: CloneManifestSummary) => void
  setDone: (result: CloneUnpackResult) => void
  setError: (error: string) => void
  /** Dismiss + reset. */
  close: () => void
}

export const useCloneToDialogStore = create<CloneToDialogState>((set) => ({
  isOpen: false,
  projectPath: null,
  projectName: null,
  host: null,
  stage: 'bundling',
  summary: null,
  result: null,
  error: null,

  start: ({ projectPath, projectName, host }) =>
    set({
      isOpen: true,
      projectPath,
      projectName,
      host,
      stage: 'bundling',
      summary: null,
      result: null,
      error: null,
    }),

  setStage: (stage) => set({ stage }),
  setSummary: (summary) => set({ summary }),
  setDone: (result) => set({ stage: 'done', result }),
  setError: (error) => set({ stage: 'error', error }),

  close: () =>
    set({
      isOpen: false,
      projectPath: null,
      projectName: null,
      host: null,
      stage: 'bundling',
      summary: null,
      result: null,
      error: null,
    }),
}))
