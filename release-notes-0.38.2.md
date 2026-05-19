# 0.38.2 — Heartbeat scheduler: replaced hand-rolled with `croner`

Closes the silent-failure bug class users have been hitting where
`k2so heartbeat list` reports `Enabled: yes` and `Last Fired: 22 days ago`
forever, with no error surfaced anywhere. C3PO issue `80ef415f`. Also
fixes the daily/scheduled variant of the same bug (Cortana, Sarah).

## The bug

Our hand-rolled `is_past_deadline` skipped any fire whose `lateness`
exceeded a 600-second grace window. Designed for small slips —
concurrency lock, brief crash, ~10-minute slop. Wrong model for long
pauses: after one missed cycle, lateness only grows; every subsequent
tick skips. **The heartbeat goes permanently dark while the scheduler
happily ticks `skipped_deadline` every interval.**

Verified live on the K2SO daemon's own DB before the fix:

```
11 workspaces  ·  triage hourly  ·  last fired 22+ days ago  ·  enabled=yes
2 workspaces   ·  daily          ·  last fired 38h–64h ago   ·  enabled=yes
```

## What we did

**Replaced our hand-rolled scheduler decision logic with [`croner`](https://crates.io/crates/croner)**
— a battle-tested Rust cron-expression library. Croner owns the time
math; we own everything else (SQL persistence, concurrency policy,
schedule-window guard, audit log).

The new model is dead simple: **"is `now >= next_fire_time`? if yes, fire."**
No deadline. No "fire is 1,974,326 seconds late." No skip-because-late.
Long pauses recover automatically — after a 22-day gap the next
scheduled time is way in the past, the comparison is trivially true,
fire.

Daily/weekly/monthly/yearly heartbeats translate to standard 5-field
cron expressions (`0 9 * * *` for "daily at 09:00", etc.) and ask
croner for the next occurrence. Hourly heartbeats with `every_seconds`
stay direct: `last_fired + every_seconds`.

## What's preserved

We kept the surrounding scheduler infrastructure intact:

- `should_project_fire` schedule-window guard (time-of-day restrictions)
- `concurrency_policy = forbid` (skip if already in flight)
- `workspace_state.heartbeat_mode = off` (per-workspace disable)
- `heartbeat_fires` audit table (new decision `not_due` replaces the old `skipped_deadline`)
- `last_fired` SQL persistence
- The `starting_deadline_secs` column on `workspace_heartbeats` (left in schema for backward compat, no longer read)

## Architecture invariant

After this release **no code path in our scheduler can produce
"heartbeat is permanently dark because the grace window expired."**
That class of bug is structurally impossible — the deadline concept
is gone.

## Verified end-to-end

After daemon restart with the new code:

- **Sarah daily** — `fired: smart_launch: resumed session, fired wakeup` ✅
- **Cortana daily-email-brief** — `fired: smart_launch: resumed session, fired wakeup` ✅
- **Previously-dark triage hourly heartbeats** across peliguard-web, SarahAI, nsi-plan01, R2D2, K2SO-website — all fired ✅
- **Zero `skipped_deadline` events** post-restart (was 2–13 per minute pre-restart)

## Tests

- 7 new unit tests in `cron_schedule.rs` including the explicit `hourly_recovers_from_22_day_pause` case
- All 31 existing heartbeat tests still pass

## Known follow-ups (not in this release)

- `failed to compose wake prompt` errors for BIG-CRM, TestingK2SO `manager`, alakazam-labs-website — separate code path, prompt-render failure not related to scheduling
- The `should_project_fire` schedule-window guard's start/end hours may still gate the 2-minute `fast-test` heartbeat during specific hours — worth a separate look

## Files touched

| File | Role |
|---|---|
| `crates/k2so-core/Cargo.toml` | Add `croner = "3"` |
| `crates/k2so-core/src/agents/cron_schedule.rs` | NEW: `is_due`, `next_fire_time_after`, `build_cron_expression` |
| `crates/k2so-core/src/agents/mod.rs` | Register new module |
| `crates/k2so-core/src/agents/heartbeat.rs` | Replace `is_past_deadline` call with `!is_due`; delete the 70-line dead function |
