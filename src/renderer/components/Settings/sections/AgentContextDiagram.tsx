import React from 'react'

type SkillTier = 'manager' | 'agent_template' | 'custom_agent'

interface Props {
  tier: SkillTier
}

interface SourceNode {
  label: string
  path: string
  /** What gets composed from this source. */
  flowsTo: 'claude_md' | 'skill_md' | 'argv' | 'task_context'
  /** Optional short explanation rendered under the label. */
  hint?: string
}

interface ArtifactNode {
  label: string
  hint: string
}

interface Delivery {
  label: string
  hint: string
}

const MANAGER_SPEC: {
  sources: SourceNode[]
  artifacts: ArtifactNode[]
  deliveries: Delivery[]
} = {
  sources: [
    { label: 'PROJECT.md', path: '.k2so/PROJECT.md', hint: 'Codebase knowledge (tech stack, conventions, key directories)', flowsTo: 'claude_md' },
    { label: 'agent.md', path: '.k2so/agents/<manager>/agent.md', hint: 'Manager persona + standing orders', flowsTo: 'claude_md' },
    { label: 'Auto layers (7)', path: 'regenerated from live DB + FS', hint: 'Identity, Connected Workspaces, Team Roster, Standing Orders, Decision Framework, Delegation + Review, Communication Commands', flowsTo: 'skill_md' },
    { label: 'Custom layers', path: '~/.k2so/templates/manager/*.md', hint: 'Your "+ Add Layer" entries — global across every manager-mode workspace', flowsTo: 'skill_md' },
    { label: 'triage wakeup.md', path: '.k2so/agents/<manager>/heartbeats/triage/wakeup.md', hint: 'Fires on the heartbeat schedule', flowsTo: 'argv' },
  ],
  artifacts: [
    { label: 'CLAUDE.md', hint: 'Regenerated at launch. Contains PROJECT.md body + agent persona + task context.' },
    { label: 'SKILL.md', hint: 'Regenerated at launch. Auto layers composed live, custom layers appended.' },
  ],
  deliveries: [
    { label: '--append-system-prompt', hint: 'CLAUDE.md + SKILL.md both ship here at launch. Every wake gets a fresh copy.' },
    { label: 'positional argv', hint: 'The wakeup.md contents arrive as the first user message on each fire.' },
  ],
}

const AGENT_TEMPLATE_SPEC: {
  sources: SourceNode[]
  artifacts: ArtifactNode[]
  deliveries: Delivery[]
} = {
  sources: [
    { label: 'PROJECT.md', path: '.k2so/PROJECT.md', hint: 'Same codebase context the manager reads — templates need it too.', flowsTo: 'claude_md' },
    { label: 'agent.md', path: '.k2so/agents/<template>/agent.md', hint: 'Template persona + role (e.g. "backend-eng").', flowsTo: 'claude_md' },
    { label: 'Auto layers (3)', path: 'regenerated at launch', hint: 'Identity, Check In + Status + Done, File Reservations', flowsTo: 'skill_md' },
    { label: 'Custom layers', path: '~/.k2so/templates/agent-template/*.md', hint: 'Your "+ Add Layer" entries — shared across every template agent.', flowsTo: 'skill_md' },
    { label: 'Delegated task', path: '.k2so/agents/<template>/work/active/<file>.md', hint: 'When `k2so delegate` fires, the task file contents become the launch prompt.', flowsTo: 'task_context' },
  ],
  artifacts: [
    { label: 'CLAUDE.md', hint: 'Written into the worktree root. Includes PROJECT.md + persona + the specific task being worked.' },
    { label: 'SKILL.md', hint: 'Regenerated at launch. Agent Template tier is minimal — just self-management commands.' },
  ],
  deliveries: [
    { label: '--append-system-prompt', hint: 'CLAUDE.md + SKILL.md ship here when the delegated agent spawns.' },
    { label: 'positional argv', hint: 'A "resume this task" kickoff message so the agent opens its task file and gets to work.' },
  ],
}

const CUSTOM_AGENT_SPEC: {
  sources: SourceNode[]
  artifacts: ArtifactNode[]
  deliveries: Delivery[]
} = {
  sources: [
    { label: 'agent.md', path: '.k2so/agents/<custom>/agent.md', hint: 'Custom agent persona — the user-defined role/behavior.', flowsTo: 'claude_md' },
    { label: 'Auto layers (4)', path: 'regenerated at launch', hint: 'Identity, Check In + Status + Done, Cross-Workspace Messaging, File Reservations', flowsTo: 'skill_md' },
    { label: 'Custom layers', path: '~/.k2so/templates/custom-agent/*.md', hint: 'Your "+ Add Layer" entries — global across every custom-agent workspace.', flowsTo: 'skill_md' },
    { label: 'heartbeat wakeup.md', path: '.k2so/agents/<custom>/heartbeats/<sched>/wakeup.md', hint: 'Per-schedule wake trigger. Custom agents can have multiple heartbeats.', flowsTo: 'argv' },
  ],
  artifacts: [
    { label: 'CLAUDE.md', hint: 'Regenerated at launch. Custom-agent CLAUDE.md does NOT include PROJECT.md (by design — custom agents may not be codebase-scoped).' },
    { label: 'SKILL.md', hint: 'Regenerated at launch. Custom-agent skill is minimal + cross-workspace messaging.' },
  ],
  deliveries: [
    { label: '--append-system-prompt', hint: 'CLAUDE.md + SKILL.md ship here at every launch.' },
    { label: 'positional argv', hint: 'Per-heartbeat wakeup.md arrives as the first user message on fire.' },
  ],
}

