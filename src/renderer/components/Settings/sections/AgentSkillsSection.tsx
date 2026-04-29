import React from 'react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import Markdown from '@/components/Markdown/Markdown'
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
  { key: 'custom_agent', label: 'Custom Agent' },
  { key: 'k2so_agent', label: 'K2SO Agent' },
  { key: 'manager', label: 'Workspace Manager' },
  { key: 'agent_template', label: 'Agent Template' },
]

/// Per-tier tagline surfaced in the "context stack" explanation block so
/// the user sees at-a-glance what this bundle gets injected into.
const TIER_BLURB: Record<SkillTier, string> = {
  custom_agent:
    'every Custom-mode agent K2SO launches (single-agent workspaces that operate autonomously on their own inbox)',
  k2so_agent:
    'the K2SO planner agent — the workspace-local agent that builds PRDs, milestones, and technical plans',
  manager:
    'the workspace manager — the top-of-stack agent that triages inboxes and delegates to sub-agents',
  agent_template:
    'every sub-agent (frontend-eng, rust-eng, qa-eng, etc.) K2SO delegates work to under a manager',
}

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
  const [activeTier, setActiveTier] = useState<SkillTier>('custom_agent')
  const [layers, setLayers] = useState<SkillLayerInfo[]>([])
  const [adding, setAdding] = useState(false)
  const [newName, setNewName] = useState('')
  const [confirmDelete, setConfirmDelete] = useState<string | null>(null)
  const [toast, setToast] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  // Expanded section keys (stable: `locked:<name>` or `user:<filename>`).
  // Multiple rows can be open at once so the user can diff two layers
  // side by side without losing their place. Cleared on tab change.
  const [expanded, setExpanded] = useState<Set<string>>(new Set())
  // Resolved user-layer body cache so expanding a layer twice doesn't
  // re-fetch from disk each time.
  const [userContent, setUserContent] = useState<Record<string, string>>({})

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

  // Clear open sections + cached user bodies on tier change so stale
  // content doesn't leak between tabs.
  useEffect(() => {
    setExpanded(new Set())
    setUserContent({})
  }, [activeTier])

  const toggleExpanded = useCallback((key: string, layer?: SkillLayerInfo) => {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(key)) {
        next.delete(key)
        return next
      }
      next.add(key)
      // If this is a user layer and we haven't cached its body yet,
      // fetch on open. Locked layers use the in-module description map.
      if (layer && userContent[layer.filename] === undefined) {
        invoke<string>('skill_layers_get_content', { tier: activeTier, filename: layer.filename })
          .then((content) => setUserContent((c) => ({
            ...c,
            [layer.filename]: content || '*Empty layer — click Edit to add content.*',
          })))
          .catch(() => setUserContent((c) => ({ ...c, [layer.filename]: '*Failed to load content.*' })))
      }
      return next
    })
  }, [activeTier, userContent])

  const locked = LOCKED_LAYERS[activeTier]
  const activeLabel = SKILL_TABS.find((t) => t.key === activeTier)?.label ?? 'this tier'

  return (
    <div className="max-w-3xl">
      <h2 className="text-sm font-medium text-[var(--color-text-primary)] mb-1">Agent Skills</h2>
      <p className="text-xs text-[var(--color-text-muted)] mb-4">
        Every K2SO agent is launched with a composed system prompt — the auto layers below plus any custom layers you add. Pick a tab to see what's shipped to that specific kind of agent.
      </p>

      {/* Tier tabs */}
      <div className="flex gap-1 mb-4 flex-wrap">
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

      {/* Context-stack explanation. Replaces the prior "click a layer to
          preview" tooltip with a per-tier description so the first thing a
          user reads on this tab explains *what* they're configuring. */}
      <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/30 px-3 py-2.5 mb-3 text-[11px] leading-relaxed text-[var(--color-text-secondary)]">
        <div className="font-medium text-[var(--color-text-primary)] mb-1">
          {activeLabel} context stack
        </div>
        <p>
          This is the stack of layers K2SO composes into the system prompt for{' '}
          <span className="text-[var(--color-text-primary)]">{TIER_BLURB[activeTier]}</span>.
          Auto layers are always included and regenerated on every launch. Custom layers
          — markdown files under <code className="text-[10px] bg-[var(--color-bg-elevated)] px-1 py-0.5 rounded">~/.k2so/templates/</code> — stack on top, so
          anything you add here applies to every workspace that ships a{' '}
          {activeLabel.toLowerCase()}. Click a layer to expand its content inline.
        </p>
      </div>

      {/* Single-column collapsible layer list. Clicking a row toggles
          inline expansion — no more right-side preview panel. Multiple
          rows can be expanded at once for side-by-side comparison. */}
      <div className="border border-[var(--color-border)]">
        {/* Locked (auto-composed) layers */}
        {locked.map((name, i) => {
          const key = `locked:${name}`
          const isOpen = expanded.has(key)
          const description = LOCKED_LAYER_DESCRIPTIONS[name] || `*Auto-generated section: ${name}*`
          return (
            <div key={`locked-${i}`} className="border-b border-[var(--color-border)] last:border-b-0">
              <button
                type="button"
                onClick={() => toggleExpanded(key)}
                className="w-full flex items-center justify-between px-3 py-2 text-left text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] hover:bg-white/[0.03] no-drag cursor-pointer transition-colors"
              >
                <div className="flex items-center gap-2 min-w-0">
                  <span
                    className={`inline-block text-[10px] text-[var(--color-text-muted)] transition-transform flex-shrink-0 w-3 ${
                      isOpen ? 'rotate-90' : ''
                    }`}
                  >
                    ▸
                  </span>
                  <span className="w-1 h-4 bg-[var(--color-text-muted)]/30 rounded-sm flex-shrink-0" />
                  <span className="text-xs truncate">{name}</span>
                </div>
                <span className="text-[10px] italic flex-shrink-0">auto</span>
              </button>
              {isOpen && (
                <div className="px-3 pb-3 pt-1 border-t border-[var(--color-border)]/50 bg-black/[0.15]">
                  <div className="prose prose-invert prose-xs max-w-none text-xs text-[var(--color-text-secondary)] leading-relaxed">
                    <Markdown remarkPlugins={[remarkGfm]}>{description}</Markdown>
                  </div>
                </div>
              )}
            </div>
          )
        })}

        {/* Custom (user-authored) layers */}
        {layers.map((layer) => {
          const key = `user:${layer.filename}`
          const isOpen = expanded.has(key)
          const body = userContent[layer.filename]
          return (
            <div key={layer.filename} className="border-b border-[var(--color-border)] last:border-b-0">
              <div
                className={`flex items-center justify-between px-3 py-2 no-drag cursor-pointer transition-colors ${
                  isOpen ? 'bg-white/[0.03]' : 'hover:bg-white/[0.03]'
                }`}
                onClick={() => toggleExpanded(key, layer)}
              >
                <div className="flex items-center gap-2 min-w-0">
                  <span
                    className={`inline-block text-[10px] text-[var(--color-text-muted)] transition-transform flex-shrink-0 w-3 ${
                      isOpen ? 'rotate-90' : ''
                    }`}
                  >
                    ▸
                  </span>
                  <span className="w-1 h-4 bg-[var(--color-accent)] rounded-sm flex-shrink-0" />
                  <div className="min-w-0">
                    <span className="text-xs text-[var(--color-text-primary)] block truncate">{layer.title}</span>
                    {layer.preview && (
                      <span className="text-[10px] text-[var(--color-text-muted)] block truncate">{layer.preview}</span>
                    )}
                  </div>
                </div>
                <div
                  className="flex items-center gap-2 flex-shrink-0"
                  onClick={(e) => e.stopPropagation()}
                >
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
              {isOpen && (
                <div className="px-3 pb-3 pt-1 border-t border-[var(--color-border)]/50 bg-black/[0.15]">
                  <div className="prose prose-invert prose-xs max-w-none text-xs text-[var(--color-text-secondary)] leading-relaxed">
                    <Markdown remarkPlugins={[remarkGfm]}>
                      {body ?? '*Loading...*'}
                    </Markdown>
                  </div>
                </div>
              )}
            </div>
          )
        })}

        {/* Add-layer inline input / trigger */}
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

      {/* Toast */}
      {toast && (
        <div className="mt-3 px-3 py-1.5 text-[10px] text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] inline-block">
          {toast}
        </div>
      )}
    </div>
  )
}
