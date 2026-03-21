import { type ReactNode, useCallback, useRef } from 'react'
import {
  SIDEBAR_MIN_WIDTH,
  SIDEBAR_MAX_WIDTH,
  SIDEBAR_DEFAULT_WIDTH
} from '../../../shared/constants'

type PanelTab = 'files' | 'changes'

const TAB_LABELS: Record<PanelTab, string> = {
  files: 'Files',
  changes: 'Changes'
}

interface TabbedPanelProps {
  tabs: PanelTab[]
  activeTab: PanelTab
  onTabChange: (tab: PanelTab) => void
  width: number
  onWidthChange: (width: number) => void
  /** Which side the resize handle is on */
  resizeSide: 'left' | 'right'
  children: ReactNode
}

function PanelResizeHandle({
  side,
  onWidthChange,
  currentWidth
}: {
  side: 'left' | 'right'
  onWidthChange: (width: number) => void
  currentWidth: number
}): React.JSX.Element {
  const isDragging = useRef(false)

  const handleMouseDown = useCallback(
    (e: React.MouseEvent) => {
      e.preventDefault()
      isDragging.current = true

      const startX = e.clientX
      const startWidth = currentWidth

      const handleMouseMove = (moveEvent: MouseEvent): void => {
        if (!isDragging.current) return
        const delta = moveEvent.clientX - startX
        const newWidth = side === 'right'
          ? Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, startWidth - delta))
          : Math.max(SIDEBAR_MIN_WIDTH, Math.min(SIDEBAR_MAX_WIDTH, startWidth + delta))
        onWidthChange(newWidth)
      }

      const handleMouseUp = (): void => {
        isDragging.current = false
        document.removeEventListener('mousemove', handleMouseMove)
        document.removeEventListener('mouseup', handleMouseUp)
        document.body.style.cursor = ''
        document.body.style.userSelect = ''
      }

      document.addEventListener('mousemove', handleMouseMove)
      document.addEventListener('mouseup', handleMouseUp)
      document.body.style.cursor = 'col-resize'
      document.body.style.userSelect = 'none'
    },
    [onWidthChange, currentWidth, side]
  )

  const handleDoubleClick = useCallback(() => {
    onWidthChange(SIDEBAR_DEFAULT_WIDTH)
  }, [onWidthChange])

  return (
    <div
      className={`absolute top-0 bottom-0 w-1 cursor-col-resize hover:bg-[var(--color-accent)] transition-colors duration-150 z-10 no-drag ${
        side === 'left' ? 'left-0' : 'right-0'
      }`}
      onMouseDown={handleMouseDown}
      onDoubleClick={handleDoubleClick}
    />
  )
}

export default function TabbedPanel({
  tabs,
  activeTab,
  onTabChange,
  width,
  onWidthChange,
  resizeSide,
  children
}: TabbedPanelProps): React.JSX.Element {
  return (
    <div
      className="relative flex flex-col h-full bg-[var(--color-bg-surface)] overflow-hidden"
      style={{ width }}
    >
      <PanelResizeHandle side={resizeSide} onWidthChange={onWidthChange} currentWidth={width} />

      {/* Tab strip */}
      <div className="flex border-b border-[var(--color-border)] flex-shrink-0">
        {tabs.map((tab) => (
          <button
            key={tab}
            className={`no-drag flex-1 px-3 py-1.5 text-[11px] font-medium tracking-wide uppercase transition-colors ${
              activeTab === tab
                ? 'text-[var(--color-text-primary)] border-b border-[var(--color-accent)]'
                : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
            }`}
            onClick={() => onTabChange(tab)}
          >
            {TAB_LABELS[tab]}
          </button>
        ))}
      </div>

      {/* Tab content */}
      <div className="flex-1 overflow-hidden">{children}</div>
    </div>
  )
}
