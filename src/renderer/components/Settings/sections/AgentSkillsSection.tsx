import React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import type { SettingEntry } from '../searchManifest'
import { AgentContextDiagram } from './AgentContextDiagram'

export const AGENT_SKILLS_MANIFEST: SettingEntry[] = [
  { id: 'agent-skills.manager', section: 'agent-skills', label: 'Workspace Manager Skills', description: 'Auto-generated + custom skill layers for the workspace manager', keywords: ['manager', 'skills', 'workspace', 'triage', 'delegate'] },
  { id: 'agent-skills.k2so-agent', section: 'agent-skills', label: 'K2SO Agent Skills', description: 'Skill layers for the K2SO planner agent (PRDs, milestones, technical plans)', keywords: ['k2so', 'agent', 'planner', 'prd', 'milestone', 'skills'] },
  { id: 'agent-skills.agent-template', section: 'agent-skills', label: 'Agent Template Skills', description: 'Skill layers shared by every team member agent', keywords: ['template', 'skills', 'agent', 'checkin'] },
  { id: 'agent-skills.custom-agent', section: 'agent-skills', label: 'Custom Agent Skills', description: 'Skill layers for custom / heartbeat-driven agents', keywords: ['custom', 'skills', 'agent', 'cross-workspace'] },
  { id: 'agent-skills.add-layer', section: 'agent-skills', label: 'Add Skill Layer', description: 'Create a new markdown skill layer', keywords: ['add', 'new', 'layer', 'skill'] },
]

type SkillTier = 'manager' | 'k2so_agent' | 'agent_template' | 'custom_agent'

interface SkillLayerInfo {
  filename: string
  title: string
  preview: string
  path: string
}

const SKILL_TABS: { key: SkillTier; label: string }[] = [
  { key: 'manager', label: 'Workspace Manager' },
  { key: 'k2so_agent', label: 'K2SO Agent' },
  { key: 'agent_template', label: 'Agent Template' },
  { key: 'custom_agent', label: 'Custom Agent' },
]

const LOCKED_LAYERS: Record<SkillTier, string[]> = {
  manager: [
    'Identity + Workspace State',
    'Connected Workspaces',
    'Team Roster',
    'Standing Orders',
    'Decision Framework',
    'Delegation + Review',
    'Communication Commands',
  ],
  k2so_agent: [
    'Identity',
    'Every Wake (k2so checkin)',
    'Report + Complete',
    'Planning (PRDs + Milestones)',
    'Your Own Heartbeats',
    'Cross-Workspace Messaging',
    'File Reservations',
    'Settings + Diagnostic',
  ],
  agent_template: [
    'Identity',
    'Check In + Status + Done',
    'File Reservations',
  ],
  custom_agent: [
    'Identity',
    'Check In + Status + Done',
    'Cross-Workspace Messaging',
    'File Reservations',
  ],
}

// Static content descriptions for locked layers (shown in preview)
const LOCKED_LAYER_DESCRIPTIONS: Record<string, string> = {
  'Identity + Workspace State': '**Auto-generated per workspace.**\n\nIncludes the workspace name, current mode (Build/Managed/Maintenance/Locked), and mode description. Each workspace gets unique identity context.',
  'Connected Workspaces': '**Auto-generated per workspace.**\n\nLists workspaces connected via workspace relations — both outgoing (workspaces this manager oversees) and incoming (agents that communicate with this workspace).',
  'Team Roster': '**Auto-generated per workspace.**\n\nLists all agent templates in this workspace with their names and roles. The manager uses this to decide which specialist to delegate work to.',
  'Standing Orders': '**Auto-generated (same for all managers).**\n\n9-step checklist run on every wake cycle:\n1. `k2so checkin`\n2. Triage messages\n3. Triage work items by priority\n4. Handle simple tasks directly\n5. Delegate complex tasks\n6. Check active agents\n7. Review completed work\n8. Update status\n9. Mark done or blocked',
  'Decision Framework': '**Auto-generated (same for all managers).**\n\nTwo decision axes:\n- **By complexity**: Simple (work directly) vs Complex (delegate)\n- **By workspace mode**: Build (full autonomy), Managed (features need approval), Maintenance (bugs only), Locked (no activity)',
  'Delegation + Review': '**Auto-generated (same for all managers).**\n\nDelegation: choose agent → create work item → `k2so delegate` → agent works in worktree → review.\n\nReview: `k2so review approve/reject/feedback` for completed agent work.',
  'Communication Commands': '**Auto-generated (same for all managers).**\n\nCore commands: `k2so checkin`, `k2so status`, `k2so done`, `k2so msg`, `k2so reserve`, `k2so release`.',
  'Identity': '**Auto-generated per agent.**\n\nThe agent name and workspace it belongs to.',
  'Check In + Status + Done': '**Auto-generated (same for all).**\n\n`k2so checkin` — wake up briefing\n`k2so status "msg"` — report progress\n`k2so done` / `k2so done --blocked "reason"` — complete or block task',
  'File Reservations': '**Auto-generated (same for all).**\n\n`k2so reserve <paths>` — claim files for exclusive editing\n`k2so release` — release claims',
  'Cross-Workspace Messaging': '**Auto-generated (same for all custom agents).**\n\n`k2so msg <workspace>:inbox "text"` — send work to connected workspaces\n`k2so msg --wake` — urgent delivery with agent wake-up',
}

