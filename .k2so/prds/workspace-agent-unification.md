# Workspace–Agent Unification

**Status:** Approved, in implementation. Single feature branch: `feature/workspace-agent-unification`.
**Captured:** 2026-04-30 · **Refreshed:** 2026-05-05
**Target release:** 0.37.0
**Pre-reqs shipped:**
- **0.36.13** — pinned chat tab is daemon-first + CLI-reachable; `agent_sessions.active_terminal_id` (migration 0037); `is_agent_live` consults both legacy + v2 maps; two-phase inject (body + 150ms settle + Enter).
- **0.36.14** — `v2_session_map` keys by project-namespaced agent name (`<projectId>:<agentName>`) with bare-name mirror for legacy callers; cleanup UPDATE scoped by `project_id`; renderer `<AgentChatTerminal>` gets `key={projectId}` for clean remount on workspace switch.
- **0.36.15** — awareness bus egress prefers prefixed lookup when `signal.to.workspace` is set; wake provider uses `signal.to.workspace` (with `signal.from` fallback for legacy callers); auto-launched sessions register under the prefixed key.

The addressing layer is **half-built today**: workspace context flows through the awareness bus end-to-end, but as a *bridge pattern* (prefixed strings, mirror entries, dual-key lookups). 0.37.0's job is to retire the bridge — promote `project_id` to a first-class column and parameter throughout the stack, drop the bare-name mirror, and collapse the dual-key lookups into single-key. Zero new functionality; substantial simplification.

## Why this PRD exists

K2SO carries a multi-agent abstraction in its filesystem layout, DB schema, and CLI surface — a leftover from the v0.20-era "manager + delegated sub-agents inside one workspace" model that the product has since moved away from. Today's product reality:

- One workspace = one agent the user actually talks to.
- "Other agents" in `.k2so/agents/<other>/` are role personas K2SO uses when the workspace's agent delegates to a worktree. They never run inside the parent workspace; they're templates, not agents.
- Worktrees are lightweight children of the parent workspace — only the parent's agent talks to them; users don't address worktree agents directly.

The CLI, DB, and filetree haven't caught up:

- `agent_sessions(project_id, agent_name)` lets multiple sessions live under one workspace; the product invariant says no.
- `.k2so/agents/<a>/work/{inbox,active,done}` and workspace-level `.k2so/work/{inbox,done}` both exist, with overlapping semantics.
- `k2so msg` takes an agent name, not a workspace.
- 166 `AgentSession::*` / `AgentHeartbeat::*` call sites pass `agent_name` as a parameter that is never ambiguous in practice.
- Per-agent `CLAUDE.md` and `SKILL.md` files are compiled outputs that duplicate `.k2so/skills/k2so/SKILL.md`.

This PRD makes the product invariant load-bearing in the code: **a workspace has exactly one agent, addressable by workspace path alone.** Templates become a separate first-class concept (workspace-local for v1, with the explicit door open for a user-global "agent-skills" repository in v2).

## Out of scope

- **Worktrees-as-full-child-workspaces.** Worktrees stay lightweight. Only the parent workspace's agent communicates with them; users don't `k2so msg` directly into a worktree.
- **User-global agent-template repository (`~/.k2so/agent-templates/`).** Future work. v1 is workspace-local.
- **Agent-template enable/disable / "skills on/off" UI.** Future work, blocked on the v2 agent-skills concept.
- **Migrating `heartbeat_fires.agent_name`.** Audit table; denormalized on purpose. Drop in a future cleanup pass.
- **Renaming `projects` table.** It's already workspace-scoped by content; renaming it ripples into too many call sites for the value. Stays.
- **The 0.36.10 onboarding flow.** Already workspace-keyed; no churn.

## Approach

Single coordinated branch, single 0.37.0 release. Phased internally — **filesystem first, then DB, then CLI** — so each layer compiles and tests pass before the next lands. Migrations 0038–0041 ship together; running 0.37.0 fresh against an existing user's workspace performs the on-disk migration once, atomically, with a visible `MIGRATION-0.37.0.md` notice listing what was archived.

