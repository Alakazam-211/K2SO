---
title: Multi-heartbeat architecture (SKILL-style folder-per-heartbeat)
status: signed-off
created: 2026-04-17
signed_off: 2026-04-17
authors: [pod-leader, cortana, rosson]
---

# Multi-heartbeat architecture

## Problem

Today a workspace has exactly **one** heartbeat schedule (`projects.heartbeat_schedule`) and — by convention — one `wakeup.md`. An agent with multiple workflows on different cadences has to cram them all into that single wakeup file and branch on `date +%u`-style logic. That works, but the costs are real:

- **Prompt pollution** — daily-brief context and weekly-financial-report context end up in the same turn's system prompt even when only one is needed. Large workflows (Cortana's Teller-pull + pipeline + writeup) balloon every daily wake, burning tokens and distracting the model.
- **Hard to reason about** — one giant branching `wakeup.md` is harder to edit, review, and version than N small focused ones.
- **No clean audit trail** — `heartbeat_fires` records a wake but not *which workflow* ran. Hard to see "did the weekly report actually execute this Sunday?" at a glance.

Surfaced during Cortana's design of daily brief + Sunday financial report in session `aeacb6e3-bb5d-436f-801e-6dc288e96fe7` on 2026-04-17.

## Invariants we're preserving

- **One top-tier agent per workspace** — Custom, K2SO Agent, or Workspace Manager. Mutually exclusive. Heartbeats implicitly bind to whichever that is; no `--agent` flag needed.
- **Agent-templates never get heartbeats** — they're delegated on-demand by the Workspace Manager.
- **Friendly-keyword frequencies only in v1** — daily / weekly / monthly / yearly with time and day/month params. `--cron` deferred as future escape hatch.

## Core design: folder-per-heartbeat (SKILL pattern)

Each heartbeat is a **named folder** containing its own `wakeup.md`. The folder name IS the heartbeat identifier. This mirrors the SKILL pattern (`<name>/SKILL.md`) users already understand, and lets each heartbeat grow siblings over time without inventing new conventions.

### Filesystem hierarchy

```
.k2so/agents/<agent>/
├── agent.md                        # core identity (persona)
├── wakeup.md                       # LEGACY — single-heartbeat fallback
└── heartbeats/
    ├── daily-brief/
    │   └── wakeup.md               # folder name = heartbeat identifier
    ├── weekly-financial/
    │   └── wakeup.md
    └── monthly-payroll/
        └── wakeup.md
```

Every heartbeat wakeup file is named `wakeup.md` — no slug-to-file-name mapping to remember. Edit experience is identical whether the user has 1 heartbeat or 50.

Future expansion lives inside each heartbeat folder: `heartbeats/daily-brief/state.json`, `heartbeats/daily-brief/runs/`, etc. We don't build that yet, but the shape doesn't need to change when we do.

### Legacy `wakeup.md`

Migrations move the legacy file to `heartbeats/default/wakeup.md`. The agent-root `wakeup.md` is deleted post-migration. One pattern, no exceptions. Rollback is a move + DB row deletion.

## CLI

`heartbeat` is the canonical noun (users already say "heartbeats are enabled"). Existing single-slot syntax stays as a deprecation-marked alias for one release.

```
k2so heartbeat add --name daily-brief \
    --daily --time 07:00

k2so heartbeat add --name weekly-financial \
    --weekly --days sun --time 07:00

k2so heartbeat add --name payroll-check \
    --monthly --days 15 --time 08:00

k2so heartbeat list                 # name, schedule, next fire, last fired, enabled
k2so heartbeat show <name>          # one heartbeat's full config
k2so heartbeat remove <name>        # removes DB row + folder (with confirm)
k2so heartbeat enable <name>
k2so heartbeat disable <name>
k2so heartbeat edit <name> [flags]  # change schedule without deleting/re-adding
k2so heartbeat fire <name>          # manually fire a named heartbeat now (debugging)
k2so heartbeat status <name>        # last 5 fires with decision + reason (observability)
```

`k2so heartbeat add` creates `.k2so/agents/<agent>/heartbeats/<name>/wakeup.md` from a default template, scaffolds the DB row, and prints the path so the user can immediately edit.

Frequency keywords mirror today's single-slot `heartbeat schedule` so users don't re-learn syntax: `--daily --time HH:MM`, `--weekly --days mon,tue --time HH:MM`, `--monthly --days 1,15 --time HH:MM`, `--yearly --months jan,apr --days 1 --time HH:MM`.

