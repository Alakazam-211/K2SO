# Workspace–Agent Unification (0.37.0)

K2SO 0.37.0 collapsed your workspace's `.k2so/agents/<name>/...` layout into the single-agent layout. Originals were preserved under `.k2so/migration/legacy/`.

## What moved

- **Primary agent (`pod-leader`)** → `.k2so/agent/AGENT.md`
- **Templates** (5) → `.k2so/agent-templates/<name>/AGENT.md`
  - qa-eng
  - frontend-eng
  - k2so-agent
  - rust-eng
  - cli-eng
- **Archived to `.k2so/migration/legacy/`**:
  - __lead__
  - agent-archive
  - wakeup.md
  - MIGRATION-0.32.7.md
  - .harvest-0.32.7-done

## Recovery

Anything in `.k2so/migration/legacy/` is safe to delete after you've verified your workspace works. Compiled SKILL/CLAUDE files were dropped (they regenerate from the AGENT.md source on next launch).

If something looks wrong, the sentinel `.k2so/.unification-0.37.0-done` can be deleted to force a re-run — but only if you've also restored the original `.k2so/agents/` layout from the legacy archive.