Tests rewritten as part of each phase, **anti-fallback**: every test asserts the post-state invariant directly (e.g., "exactly one row in `workspace_sessions` per `project_id`"), no `unwrap_or` fallbacks that would make the test pass against the old shape.

## Phase 1 — Filesystem unification

### Layout transition

| Before | After | Notes |
|---|---|---|
| `.k2so/agents/<primary>/AGENT.md` | `.k2so/agent/AGENT.md` | the workspace's one agent |
| `.k2so/agents/<primary>/work/{inbox,active,done}/` | `.k2so/work/{inbox,active,done}/` | merged into workspace inbox |
| `.k2so/agents/<primary>/CLAUDE.md` | **deleted** | redundant with `.k2so/skills/k2so/SKILL.md` |
| `.k2so/agents/<primary>/SKILL.md` | **deleted** | same |
| `.k2so/agents/<primary>/heartbeats/<sched>/wakeup.md` | `.k2so/heartbeats/<sched>/wakeup.md` | promoted to workspace level |
| `.k2so/agents/<other>/AGENT.md` (templates) | `.k2so/agent-templates/<other>/AGENT.md` | role personas for delegation |
| `.k2so/agents/<other>/work/` (templates) | **deleted** | templates don't run, they don't have inboxes |
| `.k2so/agents/__lead__/` | archived to `.k2so/migration/legacy/__lead__/` | obsoleted by single-agent invariant |
| `.k2so/agents/.archive/` | archived to `.k2so/migration/legacy/agent-archive/` | content preserved, location moves |
| `.k2so/skills/k2so-<agent>/` | `.k2so/skills/k2so/` (workspace) | one compiled SKILL per workspace |
| `.k2so/skills/k2so-<template>/` | `.k2so/skills/template-<n>/` (lazy-generated on delegate) | templates only get a SKILL when actually used in a worktree |
| `.k2so/wakeup.md` (root) | **deleted** | superseded by per-heartbeat `wakeup_path` |
| `.k2so/MIGRATION-0.32.7.md` | archived to `.k2so/migration/legacy/` | aged-out migration banner |
| `.k2so/.harvest-0.32.7-done` | archived | same |
| `.k2so/milestones/`, `.k2so/specs/` (empty) | folded into `.k2so/prds/{milestones,specs}/` subdirs | only created on first use |

After migration, a fresh single-agent workspace's `.k2so/`:

```
.k2so/
├── PROJECT.md              # workspace knowledge (source of truth)
├── config.json             # workspace config
├── agent/                  # the workspace's one agent
│   └── AGENT.md
├── agent-templates/        # delegation/worktree role personas
│   ├── frontend-eng/AGENT.md
│   ├── rust-eng/AGENT.md
│   └── …
├── heartbeats/             # workspace-level wake schedules
│   └── <name>/wakeup.md
├── work/{inbox,active,done}/
├── skills/
│   ├── k2so/SKILL.md       # canonical compiled SKILL
│   └── template-<n>/SKILL.md  # lazy: only when worktree spawned from template
├── migration/              # onboarding archives + legacy/ subdir
├── sessions/               # session-stream archives (UUID-keyed)
├── logs/
└── prds/                   # absorbs milestones/, specs/ via subdirs
```

12 → 8 top-level entries.

### Per-workspace migration algorithm

Triggered once on first 0.37.0 boot, atomic per workspace, idempotent (sentinel `.k2so/.unification-0.37.0-done`):

