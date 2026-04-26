# K2SO 0.36.0 — Multi-heartbeat workspaces + scheduler rewrite

This release ships the full **per-heartbeat sessions** feature
(P1–P4) and the **scheduler reliability rewrite** (P5) that made
it actually fire on time. After upgrading, every workspace can
have multiple named heartbeats, each with its own dedicated chat
session, and the daemon-side cron will fire them at their
configured cadence — including sub-5-minute schedules.

The headline fix: **heartbeats now reliably fire from cron.**
0.35.x had three latent bugs that combined to make scheduled
fires silent on most workspaces — see "Why heartbeats weren't
firing" below.

## What changed (user-visible)

### Heartbeats sidebar panel + per-heartbeat chats

Every workspace now shows a **Heartbeats** section in the
Workspace tab. Each row has:
- A status indicator (live / resumable / scheduled / archived)
- The heartbeat's name + compact schedule summary ("Daily 9 AM",
  "Every 30m", "Mon/Wed 7 AM", etc.)
- A **Launch** button that fires the wakeup immediately

Clicking the row opens or focuses the heartbeat's chat — each
heartbeat keeps its own dedicated Claude session distinct from
the agent's primary chat. Past sessions are resumable from the
sidebar entry even after the daemon restarts.

### Smart launch (Launch button + cron + CLI converge)

A single decision tree handles every wake path:

1. **Fresh fire** — no saved session yet → spawn a new PTY with
   the WAKEUP.md as `--append-system-prompt`.
2. **Inject** — saved session is currently running → write the
   wakeup body into the live PTY's stdin so it lands as a turn
   message in the existing chat.
3. **Resume + fire** — saved session exists, no live PTY →
   spawn fresh with both `--resume <session_id>` and
   `--append-system-prompt <body>`.

This logic lives in the daemon (`heartbeat_launch.rs`), so the
Launch button, the `k2so heartbeat launch` CLI verb, and the
scheduler tick all converge on identical behavior.

### Workspace panel cleanup

The top of the Workspace tab now shows two static labels:

```
Workspace Type  Manager
Agent Name      sarah
```

Replaces the legacy heartbeat-status indicator (deprecated now
that workspaces can have multiple heartbeats). The agent-name
resolution is mode-aware — Sarah workspace correctly shows
`sarah` instead of the alphabetical-first agent.

## Why heartbeats weren't firing (and what P5 fixed)

The pre-0.36 scheduler had four compounding bugs:

### 1. Stale projects-list file

`~/.k2so/heartbeat-projects.txt` was the gate — if a workspace
wasn't in this file, cron skipped it forever. The file was only
written by the Tauri command `k2so_agents_install_heartbeat`,
which ran once at first install. New workspaces, archived rows,
and changed schedules never updated it.

**P5.6 fix:** new endpoint `/cli/heartbeat/active-projects`
returns the list directly from `agent_heartbeats` on every
tick. heartbeat.sh queries the daemon instead of reading a
file. The legacy `heartbeat-projects.txt` is removed at daemon
boot.

### 2. Frequency mode mismatch — daily/weekly/monthly never fired

`agent_heartbeats.frequency` stored values like `"daily"` or
`"weekly"`, but `scheduler.rs::should_project_fire` only
matched `"hourly"` and `"scheduled"`. The match arm fell
through to `_ => false`, silently skipping every daily and
weekly heartbeat from cron. Manual Launch button clicks worked
because they bypass the schedule check.

**P5.5 fix:** `daily`/`weekly`/`monthly`/`yearly` are now
aliased to `scheduled` at the boundary, so legacy data and
cron tick converge without any DB migration.

### 3. launchd `StartInterval=300` floored cadence at 5 minutes

Even with the projects file populated, sub-5-minute schedules
(e.g. hourly with `every_seconds=120`) couldn't fire faster
than the launchd tick. A heartbeat configured for "every 2
minutes" would fire every 5 at best.

**P5.7 fix:** default `wake_scheduler.interval_minutes`
dropped from 5 to 1. New installs tick at 60s. Existing users
can set 1 in Settings → Wake Scheduler. Empty ticks return in
microseconds, so the cost increase is negligible.

### 4. No concurrency control — overlapping ticks could double-fire

