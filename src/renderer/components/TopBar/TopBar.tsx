import { useEffect, useState } from 'react'
import { TOPBAR_HEIGHT } from '../../../shared/constants'
import { trpc } from '@/lib/trpc'
import { useSettingsStore } from '@/stores/settings'

interface TopBarProps {
  projectName?: string
  projectPath?: string
  workspaceName?: string
  primarySidebarVisible?: boolean
  leftPanelVisible?: boolean
  rightPanelVisible?: boolean
  onTogglePrimarySidebar?: () => void
  onToggleLeftPanel?: () => void
  onToggleRightPanel?: () => void
  onRunCommand?: (command: string) => void
}

export default function TopBar({
  projectName,
  projectPath,
  workspaceName,
  primarySidebarVisible = true,
  leftPanelVisible = false,
  rightPanelVisible = false,
  onTogglePrimarySidebar,
  onToggleLeftPanel,
  onToggleRightPanel,
  onRunCommand
}: TopBarProps): React.JSX.Element {
  const [hasRun, setHasRun] = useState(false)

  useEffect(() => {
    if (!projectPath) {
      setHasRun(false)
      return
    }

    let cancelled = false
    trpc.projectConfig.hasRunCommand
      .query({ path: projectPath })
      .then((result) => {
        if (!cancelled) setHasRun(result)
      })
      .catch(() => {
        if (!cancelled) setHasRun(false)
      })

    return () => {
      cancelled = true
    }
  }, [projectPath])

  const handleRun = async (): Promise<void> => {
    if (!projectPath || !onRunCommand) return
    try {
      const result = await trpc.projectConfig.runCommand.mutate({ path: projectPath })
      onRunCommand(result.command)
    } catch {
      // No run command configured
    }
  }
  return (
    <div
      className="flex items-center justify-between border-b border-[var(--color-border)] bg-[var(--color-bg-surface)] px-3 select-none"
      style={{
        height: TOPBAR_HEIGHT,
        minHeight: TOPBAR_HEIGHT,
        // @ts-expect-error -- Electron-specific CSS property
        WebkitAppRegion: 'drag'
      }}
    >
      {/* Left: traffic lights spacer + K2SO branding + primary sidebar toggle */}
      <div className="flex items-center gap-2" style={{ minWidth: 130 }}>
        {/* Traffic lights occupy ~70px on macOS */}
        <div style={{ width: 70 }} />
        {/* App name */}
        <span className="text-[10px] font-bold tracking-widest text-[var(--color-text-muted)] uppercase flex-shrink-0">K2SO</span>
        {/* Primary sidebar toggle */}
        <button
          onClick={onTogglePrimarySidebar}
          className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
          style={{
            // @ts-expect-error -- Electron-specific CSS property
            WebkitAppRegion: 'no-drag'
          }}
          title="Toggle workspaces sidebar"
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 14 14"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            {primarySidebarVisible ? (
              <>
                <rect x="1" y="2" width="12" height="10" rx="0" />
                <line x1="5" y1="2" x2="5" y2="12" />
              </>
            ) : (
              <>
                <rect x="1" y="2" width="12" height="10" rx="0" />
                <line x1="5" y1="2" x2="5" y2="12" strokeDasharray="1.5 1.5" />
              </>
            )}
          </svg>
        </button>
        {/* Settings gear */}
        <button
          onClick={() => useSettingsStore.getState().openSettings()}
          className="flex h-6 w-6 items-center justify-center text-[var(--color-text-muted)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
          style={{
            // @ts-expect-error -- Electron-specific CSS property
            WebkitAppRegion: 'no-drag'
          }}
          title="Settings (⌘,)"
        >
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round">
            <circle cx="12" cy="12" r="3" />
            <path d="M19.4 15a1.65 1.65 0 00.33 1.82l.06.06a2 2 0 010 2.83 2 2 0 01-2.83 0l-.06-.06a1.65 1.65 0 00-1.82-.33 1.65 1.65 0 00-1 1.51V21a2 2 0 01-4 0v-.09A1.65 1.65 0 009 19.4a1.65 1.65 0 00-1.82.33l-.06.06a2 2 0 01-2.83-2.83l.06-.06A1.65 1.65 0 004.68 15a1.65 1.65 0 00-1.51-1H3a2 2 0 010-4h.09A1.65 1.65 0 004.6 9a1.65 1.65 0 00-.33-1.82l-.06-.06a2 2 0 012.83-2.83l.06.06A1.65 1.65 0 009 4.68a1.65 1.65 0 001-1.51V3a2 2 0 014 0v.09a1.65 1.65 0 001 1.51 1.65 1.65 0 001.82-.33l.06-.06a2 2 0 012.83 2.83l-.06.06A1.65 1.65 0 0019.4 9a1.65 1.65 0 001.51 1H21a2 2 0 010 4h-.09a1.65 1.65 0 00-1.51 1z" />
          </svg>
        </button>
      </div>

      {/* Center: workspace + worktree name */}
      <div className="flex items-center gap-1.5 text-xs">
        {projectName ? (
          <>
            <span className="text-[var(--color-text-secondary)]">{projectName}</span>
            {workspaceName && (
              <>
                <span className="text-[var(--color-text-muted)]">/</span>
                <span className="text-[var(--color-text-primary)] font-medium">
                  {workspaceName}
                </span>
              </>
            )}
          </>
        ) : (
          <span className="text-[var(--color-text-muted)]">No workspace selected</span>
        )}
      </div>

      {/* Right: run button + panel toggles */}
      <div className="flex items-center gap-1">
        {/* Run command button — only visible when project has a run command */}
        {hasRun && (
          <button
            onClick={handleRun}
            className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[#4ec9b0] transition-colors"
            style={{
              // @ts-expect-error -- Electron-specific CSS property
              WebkitAppRegion: 'no-drag'
            }}
            title="Run workspace command"
          >
            <svg
              width="12"
              height="12"
              viewBox="0 0 12 12"
              fill="currentColor"
              stroke="none"
            >
              <polygon points="2,0 2,12 11,6" />
            </svg>
          </button>
        )}

        {/* Left panel toggle (opens panel to the left of terminal) */}
        <button
          onClick={onToggleLeftPanel}
          className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
          style={{
            // @ts-expect-error -- Electron-specific CSS property
            WebkitAppRegion: 'no-drag'
          }}
          title="Toggle left panel"
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 14 14"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            {leftPanelVisible ? (
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

        {/* Right panel toggle (opens panel to the right of terminal) */}
        <button
          onClick={onToggleRightPanel}
          className="flex h-6 w-6 items-center justify-center text-[var(--color-text-secondary)] hover:bg-[var(--color-bg-elevated)] hover:text-[var(--color-text-primary)] transition-colors"
          style={{
            // @ts-expect-error -- Electron-specific CSS property
            WebkitAppRegion: 'no-drag'
          }}
          title="Toggle right panel"
        >
          <svg
            width="14"
            height="14"
            viewBox="0 0 14 14"
            fill="none"
            stroke="currentColor"
            strokeWidth="1.3"
            strokeLinecap="round"
            strokeLinejoin="round"
          >
            {rightPanelVisible ? (
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
  )
}