**The `--wakeup <file>` flag is gone.** The heartbeat *name* determines the file location (`heartbeats/<name>/wakeup.md`), matching the SKILL pattern. One less thing to configure and no room for name/file drift.

### Name validation

Heartbeat names are strict: **lowercase letters, digits, and hyphens only** (`^[a-z][a-z0-9-]*[a-z0-9]$`). This ensures:
- Folder names collide predictably on APFS (case-insensitive by default) — `Daily-Brief` and `daily-brief` resolving to the same directory would be a real bug source
- Paths stay URL-safe if we ever surface them in the web/mobile companion
- No surprises with shell quoting

Reserved names: `default` (claimed by migration), `legacy` (reserved for future transition needs). CLI rejects both with a clear error at `add`.

### `k2so heartbeat fire <name>` — manual trigger for debugging

Forces an immediate spawn of the named heartbeat's wakeup regardless of schedule. Bypasses `should_heartbeat_fire` but still respects `is_agent_locked` (won't double-launch). Writes a `heartbeat_fires` row with `decision='fired_manual'` so manual fires are distinguishable from scheduled ones in the audit trail. Indispensable for iterating on a new wakeup.md without waiting for the natural schedule.

### `k2so heartbeat status <name>` — observability for debugging

Shows the last 5 fires (or `--limit N`) for a given heartbeat with timestamp, decision, reason, and duration. Filters `heartbeat_fires` by `schedule_name = <name>`. Answers "did my weekly report actually fire last Sunday, and if not, why?" in one command instead of SQL grep. Without a per-name filter, diagnosing a single heartbeat in a multi-heartbeat workspace turns into hunt-and-peck through the full audit log — `status <name>` makes each heartbeat individually inspectable.

## UX: Persona editor changes

### AIFileEditor collapses back to single-file

CLAUDE.md files are symlinked to `agent.md`, so the multi-file watching we added was solving a problem that no longer exists in the final shape. Collapse `AIFileEditor` back to **one target file per edit session**. One file = one AI = one focused context = one edit experience reused for persona editing AND per-heartbeat wakeup editing.

### Heartbeats panel in the workspace Settings page

Workspace Settings gains a "Heartbeats" section with a three-column table listing every heartbeat for the workspace's agent:

```
┌──────────────────────────────────────────────────────────────────────────┐
│ Heartbeats                                                     [+ Add]   │
├──────────────────┬─────────────────────────────────┬─────────────────────┤
│  Heartbeat Name  │  Schedule                       │  Configure Wakeup   │
├──────────────────┼─────────────────────────────────┼─────────────────────┤
│  daily-brief     │  Every day at 07:00             │  [ Edit wakeup.md ] │
│  weekly-financial│  Sundays at 07:00               │  [ Edit wakeup.md ] │
│  payroll-check   │  Day 15 of each month at 08:00  │  [ Edit wakeup.md ] │
└──────────────────┴─────────────────────────────────┴─────────────────────┘
```

- **Heartbeat Name** — the identifier (matches the folder name under `heartbeats/`). Rename via `[+ Add]` + `Remove` in v1; an inline rename action can come later.
- **Schedule** — human-readable rendering of the frequency + spec. The edit control (modal when the user clicks or adds) is a **GUI picker** — frequency dropdown (daily / weekly / monthly / yearly), day selectors, time picker — producing the same `spec_json` the CLI writes. Users never type cron expressions. Internally the spec is cron-equivalent (we can render as cron for advanced users later, but the primary representation is the structured picker).
- **Configure Wakeup** — button that opens the `AIFileEditor` (single-file mode) on `wakeup_path` for that row. The editor's AI terminal gets the enriched system prompt described below: full persona + summary of the *other* heartbeats + permission to `cat` them on demand.

Row-level actions not shown in the table body: enabled toggle (inline switch), last-fired timestamp (small hover tooltip or secondary row), delete (confirmation modal). These are UI polish decisions that don't affect the data model.

### AI context when editing a heartbeat

When `AIFileEditor` is editing a specific heartbeat's `wakeup.md`, the system prompt gives the AI awareness of:

