import { useEffect, useState } from 'react'
import { daemonCliGet } from '@/lib/daemon-cli'
import { type RoleSkill, roleSkillLabel } from './canonicalAgentSeeds'
import { type HarnessProbe, anyHarnessUnified } from './canonicalState'

// The value-pitch WHY copy relocated VERBATIM from the removed consent page
// (AddWorkspaceDialog:147-164) into the canonical button subtitle + the skill
// briefing (canonical-agents PRD §7). This is where harness unification is
// actually opt-in, so the pitch belongs here.
export const CANONICAL_PITCH_SUBTITLE =
  'Tell K2SO once, every AI tool listens. Each AI coding tool reads its project notes from a different file; write your context once and every tool sees the same picture.'

/**
 * Role-skill button (Workspace Manager / K2 Agent). Opens the normal
 * AIFileEditor on AGENT.md (PRD §9.1). Label gates on skill-present state:
 * "Set up …" when no SKILL.md exists yet, "Re-run …" once it does (§9.3).
 */
export function RoleSkillButton({
  role,
  projectPath,
  onOpen,
}: {
  role: RoleSkill
  projectPath: string
  onOpen: () => void
}): React.JSX.Element {
  const label = roleSkillLabel(role)
  const [skillPresent, setSkillPresent] = useState(false)

  useEffect(() => {
    let cancelled = false
    daemonCliGet<{ content: string }>('fs/read-file', {
      path: `${projectPath}/.k2so/skills/${role}/SKILL.md`,
    })
      .then(() => { if (!cancelled) setSkillPresent(true) })
      .catch(() => { if (!cancelled) setSkillPresent(false) })
    return () => { cancelled = true }
  }, [projectPath, role])

  return (
    <div className="flex items-center justify-between">
      <div className="min-w-0">
        <span className="text-xs text-[var(--color-text-secondary)]">{label} skill</span>
        <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5">
          Weaves the {label} role guidance into{' '}
          <span className="font-mono">.k2so/agent/AGENT.md</span> organically — your existing
          context is preserved, never overwritten with a templated block.
        </p>
      </div>
      <button
        onClick={onOpen}
        className="px-2.5 py-1 text-[11px] font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] whitespace-nowrap no-drag cursor-pointer"
      >
        {skillPresent ? `Re-run ${label}` : `Set up ${label}`}
      </button>
    </div>
  )
}

/**
 * K2 Canonical Agent button — shown ALWAYS, every mode incl. custom + off
 * (PRD §9.3). Opens the canonical ceremony modal. Label gates on
 * detect_canonical_state: "Manage / Undo" when any harness is already
 * canonicalized, "Set up …" otherwise.
 */
export function CanonicalAgentButton({
  probes,
  onOpen,
}: {
  probes: HarnessProbe[]
  onOpen: (mode: 'setup' | 'manage') => void
}): React.JSX.Element {
  const unified = anyHarnessUnified(probes)

  return (
    <div className="flex items-center justify-between">
      <div className="min-w-0">
        <span className="text-xs text-[var(--color-text-secondary)]">K2 Canonical Agent</span>
        <p className="text-[9px] text-[var(--color-text-muted)] mt-0.5">{CANONICAL_PITCH_SUBTITLE}</p>
      </div>
      <button
        onClick={() => onOpen(unified ? 'manage' : 'setup')}
        className="px-2.5 py-1 text-[11px] font-medium text-[var(--color-bg)] bg-[var(--color-text-primary)] hover:bg-[var(--color-text-secondary)] whitespace-nowrap no-drag cursor-pointer"
      >
        {unified ? 'Manage / Undo' : 'Set up canonical'}
      </button>
    </div>
  )
}
