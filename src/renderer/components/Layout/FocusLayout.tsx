import { type ReactNode } from 'react'
import { TOPBAR_HEIGHT } from '../../../shared/constants'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { usePanelsStore } from '../../stores/panels'

interface FocusLayoutProps {
  children: ReactNode
  projectName?: string
  workspaceBar?: ReactNode
  leftPanel?: ReactNode
  rightPanel?: ReactNode
}

export default function FocusLayout({
  children,
  projectName,
  workspaceBar,
  leftPanel,
  rightPanel
}: FocusLayoutProps): React.JSX.Element {
  const leftPanelOpen = usePanelsStore((s) => s.leftPanelOpen)
  const rightPanelOpen = usePanelsStore((s) => s.rightPanelOpen)
  const toggleLeftPanel = usePanelsStore((s) => s.toggleLeftPanel)
  const toggleRightPanel = usePanelsStore((s) => s.toggleRightPanel)

  return (
    <div className="flex h-screen w-screen flex-col overflow-hidden bg-[var(--color-bg)]">
      {/* Top bar — no primary sidebar toggle, but has left/right panel toggles */}
      <div
        className="flex items-center justify-between border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 select-none"
        data-tauri-drag-region
        onMouseDown={(e) => {
          if ((e.target as HTMLElement).closest('button, input, select, .no-drag')) return
          getCurrentWindow().startDragging()
        }}
        style={{
          height: TOPBAR_HEIGHT,
          minHeight: TOPBAR_HEIGHT
        }}
      >
        {/* Left: traffic lights spacer */}
        <div style={{ width: 70 }} />

        {/* Center: workspace name */}
        <div className="flex items-center gap-1.5 text-xs">
          {projectName ? (
            <span className="text-[var(--color-text-primary)] font-medium">{projectName}</span>
          ) : (
            <span className="text-[var(--color-text-muted)]">Focus Window</span>
          )}
        </div>

        {/* Right: left/right panel toggles */}
        <div className="flex items-center gap-1">
          <button
            onClick={toggleLeftPanel}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
            style={{
              // @ts-expect-error -- Electron-specific CSS property
              WebkitAppRegion: 'no-drag'
            }}
            title="Toggle left panel"
          >
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
              {leftPanelOpen ? (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="5.5" y1="2" x2="5.5" y2="12" />
                  <line x1="3" y1="5" x2="3" y2="9" strokeWidth="1.5" />
                </>
              ) : (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="5.5" y1="2" x2="5.5" y2="12" strokeDasharray="1.5 1.5" />
                </>
              )}
            </svg>
          </button>

          <button
            onClick={toggleRightPanel}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
            style={{
              // @ts-expect-error -- Electron-specific CSS property
              WebkitAppRegion: 'no-drag'
            }}
            title="Toggle right panel"
          >
            <svg width="14" height="14" viewBox="0 0 14 14" fill="none" stroke="currentColor" strokeWidth="1.3" strokeLinecap="round" strokeLinejoin="round">
              {rightPanelOpen ? (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="8.5" y1="2" x2="8.5" y2="12" />
                  <line x1="11" y1="5" x2="11" y2="9" strokeWidth="1.5" />
                </>
              ) : (
                <>
                  <rect x="1" y="2" width="12" height="10" rx="0" />
                  <line x1="8.5" y1="2" x2="8.5" y2="12" strokeDasharray="1.5 1.5" />
                </>
              )}
            </svg>
          </button>
        </div>
      </div>

      {/* Worktree bar */}
      {workspaceBar}

      {/* Content area with optional left/right panels */}
      <div className="flex flex-1 overflow-hidden">
        {/* Left auxiliary panel */}
        {leftPanelOpen && leftPanel && (
          <div className="flex-shrink-0 border-r border-[var(--color-border)]">
            {leftPanel}
          </div>
        )}

        {/* Main content (terminal) */}
        <div className="flex-1 overflow-hidden">{children}</div>

        {/* Right auxiliary panel */}
        {rightPanelOpen && rightPanel && (
          <div className="flex-shrink-0 border-l border-[var(--color-border)]">
            {rightPanel}
          </div>
        )}
      </div>
    </div>
  )
}
