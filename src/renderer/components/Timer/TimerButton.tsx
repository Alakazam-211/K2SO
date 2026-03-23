import { useState, useEffect } from 'react'
import { useTimerStore, getElapsedMs, formatElapsed } from '@/stores/timer'

export default function TimerButton(): React.JSX.Element | null {
  const status = useTimerStore((s) => s.status)
  const visible = useTimerStore((s) => s.visible)
  const pausedElapsed = useTimerStore((s) => s.pausedElapsed)
  const resumeTime = useTimerStore((s) => s.resumeTime)
  const beginCountdownOrStart = useTimerStore((s) => s.beginCountdownOrStart)
  const pauseTimer = useTimerStore((s) => s.pauseTimer)
  const resumeTimer = useTimerStore((s) => s.resumeTimer)
  const stopTimer = useTimerStore((s) => s.stopTimer)

  // Re-render every second when running
  const [, setTick] = useState(0)
  useEffect(() => {
    if (status !== 'running') return
    const interval = setInterval(() => setTick((t) => t + 1), 1000)
    return () => clearInterval(interval)
  }, [status])

  if (!visible) return null

  const elapsed = getElapsedMs({ status, pausedElapsed, resumeTime })
  const elapsedText = formatElapsed(elapsed)

  const btnClass =
    'flex h-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors'

  // Idle state: show clock icon
  if (status === 'idle') {
    return (
      <button
        onClick={beginCountdownOrStart}
        className={`${btnClass} w-6`}
        style={{
          // @ts-expect-error -- Tauri-specific CSS property
          WebkitAppRegion: 'no-drag',
        }}
        title="Start timer"
      >
        <svg
          width="14"
          height="14"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="1.8"
          strokeLinecap="round"
          strokeLinejoin="round"
        >
          <circle cx="12" cy="12" r="10" />
          <polyline points="12 6 12 12 16 14" />
        </svg>
      </button>
    )
  }

  // Running or paused: show elapsed time + inline controls
  return (
    <div className="flex items-center gap-0.5">
      {/* Elapsed time display */}
      <span
        className={`text-[11px] font-mono tabular-nums px-1 select-none ${
          status === 'paused' ? 'text-[var(--color-text-muted)] animate-pulse' : 'text-[var(--color-accent)]'
        }`}
      >
        {elapsedText}
      </span>

      {/* Pause / Resume */}
      {status === 'running' ? (
        <button
          onClick={pauseTimer}
          className={`${btnClass} w-5`}
          style={{
            // @ts-expect-error -- Tauri-specific CSS property
            WebkitAppRegion: 'no-drag',
          }}
          title="Pause timer"
        >
          <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" stroke="none">
            <rect x="5" y="3" width="4" height="18" rx="1" />
            <rect x="15" y="3" width="4" height="18" rx="1" />
          </svg>
        </button>
      ) : (
        <button
          onClick={resumeTimer}
          className={`${btnClass} w-5`}
          style={{
            // @ts-expect-error -- Tauri-specific CSS property
            WebkitAppRegion: 'no-drag',
          }}
          title="Resume timer"
        >
          <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" stroke="none">
            <polygon points="5,3 19,12 5,21" />
          </svg>
        </button>
      )}

      {/* Stop */}
      <button
        onClick={stopTimer}
        className={`${btnClass} w-5`}
        style={{
          // @ts-expect-error -- Tauri-specific CSS property
          WebkitAppRegion: 'no-drag',
        }}
        title="Stop timer"
      >
        <svg width="10" height="10" viewBox="0 0 24 24" fill="currentColor" stroke="none">
          <rect x="4" y="4" width="16" height="16" rx="2" />
        </svg>
      </button>
    </div>
  )
}
