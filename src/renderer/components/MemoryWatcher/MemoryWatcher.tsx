import { useEffect, useRef } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useToastStore } from '@/stores/toast'

interface RendererMemoryStatus {
  resident_bytes: number
  virtual_bytes: number
  pid: number
}

/**
 * 0.38.12 — Renderer memory watcher.
 *
 * Polls the daemon-side `renderer_memory_status` Tauri command every
 * 5 minutes and logs the Tauri process's resident memory to the
 * renderer console (`[k2so/memory]`). At 800 MB the watcher surfaces
 * a single warning toast with a 1-hour cooldown so chronic-leak
 * sessions don't drown the user in notifications.
 *
 * Context: C3PO ticket `c9b0d9a9` reported the Tauri WebView
 * resident-memory baseline drifting from ~140 MB → 1+ GB across a
 * day of sustained use, causing macOS RunningBoard to reap the app
 * under Jetsam pressure. WebKit doesn't expose
 * `performance.memory.usedJSHeapSize` (Chromium-only API), so the
 * renderer can't self-measure JS heap directly. The Tauri side has
 * to do it via `proc_pidinfo` and surface the value.
 *
 * The watcher itself is intentionally tiny — one setInterval, one
 * ref for the cooldown timestamp, one console.info per tick. If the
 * watcher itself were leaking we'd be in trouble. Returns null
 * (renders no DOM) and mounts at the app root next to WhatsNewModal.
 */
const POLL_MS = 5 * 60 * 1000 // 5 minutes
const WARN_THRESHOLD_BYTES = 800 * 1024 * 1024 // 800 MB
const TOAST_COOLDOWN_MS = 60 * 60 * 1000 // 1 hour between warning toasts

export default function MemoryWatcher(): React.JSX.Element | null {
  const lastWarnAtRef = useRef<number>(0)
  const addToast = useToastStore((s) => s.addToast)

  useEffect(() => {
    let cancelled = false
    let timer: ReturnType<typeof setInterval> | null = null

    const sample = async (): Promise<void> => {
      if (cancelled) return
      try {
        const status = await invoke<RendererMemoryStatus>('renderer_memory_status')
        if (cancelled) return
        const mb = Math.round(status.resident_bytes / 1024 / 1024)
        const vmb = Math.round(status.virtual_bytes / 1024 / 1024)
        // eslint-disable-next-line no-console
        console.info(
          `[k2so/memory] pid=${status.pid} rss=${mb}MB vsize=${vmb}MB`
        )
        if (
          status.resident_bytes > WARN_THRESHOLD_BYTES &&
          Date.now() - lastWarnAtRef.current > TOAST_COOLDOWN_MS
        ) {
          lastWarnAtRef.current = Date.now()
          addToast(
            `K2SO is using ${mb} MB of memory — consider restarting the app to avoid being reaped by macOS.`,
            'error',
            10_000
          )
        }
      } catch (e) {
        // Silent fail — the watcher is non-critical and shouldn't
        // spam the console if the daemon is briefly unreachable.
        // eslint-disable-next-line no-console
        console.debug('[k2so/memory] sample failed:', e)
      }
    }

    // First sample fires 5s after mount so the renderer can finish
    // its initial layout work before we ask the OS for our RSS.
    // Subsequent samples are on the regular interval.
    const startTimer = setTimeout(() => {
      void sample()
      timer = setInterval(() => {
        void sample()
      }, POLL_MS)
    }, 5_000)

    return () => {
      cancelled = true
      clearTimeout(startTimer)
      if (timer) clearInterval(timer)
    }
  }, [addToast])

  return null
}
