import { useEffect, useRef } from 'react'
import { useToastStore } from '@/stores/toast'
import type { Toast as ToastData } from '@/stores/toast'

const BORDER_COLORS: Record<ToastData['type'], string> = {
  success: '#22c55e',
  error: '#ef4444',
  info: '#3b82f6'
}

function ToastItem({ toast }: { toast: ToastData }): React.JSX.Element {
  const removeToast = useToastStore((s) => s.removeToast)
  const progressRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const el = progressRef.current
    if (!el) return
    // Animate the progress bar from full width to zero
    el.style.transition = `width ${toast.duration}ms linear`
    requestAnimationFrame(() => {
      el.style.width = '0%'
    })
  }, [toast.duration])

  return (
    <div
      className="flex items-start gap-2 bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)] text-xs shadow-lg min-w-[240px] max-w-[360px] overflow-hidden"
      style={{ borderLeft: `3px solid ${BORDER_COLORS[toast.type]}` }}
    >
      <div className="flex-1 flex flex-col">
        <div className="px-3 py-2.5 flex flex-col gap-1.5">
          <div className="flex items-start gap-2">
            <span className="flex-1 leading-relaxed">{toast.message}</span>
            <button
              className="flex-shrink-0 w-4 h-4 flex items-center justify-center text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] transition-colors"
              onClick={() => removeToast(toast.id)}
            >
              <svg
                width="8"
                height="8"
                viewBox="0 0 8 8"
                fill="none"
                stroke="currentColor"
                strokeWidth="1.5"
              >
                <line x1="1" y1="1" x2="7" y2="7" />
                <line x1="7" y1="1" x2="1" y2="7" />
              </svg>
            </button>
          </div>
          {toast.action && (
            <button
              className="self-start text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent)]/80 font-mono cursor-pointer transition-colors"
              onClick={() => {
                toast.action!.onClick()
                removeToast(toast.id)
              }}
            >
              {toast.action.label}
            </button>
          )}
        </div>
        <div
          ref={progressRef}
          className="h-[2px] w-full"
          style={{ backgroundColor: BORDER_COLORS[toast.type], opacity: 0.4 }}
        />
      </div>
    </div>
  )
}

export default function Toast(): React.JSX.Element | null {
  const toasts = useToastStore((s) => s.toasts)

  if (toasts.length === 0) return null

  return (
    <div className="fixed bottom-4 left-4 z-[9999] flex flex-col gap-2 pointer-events-auto">
      {toasts.map((toast) => (
        <ToastItem key={toast.id} toast={toast} />
      ))}
    </div>
  )
}
