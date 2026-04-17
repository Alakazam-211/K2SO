import React from 'react'
import { useSettingsStore } from '@/stores/settings'
import { invoke } from '@tauri-apps/api/core'

export function AgenticSystemsToggle(): React.JSX.Element {
  const enabled = useSettingsStore((s) => s.agenticSystemsEnabled)

  const toggle = async () => {
    const newVal = !enabled
    useSettingsStore.setState({ agenticSystemsEnabled: newVal })
    await invoke('settings_update', { updates: { agenticSystemsEnabled: newVal } }).catch(console.error)
  }

  return (
    <div className="flex items-center justify-between py-2 border-b border-[var(--color-border)]">
      <div className="flex-1 min-w-0 mr-3">
        <span className="text-xs text-[var(--color-text-secondary)]">Agentic Systems <span className="text-[9px] text-[var(--color-text-muted)]">(BETA)</span></span>
        <p className="text-[10px] text-[var(--color-text-muted)] mt-0.5">
          {enabled
            ? 'AI agents, workspace manager, heartbeat, and review queue are active'
            : 'Enable to unlock AI agent orchestration across workspaces'}
        </p>
      </div>
      <button
        onClick={toggle}
        className="no-drag cursor-pointer flex-shrink-0 relative"
        style={{
          width: 36,
          height: 20,
          backgroundColor: enabled ? 'var(--color-accent)' : '#333',
          border: 'none',
          transition: 'background-color 150ms',
        }}
      >
        <span
          style={{
            position: 'absolute',
            top: 2,
            left: enabled ? 18 : 2,
            width: 16,
            height: 16,
            backgroundColor: '#fff',
            transition: 'left 150ms',
          }}
        />
      </button>
    </div>
  )
}