export function AgentSkillsSection(): React.JSX.Element {
  const [activeTier, setActiveTier] = useState<SkillTier>('manager')
  const [layers, setLayers] = useState<SkillLayerInfo[]>([])
  const [adding, setAdding] = useState(false)
  const [newName, setNewName] = useState('')
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)
  const [toast, setToast] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const [selectedLayer, setSelectedLayer] = useState<{ type: 'locked' | 'user'; name: string; filename?: string } | null>(null)
  const [previewContent, setPreviewContent] = useState<string>('')

  const loadLayers = useCallback(async (tier: SkillTier) => {
    try {
      const list = await invoke<SkillLayerInfo[]>('skill_layers_list', { tier })
      setLayers(list)
    } catch (err) {
      console.error('[agent-skills] Failed to load layers:', err)
      setLayers([])
    }
  }, [])

  useEffect(() => {
    loadLayers(activeTier)
  }, [activeTier, loadLayers])

  useEffect(() => {
    if (adding && inputRef.current) {
      inputRef.current.focus()
    }
  }, [adding])

  useEffect(() => {
    if (toast) {
      const t = setTimeout(() => setToast(null), 2000)
      return () => clearTimeout(t)
    }
  }, [toast])

  const handleCreate = useCallback(async () => {
    const name = newName.trim()
    if (!name) return
    try {
      await invoke<SkillLayerInfo>('skill_layers_create', { tier: activeTier, name })
      setNewName('')
      setAdding(false)
      loadLayers(activeTier)
    } catch (err) {
      console.error('[agent-skills] Create failed:', err)
    }
  }, [newName, activeTier, loadLayers])

  const handleDelete = useCallback(async (filename: string) => {
    try {
      await invoke('skill_layers_delete', { tier: activeTier, filename })
      setConfirmDelete(null)
      loadLayers(activeTier)
    } catch (err) {
      console.error('[agent-skills] Delete failed:', err)
    }
  }, [activeTier, loadLayers])

  const handleEdit = useCallback((layer: SkillLayerInfo) => {
    navigator.clipboard.writeText(layer.path).then(() => {
      setToast('Copied path — open in your editor')
    }).catch(() => {
      setToast(layer.path)
    })
  }, [])

  // Load preview content when a layer is selected
  useEffect(() => {
    if (!selectedLayer) { setPreviewContent(''); return }
    if (selectedLayer.type === 'locked') {
      setPreviewContent(LOCKED_LAYER_DESCRIPTIONS[selectedLayer.name] || `*Auto-generated section: ${selectedLayer.name}*`)
    } else if (selectedLayer.filename) {
      invoke<string>('skill_layers_get_content', { tier: activeTier, filename: selectedLayer.filename })
        .then((content) => setPreviewContent(content || '*Empty layer — click Edit to add content.*'))
        .catch(() => setPreviewContent('*Failed to load content.*'))
    }
  }, [selectedLayer, activeTier])

  // Clear selection on tier change
  useEffect(() => { setSelectedLayer(null) }, [activeTier])

  const locked = LOCKED_LAYERS[activeTier]

  return (
    <div className="max-w-3xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Agent Skills</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-4">
        Skill layers are injected into agent system prompts. Click a layer to preview its content.
      </p>

      {/* Tier tabs */}
      <div className="flex gap-1 mb-4">
        {SKILL_TABS.map(({ key, label }) => (
          <button
            key={key}
            onClick={() => { setActiveTier(key); setAdding(false); setConfirmDelete(null) }}
            className={`px-3 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
              activeTier === key
                ? 'bg-[var(--color-accent)] text-white'
                : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)]'
            }`}
          >
            {label}
          </button>
        ))}
      </div>

      {/* Context flow diagram — changes per tier so users can see where each
          file ends up at launch / wake. */}
      <AgentContextDiagram tier={activeTier} />

      {/* Split layout: layer list + preview */}
      <div className="flex gap-3">
        {/* Left: Hamburger layer list */}
        <div className="border border-[var(--color-border)] flex-1 min-w-0">
        {/* Locked layers */}
        {locked.map((name, i) => {
          const isSelected = selectedLayer?.type === 'locked' && selectedLayer.name === name
          return (
          <div
            key={`locked-${i}`}
            className={`flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] last:border-b-0 cursor-pointer transition-colors ${
              isSelected ? 'bg-white/[0.06] text-[var(--color-text-secondary)]' : 'text-[var(--color-text-muted)] opacity-50 hover:opacity-70'
            }`}
            onClick={() => setSelectedLayer({ type: 'locked', name })}
          >
            <div className="flex items-center gap-2">
              <span className="w-1 h-4 bg-[var(--color-text-muted)]/30 rounded-sm flex-shrink-0" />
              <span className="text-xs">{name}</span>
            </div>
            <span className="text-[10px] italic">auto</span>
          </div>
          )
        })}

        {/* User layers */}
        {layers.map((layer) => {
          const isSelected = selectedLayer?.type === 'user' && selectedLayer.filename === layer.filename
          return (
          <div
            key={layer.filename}
            className={`flex items-center justify-between px-3 py-2 border-b border-[var(--color-border)] last:border-b-0 cursor-pointer transition-colors ${
              isSelected ? 'bg-white/[0.06]' : 'hover:bg-white/[0.03]'
            }`}
            onClick={() => setSelectedLayer({ type: 'user', name: layer.title, filename: layer.filename })}
          >
            <div className="flex items-center gap-2 min-w-0">
              <span className="w-1 h-4 bg-[var(--color-accent)] rounded-sm flex-shrink-0" />
              <div className="min-w-0">
                <span className="text-xs text-[var(--color-text-primary)] block truncate">{layer.title}</span>
                {layer.preview && (
                  <span className="text-[10px] text-[var(--color-text-muted)] block truncate">{layer.preview}</span>
                )}
              </div>
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {confirmDelete === layer.filename ? (
                <>
                  <span className="text-[10px] text-[var(--color-text-muted)]">Delete?</span>
                  <button
                    onClick={() => handleDelete(layer.filename)}
                    className="text-[10px] text-red-400 hover:text-red-300 no-drag cursor-pointer"
                  >
                    Yes
                  </button>
                  <button
                    onClick={() => setConfirmDelete(null)}
                    className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
                  >
                    No
                  </button>
                </>
              ) : (
                <>
                  <button
                    onClick={() => handleEdit(layer)}
                    className="text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent-hover)] no-drag cursor-pointer"
                  >
                    Edit
                  </button>
                  <button
                    onClick={() => setConfirmDelete(layer.filename)}
                    className="text-[10px] text-red-400 hover:text-red-300 no-drag cursor-pointer"
                  >
                    Delete
                  </button>
                </>
              )}
            </div>
          </div>
          )
        })}

        {/* Add layer inline input */}
        {adding ? (
          <div className="flex items-center gap-2 px-3 py-2 border-b border-[var(--color-border)] last:border-b-0">
            <input
              ref={inputRef}
              type="text"
              value={newName}
              onChange={(e) => setNewName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') handleCreate()
                if (e.key === 'Escape') { setAdding(false); setNewName('') }
              }}
              placeholder="Layer name..."
              className="flex-1 text-xs bg-transparent border border-[var(--color-border)] px-2 py-1 text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none focus:border-[var(--color-accent)]"
            />
            <button
              onClick={handleCreate}
              className="text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent-hover)] no-drag cursor-pointer"
            >
              Create
            </button>
            <button
              onClick={() => { setAdding(false); setNewName('') }}
              className="text-[10px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] no-drag cursor-pointer"
            >
              Cancel
            </button>
          </div>
        ) : (
          <button
            onClick={() => setAdding(true)}
            className="w-full text-left px-3 py-2 text-[10px] text-[var(--color-accent)] hover:bg-[var(--color-bg-elevated)] no-drag cursor-pointer transition-colors"
          >
            + Add Layer
          </button>
        )}
        </div>

        {/* Right: Preview panel */}
        <div className="w-64 flex-shrink-0 border border-[var(--color-border)] flex flex-col min-h-[300px]">
          {selectedLayer ? (
            <>
              <div className="px-3 py-2 border-b border-[var(--color-border)] flex items-center justify-between flex-shrink-0">
                <div>
                  <span className="text-xs font-medium text-[var(--color-text-primary)]">{selectedLayer.name}</span>
                  <span className={`ml-2 text-[10px] ${selectedLayer.type === 'locked' ? 'text-[var(--color-text-muted)] italic' : 'text-[var(--color-accent)]'}`}>
                    {selectedLayer.type === 'locked' ? 'auto' : 'custom'}
                  </span>
                </div>
                {selectedLayer.type === 'user' && selectedLayer.filename && (
                  <button
                    onClick={() => {
                      const layer = layers.find((l) => l.filename === selectedLayer.filename)
                      if (layer) handleEdit(layer)
                    }}
                    className="px-2 py-0.5 text-[10px] text-white bg-[var(--color-accent)] hover:opacity-90 no-drag cursor-pointer"
                  >
                    Edit
                  </button>
                )}
              </div>
              <div className="flex-1 overflow-y-auto px-3 py-2">
                <div className="prose prose-invert prose-xs max-w-none text-xs text-[var(--color-text-secondary)] leading-relaxed">
                  <ReactMarkdown remarkPlugins={[remarkGfm]}>{previewContent}</ReactMarkdown>
                </div>
              </div>
            </>
          ) : (
            <div className="flex items-center justify-center h-full text-[10px] text-[var(--color-text-muted)]">
              Click a layer to preview
            </div>
          )}
        </div>
      </div>

      {/* Toast */}
      {toast && (
        <div className="mt-3 px-3 py-1.5 text-[10px] text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] inline-block">
          {toast}
        </div>
      )}
    </div>
  )
}