The pre-0.36 path had a TOCTOU race: scheduler eval checked
`is_agent_locked` at one moment, spawn ran later. Two ticks
arriving in the same window could both pass the check and both
spawn, leading to duplicate Claude processes for the same
heartbeat.

This race was masked by `StartInterval=300` (ticks were 5min
apart). Dropping cadence to 60s would have exposed it.

**P5.2 fix:** new `try_acquire_heartbeat` CAS in
`agent_heartbeats` using `BEGIN IMMEDIATE`. A 20-thread
contention test (`db::schema::concurrency_tests::try_acquire_heartbeat_exactly_one_winner_under_parallel_contention`)
proves exactly one winner under load.

## Scheduler architecture (for the curious)

Six new daemon-side primitives, all derived from K8s CronJob /
River / Oban:

| Primitive | Source pattern | What it does |
|---|---|---|
| `concurrency_policy` (`forbid`/`allow`/`replace`) | K8s `concurrencyPolicy` | Per-row toggle for "what if previous fire is still running?" |
| `starting_deadline_secs` | K8s `startingDeadlineSeconds` | Skip a fire that's more than N seconds late (default 600s) |
| `active_deadline_secs` | K8s `activeDeadlineSeconds` | Per-spawn timeout (default 30s) — wraps `smart_launch` in `tokio::time::timeout` |
| `in_flight_started_at` lease | River / Oban | RFC3339 timestamp; cleared by `stamp_fired_and_release` on success or boot-time `sweep_stale_leases` (5 min) |
| `Semaphore::new(6)` + `JoinSet` | tokio | Bounded parallel fan-out over candidates per tick |
| `BEGIN IMMEDIATE` CAS | SQLite | Atomic check-and-set of the in-flight lease — closes the TOCTOU race |

Each primitive is independently revertable. Phased rollout
recorded in `~/.k2so/k2so.db` migrations 0034–0035.

## Test coverage

Total: **283 tests pass** (was 276 before P5).

New scheduler / concurrency tests:
- `try_acquire_heartbeat_exactly_one_winner_under_parallel_contention` — 20 threads race to claim the same heartbeat row; exactly one wins.
- `try_acquire_heartbeat_release_allows_reacquire` — full acquire → release → re-acquire cycle.
- `stamp_fired_and_release_clears_lease` — success path stamps `last_fired` and clears the lease atomically.
- `sweep_stale_leases_clears_old_in_flight_rows` — boot-time recovery from a daemon that crashed mid-spawn.
- `try_acquire_heartbeat_allow_policy_skips_lease_check` — `concurrency_policy='allow'` permits parallel spawns.
- `daily_mode_aliases_to_scheduled_and_fires` — regression test for the silent-skip bug above.
- `weekly_mode_aliases_to_scheduled` — same pattern for weekly schedules.

## Migration notes

- **Schema migration `0035_heartbeat_concurrency_policy.sql`** runs
  automatically on first daemon boot post-upgrade. Adds four
  columns to `agent_heartbeats` with safe defaults that preserve
  current behavior.
- **`heartbeat-projects.txt` deleted** on first daemon boot — the
  daemon scans `agent_heartbeats` directly now.
- **launchd plist NOT auto-reloaded** — existing users who want
  the faster cadence should open Settings → Wake Scheduler and
  set interval to 1 minute (or click Apply with their current
  settings to refresh the plist).
- **No frontend changes required** — the heartbeats UI was already
  shipped in P3 (0.35.x).

## Rollback

Each P5 phase is independently revertable:
- Phase 1 (schema): no reversal needed; columns sit unused if
  P5.2+ are reverted.
- Phase 2 (CAS): revert `heartbeat_launch.rs` and `triage.rs`;
  helpers stay in `schema.rs` as dead code.
- Phase 3 (lease lifecycle): same as P5.2.
- Phase 4 (bounded pool): revert the `run_candidates_bounded`
  function; the serial loop is restored.
- Phase 5 (policy + frequency aliasing): revert
  `scheduler.rs` + `heartbeat.rs` aliasing; daily/weekly stop
  firing again (preserves pre-0.36 behavior, broken as it was).
- Phase 6 (projects.txt deprecation): revert
  `k2so_agents.rs` writer + `triage.rs` endpoint; heartbeat.sh
  template falls back to projects.txt iteration.
- Phase 7 (StartInterval): change default back to 5.
