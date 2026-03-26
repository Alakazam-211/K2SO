import { useState, useEffect, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useTabsStore } from '@/stores/tabs'

// ── Types ───────────────────────────────────────────────────────────────

interface WorkItem {
  filename: string
  title: string
  priority: string
  assignedBy: string
  created: string
  itemType: string
  folder: string
}

interface AgentProfile {
  name: string
  role: string
  podLeader: boolean
  raw: string // full agent.md content
}

// ── Props ───────────────────────────────────────────────────────────────

interface AgentPaneProps {
  agentName: string
  projectPath: string
  onClose?: () => void
}

// ── Component ───────────────────────────────────────────────────────────

export function AgentPane({ agentName, projectPath }: AgentPaneProps): React.JSX.Element {
  const [profile, setProfile] = useState<AgentProfile | null>(null)
  const [claudeMd, setClaudeMd] = useState<string>('')
  const [workItems, setWorkItems] = useState<WorkItem[]>([])
  const [activeSection, setActiveSection] = useState<'profile' | 'claude-md' | 'work'>('profile')

  const agentDir = `${projectPath}/.k2so/agents/${agentName}`

  const fetchProfile = useCallback(async () => {
    try {
      const content = await invoke<{ content: string }>('k2so_agents_get_profile', {
        projectPath,
        agentName,
      })
      // Parse frontmatter
      const raw = content.content || ''
      const fmMatch = raw.match(/^---\n([\s\S]*?)\n---/)
      let name = agentName
      let role = ''
      let podLeader = false
      if (fmMatch) {
        const fm = fmMatch[1]
        const nameMatch = fm.match(/^name:\s*(.+)$/m)
        const roleMatch = fm.match(/^role:\s*(.+)$/m)
        const leaderMatch = fm.match(/^pod_leader:\s*(.+)$/m)
        if (nameMatch) name = nameMatch[1].trim()
        if (roleMatch) role = roleMatch[1].trim()
        if (leaderMatch) podLeader = leaderMatch[1].trim() === 'true'
      }
      setProfile({ name, role, podLeader, raw })
    } catch {
      setProfile(null)
    }
  }, [projectPath, agentName])

  const fetchClaudeMd = useCallback(async () => {
    try {
      const content = await invoke<string>('k2so_agents_generate_claude_md', {
        projectPath,
        agentName,
      })
      setClaudeMd(content)
    } catch {
      setClaudeMd('')
    }
  }, [projectPath, agentName])

  const fetchWork = useCallback(async () => {
    try {
      const items = await invoke<WorkItem[]>('k2so_agents_work_list', {
        projectPath,
        agentName,
        folder: null,
      })
      setWorkItems(items)
    } catch {
      setWorkItems([])
    }
  }, [projectPath, agentName])

  useEffect(() => {
    fetchProfile()
    fetchClaudeMd()
    fetchWork()
    const interval = setInterval(fetchWork, 10_000)
    return () => clearInterval(interval)
  }, [fetchProfile, fetchClaudeMd, fetchWork])

  const openFile = (filePath: string) => {
    const tab = useTabsStore.getState().getActiveTab()
    if (tab) {
      useTabsStore.getState().openFileInPane(tab.id, filePath)
    }
  }

  const priorityColor = (p: string) => {
    if (p === 'critical') return 'text-red-400'
    if (p === 'high') return 'text-orange-400'
    if (p === 'low') return 'text-[var(--color-text-muted)]'
    return 'text-[var(--color-text-secondary)]'
  }

  const folderColor = (f: string) => {
    if (f === 'active') return 'text-yellow-400'
    if (f === 'done') return 'text-green-400'
    return 'text-[var(--color-text-muted)]'
  }

  const inbox = workItems.filter((w) => w.folder === 'inbox')
  const active = workItems.filter((w) => w.folder === 'active')
  const done = workItems.filter((w) => w.folder === 'done')

  return (
    <div className="h-full flex flex-col bg-[var(--color-bg-primary)] overflow-hidden">
      {/* Header */}
      <div className="px-4 py-3 border-b border-[var(--color-border)] flex-shrink-0">
        <div className="flex items-center gap-2">
          <span className="text-sm font-semibold text-[var(--color-text-primary)]">{agentName}</span>
          {profile?.podLeader && (
            <span className="text-[9px] font-medium text-[var(--color-accent)] bg-[var(--color-accent)]/10 px-1.5 py-0.5">
              LEADER
            </span>
          )}
        </div>
        {profile && (
          <p className="text-[11px] text-[var(--color-text-muted)] mt-0.5">{profile.role}</p>
        )}
      </div>

      {/* Section tabs */}
      <div className="flex border-b border-[var(--color-border)] flex-shrink-0">
        {(['profile', 'claude-md', 'work'] as const).map((section) => {
          const labels = { profile: 'Profile', 'claude-md': 'CLAUDE.md', work: 'Work Queue' }
          const isActive = activeSection === section
          return (
            <button
              key={section}
              onClick={() => setActiveSection(section)}
              className={`flex-1 px-3 py-2 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                isActive
                  ? 'text-[var(--color-accent)] border-b-2 border-[var(--color-accent)]'
                  : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
              }`}
            >
              {labels[section]}
              {section === 'work' && workItems.length > 0 && (
                <span className="ml-1 text-[9px] text-[var(--color-text-muted)]">({workItems.length})</span>
              )}
            </button>
          )
        })}
      </div>

      {/* Content */}
      <div className="flex-1 overflow-y-auto">
        {activeSection === 'profile' && (
          <div className="p-4">
            <div className="flex items-center justify-between mb-3">
              <h3 className="text-[11px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">agent.md</h3>
              <button
                onClick={() => openFile(`${agentDir}/agent.md`)}
                className="text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent)]/80 no-drag cursor-pointer"
              >
                Edit
              </button>
            </div>
            <pre className="text-[11px] text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono bg-[var(--color-bg-elevated)] border border-[var(--color-border)] p-3 leading-relaxed">
              {profile?.raw || 'No agent.md found'}
            </pre>
          </div>
        )}

        {activeSection === 'claude-md' && (
          <div className="p-4">
            <div className="flex items-center justify-between mb-3">
              <h3 className="text-[11px] font-semibold text-[var(--color-text-muted)] uppercase tracking-wider">CLAUDE.md</h3>
              <button
                onClick={() => openFile(`${agentDir}/CLAUDE.md`)}
                className="text-[10px] text-[var(--color-accent)] hover:text-[var(--color-accent)]/80 no-drag cursor-pointer"
              >
                Edit
              </button>
            </div>
            {claudeMd ? (
              <pre className="text-[11px] text-[var(--color-text-secondary)] whitespace-pre-wrap font-mono bg-[var(--color-bg-elevated)] border border-[var(--color-border)] p-3 leading-relaxed">
                {claudeMd}
              </pre>
            ) : (
              <p className="text-[11px] text-[var(--color-text-muted)]">No CLAUDE.md generated yet. Launch the agent to generate one.</p>
            )}
          </div>
        )}

        {activeSection === 'work' && (
          <div className="p-4 space-y-4">
            {/* Work sections */}
            {[
              { label: 'Active', items: active, color: 'text-yellow-400' },
              { label: 'Inbox', items: inbox, color: 'text-[var(--color-accent)]' },
              { label: 'Done', items: done, color: 'text-green-400' },
            ].map(({ label, items, color }) => (
              <div key={label}>
                <h3 className={`text-[10px] font-semibold uppercase tracking-wider mb-2 ${color}`}>
                  {label} {items.length > 0 && `(${items.length})`}
                </h3>
                {items.length === 0 ? (
                  <p className="text-[10px] text-[var(--color-text-muted)] pl-2">No items</p>
                ) : (
                  <div className="border border-[var(--color-border)]">
                    {items.map((item, i) => (
                      <div
                        key={item.filename}
                        onClick={() => openFile(`${agentDir}/work/${item.folder}/${item.filename}`)}
                        className={`px-3 py-2 cursor-pointer hover:bg-[var(--color-bg-elevated)] transition-colors ${
                          i < items.length - 1 ? 'border-b border-[var(--color-border)]' : ''
                        }`}
                      >
                        <div className="flex items-center justify-between">
                          <span className="text-xs text-[var(--color-text-primary)]">{item.title}</span>
                          <span className={`text-[9px] ${priorityColor(item.priority)}`}>{item.priority}</span>
                        </div>
                        <div className="flex items-center gap-2 mt-0.5">
                          <span className={`text-[9px] ${folderColor(item.folder)}`}>{item.folder}</span>
                          <span className="text-[9px] text-[var(--color-text-muted)]">{item.itemType}</span>
                          <span className="text-[9px] text-[var(--color-text-muted)]">{item.created}</span>
                        </div>
                      </div>
                    ))}
                  </div>
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  )
}
