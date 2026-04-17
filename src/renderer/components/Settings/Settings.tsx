import React from 'react'
import { useEffect } from 'react'
import { useSettingsStore } from '@/stores/settings'
import type { SettingsSection } from '@/stores/settings'
import { SectionErrorBoundary } from './SectionErrorBoundary'
import { GeneralSection } from './sections/GeneralSection'
import { TerminalSection } from './sections/TerminalSection'
import { CodeEditorSettingsSection } from './sections/CodeEditorSettingsSection'
import { EditorsAgentsSection } from './sections/EditorsAgentsSection'
import { KeybindingsSection } from './sections/KeybindingsSection'
import { TimerSection } from './sections/TimerSection'
import { CompanionSection } from './sections/CompanionSection'
import { ProjectsSection } from './sections/ProjectsSection'
import { WorkspaceStatesSection } from './sections/WorkspaceStatesSection'
import { AgentSkillsSection } from './sections/AgentSkillsSection'

// ── Section nav items ────────────────────────────────────────────────
const SECTIONS: { id: SettingsSection; label: string; agenticOnly?: boolean }[] = [
  { id: 'general', label: 'General' },
  { id: 'projects', label: 'Workspaces' },
  { id: 'workspace-states', label: 'Workspace States', agenticOnly: true },
  { id: 'agent-skills', label: 'Agent Skills', agenticOnly: true },
  { id: 'terminal', label: 'Terminal' },
  { id: 'code-editor', label: 'Code Editor' },
  { id: 'editors-agents', label: 'Editors & Agents' },
  { id: 'keybindings', label: 'Keybindings' },
  { id: 'timer', label: 'Timer' },
  { id: 'companion', label: 'Mobile Companion' },
]

// ── Main Settings component ──────────────────────────────────────────
// This component is a router only — each section lives in its own file
// under ./sections/. Navigation callers (update toasts, "jump to
// settings" buttons, workspace-relation shortcuts) use
// `useSettingsStore.setState({ activeSection: '<id>' })` — the section
// IDs here are the stable contract.
export default function Settings(): React.JSX.Element {
  const activeSection = useSettingsStore((s) => s.activeSection)
  const setSection = useSettingsStore((s) => s.setSection)
  const closeSettings = useSettingsStore((s) => s.closeSettings)

  useEffect(() => {
    const handler = (e: KeyboardEvent): void => {
      if (e.key === 'Escape') {
        e.preventDefault()
        closeSettings()
      }
    }
    window.addEventListener('keydown', handler)
    return () => window.removeEventListener('keydown', handler)
  }, [closeSettings])

  return (
    <div className="flex h-full w-full min-h-0 bg-[var(--color-bg)]">
      {/* Left nav */}
      <div className="w-48 flex-shrink-0 border-r border-[var(--color-border)] bg-[var(--color-bg-surface)] flex flex-col min-h-0">
        <div className="px-4 py-3 border-b border-[var(--color-border)] flex-shrink-0">
          <span className="text-xs font-medium text-[var(--color-text-secondary)] uppercase tracking-wider">
            Settings
          </span>
        </div>
        <nav className="flex-1 py-1 overflow-y-auto">
          {SECTIONS.filter((s) => !s.agenticOnly || useSettingsStore.getState().agenticSystemsEnabled).map((s) => (
            <button
              key={s.id}
              onClick={() => setSection(s.id)}
              className={`w-full text-left px-4 py-1.5 text-xs no-drag cursor-pointer transition-colors ${
                activeSection === s.id
                  ? 'bg-[var(--color-bg-elevated)] text-[var(--color-text-primary)]'
                  : 'text-[var(--color-text-secondary)] hover:text-[var(--color-text-primary)] hover:bg-[var(--color-bg-elevated)]'
              }`}
            >
              {s.label}
            </button>
          ))}
        </nav>
        <div className="px-4 py-3 border-t border-[var(--color-border)] flex-shrink-0">
          <button
            onClick={closeSettings}
            className="flex items-center gap-2 text-xs text-[var(--color-text-primary)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
          >
            &larr; Back
            <span className="text-[10px] text-[var(--color-text-muted)]">Esc</span>
          </button>
        </div>
      </div>

      {/* Content area */}
      <div className={`flex-1 min-h-0 relative ${activeSection === 'projects' ? 'overflow-hidden p-0' : 'overflow-y-auto p-6'}`}>
        {activeSection === 'general' && <GeneralSection />}
        {activeSection === 'terminal' && <TerminalSection />}
        {activeSection === 'code-editor' && <CodeEditorSettingsSection />}
        {activeSection === 'editors-agents' && <EditorsAgentsSection />}
        {activeSection === 'keybindings' && <KeybindingsSection />}
        {activeSection === 'timer' && (
          <SectionErrorBoundary>
            <TimerSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'companion' && (
          <SectionErrorBoundary>
            <CompanionSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'projects' && (
          <SectionErrorBoundary>
            <ProjectsSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'workspace-states' && (
          <SectionErrorBoundary>
            <WorkspaceStatesSection />
          </SectionErrorBoundary>
        )}
        {activeSection === 'agent-skills' && (
          <SectionErrorBoundary>
            <AgentSkillsSection />
          </SectionErrorBoundary>
        )}
      </div>
    </div>
  )
}