```text
1. Read projects.agent_mode. Determine the primary agent name:
   - "manager" / "coordinator" / "pod-leader" → primary is the manager-tier agent in agents/
   - "k2so-agent" → primary is the k2so-agent
   - "custom" → primary is the user-named agent in agents/ (first/only one)
   - "off" → no primary; skip primary migration, only template+cleanup
2. mkdir .k2so/agent/, .k2so/agent-templates/, .k2so/heartbeats/, .k2so/migration/legacy/
3. If primary exists:
   a. Move <primary>/AGENT.md → .k2so/agent/AGENT.md
   b. Merge <primary>/work/inbox/* → .k2so/work/inbox/ (skipping conflicts; user resolves)
      Same for active/, done/.
   c. Move <primary>/heartbeats/<sched>/* → .k2so/heartbeats/<sched>/*
   d. Delete <primary>/CLAUDE.md, <primary>/SKILL.md (compiled, regenerable)
4. For each other dir in agents/ (not primary, not __lead__, not .archive):
   a. Move <other>/AGENT.md → agent-templates/<other>/AGENT.md
   b. Delete <other>/work/ (templates have no inbox)
   c. Delete <other>/CLAUDE.md, <other>/SKILL.md
5. Move .k2so/agents/__lead__/ → .k2so/migration/legacy/__lead__/
6. Move .k2so/agents/.archive/ → .k2so/migration/legacy/agent-archive/
7. rmdir .k2so/agents/  (now empty)
8. Delete .k2so/wakeup.md, .k2so/MIGRATION-0.32.7.md, .k2so/.harvest-0.32.7-done
   (move to migration/legacy/ if non-empty)
9. Stamp .k2so/.unification-0.37.0-done with timestamp + summary
10. Write .k2so/MIGRATION-0.37.0.md listing what was archived (user-facing notice).
11. Run workspace SKILL regen so .k2so/skills/k2so/SKILL.md regenerates from new layout.
```

For `agent_mode = "off"` workspaces: only steps 4-10 run (no primary, just cleanup).

For workspaces with `find_primary_agent()` returning None despite `agent_mode != "off"` (data drift): emit a warning to stderr listing the agents found, write `.k2so/MIGRATION-0.37.0.md` with the manual-resolution steps, and skip primary migration. The workspace stays operational; user resolves later.

## Phase 2 — DB schema

### Migration 0038 — Rename `agent_sessions` → `workspace_sessions`

Carries every column shipped through 0.36.x: `surfaced` (added 0036), `active_terminal_id` (added 0037), and the original session-tracking columns. Drops only `agent_name`.

```sql
-- Rename + drop agent_name (one row per workspace post-migration)
CREATE TABLE workspace_sessions (
  id                  TEXT PRIMARY KEY,
  project_id          TEXT NOT NULL UNIQUE REFERENCES projects(id) ON DELETE CASCADE,
  terminal_id         TEXT,
  active_terminal_id  TEXT,                                    -- 0.36.13 introduced this on agent_sessions (migration 0037)
  surfaced            INTEGER NOT NULL DEFAULT 0,              -- 0.36.11 introduced this on agent_sessions (migration 0036)
  session_id          TEXT,
  harness             TEXT NOT NULL DEFAULT 'claude',
  owner               TEXT NOT NULL DEFAULT 'system',          -- 'system' | 'user' (kept; needed for safe pinned-chat replacement)
  status              TEXT NOT NULL DEFAULT 'sleeping',
  status_message      TEXT,
  last_activity_at    INTEGER,
  wake_counter        INTEGER NOT NULL DEFAULT 0,
  created_at          INTEGER NOT NULL DEFAULT (unixepoch())
);

-- Data migration: collapse agent_sessions rows into one per project_id.
-- For workspaces with multiple rows, prefer the row matching projects.agent_mode;
-- fall back to the lexicographically-first agent_name; archive surplus rows
-- in workspace_sessions_legacy_archive (audit-only).

CREATE TABLE workspace_sessions_legacy_archive AS
  SELECT * FROM agent_sessions;  -- full snapshot for safety net

INSERT INTO workspace_sessions (id, project_id, terminal_id, active_terminal_id, surfaced, session_id, harness, owner, status, status_message, last_activity_at, wake_counter, created_at)
  SELECT id, project_id, terminal_id, active_terminal_id, surfaced, session_id, harness, owner, status, status_message, last_activity_at, wake_counter, created_at
  FROM agent_sessions
  WHERE rowid IN (
    SELECT MIN(rowid) FROM agent_sessions GROUP BY project_id
  );

DROP TABLE agent_sessions;
```

### Migration 0039 — Rename `agent_heartbeats` → `workspace_heartbeats`

