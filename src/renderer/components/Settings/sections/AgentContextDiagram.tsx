import React from 'react'

type SkillTier = 'manager' | 'k2so_agent' | 'agent_template' | 'custom_agent'

interface Props {
  tier: SkillTier
}

/**
 * Where a source flows to. As of 0.32.7 there is ONE derived artifact —
 * the canonical `SKILL.md` — which fans out to every CLI LLM via
 * symlinks / marker injection. `argv` is per-launch injection used for
 * content that can't live in the shared workspace file (heartbeat
 * trigger messages + per-delegation task kickoffs).
 */
type FlowDestination = 'skill_md' | 'argv'

interface SourceNode {
  label: string
  path: string
  flowsTo: FlowDestination
  hint?: string
}

interface DeliveryChannel {
  label: string
  kind: 'file' | 'argv'
  hint: string
  /** Short list of discovery paths or CLI flags this channel covers. */
  reaches: string[]
}

const FILE_DISCOVERY: DeliveryChannel = {
  label: 'File discovery',
  kind: 'file',
  hint: 'Every CLI LLM opened in this workspace sees the same canonical context — K2SO-launched or not.',
  reaches: [
    './CLAUDE.md → SKILL.md',
    './GEMINI.md → SKILL.md',
    './AGENT.md → SKILL.md (agent.md spec)',
    './.goosehints → SKILL.md',
    './.cursor/rules/k2so.mdc (generated)',
    './AGENTS.md (marker-injected)',
    '.github/copilot-instructions.md (marker-injected)',
    '.aider.conf.yml → read: SKILL.md',
  ],
}

const ARGV_INJECTION_AGENT: DeliveryChannel = {
  label: 'argv injection',
  kind: 'argv',
  hint: 'Only the specific agent K2SO is launching at that moment — per-agent persona + heartbeat trigger.',
  reaches: [
    '--append-system-prompt (Claude, Codex, Cursor)',
    '--system (OpenCode)',
    'Modelfile (Ollama, per-launch)',
    'profile YAML (Open Interpreter)',
    'positional argv — heartbeat WAKEUP.md',
  ],
}

const ARGV_INJECTION_TEMPLATE: DeliveryChannel = {
  label: 'argv injection',
  kind: 'argv',
  hint: 'Per-delegation — each worktree gets a fresh launch with the task file as the kickoff.',
  reaches: [
    '--append-system-prompt (launched in worktree)',
    'positional argv — "resume this task" kickoff',
    'per-worktree CLAUDE.md — task title + priority + kickoff',
  ],
}

interface TierSpec {
  sources: SourceNode[]
  /** Per-tier description of the single canonical SKILL.md. */
  canonical: string
  deliveries: DeliveryChannel[]
}

const MANAGER_SPEC: TierSpec = {
  sources: [
    { label: 'PROJECT.md', path: '.k2so/PROJECT.md', hint: 'Codebase knowledge shared by every agent in this workspace.', flowsTo: 'skill_md' },
    { label: 'AGENT.md (manager)', path: '.k2so/agents/<manager>/AGENT.md', hint: 'Manager persona — workspace-global, embedded in SKILL.md.', flowsTo: 'skill_md' },
    { label: 'AGENT.md (sub-agent)', path: '.k2so/agents/<sub-agent>/AGENT.md', hint: 'Sub-agent personas stay per-launch — argv-injected when the specific agent spawns.', flowsTo: 'argv' },
    { label: 'WAKEUP.md (triage)', path: '.k2so/agents/<manager>/heartbeats/triage/WAKEUP.md', hint: 'Fires on the manager heartbeat schedule — delivered as the first user message.', flowsTo: 'argv' },
  ],
  canonical: 'Manager workspaces: SKILL.md carries PROJECT.md + manager AGENT.md + K2SO-managed layers. Sub-agent personas never live here — they would collide if 5 sub-agents stacked their AGENT.md bodies into one file.',
  deliveries: [FILE_DISCOVERY, ARGV_INJECTION_AGENT],
}