1. **Full persona** (`agent.md`) — the core identity. The AI has to understand who the agent *is* before writing instructions for one of its jobs.
2. **Summary list of other heartbeats** — each shown as name + schedule + one-line description (extracted from frontmatter `description` field or the first paragraph of each `wakeup.md`). **Not full content** — that would balloon prompt size as heartbeats accumulate.
3. **A note that the AI can `cat` any other heartbeat's `wakeup.md` on demand** if it needs details to avoid duplicating work or check for conflicts.

This lets the AI catch things like:
- "You already have a `daily-brief` heartbeat that drafts email replies — do you want this new `morning-triage` to hand off to it instead?"
- "Your persona says Cortana only drafts, never sends — this wakeup asks her to send directly. Intentional?"

Per-heartbeat `wakeup.md` should start with a frontmatter `description:` one-liner for exactly this purpose. Template ships with it.

## Data model

New `agent_heartbeats` table:

```sql
CREATE TABLE agent_heartbeats (
  id            TEXT PRIMARY KEY,           -- uuid
  project_id    TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
  name          TEXT NOT NULL,              -- user-supplied, unique per project, == folder name
  frequency     TEXT NOT NULL,              -- daily | weekly | monthly | yearly | hourly
  spec_json     TEXT NOT NULL,              -- {time, days, months, ...} params
  wakeup_path   TEXT NOT NULL,              -- path to the wakeup.md this heartbeat fires
  enabled       INTEGER NOT NULL DEFAULT 1,
  last_fired    TEXT,                       -- RFC3339
  created_at    INTEGER NOT NULL DEFAULT (unixepoch()),
  UNIQUE (project_id, name)
);
CREATE INDEX idx_agent_heartbeats_project_enabled ON agent_heartbeats(project_id, enabled);
```

Notable shape choices:
- **No `agent_name` column.** The workspace's one top-tier agent is implicit. If we ever lift that invariant, we add the column then.
- **`wakeup_path` is explicit, not derived.** `k2so heartbeat add` populates it with the conventional location (`.k2so/agents/<agent>/heartbeats/<name>/wakeup.md`) and creates the folder, but the scheduler always reads the stored path — no "compute-from-name" logic in the hot path. This is the single source of truth for what prompt fires when this heartbeat wakes. Stored as workspace-relative (e.g. `.k2so/agents/cortana/heartbeats/daily-brief/wakeup.md`) so project moves don't break rows.

The stored-path approach also unlocks later flexibility we don't need yet but cost nothing now: a heartbeat could point at a shared wakeup file outside the usual location (reused across agents, pulled from a library, etc.) without a schema change. The convention stays the default; the column allows override.

`heartbeat_fires.schedule_name` (added in migration `0029`) is a denormalized `TEXT` column, **not a foreign key** to `agent_heartbeats(name)`. The audit trail must survive heartbeat deletion — if a user removes `weekly-financial`, we still want `heartbeat_fires` to show that `weekly-financial` fired on March 15th. Implementors: do not turn this into a FK by reflex.

The existing `projects.heartbeat_schedule` column stays during transition and is deprecated. Migration copies any populated value into an `agent_heartbeats` row named `default` and moves the legacy `wakeup.md` file into `heartbeats/default/wakeup.md`.

## Scheduler loop changes

`k2so_agents_scheduler_tick` currently:
1. Reads workspace-level `heartbeat_schedule`
2. Decides if it's time to fire
3. Picks the workspace's agent to wake
4. Spawns with `wakeup.md` as the prompt

New loop:
1. Query `agent_heartbeats WHERE project_id = ? AND enabled = 1`
2. For each row, evaluate `should_heartbeat_fire(frequency, spec, last_fired)` (existing `should_project_fire` logic, per-row)
3. For matching heartbeats: skip if the workspace's agent is already locked; otherwise `spawn_wake_pty` reading from `heartbeats/<name>/wakeup.md`
4. Stamp `last_fired` on the heartbeat row **only on successful spawn**
5. Audit each decision into `heartbeat_fires` with `schedule_name = heartbeat.name`

`spawn_wake_pty` + `compose_wake_prompt_for_agent` grow a `wakeup_path: Option<String>` parameter. When present, the wake prompt is loaded from exactly that workspace-relative path. When `None`, falls back to the legacy agent-root `wakeup.md` (should be gone after migration; kept as a safety net). The scheduler passes `heartbeat.wakeup_path` straight through — no path computation at wake time.

### `last_fired` semantics

Stamped **only on successful spawn**, never on the decision to fire. If a heartbeat is eligible at T but the agent is locked, `heartbeat_fires` records `decision='skipped_locked'` and `last_fired` is NOT stamped. The heartbeat stays eligible for the next tick. Prevents silent drops like "my weekly report never fired because daily-brief ran long."