Pure rename (the multi-schedule PRD already removed `agent_name` — verified). Rename the index alongside.

```sql
ALTER TABLE agent_heartbeats RENAME TO workspace_heartbeats;
DROP INDEX IF EXISTS idx_agent_heartbeats_project_enabled;
CREATE INDEX idx_workspace_heartbeats_project_enabled
  ON workspace_heartbeats(project_id, enabled);
```

### Migration 0040 — `activity_feed` workspace-keyed audit

```sql
-- Replace agent-keyed columns with workspace-keyed. Schema-level rename
-- + value preservation (agent_name copied into actor for audit
-- continuity; from_agent → from_workspace where resolvable).
ALTER TABLE activity_feed RENAME COLUMN agent_name TO actor;
ALTER TABLE activity_feed RENAME COLUMN from_agent TO from_workspace;
ALTER TABLE activity_feed RENAME COLUMN to_agent TO to_workspace;
-- (sqlite supports RENAME COLUMN since 3.25.0; MIN_VERSION already higher)
```

`actor` becomes a free-form string (`agent`, `user`, `heartbeat`, `cli`, `sms-bridge`, an external workspace path) — generalizes audit for cross-workspace events without losing audit fidelity.

### Migration 0041 — Drop legacy columns

```sql
-- projects.heartbeat_schedule: superseded by workspace_heartbeats since 0028.
-- Verified zero remaining readers via grep.
ALTER TABLE projects DROP COLUMN heartbeat_schedule;

-- projects.agent_mode: KEEP. Workspace-level mode flag (off/manager/k2so-agent/custom)
-- is still load-bearing for AGENT.md template selection on first-time setup
-- and for mode-swap behavior. Per user decision.
```

### Final SQL surface (post-0.37.0)

| Table | Cardinality | Owner |
|---|---|---|
| `projects` | one row per workspace | core |
| `workspace_sessions` | exactly one row per project_id | NEW (was `agent_sessions`) |
| `workspace_heartbeats` | many per project_id | renamed from `agent_heartbeats` |
| `heartbeat_fires` | append-only audit | unchanged (denormalized; `agent_name` column ages out later) |
| `activity_feed` | append-only audit | columns renamed |
| `agent_presets` | global preset list | unchanged |
| `workspace_relations` | cross-workspace links | unchanged |
| `focus_groups` | UI organization | unchanged |
| `_migrations` | applied migration log | unchanged |

The contract: **`workspace_sessions` PK on `project_id` makes the one-agent invariant load-bearing at the schema level.** No application code can violate it.

## Phase 3 — Code refactor

The original PRD estimate was 166 `AgentSession::*` / `AgentHeartbeat::*` call sites. With 0.36.14/15's groundwork, ~30% of those already operate against workspace-keyed plumbing (prefixed strings carrying `project_id`). The Phase 3 refactor is in three parts:

1. **Mechanical column rename** — drop `agent_name` from method signatures, rename helper functions to `workspace_*` forms (≈80 call sites, mostly find-and-replace).
2. **Bridge retirement** — delete the prefixed-string scaffolding from 0.36.14/15 (see section below). Each retirement is a small targeted deletion.
3. **Trait surface change** — `InjectProvider::{inject, is_live}` parameter rename from `agent: &str` to `workspace_id: &str`. Two impls (`DaemonInjectProvider` + a test mock or two) update mechanically.

### Helper rename

```rust
// Before:
pub fn find_primary_agent(project_path: &str) -> Option<String> { … }
pub fn agent_dir(project_path: &str, agent_name: &str) -> PathBuf { … }
pub fn agents_dir(project_path: &str) -> PathBuf { … }

// After:
pub fn workspace_agent_path(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".k2so/agent")
}
pub fn workspace_agent_md_path(project_path: &str) -> PathBuf {
    workspace_agent_path(project_path).join("AGENT.md")
}
pub fn agent_templates_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".k2so/agent-templates")
}
pub fn agent_template_dir(project_path: &str, template_name: &str) -> PathBuf {
    agent_templates_dir(project_path).join(template_name)
}
pub fn workspace_heartbeats_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path).join(".k2so/heartbeats")
}
```

