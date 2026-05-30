# K2SO 0.39.10 — `k2so read` + msg-length docs + `.k2so/agents/` generation fix

Bundles two commits landed on `main` since 0.39.9:
`feat(cli): k2so read` (56e87d48) and `fix(agents): stop scaffolding
legacy .k2so/agents/` (f1c3f9a8). No daemon-protocol changes.

## 1. `k2so read <workspace>` — peek another agent's live terminal

The "read" complement to the messaging verbs: **msg = talk live ·
inbox = mail · read = look over their shoulder.** Addresses by
workspace name exactly like `msg`. Primary use case: human-in-the-loop
— peek what an agent is doing or waiting on *before* injecting a `msg`,
or diagnose a stuck/quiet agent.

```
k2so read <workspace>                 # last 50 lines of primary session
k2so read <workspace> --lines 120     # more history
k2so read <workspace> --agent <name>  # a specific agent's session
```

The capability already existed (`k2so terminal read <id>`) but required
a session UUID / canonical key and wasn't in the SKILL. This adds
workspace-name addressing + documents it.

**Daemon** (`terminal_routes.rs::handle_read`): new `workspace=<name>`
param (+ optional `agent=`). Resolves the name via the same
`workspace_msg::resolve_workspace` that `msg` uses →
`canonical_session::lookup_project_id` → canonical session key
(`<project_id>` for the primary, `<project_id>:<agent>` with `--agent`)
→ `read_v2_grid_lines`. The existing `id=<uuid>` / `id=<pid>:<agent>`
forms are unchanged. New pure `build_canonical_read_key` helper + 3
unit tests; clean errors for unknown-workspace and asleep cases.

**CLI** (`cli/k2so`): new top-level `read` verb (co-located with
`msg`/`inbox`) + `cmd_help_read` + daily-help listing.

**SKILL** (`skills/content.rs`): `read` documented in all three
messaging blocks (manager / custom-agent / comprehensive K2SO-agent).

## 2. `msg` length-limit documentation

Live `msg` is injected into the recipient's running input line (typed +
Enter), so it's for **short, single-line** messages — long or
multi-line content gets truncated by the recipient's terminal input
widget (Claude Code / Codex / Gemini paste handling), not by an
explicit K2SO cap (`render_signal_for_inject` sends the full text). The
fix is documentation, not a behavior change: every SKILL messaging
block + `k2so msg --help` now state the limit and point to `--inbox`
(no length limit) for anything substantial. Also fixed a stale
hard-deprecated `work send` reference in the msg help.

## 3. Fix: agent create no longer scaffolds legacy `.k2so/agents/`

**Bug** (reported by a user whose agent docs landed in `.k2so/agents/`
instead of `.k2so/agent/AGENT.md`): `workspace::agent::create()` — hit
by the Settings → Projects "Manage Persona" / set-agent-mode flow —
used `agent_dir()`, a *resolver* whose final fallback is the legacy
plural `.k2so/agents/<name>/`, as its **creation target**. For a fresh
workspace (no canonical `.k2so/agent/AGENT.md` yet) that scaffolded the
retired layout and wrote the persona there. Phase 2.5 retired the
*readers* and *templates* of `.k2so/agents/`, but `create()` for a
brand-new agent was never repointed.

**Fix** (`k2so-core/src/workspace/agent.rs`):
- `create()` now scaffolds at the canonical `workspace_agent_path()` =
  `.k2so/agent/` and writes `AGENT.md` there. Dropped the legacy
  `.k2so/agents/<name>/work/{inbox,active,done}` scaffolding (the
  canonical `.k2so/inbox/` is still created; `SKILL.md` still goes to
  `.k2so/skills/`).
- New idempotent `repoint_stray_legacy_agent(project)` — self-heals
  already-affected workspaces: if canonical `AGENT.md` is absent but a
  stray `.k2so/agents/<name>/` (non-`.archive`, has `AGENT.md`) exists,
  moves ALL its entries (persona + any user docs) into `.k2so/agent/`.
  Never clobbers an existing canonical persona; leaves the emptied
  legacy dir (not trashed — avoids macOS Finder Touch-ID prompts during
  the headless boot sweep). Wired into the daemon's per-workspace boot
  migration sweep.

Destination rationale: the skills-consolidation migration sends OLD
per-named-agent *definitions* → `.k2so/skills/<name>/SKILL.md`, but
`create()` writes the workspace **persona** (name/role/type
frontmatter), whose canonical home is `.k2so/agent/AGENT.md`. The
strays are misfiled personas, so `.k2so/agent/` is the correct target.

## Tested

- **k2so-core: 665 passed, 0 failed** (incl. 3 read-key + 4 repoint
  unit tests; rewrote the stale `create_*_pre_unification` test that
  asserted the old buggy legacy-write).
- **k2so-daemon: 199 passed, 0 failed.**
- **Renderer typecheck / vitest**: unchanged baseline (read work was
  CLI/daemon/SKILL only).
- **Live sandbox smoke (`k2so read`): 11/11** — read by workspace name
  (raw route + real `k2so` CLI), `--agent`, primary/agent key
  isolation, unknown-workspace error, asleep error, CLI non-zero exit,
  legacy `id=` back-compat.
- **Live sandbox smoke (`.k2so/agents/` fix): 9/9** — `create()` →
  canonical (no `.k2so/agents/`); boot sweep repoints a stray in an
  already-migrated workspace → `.k2so/agent/`, preserves persona +
  extra docs, idempotent across restarts.

## Upgrade notes

- Any 0.39.x → 0.39.10: clean update. The `.k2so/agents/` repoint runs
  on first boot for any affected workspace (idempotent, content
  preserved); no-op for everyone else.
- Users on 0.38.x → 0.39.10: full 0.39.0 + 0.39.1 migration sequence
  fires on first boot, gated behind the 0.39.5 `/boot-status` handshake.

## What else shipped in this release

Nothing else. See `release-notes-0.39.0.md` through
`release-notes-0.39.9.md` for prior content.