## Migration

- `0028_agent_heartbeats.sql` — new table
- `0029_heartbeat_fires_schedule_name.sql` — `ALTER TABLE heartbeat_fires ADD COLUMN schedule_name TEXT`
- One-time startup migration (Rust, runs once, flag-gated):
  - For every `projects` row with non-null `heartbeat_schedule`:
    1. Create `.k2so/agents/<agent>/heartbeats/default/` directory
    2. Move existing `.k2so/agents/<agent>/wakeup.md` → `heartbeats/default/wakeup.md` (if present)
    3. Insert one `agent_heartbeats` row named `default`, copying frequency/spec, with `wakeup_path = .k2so/agents/<agent>/heartbeats/default/wakeup.md`
  - For workspaces with heartbeat disabled: no-op, no directory created
- `projects.heartbeat_schedule` write paths redirect to `agent_heartbeats`; column dropped in a future release

## CLI canonicalization

`k2so heartbeat <verb>` is the canonical surface going forward. Both the old single-slot `k2so heartbeat schedule daily --time X` AND the shape introduced in this PRD (`heartbeat add|list|remove|...`) use the same top-level `heartbeat` noun. No confusing rename to `k2so schedule`. Help text and docs reflect the `add|list|remove|enable|disable|edit|show` verbs from day one.

## Edge cases

| Case | Resolution |
|---|---|
| Last business day of month | `heartbeats/close/wakeup.md` early-exits on non-matching dates. No new primitive. |
| Biweekly | `heartbeats/biweekly/wakeup.md` early-exits on alternating weeks. |
| First Monday of month | Same pattern. |
| Two heartbeats fire in the same tick | Second skips with `is_agent_locked` (one agent, one running session at a time). `heartbeat_fires` records the skip reason with `schedule_name`. |
| User disables at workspace level | `projects.heartbeat_enabled = 0` is a master kill switch; individual `agent_heartbeats.enabled` controls finer granularity. |
| Agent-mode swap pollutes the workspace | Separate bug: `.k2so/work/inbox/agent-mode-swap-cleanup.md`. Also cleans up `heartbeats/` directories on mode swap. |
| In-progress `active/` work conflicts with a new wake | Each `wakeup.md` is responsible for reconciling with the agent's `active/` queue before starting fresh work. Already true for single `wakeup.md`; multi-heartbeat makes the conflict more visible. Per-heartbeat prompts should be defensive (check `active/` first, resume vs. start new). |
| Heartbeat removed with fires in audit | `heartbeat_fires.schedule_name` is denormalized TEXT; audit rows survive. |
| Rename a heartbeat | v1: remove + re-add (folder rename is destructive via FS move). Could add `k2so heartbeat rename <old> <new>` later. |
| User manually deletes a heartbeat folder | DB row outlives the folder. On next matching tick, scheduler reads `wakeup_path`, finds missing file → log a warning, write `heartbeat_fires` row with `decision='wakeup_file_missing'`, auto-disable the heartbeat (`enabled=0`) so the user sees it needs attention in `k2so heartbeat list` instead of silently skipping every tick. A subsequent `k2so heartbeat enable <name>` re-checks and either succeeds (file restored) or fails loudly. |
| User manually renames a heartbeat folder outside the CLI | Equivalent to "deleted" from the scheduler's perspective — `wakeup_path` now stale. Same auto-disable recovery path. User can fix via `k2so heartbeat edit <name> --wakeup-path <new>` (escape hatch) or delete + re-add. |
| Heartbeat removed mid-run | The running PTY is keyed on `terminal_id`, not heartbeat name — it finishes cleanly. When the Stop hook fires and the scheduler tries to stamp `last_fired` on the now-gone row, the UPDATE affects 0 rows — handled as a silent no-op in the Rust helper. No crash, no orphan stamp. `heartbeat_fires` rows from that run remain (audit survives by design). |
| Name validation conflicts | See CLI section: reserved names (`default`, `legacy`) rejected at `add`; strict lowercase-hyphens-digits pattern enforced; APFS case-insensitivity surfaces as an explicit uniqueness check against existing lowercase names. |
| Agent-mode swap pollutes `heartbeats/` | The mode-swap cleanup bug (`.k2so/work/inbox/agent-mode-swap-cleanup.md`) is updated to cover `heartbeats/` folders AND `agent_heartbeats` rows. When a workspace's agent mode changes, the prior agent's heartbeats are archived alongside the agent's other directories. Otherwise a Custom → Manager swap leaves orphan cron slots pointing at dead wakeup paths. |