`workspace_agent_path` is **infallible** — every workspace has the dir post-migration. Callers that today pattern-match `Some(name)` simplify to using the path directly.

### DB API surface

The 166 `AgentSession::*` / `AgentHeartbeat::*` call sites collapse to ~80 after dropping the `agent_name` parameter:

```rust
// Before:
AgentSession::upsert(&conn, &uuid, &project_id, "manager", terminal_id, ...);
AgentSession::get_by_agent(&conn, &project_id, "manager")?;

// After:
WorkspaceSession::upsert(&conn, &uuid, &project_id, terminal_id, ...);
WorkspaceSession::get(&conn, &project_id)?;
```

`get_by_terminal_id` keeps existing shape — it's a reverse lookup, not by agent name. `list_by_project` returns at most one row now (could return `Option<WorkspaceSession>`, but keeping `Vec` for one-version source compatibility).

### Awareness bus

`is_agent_live(agent_name, signal)` (current shape, post-0.36.15) becomes `is_workspace_live(project_id)`. The `InjectProvider::is_live(agent: &str)` trait method's `agent` parameter becomes `workspace_id: &str` — same primitive (a string identifier), different semantics. Same for `InjectProvider::inject(agent, bytes)` → `inject(workspace_id, bytes)`. Cross-workspace messaging targets become `project_id` instead of agent names; the awareness bus's signal envelope's `to.name` field is dropped.

### Bridge retirement (the cleanup that makes 0.36.14/15 work redundant)

The hotfixes shipped a *bridge pattern* that uses prefixed strings (`<projectId>:<agentName>`) and bare-name mirrors to retrofit workspace-aware addressing onto an agent-name-keyed substrate. 0.37.0's structural changes make every bridge unnecessary; the cleanup is mechanical:

| Bridge mechanism | Why it exists | What replaces it |
|---|---|---|
| `v2_session_map`'s **bare-name mirror** (0.36.14) — every prefixed-key registration also writes a bare entry as last-write-wins back-compat | Legacy callers (`k2so msg manager` pre-0.36.15, awareness bus inject for sessions registered without workspace context) used bare lookup | Drop the mirror. Every callsite passes `project_id`; the map's key type becomes `(project_id, …)`. `lookup_by_agent_name` retires entirely; the new `lookup_by_workspace(project_id)` is the only lookup. |
| `v2_session_map::register`'s **prefix split** (0.36.14) — derives bare name from `<pid>:<bare>` to populate the mirror | Same as above | Drop. Register takes `project_id` directly. |
| `v2_session_map::unregister`'s **dual-cleanup logic** (0.36.14) — special-cases prefixed keys to scope the SQL UPDATE by `project_id` | Same as above | Drop. UPDATE always scopes by `project_id` because that's the only key. |
| `egress::is_agent_live`'s and `try_inject`'s **prefix-construction-then-lookup** (0.36.15) | Workspace context lives in the signal envelope; the trait methods don't accept it | Trait methods take `project_id` directly. No string concatenation. |
| `DaemonWakeProvider::try_auto_launch`'s **`signal.from.workspace` fallback** (0.36.15) — used when `signal.to.workspace` was missing | Cross-workspace messaging is rare today; `signal.to` may not always be populated correctly | Drop. `signal.to.workspace` is required and authoritative. Validation at ingress; missing `to.workspace` → 400. |
| `AgentChatPane.tsx`'s **`attachAgentName=${projectId}:${agentName}` prop** (0.36.14) | The daemon needed a single string key from the renderer | Drop the agent-name half. Renderer passes `attachWorkspaceId={projectId}` (or just `projectId` since the prop name simplifies). |
| `v2_spawn::handle_v2_spawn`'s **prefix-split before `save_active_terminal_id`** (0.36.14) — strips the prefix because the DB column stored bare name | DB schema lagged behind the registry's keying scheme | Drop. After migration 0038, `workspace_sessions` is keyed on `project_id` directly. |
| `pending_live` **dual-key drain** (0.36.14) — drains under both prefixed and bare keys | Awareness bus enqueued under bare, v2_spawn registered under prefixed | Drop. Enqueue and drain both use `project_id`. |