const K2SO_AGENT_SPEC: TierSpec = {
  sources: [
    { label: 'PROJECT.md', path: '.k2so/PROJECT.md', hint: 'Workspace-scope codebase context.', flowsTo: 'skill_md' },
    { label: 'AGENT.md', path: '.k2so/agents/<k2so-agent>/AGENT.md', hint: 'Sole agent persona — embedded in SKILL.md since this is a single-agent workspace.', flowsTo: 'skill_md' },
    { label: 'WAKEUP.md (each heartbeat)', path: '.k2so/agents/<k2so-agent>/heartbeats/<sched>/WAKEUP.md', hint: 'Per-schedule wake trigger — delivered as the first user message on fire.', flowsTo: 'argv' },
  ],
  canonical: 'K2SO Agent workspaces: SKILL.md carries the sole agent\'s persona + PROJECT.md + the K2SO planning surface (PRDs, milestones, cross-workspace messaging).',
  deliveries: [FILE_DISCOVERY, ARGV_INJECTION_AGENT],
}

const CUSTOM_AGENT_SPEC: TierSpec = {
  sources: [
    { label: 'PROJECT.md', path: '.k2so/PROJECT.md', hint: 'Codebase knowledge (if the custom agent is codebase-scoped).', flowsTo: 'skill_md' },
    { label: 'AGENT.md', path: '.k2so/agents/<custom>/AGENT.md', hint: 'User-defined agent persona — embedded in SKILL.md for single-agent workspaces.', flowsTo: 'skill_md' },
    { label: 'WAKEUP.md (each heartbeat)', path: '.k2so/agents/<custom>/heartbeats/<sched>/WAKEUP.md', hint: 'Per-schedule wake trigger.', flowsTo: 'argv' },
  ],
  canonical: 'Custom Agent workspaces: SKILL.md carries the custom persona + optional PROJECT.md. Single-agent workspace, so the sole agent is the workspace-level context.',
  deliveries: [FILE_DISCOVERY, ARGV_INJECTION_AGENT],
}

const AGENT_TEMPLATE_SPEC: TierSpec = {
  sources: [
    { label: 'PROJECT.md', path: '.k2so/PROJECT.md', hint: 'Templates inherit the workspace PROJECT.md via the file-discovery symlink walk-up.', flowsTo: 'skill_md' },
    { label: 'AGENT.md', path: '.k2so/agents/<template>/AGENT.md', hint: 'Template persona — argv-injected per delegation so each worktree launch carries it fresh.', flowsTo: 'argv' },
    { label: 'Task file', path: '.k2so/agents/<template>/work/active/<task>.md', hint: 'When `k2so delegate` fires, the task file becomes the launch kickoff.', flowsTo: 'argv' },
  ],
  canonical: 'Agent Templates: SKILL.md ships workspace-level context (PROJECT.md + K2SO-managed layers) so the template sees the codebase. The template\'s own persona is argv-only — each delegated worktree gets it via --append-system-prompt.',
  deliveries: [FILE_DISCOVERY, ARGV_INJECTION_TEMPLATE],
}

function specForTier(tier: SkillTier): TierSpec {
  switch (tier) {
    case 'manager': return MANAGER_SPEC
    case 'k2so_agent': return K2SO_AGENT_SPEC
    case 'agent_template': return AGENT_TEMPLATE_SPEC
    case 'custom_agent': return CUSTOM_AGENT_SPEC
  }
}

function flowAccent(flow: FlowDestination): string {
  switch (flow) {
    case 'skill_md': return 'border-sky-400/50 bg-sky-400/5 text-sky-200'
    case 'argv': return 'border-emerald-400/50 bg-emerald-400/5 text-emerald-200'
  }
}

function flowLabel(flow: FlowDestination): string {
  return flow === 'skill_md' ? '→ SKILL.md' : '→ argv'
}

