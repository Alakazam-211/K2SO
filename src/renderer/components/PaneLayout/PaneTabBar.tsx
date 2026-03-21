import { useCallback } from 'react'

// ── Types (from tabs store — will be available after store refactor) ──

interface TerminalItemData {
  terminalId: string
  cwd: string
  command?: string
  args?: string[]
}

interface FileViewerItemData {
  filePath: string
}

interface Item {
  id: string
  type: 'terminal' | 'file-viewer'
  data: TerminalItemData | FileViewerItemData
  pinned?: boolean
}

// ── Helpers ──────────────────────────────────────────────────────────────

function getTabLabel(item: Item): string {
  if (item.type === 'terminal') {
    const data = item.data as TerminalItemData
    if (data.command) {
      // Show the command name (last segment of path)
      const name = data.command.split('/').pop() || data.command
      return name
    }
    return 'Terminal'
  }
  if (item.type === 'file-viewer') {
    const data = item.data as FileViewerItemData
    return data.filePath.split('/').pop() || data.filePath
  }
  return 'Unknown'
}

// ── Props ────────────────────────────────────────────────────────────────

interface PaneTabBarProps {
  items: Item[]
  activeItemIndex: number
  onActivate: (index: number) => void
  onClose: (itemId: string) => void
  onClosePane?: () => void  // Close the entire pane/split
}

// ── Component ────────────────────────────────────────────────────────────

export function PaneTabBar({
  items,
  activeItemIndex,
  onActivate,
  onClose,
  onClosePane
}: PaneTabBarProps): React.JSX.Element {
  const handleClose = useCallback(
    (e: React.MouseEvent, itemId: string) => {
      e.stopPropagation()
      onClose(itemId)
    },
    [onClose]
  )

  return (
    <div
      className="flex items-center border-b border-[var(--color-border)] overflow-x-auto"
      style={{
        height: '24px',
        minHeight: '24px',
        background: '#111',
        fontFamily: "'MesloLGM Nerd Font', Menlo, Monaco, monospace",
        fontSize: '11px'
      }}
    >
      <div className="flex flex-1 items-center overflow-x-auto">
        {items.map((item, index) => {
          const isActive = index === activeItemIndex
          return (
            <div
              key={item.id}
              className={`flex items-center cursor-pointer select-none shrink-0 ${
                isActive
                  ? 'text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
              }`}
              style={{
                height: '24px',
                paddingLeft: '8px',
                paddingRight: '4px',
                background: isActive ? '#1a1a1a' : 'transparent',
                borderBottom: isActive ? '1px solid var(--color-accent)' : '1px solid transparent',
                maxWidth: '160px'
              }}
              onClick={() => onActivate(index)}
            >
              <span className="truncate" style={{ lineHeight: '24px' }}>
                {getTabLabel(item)}
              </span>

              <button
                className="ml-1.5 flex items-center justify-center shrink-0 hover:bg-white/10"
                style={{ width: '14px', height: '14px' }}
                onClick={(e) => handleClose(e, item.id)}
                title="Close tab"
              >
                <svg
                  width="7"
                  height="7"
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
          )
        })}
      </div>

      {/* Close pane button — removes this split */}
      {onClosePane && (
        <button
          className="flex items-center justify-center shrink-0 text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/10"
          style={{ width: '24px', height: '24px' }}
          onClick={(e) => {
            e.stopPropagation()
            onClosePane()
          }}
          title="Close pane"
        >
          <svg
            width="10"
            height="10"
            viewBox="0 0 10 10"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.5"
          >
            <line x1="1" y1="1" x2="9" y2="9" />
            <line x1="9" y1="1" x2="1" y2="9" />
          </svg>
        </button>
      )}
    </div>
  )
}