Net effect: ~150 lines of bridge logic removed across `v2_session_map`, `v2_spawn`, `egress`, `providers`, `AgentChatPane`. Every retirement is a `git revert`-like targeted deletion, not a behavioral change.

### Modules touched (high level)

| Module | Surface area |
|---|---|
| `crates/k2so-core/src/db/schema.rs` | rename `AgentSession` → `WorkspaceSession`, `AgentHeartbeat` → `WorkspaceHeartbeat`, drop `agent_name` parameters |
| `crates/k2so-core/src/agents/mod.rs` | new helper functions (above), retire `find_primary_agent`, `agents_dir` |
| `crates/k2so-core/src/agents/scheduler.rs`, `wake.rs`, `heartbeat.rs`, `session.rs`, `checkin.rs`, `commands.rs`, `delegate.rs`, `reviews.rs`, `skill_writer.rs`, `skill_content.rs` | update to new helpers + DB API |
| `crates/k2so-core/src/awareness/{egress,ingress,roster,signal_format}.rs` | workspace-keyed addressing |
| `crates/k2so-daemon/src/cli.rs`, `terminal_routes.rs`, `companion_routes.rs`, `triage.rs`, `agents_routes.rs` | route handlers updated |
| `src-tauri/src/commands/k2so_agents.rs` | mirror updates |
| `src/renderer/...` (frontend) | Tauri command param shapes; `AgentChatPane`, `AgentPersonaEditor`, sidebar, settings |
| `cli/k2so` | verb redesign (Phase 4) |
| `crates/k2so-core/drizzle_sql/00{38,39,40,41}_*.sql` | new migrations |

### `skill_writer::generate_default_agent_body` updates

The K2SO-shipped default AGENT.md bodies (manager, k2so-agent, custom) need to be refreshed for:
- New CLI verb names (Phase 4 below)
- New filesystem layout references (`.k2so/agent/AGENT.md` instead of `.k2so/agents/<n>/AGENT.md`)
- New work paths (`.k2so/work/inbox` instead of `.k2so/agents/<n>/work/inbox`)

Replaced inline; users who ran the migration get the new bodies on next workspace SKILL regen.

## Phase 4 — CLI redesign

### Verb mapping

| Today | After | Notes |
|---|---|---|
| `k2so msg <agent\|workspace:inbox> "text" [--wake]` | `k2so msg "text" [--workspace <path>] [--inbox]` | workspace-keyed; live default; `--inbox` for async |
| `k2so msg --agent <n> ...` | **removed** | one agent per workspace; agent flag retired |
| `k2so signal <target> <kind> <payload>` | `k2so signal --workspace <path> <kind> <payload>` | workspace-keyed |
| `k2so agents running` | `k2so workspaces running` | listed-by-workspace |
| `k2so agents list` | `k2so workspaces list` | yellow-pages: every workspace + alive/asleep |
| `k2so agents launch` | `k2so workspace launch [--workspace <path>]` | spawn-or-attach |
| `k2so agents work [--agent <n>]` | `k2so work` | workspace inbox (no agent flag) |
| `k2so agent profile <n>` | `k2so workspace profile [--workspace <path>]` | reads `.k2so/agent/AGENT.md` |
| `k2so agent update --name <n> ...` | `k2so workspace update ...` | edits the workspace's one agent |
| `k2so agents create / delete` | **removed** | retired with multi-agent surface |
| `k2so agent template list` | **NEW** `k2so template list` | manage `.k2so/agent-templates/` |
| `k2so agent template create / delete` | **NEW** `k2so template {create,delete}` | scaffold from K2SO defaults |
| `k2so heartbeat *` | unchanged | already workspace-keyed |
| `k2so checkin / done / feed / connections / work create` | unchanged | already workspace-keyed |
| `k2so terminal write <id> "..."` | unchanged | low-level, kept for power users |