function specForTier(tier: SkillTier): typeof MANAGER_SPEC {
  switch (tier) {
    case 'manager': return MANAGER_SPEC
    case 'agent_template': return AGENT_TEMPLATE_SPEC
    case 'custom_agent': return CUSTOM_AGENT_SPEC
  }
}

function flowsToLabel(flow: SourceNode['flowsTo']): string {
  switch (flow) {
    case 'claude_md': return 'CLAUDE.md'
    case 'skill_md': return 'SKILL.md'
    case 'argv': return 'user message'
    case 'task_context': return 'CLAUDE.md (task section)'
  }
}

function flowAccent(flow: SourceNode['flowsTo']): string {
  switch (flow) {
    case 'claude_md': return 'border-amber-400/50 bg-amber-400/5 text-amber-200'
    case 'skill_md': return 'border-sky-400/50 bg-sky-400/5 text-sky-200'
    case 'argv': return 'border-emerald-400/50 bg-emerald-400/5 text-emerald-200'
    case 'task_context': return 'border-amber-400/50 bg-amber-400/5 text-amber-200'
  }
}

export function AgentContextDiagram({ tier }: Props): React.JSX.Element {
  const spec = specForTier(tier)
  const tierName = tier === 'manager' ? 'Workspace Manager' : tier === 'agent_template' ? 'Agent Template' : 'Custom Agent'

  return (
    <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/30 px-4 py-3 mb-4">
      <div className="flex items-center justify-between mb-2">
        <h3 className="text-[11px] font-semibold text-[var(--color-text-primary)]">
          How context flows to a {tierName}
        </h3>
        <div className="flex items-center gap-3 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-amber-400/50 bg-amber-400/10" /> CLAUDE.md</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-sky-400/50 bg-sky-400/10" /> SKILL.md</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-emerald-400/50 bg-emerald-400/10" /> user msg</span>
        </div>
      </div>

      <div className="grid gap-3" style={{ gridTemplateColumns: 'minmax(0,1.4fr) auto minmax(0,1fr) auto minmax(0,1fr)' }}>
        {/* Col 1: sources */}
        <div className="flex flex-col gap-1.5">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">You edit</div>
          {spec.sources.map((s) => (
            <div
              key={s.label}
              className={`border px-2 py-1.5 ${flowAccent(s.flowsTo)}`}
              title={s.hint}
            >
              <div className="text-[11px] font-medium">{s.label}</div>
              <div className="text-[9px] font-mono opacity-70 truncate">{s.path}</div>
            </div>
          ))}
        </div>

        {/* Arrow col → */}
        <div className="flex flex-col justify-center text-[var(--color-text-muted)] text-xs">
          <div className="h-full flex items-center">→</div>
        </div>

        {/* Col 2: artifacts */}
        <div className="flex flex-col gap-1.5 justify-center">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">K2SO composes</div>
          {spec.artifacts.map((a) => (
            <div key={a.label} className="border border-[var(--color-border)] bg-black/20 px-2 py-1.5">
              <div className="text-[11px] font-medium text-[var(--color-text-primary)]">{a.label}</div>
              <div className="text-[9px] text-[var(--color-text-muted)] leading-snug mt-0.5">{a.hint}</div>
            </div>
          ))}
          <div className="text-[9px] text-[var(--color-text-muted)] italic mt-1">
            Regenerated on every agent launch.
          </div>
        </div>

        {/* Arrow col → */}
        <div className="flex flex-col justify-center text-[var(--color-text-muted)] text-xs">
          <div className="h-full flex items-center">→</div>
        </div>

        {/* Col 3: deliveries */}
        <div className="flex flex-col gap-1.5 justify-center">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">Claude receives</div>
          {spec.deliveries.map((d) => (
            <div key={d.label} className="border border-[var(--color-border)] bg-black/20 px-2 py-1.5">
              <div className="text-[11px] font-medium text-[var(--color-text-primary)]">{d.label}</div>
              <div className="text-[9px] text-[var(--color-text-muted)] leading-snug mt-0.5">{d.hint}</div>
            </div>
          ))}
        </div>
      </div>

      {/* Footer: when does each flow fire? */}
      <div className="mt-3 pt-2 border-t border-[var(--color-border)] flex items-center justify-between text-[9px] text-[var(--color-text-muted)]">
        <div>
          <span className="text-[var(--color-text-secondary)] font-medium">At launch:</span> CLAUDE.md + SKILL.md compose and ship via <span className="font-mono">--append-system-prompt</span>.
        </div>
        <div>
          {tier === 'agent_template' ? (
            <><span className="text-[var(--color-text-secondary)] font-medium">On delegation:</span> task context becomes the argv kickoff.</>
          ) : (
            <><span className="text-[var(--color-text-secondary)] font-medium">On wake:</span> heartbeat wakeup.md becomes argv user message.</>
          )}
        </div>
      </div>
    </div>
  )
}
