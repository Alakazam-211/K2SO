import { useEffect, useState, useRef, useCallback } from 'react'
import { usePresetsStore } from '@/stores/presets'
import { trpc } from '@/lib/trpc'
import { showContextMenu } from '@/lib/context-menu'
import AgentIcon from '@/components/AgentIcon/AgentIcon'

interface PresetsBarProps {
  cwd: string
}

interface InlineFormState {
  visible: boolean
  editingId: string | null
  label: string
  command: string
  icon: string
}

export function PresetsBar({ cwd }: PresetsBarProps): React.JSX.Element | null {
  const { presets, showPresetsBar, fetchPresets, launchPreset } = usePresetsStore()

  const [form, setForm] = useState<InlineFormState>({
    visible: false,
    editingId: null,
    label: '',
    command: '',
    icon: ''
  })

  const formLabelRef = useRef<HTMLInputElement>(null)

  // Fetch presets on mount
  useEffect(() => {
    fetchPresets()
  }, [fetchPresets])

  // Focus label input when form opens
  useEffect(() => {
    if (form.visible) {
      requestAnimationFrame(() => formLabelRef.current?.focus())
    }
  }, [form.visible])

  const enabledPresets = presets.filter((p) => p.enabled)

  const handleClick = useCallback(
    (presetId: string) => {
      launchPreset(presetId, cwd, 'tab')
    },
    [launchPreset, cwd]
  )

  const handleContextMenu = useCallback(
    async (e: React.MouseEvent, presetId: string) => {
      e.preventDefault()

      const menuItems = [
        { id: 'tab', label: 'Open in New Tab' },
        { id: 'split', label: 'Split Pane' },
        { id: 'separator', label: '', type: 'separator' as const },
        { id: 'edit', label: 'Edit' },
        { id: 'disable', label: 'Disable' }
      ]

      const clickedId = await showContextMenu(menuItems)
      if (!clickedId) return

      switch (clickedId) {
        case 'tab':
          launchPreset(presetId, cwd, 'tab')
          break
        case 'split':
          launchPreset(presetId, cwd, 'split')
          break
        case 'edit': {
          const preset = presets.find((p) => p.id === presetId)
          if (preset) {
            setForm({
              visible: true,
              editingId: presetId,
              label: preset.label,
              command: preset.command,
              icon: preset.icon ?? ''
            })
          }
          break
        }
        case 'disable': {
          await trpc.presets.update.mutate({ id: presetId, enabled: 0 })
          fetchPresets()
          break
        }
      }
    },
    [presets, launchPreset, cwd, fetchPresets]
  )

  const openNewForm = useCallback(() => {
    setForm({ visible: true, editingId: null, label: '', command: '', icon: '' })
  }, [])

  const cancelForm = useCallback(() => {
    setForm({ visible: false, editingId: null, label: '', command: '', icon: '' })
  }, [])

  const submitForm = useCallback(async () => {
    if (!form.label.trim() || !form.command.trim()) return

    try {
      if (form.editingId) {
        await trpc.presets.update.mutate({
          id: form.editingId,
          label: form.label.trim(),
          command: form.command.trim(),
          icon: form.icon.trim() || undefined
        })
      } else {
        await trpc.presets.create.mutate({
          label: form.label.trim(),
          command: form.command.trim(),
          icon: form.icon.trim() || undefined
        })
      }
      cancelForm()
      fetchPresets()
    } catch (err) {
      console.error('Failed to save preset:', err)
    }
  }, [form, cancelForm, fetchPresets])

  const handleFormKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Enter') {
        e.preventDefault()
        submitForm()
      } else if (e.key === 'Escape') {
        e.preventDefault()
        cancelForm()
      }
    },
    [submitForm, cancelForm]
  )

  if (!showPresetsBar) return null

  return (
    <div className="relative flex items-center gap-2 border-b border-[#1e1e1e] bg-[#0d0d0d] px-3 py-1.5">
      {/* Preset buttons */}
      {enabledPresets.map((preset, idx) => (
        <button
          key={preset.id}
          onClick={() => handleClick(preset.id)}
          onContextMenu={(e) => handleContextMenu(e, preset.id)}
          className="flex h-[28px] items-center gap-1.5 px-2.5 text-xs text-[#a0a0a0] transition-colors hover:bg-[#1a1a1a] hover:text-[#e0e0e0] active:bg-[#252525]"
          title={`${preset.label} (Ctrl+${idx + 1})\n${preset.command}`}
        >
          <AgentIcon agent={preset.label} size={14} />
          <span className="truncate">{preset.label}</span>
        </button>
      ))}

      {/* Inline form */}
      {form.visible && (
        <div className="absolute left-0 top-full z-50 flex items-center gap-1 border-b border-[#1e1e1e] bg-[#111111] px-2 py-1.5 shadow-lg"
          onKeyDown={handleFormKeyDown}
        >
          <input
            ref={formLabelRef}
            type="text"
            value={form.icon}
            onChange={(e) => setForm((s) => ({ ...s, icon: e.target.value }))}
            placeholder="Icon"
            className="h-[26px] w-[40px]border border-[#2a2a2a] bg-[#0a0a0a] px-1 text-center text-xs text-[#e0e0e0] placeholder-[#404040] outline-none focus:border-[#3b82f6]"
          />
          <input
            type="text"
            value={form.label}
            onChange={(e) => setForm((s) => ({ ...s, label: e.target.value }))}
            placeholder="Label"
            className="h-[26px] w-[80px]border border-[#2a2a2a] bg-[#0a0a0a] px-1.5 text-xs text-[#e0e0e0] placeholder-[#404040] outline-none focus:border-[#3b82f6]"
          />
          <input
            type="text"
            value={form.command}
            onChange={(e) => setForm((s) => ({ ...s, command: e.target.value }))}
            placeholder="Command (e.g. aider --model gpt-4)"
            className="h-[26px] w-[220px]border border-[#2a2a2a] bg-[#0a0a0a] px-1.5 text-xs text-[#e0e0e0] placeholder-[#404040] outline-none focus:border-[#3b82f6]"
          />
          <button
            onClick={submitForm}
            className="h-[26px]bg-[#3b82f6] px-2 text-xs text-white hover:bg-[#2563eb]"
          >
            {form.editingId ? 'Save' : 'Add'}
          </button>
          <button
            onClick={cancelForm}
            className="h-[26px]px-2 text-xs text-[#808080] hover:text-[#e0e0e0]"
          >
            Cancel
          </button>
        </div>
      )}

    </div>
  )
}