Deprecated verbs move to `k2so help-deprecated` (not deleted; printed when invoked, with one-version warning, then removed in 0.38.0).

### `msg` two delivery modes

```bash
# Live (default) — wakes the agent's PTY if needed, injects message
k2so msg "ship it"
k2so msg --workspace /path/to/other "look at issue #42"

# Inbox — drops a notice file in .k2so/work/inbox/, async
k2so msg --inbox "consider this for next sprint"
```

`--exchange` flag (synchronous wait-for-reply) deferred — covered by separate JSONL-tail RPC effort that builds on this branch but ships in 0.37.x.

### `k2so workspaces list` (yellow pages)

```
$ k2so workspaces list
PATH                           AGENT                STATUS    LAST ACTIVITY
/Users/me/dev/foo              foo (custom)         alive     2m ago
/Users/me/dev/k2so             pod-leader (manager) sleeping  3h ago
/Users/me/dev/sms-router       k2so-agent           alive     12s ago
```

Single command surfaces every registered workspace with its agent + liveness + last activity. Replaces the `k2so agents running` + filesystem-grep dance.

## Test rewriting plan (anti-fallback)

All affected tests rewritten to assert the new invariant directly. **No `unwrap_or` / `if let Some` fallbacks** that mask old-shape bugs.

### Schema tests

- `workspace_sessions_unique_per_project` — insert two rows with same `project_id` → expect SQL UNIQUE violation.
- `workspace_heartbeats_renamed_no_agent_name` — `PRAGMA table_info(workspace_heartbeats)` → assert no `agent_name` column.
- `activity_feed_columns_renamed` — `actor`, `from_workspace`, `to_workspace` exist; `agent_name`/`from_agent`/`to_agent` do not.
- `migration_0038_collapses_multi_agent_workspaces` — seed `agent_sessions` with 3 rows for one project_id, run migration, assert exactly 1 row in `workspace_sessions` + 3 rows in `workspace_sessions_legacy_archive`.

### Filesystem tests

- `unification_migrates_primary_agent` — create a workspace with `.k2so/agents/{manager,rust-eng,__lead__}/`, set `agent_mode="manager"`, run migration, assert `.k2so/agent/AGENT.md` exists, `.k2so/agent-templates/rust-eng/AGENT.md` exists, `.k2so/migration/legacy/__lead__/` exists, `.k2so/agents/` removed.
- `unification_merges_per_agent_inbox_into_workspace` — pre-seed `.k2so/agents/manager/work/inbox/foo.md`, run migration, assert `.k2so/work/inbox/foo.md` exists.
- `unification_deletes_template_work_dirs` — pre-seed `.k2so/agents/rust-eng/work/inbox/bar.md`, run migration, assert `.k2so/agent-templates/rust-eng/work/` does NOT exist.
- `unification_idempotent` — run migration twice, assert second run is no-op (sentinel respected).
- `unification_off_mode` — workspace with `agent_mode="off"`, only template+cleanup runs, no primary agent dir created.

### Helper tests

- `workspace_agent_path_returns_pathbuf_not_option` — compile-time signature check.
- `workspace_agent_path_consistent_with_dir` — path returned by helper matches actual on-disk dir post-migration.

### CLI tests

- `msg_default_is_live_delivery` — invoke `k2so msg "test"` against a live workspace, assert PTY received the inject (no inbox file written).
- `msg_inbox_writes_file_only` — invoke `k2so msg --inbox "test"`, assert `.k2so/work/inbox/<file>.md` exists, PTY did not receive inject.
- `msg_no_agent_flag` — invoke `k2so msg --agent foo "test"` (legacy form), assert exit code != 0 with deprecation message pointing to `--workspace`.
- `workspaces_list_yellow_pages` — register 3 workspaces, invoke `k2so workspaces list`, assert all 3 in output with correct status.

### Awareness bus tests

- `signal_workspace_keyed_addressing` — send signal to workspace path, assert delivery to that workspace's session, audit row has `from_workspace`/`to_workspace`.
- `is_workspace_live_walks_v2_map` — register v2 session, query liveness, expect true.

