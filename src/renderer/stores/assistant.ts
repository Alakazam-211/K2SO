import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

/** Maximum number of commands to keep in history. */
const MAX_HISTORY = 50
/** Maximum number of interaction log entries to keep. */
const MAX_LOG = 30

/** A single LLM inference pass (matches backend DebugPass). */
export interface DebugPass {
  prompt: string
  rawOutput: string
}

/** A full interaction log entry — one per user command. */
export interface InteractionLogEntry {
  /** When this command was sent. */
  timestamp: number
  /** The user's original message. */
  message: string
  /** The result summary shown to the user (or error). */
  result: string
  /** The parsed tool calls / message from the LLM. */
  parsed: unknown
  /** Raw LLM output from each inference pass. */
  debugPasses: DebugPass[]
}

interface AssistantState {
  isOpen: boolean
  isLoading: boolean
  isDownloading: boolean
  downloadProgress: number
  modelLoaded: boolean
  lastResult: string | null
  /** Command history, most recent last. */
  history: string[]
  /** Full interaction log for debugging, most recent last. */
  interactionLog: InteractionLogEntry[]
  /** Whether the debug log panel is visible. */
  showDebugLog: boolean

  open: () => void
  close: () => void
  toggle: () => void
  setLoading: (loading: boolean) => void
  setDownloading: (downloading: boolean, progress?: number) => void
  setModelLoaded: (loaded: boolean) => void
  setLastResult: (result: string | null) => void
  /** Add a command to history (deduplicates consecutive repeats). */
  addToHistory: (command: string) => void
  /** Log a full interaction for debugging. */
  logInteraction: (entry: InteractionLogEntry) => void
  /** Toggle the debug log panel. */
  toggleDebugLog: () => void
  /** Clear the interaction log. */
  clearLog: () => void
}

export const useAssistantStore = create<AssistantState>((set) => ({
  isOpen: false,
  isLoading: false,
  isDownloading: false,
  downloadProgress: 0,
  modelLoaded: false,
  lastResult: null,
  history: [],
  interactionLog: [],
  showDebugLog: false,

  open: () => set({ isOpen: true }),
  close: () => set({ isOpen: false, lastResult: null }),
  toggle: () => set((s) => ({ isOpen: !s.isOpen })),
  setLoading: (loading) => set({ isLoading: loading }),
  setDownloading: (downloading, progress) =>
    set({
      isDownloading: downloading,
      downloadProgress: progress ?? (downloading ? 0 : 100)
    }),
  setModelLoaded: (loaded) => set({ modelLoaded: loaded }),
  setLastResult: (result) => set({ lastResult: result }),
  addToHistory: (command) =>
    set((s) => {
      // Don't add consecutive duplicates
      if (s.history.length > 0 && s.history[s.history.length - 1] === command) {
        return s
      }
      const updated = [...s.history, command]
      // Trim to max size
      if (updated.length > MAX_HISTORY) {
        updated.splice(0, updated.length - MAX_HISTORY)
      }
      return { history: updated }
    }),
  logInteraction: (entry) =>
    set((s) => {
      const updated = [...s.interactionLog, entry]
      if (updated.length > MAX_LOG) {
        updated.splice(0, updated.length - MAX_LOG)
      }
      return { interactionLog: updated }
    }),
  toggleDebugLog: () => set((s) => ({ showDebugLog: !s.showDebugLog })),
  clearLog: () => set({ interactionLog: [] }),
}))

// Poll backend status until model is loaded (it loads async on startup)
const pollModelStatus = (): void => {
  invoke<{ loaded: boolean; modelPath: string | null; downloading: boolean }>('assistant_status')
    .then((status) => {
      const store = useAssistantStore.getState()
      if (status.loaded) {
        store.setModelLoaded(true)
      } else if (status.downloading) {
        store.setDownloading(true)
        // Keep polling
        setTimeout(pollModelStatus, 2000)
      } else {
        // Model not loaded yet, might still be initializing — retry a few times
        setTimeout(pollModelStatus, 2000)
      }
    })
    .catch((err) => {
      console.error('[assistant] Failed to poll model status:', err)
    })
}
// Start polling after a brief delay to let the backend initialize
setTimeout(pollModelStatus, 1000)

// Listen for download progress events from Tauri backend
// Rust emits: { percent: f64, bytes_downloaded: u64, total_bytes: u64 }
try {
  listen<{ percent: number; bytesDownloaded: number; totalBytes: number }>(
    'assistant:download-progress',
    (event) => {
      const { percent, bytesDownloaded: bytes_downloaded, totalBytes: total_bytes } = event.payload
      const store = useAssistantStore.getState()

      if (percent >= 100 || (total_bytes > 0 && bytes_downloaded >= total_bytes)) {
        store.setDownloading(false)
        store.setModelLoaded(true)
      } else {
        store.setDownloading(true, percent)
      }
    }
  )
} catch {
  // Ignore — not in Tauri environment (e.g., tests)
}
