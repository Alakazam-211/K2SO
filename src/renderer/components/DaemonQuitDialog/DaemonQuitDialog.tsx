import { useCallback, useEffect, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

// Backend emits `daemon-quit-prompt` from `RunEvent::ExitRequested` in
// release builds when the daemon is running. This dialog listens for
// that event, shows the user their three options, and bounces the
// result back via the `confirm_quit` Tauri command, which sets an
// atomic and re-requests exit. Dev builds never see this — the
// emitter is `#[cfg(not(debug_assertions))]` — so CLI-based kill
// workflows aren't affected.
//
// Keep the component entirely self-contained (listener + state +
// render). Parent just mounts it once at the App root.

type QuitAction = 'keep' | 'stop' | 'cancel'

export default function DaemonQuitDialog(): React.JSX.Element | null {
  const [open, setOpen] = useState(false)
  const [busy, setBusy] = useState<QuitAction | null>(null)

  // Listen for the backend's quit-prompt event. Fires every time the
  // user hits Cmd+Q with a live daemon; clears on confirm_quit.
  useEffect(() => {
    let unlisten: (() => void) | undefined
    listen('daemon-quit-prompt', () => setOpen(true))
      .then((fn) => {
        unlisten = fn
      })
      .catch((e) => console.warn('[daemon-quit]', e))
    return () => {
      unlisten?.()
    }
  }, [])

  // Escape key is "cancel" — keeps the window open, clears the prompt.
  useEffect(() => {
    if (!open) return
    const handler = async (e: KeyboardEvent): Promise<void> => {
      if (e.key === 'Escape') {
        e.preventDefault()
        await handleAction('cancel')
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open])

  const handleAction = useCallback(async (action: QuitAction) => {
    setBusy(action)
    try {
      await invoke('confirm_quit', { action })
      // For "keep" / "stop" the backend is about to exit the process,
      // so the next line never runs — but for "cancel", we close the
      // dialog locally.
      if (action === 'cancel') {
        setOpen(false)
      }
    } catch (e) {
      console.error('[daemon-quit]', e)
    } finally {
      setBusy(null)
    }
  }, [])

  if (!open) return null

  return (
    <div
      className="fixed inset-0 z-[1000] flex items-center justify-center"
      style={{ backgroundColor: 'rgba(0,0,0,0.6)' }}
      onClick={(e) => {
        // Backdrop click = cancel
        if (e.target === e.currentTarget) void handleAction('cancel')
      }}
    >
      <div
        className="max-w-md w-full mx-4"
        style={{
          backgroundColor: '#111',
          border: '1px solid var(--color-border)',
        }}
      >
        <div className="px-5 pt-5 pb-3">
          <h3 className="text-sm font-medium text-[var(--color-text-primary)]">
            Keep K2SO running in the background?
          </h3>
          <p className="text-xs text-[var(--color-text-muted)] mt-2 leading-relaxed">
            The K2SO daemon is active. If you keep it running, your agents
            continue working while the app is closed — scheduled heartbeats
            still fire and the mobile companion stays reachable. You can
            always reopen the app to see progress.
          </p>
          <p className="text-xs text-[var(--color-text-muted)] mt-2 leading-relaxed">
            Stopping the daemon freezes every agent until you open K2SO
            again.
          </p>
        </div>

        <div className="px-5 pb-5 flex flex-col gap-2">
          <button
            onClick={() => void handleAction('keep')}
            disabled={busy !== null}
            className="w-full px-3 py-2 text-xs font-medium bg-[var(--color-accent)] text-white hover:opacity-90 transition-opacity no-drag cursor-pointer disabled:opacity-50 text-left"
          >
            <span className="block">
              {busy === 'keep' ? 'Quitting…' : 'Keep daemon running'}
            </span>
            <span className="block text-[10px] opacity-80 mt-0.5">
              Quit the window; agents continue in the background
            </span>
          </button>
          <button
            onClick={() => void handleAction('stop')}
            disabled={busy !== null}
            className="w-full px-3 py-2 text-xs font-medium bg-red-500/20 text-red-400 border border-red-500/40 hover:bg-red-500/30 transition-colors no-drag cursor-pointer disabled:opacity-50 text-left"
          >
            <span className="block">
              {busy === 'stop' ? 'Stopping…' : 'Stop everything and quit'}
            </span>
            <span className="block text-[10px] opacity-80 mt-0.5">
              Unload the daemon; agents freeze until you reopen the app
            </span>
          </button>
          <button
            onClick={() => void handleAction('cancel')}
            disabled={busy !== null}
            className="w-full px-3 py-2 text-xs text-[var(--color-text-muted)] border border-[var(--color-border)] hover:text-[var(--color-text-primary)] hover:border-[var(--color-text-muted)] transition-colors no-drag cursor-pointer disabled:opacity-50"
          >
            Cancel (stay open)
          </button>
        </div>
      </div>
    </div>
  )
}