export function AgentContextDiagram({ tier }: Props): React.JSX.Element {
  const spec = specForTier(tier)
  const tierName =
    tier === 'manager' ? 'Workspace Manager' :
    tier === 'k2so_agent' ? 'K2SO Agent' :
    tier === 'agent_template' ? 'Agent Template' :
    'Custom Agent'

  return (
    <div className="border border-[var(--color-border)] bg-[var(--color-bg-elevated)]/30 px-4 py-3 mb-4">
      <div className="flex items-center justify-between mb-2">
        <h3 className="text-[11px] font-semibold text-[var(--color-text-primary)]">
          How context flows to a {tierName}
        </h3>
        <div className="flex items-center gap-3 text-[9px] uppercase tracking-wider text-[var(--color-text-muted)]">
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-sky-400/50 bg-sky-400/10" /> → SKILL.md</span>
          <span className="flex items-center gap-1"><span className="w-2 h-2 border border-emerald-400/50 bg-emerald-400/10" /> → argv</span>
        </div>
      </div>

      <div className="grid gap-3" style={{ gridTemplateColumns: 'minmax(0,1.3fr) auto minmax(0,1fr) auto minmax(0,1.2fr)' }}>
        {/* Col 1: sources (three user-editable file types) */}
        <div className="flex flex-col gap-1.5">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">You edit — 3 file types</div>
          {spec.sources.map((s) => (
            <div
              key={s.label + s.path}
              className={`border px-2 py-1.5 ${flowAccent(s.flowsTo)}`}
              title={s.hint}
            >
              <div className="flex items-center justify-between gap-2">
                <div className="text-[11px] font-medium">{s.label}</div>
                <div className="text-[9px] opacity-70 flex-shrink-0">{flowLabel(s.flowsTo)}</div>
              </div>
              <div className="text-[9px] font-mono opacity-70 truncate">{s.path}</div>
            </div>
          ))}
        </div>

        {/* Arrow col → */}
        <div className="flex flex-col justify-center text-[var(--color-text-muted)] text-xs">
          <div className="h-full flex items-center">→</div>
        </div>

        {/* Col 2: single canonical SKILL.md */}
        <div className="flex flex-col gap-1.5 justify-center">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">K2SO composes — 1 file</div>
          <div className="border border-sky-400/50 bg-sky-400/10 px-2 py-2">
            <div className="flex items-center justify-between gap-2">
              <div className="text-[12px] font-semibold text-sky-200">SKILL.md</div>
              <div className="text-[8px] uppercase tracking-wider px-1.5 py-0.5 bg-sky-400/20 text-sky-100 rounded-sm">canonical</div>
            </div>
            <div className="text-[9px] text-sky-100/70 leading-snug mt-1">{spec.canonical}</div>
            <div className="text-[9px] font-mono text-sky-200/60 mt-1 truncate">.k2so/skills/k2so/SKILL.md</div>
          </div>
          <div className="text-[9px] text-[var(--color-text-muted)] italic mt-1 leading-snug">
            Regenerated on every launch. Every CLI LLM — 12 harnesses — sees this same file.
          </div>
        </div>

        {/* Arrow col → */}
        <div className="flex flex-col justify-center text-[var(--color-text-muted)] text-xs">
          <div className="h-full flex items-center">→</div>
        </div>

        {/* Col 3: two delivery channels */}
        <div className="flex flex-col gap-1.5 justify-center">
          <div className="text-[9px] uppercase tracking-wider text-[var(--color-text-muted)] mb-0.5">Reaches agents — 2 channels</div>
          {spec.deliveries.map((d) => {
            const accent = d.kind === 'file'
              ? 'border-sky-400/40 bg-sky-400/5'
              : 'border-emerald-400/40 bg-emerald-400/5'
            const labelColor = d.kind === 'file' ? 'text-sky-200' : 'text-emerald-200'
            return (
              <div key={d.label} className={`border px-2 py-1.5 ${accent}`} title={d.hint}>
                <div className={`text-[11px] font-medium ${labelColor}`}>{d.label}</div>
                <div className="text-[9px] text-[var(--color-text-muted)] leading-snug mt-0.5">{d.hint}</div>
                <div className="text-[9px] font-mono opacity-60 mt-1 space-y-0.5">
                  {d.reaches.slice(0, 4).map((r) => (
                    <div key={r} className="truncate">• {r}</div>
                  ))}
                  {d.reaches.length > 4 ? (
                    <div className="italic opacity-70">+{d.reaches.length - 4} more</div>
                  ) : null}
                </div>
              </div>
            )
          })}
        </div>
      </div>

      {/* Footer: summary of the authoring contract */}
      <div className="mt-3 pt-2 border-t border-[var(--color-border)] text-[9px] text-[var(--color-text-muted)] leading-snug">
        <span className="text-[var(--color-text-secondary)] font-medium">You edit 3 file types</span>
        {' '}(AGENT.md, PROJECT.md, WAKEUP.md).{' '}
        <span className="text-[var(--color-text-secondary)] font-medium">K2SO composes 1 canonical file</span>
        {' '}(SKILL.md) and fans it out to every harness —{' '}
        <span className="font-mono text-[var(--color-text-secondary)]">./CLAUDE.md</span>,
        {' '}<span className="font-mono text-[var(--color-text-secondary)]">./GEMINI.md</span>,
        {' '}<span className="font-mono text-[var(--color-text-secondary)]">./AGENTS.md</span>, and 9 more.
        Per-launch content (sub-agent persona, heartbeat trigger, task kickoff) arrives via argv.
      </div>
    </div>
  )
}