## Out of scope for v1

- `--cron` raw expressions
- Heartbeat-to-heartbeat dependencies ("don't run B if A ran today")
- Per-heartbeat time zones other than system-local
- Scheduling agent-templates
- Per-heartbeat sibling files (state.json, runs/, etc. — structure allows them, but implementation deferred)
- Heartbeat rename (v1 is remove + re-add)

## Test plan

### Unit
- `should_heartbeat_fire` with each frequency keyword × various `last_fired` values
- Migration creates `default` heartbeat row correctly AND moves `wakeup.md` to the right location
- Migration is idempotent (second run is a no-op)

### Tier 2 (live pipeline)
- Create 3 heartbeats via CLI, trigger `/cli/scheduler-tick`, assert each fires at the right moment, `last_fired` stamped per-row, `heartbeat_fires` rows include correct `schedule_name`
- **Lock-skip preserves eligibility**: heartbeat eligible at T with agent locked → `heartbeat_fires` records `skipped_locked`, `last_fired` NOT stamped → agent unlocks mid-tick → next tick fires the heartbeat and THEN stamps `last_fired`
- **Concurrent-tick**: two heartbeats same fire time (`daily-brief 07:00` + `weekly-financial 07:00 Sunday`) → first spawns, second sees lock + skips with `decision='skipped_locked'` → first's Stop fires → next tick fires the second. Both `heartbeat_fires` rows land with correct `schedule_name`
- Migration integration: create a project with legacy `heartbeat_schedule` + `wakeup.md`, run migration, assert DB row + folder + moved file + no residual `wakeup.md` at agent root

### Tier 3 (structural)
- CLI rejects duplicate `--name`, rejects unknown frequency, rejects adding a heartbeat when workspace has no scheduleable agent
- `k2so heartbeat add <name>` creates `heartbeats/<name>/wakeup.md` from template
- `k2so heartbeat remove <name>` deletes DB row AND folder (confirms with user first)
- Persona editor: AIFileEditor single-file mode (no `files` prop, no `showTabs`)
- Persona editor: Heartbeats panel renders one row per `agent_heartbeats` row, clicking opens single-file editor for that heartbeat's `wakeup.md`
- AI system prompt includes persona + heartbeat summaries (not full other-heartbeat content)

## Sign-off trail

- 2026-04-17 `pod-leader` initial draft (flat `wakeup-<slug>.md` file-naming scheme)
- 2026-04-17 `cortana` sign-off with 5 tightenings (canonical CLI name, `last_fired` semantics, active-work reconciliation, concurrent-tick test, `schedule_name` denormalization)
- 2026-04-17 `rosson` pivots to SKILL-style folder-per-heartbeat (`heartbeats/<name>/wakeup.md`), AIFileEditor collapse to single-file, Heartbeats panel in Persona editor, AI context shape (persona + summaries). Canonical CLI noun is `heartbeat` not `schedule`. All folded in above.
- 2026-04-17 `rosson` clarifies SQL stores the wakeup path explicitly (`wakeup_path` column) — convention-derived was too clever; explicit path is the single source of truth for the scheduler and unlocks future override flexibility (shared wakeups, library wakeups) at zero additional schema cost.
- 2026-04-17 `rosson` specifies Heartbeats Settings-page UX — three-column table (Heartbeat Name, Schedule, Configure Wakeup) with GUI-based schedule picker (no raw cron typing), "Configure Wakeup" button launching `AIFileEditor` single-file mode on `wakeup_path`. Display is human-readable, underlying spec is cron-equivalent, editing is structured widgets.
- 2026-04-17 `rosson` + `pod-leader` stress-test edge cases and add: FS-tampering recovery (auto-disable on missing wakeup), strict name validation (lowercase-hyphens, reserved words blocked), mid-run deletion no-op for `last_fired` stamp, agent-mode-swap cleanup extended to `heartbeats/` + `agent_heartbeats` rows, manual-fire CLI (`k2so heartbeat fire <name>`), per-heartbeat status CLI (`k2so heartbeat status <name>`) for observability/debugging.

## Open questions

None after 2026-04-17 alignment. Green-lit for implementation when prioritized.