## Rollout

Single 0.37.0 release. No phased rollout — the layers are too interdependent to land separately without flag gymnastics. Migration is on first launch, atomic per workspace, idempotent.

### Release-notes user-facing summary

> 0.37.0 unifies the workspace and its agent into one concept. Your existing workspace's primary agent moves to `.k2so/agent/`; role personas you used for delegation move to `.k2so/agent-templates/`; heartbeats live at `.k2so/heartbeats/`. Originals are archived in `.k2so/migration/legacy/`. The CLI's `msg` verb is workspace-keyed: `k2so msg "..."` (live) and `k2so msg --inbox "..."` (async). New `k2so workspaces list` shows every workspace with its agent + liveness in one command.

### Risk + rollback

- **Worst-case scenario:** migration corrupts a workspace's `.k2so/`. Mitigation: full snapshot to `.k2so/migration/legacy/` before mutating; sentinel `.unification-0.37.0-done` blocks re-run; `MIGRATION-0.37.0.md` documents manual recovery.
- **DB rollback:** every renamed table has a `_legacy_archive` snapshot. Rolling back to 0.36.15 means restoring those tables; documented in the migration script.
- **Per-user opt-out:** none. The unification is the product invariant.

## Open questions / forward-compat

- **User-global agent-skills repository (`~/.k2so/agent-templates/`)** — v2 follow-up. Workspace-local templates inherit/override from user-global ones. Surface gives users a "library" of role personas to import.
- **Agent-skills enable/disable per workspace** — v2. Today every template in `.k2so/agent-templates/` is referenceable; the user wanted control over which are "active" for the workspace's agent. Likely a manifest at `.k2so/agent-templates/manifest.json` listing enabled templates.
- **Worktree as full child workspace** — explicitly deferred. Current model (lightweight, parent-only addressable) preserved.
- **Synchronous `msg --exchange` (JSONL-tail-and-return)** — deferred to 0.37.x follow-up. Builds on this branch.
- **Default-profile fallback for offline workspace agents (potential 0.36.16)** — when `load_launch_profile` returns `None`, synthesize a default (`claude --dangerously-skip-permissions [--resume <id>]`, cwd = project root). Surfaced by 0.36.15's correct routing — `k2so msg --wake` against an offline agent now reaches the wake provider, which currently bails for workspaces without an explicit AGENT.md launch YAML. Aligned with 0.37.0's design (workspace's agent has known location, sensible default behavior). If shipped pre-0.37.0 as 0.36.16, the synthesizer's path strings (`.k2so/agents/<a>/` → `.k2so/agent/`) update during the unification rename — but the synthesizer logic itself stays. Not a blocker for starting 0.37.0; can land in either order.

## Definition of done

1. All four DB migrations land + tests pass.
2. Filesystem migration runs idempotently on first 0.37.0 launch + tests pass.
3. New CLI verbs ship + deprecated verbs warn-redirect + tests pass.
4. K2SO's own `.k2so/` migrates cleanly when the dev runs 0.37.0 against this repo (`.k2so/agent/` = pod-leader content, `.k2so/agent-templates/{rust-eng,frontend-eng,qa-eng,cli-eng}/`, etc.).
5. `k2so msg "..."` (live) and `k2so msg --inbox "..."` both work end-to-end against a live workspace.
6. `k2so workspaces list` returns every registered workspace with correct agent + liveness.
7. `find_primary_agent` removed from codebase; `workspace_agent_path` infallible everywhere.
8. No test passes via `unwrap_or` / fallback — verified by grep audit on the test suite.

End state: a workspace IS its agent. The product invariant is the schema constraint. The CLI surface is half its current size, every remaining verb is one of {`workspace`, `work`, `template`, `heartbeat`, `msg`, `signal`, `checkin`, `done`, `feed`, `connections`, `terminal`, `chat`, `delegate`, `commit`, `reviews`, `mode`, `settings`, `daemon`, `state`, `agentic`, `hooks`, `signal`}, and the dead overlap between agent and workspace is gone.
