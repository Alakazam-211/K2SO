# Phase 3.2 — Hardening Plan (before user-visible 0.34.0 release)

**Branch:** `feat/session-stream` (continues from Phase 3.1)
**Status:** PLANNED, not yet started
**Start:** 2026-04-20
**Scope:** 6 commits (PRD estimate: 5-7)
**Engineering stance:** complete, not quick. No corner-cuts; no gold-plating.

---

## Why this phase exists

Phase 3.1 proved peer-to-peer delivery works end-to-end. Phase 3.2
closes five durability/correctness gaps identified during 3.1 that
every one of them is a surprise-waiting-to-happen if we expose the
pipeline to real users in Phase 4.5:

1. **Wedged harnesses stay wedged forever** — no watchdog.
2. **500MB hard-freeze on archive** — a long-lived session silently
   loses audit data past that point.
3. **Offline agents never get their signals auto-injected** —
   `DaemonWakeProvider` queues but doesn't launch.
4. **Noisy agents can flood the bus** — no budget enforcement.
5. **Users edit SQL to flip `use_session_stream`** — no UI.

None of these block Phase 3.1's automated tests. All five block a
clean user-facing 0.34.0.

---

## The six commits

### G1 — Session activity tracking + harness watchdog

**What it delivers:** `SessionEntry` records `created_at` +
`last_frame_at`. A daemon-side `session::watchdog` task polls every
N seconds; for each session past an idle threshold, it escalates:
Ctrl+C (0x03) into the PTY → wait → SIGTERM the child → wait →
SIGKILL. Each stage logs + emits a `SemanticEvent` frame so
subscribers (and activity_feed) see it.

**Changes:**
- `crates/k2so-core/src/session/entry.rs` — add
  `created_at: std::time::Instant`, `last_frame_at: Mutex<Instant>`;
  update on every `publish()`. Expose accessors.
- `crates/k2so-core/src/session/watchdog.rs` *(new)* — `WatchdogConfig`
  (warn_after, sigterm_after, sigkill_after, poll_interval) + a pure
  `evaluate(entry, now) -> Escalation` function + a `spawn()` that
  runs a tokio loop driving it.
- `crates/k2so-core/src/session/mod.rs` — re-export.
- `crates/k2so-daemon/src/main.rs` — start watchdog on boot.
- `crates/k2so-daemon/src/session_map.rs` — extend so the watchdog
  can resolve `(agent_name, session_id) → Arc<SessionStreamSession>`
  for the PTY-write and child-kill paths. (Today it only maps by
  agent name.)
- `crates/k2so-core/src/terminal/session_stream_pty.rs` — the
  session needs to expose a `kill()` method; wire through to
  portable-pty's child. Add `child_pid()` accessor (Unix-only via
  `process_id()`).

**Invariants preserved:**
- SessionEntry has no alacritty imports. Watchdog has no alacritty
  imports (it reads PTY bytes indirectly, never the grid).
- Escalation is opt-in on a per-project basis: if
  `WatchdogConfig::disabled()` (all thresholds = None), watchdog
  loop is a no-op. Tests default to disabled to avoid interfering
  with short-lived fixtures.

**Tests:**
- `crates/k2so-core/tests/session_stream_watchdog.rs` — pure
  `evaluate()` tests for each escalation boundary; a
  tokio::test that feeds a fake session and observes escalation.
- `crates/k2so-daemon/tests/watchdog_integration.rs` — real PTY
  spawn + forced idle + assert child killed.

**Complete-not-corner-cut criteria:**
- All three escalation stages reachable + tested
- Configurable thresholds via env vars (K2SO_WATCHDOG_*) for
  prod override; defaults shipped in-code
- Escalation emits `SemanticEvent` frame so activity_feed sees it
- Child PID resolution handles both launched-by-us and already-
  orphaned cases (test with both)
- Watchdog task honors tokio shutdown (no zombie tasks on daemon
  restart)

**Commit:** `G1 (session-stream): SessionEntry activity + harness watchdog`

---

### G2 — Archive NDJSON rotation + `k2so session compact`

**What it delivers:** Archive writer rotates to `archive-000.ndjson`,
`archive-001.ndjson`, … at size boundary (default 50MB; configurable).
Old rotated files can be gzipped via `k2so session compact <id>`.
Hard-freeze behavior removed — infinite audit as long as disk
allows, with retention policy to cap total usage.

**Changes:**
- `crates/k2so-core/src/session/archive.rs` — rewrite the writer
  loop. Track current segment index + bytes; rotate at
  `ROTATE_BYTES`. Keep `HARD_LIMIT_BYTES` as "freeze writing if
  aggregate across rotated files exceeds N" (default 5GB) — still
  fail-open for session liveness.
- `crates/k2so-core/src/session/archive_compact.rs` *(new)* — pure
  helper that finds rotated segments, gzips oldest-first, deletes
  originals. Skips the segment currently being written.
- `cli/k2so` — new `sessions compact <session-id>` subcommand
  dispatches to a new daemon route
  `POST /cli/sessions/compact` or runs the local helper directly
  (local is simpler + tests easier; pick local).
- `cli/k2so` — new `sessions list` subcommand (reads
  `~/.k2so/projects/<id>/.k2so/sessions/*/archive*.ndjson*`).

**Tests:**
- `tests/session_stream_archive_rotation.rs` — feed enough frames
  to cross rotation boundary, assert multiple files exist + none
  exceed `ROTATE_BYTES`.
- `tests/session_stream_archive_compact.rs` — create fake rotated
  segments, run compact, assert gzip files + original removed.

**Complete criteria:**
- Rotation filename scheme sortable lex (zero-padded index)
- Rotation boundary never splits a frame (rotate before write, not
  mid-write)
- Compact is idempotent (safe to re-run)
- Compact never touches the active segment
- `k2so sessions list` output is human-readable + machine-parseable
  via `--json`

**Commit:** `G2 (session-stream): archive rotation + k2so sessions {compact,list}`

---

### G3 — Agent launch profiles (frontmatter schema + parser)

**What it delivers:** Extend `.k2so/agents/<name>/agent.md` YAML
frontmatter with an optional `launch:` block:

```yaml
---
name: bar
role: Example agent
launch:
  command: bash
  args: []
  cwd: "."         # relative to project root; ~ expanded
  cols: 120
  rows: 40
  env:
    FOO: bar
coordination_level: moderate  # none | minimal | moderate | chatty
---
```

This is the substrate for G4 (scheduler-wake) and G5 (budgets).
The field is optional everywhere; missing = use caller-supplied
defaults.

**Changes:**
- `crates/k2so-core/src/agents/launch_profile.rs` *(new)* —
  `LaunchProfile` struct + parser over existing frontmatter. Pure
  function: `load_launch_profile(project_root, agent_name) ->
  Option<LaunchProfile>`.
- `crates/k2so-core/src/agents/mod.rs` — extend
  `parse_frontmatter` if needed (today it's key:value; `launch:`
  is nested — may need to switch to `serde_yaml`; pin the crate).
- Sanity tests for every field shape.

**Tests:**
- `tests/agent_launch_profile.rs` — parse missing `launch:` block;
  parse minimal; parse full; parse malformed.

**Complete criteria:**
- Parser handles the full nested YAML schema
- `~` expansion on `cwd`
- env map merged with process env (profile takes precedence)
- Clear error messages on malformed frontmatter
- Backwards-compatible: every existing agent.md without `launch:`
  still parses

**Commit:** `G3 (session-stream): agent.md launch profile + coord_level schema`

---

### G4 — Real scheduler-wake (DaemonWakeProvider spawns sessions)

**What it delivers:** When `DaemonWakeProvider::wake` fires for an
offline agent, the daemon:

1. Looks up the agent's `LaunchProfile` (via G3).
2. If found: spawns a session via the existing `spawn_session_stream`
   path — same code path as `POST /cli/sessions/spawn`.
3. Registers the new session in `session_map`, drains pending-live
   queue, injects signals. Caller's signal becomes the target's
   first input.
4. If no profile: falls back to current queue-only behavior. Logs
   a warning + includes fallback reason in the DeliveryReport so
   senders know delivery is deferred.

**Changes:**
- `crates/k2so-daemon/src/providers.rs` — `DaemonWakeProvider::wake`
  grows a spawn arm. Factor out the spawn logic from
  `awareness_ws::handle_sessions_spawn` into a reusable
  `crate::spawn::spawn_agent_session(config) -> Result<SpawnOutcome>`
  so both call sites share code.
- `crates/k2so-daemon/src/spawn.rs` *(new)* — the shared spawn
  helper.
- `awareness_ws::handle_sessions_spawn` — now delegates to
  `crate::spawn::spawn_agent_session`.
- `DeliveryReport` gains `scheduler_wake_launched: bool` or similar
  to surface the outcome distinctly from `woke_offline_target`.

**Tests:**
- `crates/k2so-daemon/tests/scheduler_wake_integration.rs` —
  queue a signal for an agent with a configured launch profile;
  observe the session spawn + signal inject.
- Negative: queue a signal for an agent without a launch profile;
  observe fallback to queue-only behavior + correct DeliveryReport.

**Complete criteria:**
- Race-free: concurrent wakes for the same agent don't spawn
  multiple sessions (check session_map before spawning; single-
  flight via `DashMap::entry` or a Mutex).
- Spawn failure is reported back to caller (signal stays queued).
- Full audit: activity_feed row records whether the launch was
  scheduler-triggered.

**Commit:** `G4 (session-stream): DaemonWakeProvider auto-launches via launch profile`

---

### G5 — Per-coordination-level message budgets

**What it delivers:** Every agent has a `coordination_level` (read
from G3's frontmatter; default `moderate`). Each level maps to a
per-(agent, session) emit budget:

| Level | Budget | Semantics |
|---|---|---|
| none | 0 | agent may not emit to the bus at all |
| minimal | 2 | two emits per session |
| moderate | 5 | default — five emits per session |
| chatty | 10 | ten emits per session |

When an agent exceeds budget, `egress::deliver` drops the signal
AND emits a `SemanticEvent::BudgetExceeded` that still writes to
activity_feed (audit preserved) but does NOT inject/inbox.

**Exemptions:** `Priority::Urgent` signals always pass. `kind::status`
updates don't count against budget (telemetry, not coordination).

**Changes:**
- `crates/k2so-core/src/awareness/budget.rs` *(new)* — pure
  `BudgetState` struct keyed by `(agent_name, session_id)`.
  `check_and_increment(&self, level, priority, kind) ->
  BudgetDecision`. Single global instance in `awareness::` (same
  ambient pattern as `bus`).
- `crates/k2so-core/src/awareness/egress.rs` — `deliver` calls
  budget check first. On `BudgetDecision::Deny`, writes activity
  row with `event_type = 'signal:budget-exceeded'` and returns
  an augmented DeliveryReport.
- `DeliveryReport` gains `budget_denied: bool`.
- `crates/k2so-core/src/agents/launch_profile.rs` — expose
  `coordination_level()` helper.

**Tests:**
- `tests/session_stream_budget.rs` — feed signals past budget
  boundary, assert shed behavior, assert activity_feed records
  denials, assert Urgent passes, assert status passes.

**Complete criteria:**
- Budget state survives session replay but resets per-session
  (not per-daemon-restart — sessions are ephemeral anyway)
- Budget decisions are logged at the same verbosity as
  existing delivery logs
- Activity feed records the DENIAL (never silent drops)
- No coordination_level → default moderate

**Commit:** `G5 (session-stream): per-coordination-level budgets + activity_feed audit`

---

### G6 — Tauri Settings UI: `use_session_stream` toggle

**What it delivers:** A toggle in the K2SO project settings panel
that flips `use_session_stream` per-project. No more SQL.

**Changes:**
- `src-tauri/src/commands/projects_settings.rs` *(or nearest
  existing settings command file)* — add Tauri commands
  `projects_get_setting(project_id, key)` and
  `projects_update_setting(project_id, key, value)`. Internally
  use `crates/k2so-core/src/agents/settings.rs`'s existing
  allowlist-backed `update_project_setting`.
- `src/renderer/components/Settings/ProjectSettingsPanel.tsx`
  *(or nearest)* — add a toggle row with:
  - Label: "Session Stream (beta)"
  - Description: "Subscribe to the new typed event stream
    instead of raw alacritty grid emission. Reversible."
  - Reads current value on panel mount; writes on toggle.
  - Shows "Restart needed for running sessions" hint on toggle.

**Tests:**
- Unit test the Tauri command's input validation (on/off only).
- Manual: Rosson flips the toggle; value persists; sqlite shows
  the update.

**Complete criteria:**
- Frontend + backend wired end-to-end
- Value persists across app restart
- Invalid input (somehow) handled without panic
- Pattern extensible to future project-level toggles

**Commit:** `G6 (session-stream): Tauri Settings UI for use_session_stream`

---

## Sequencing rationale

| # | Commit | Why this order |
|---|---|---|
| G1 | watchdog | Smallest, self-contained. Proves the pattern for
later observability commits. |
| G2 | rotation + compact | Independent of G1. Incremental on top of
existing archive.rs — low risk. |
| G3 | launch profile schema | Prereq for G4 (spawn config) and G5
(coord_level). Just parsing — no spawn logic yet. |
| G4 | scheduler-wake | Depends on G3. Biggest item. |
| G5 | budgets | Depends on G3 for coord_level field. Independent of G4
spawn code. |
| G6 | Settings UI | Leaves the feature user-visible ready. |

G1, G2, G3 can technically ship in parallel worktrees. G4 waits on
G3. G5 waits on G3. G6 waits on nothing.

---

## Architectural invariants (carried + preserved)

All five Phase 3/3.1 invariants continue to hold:

1. **No alacritty imports in awareness/session/daemon-ws.**
   Watchdog especially — it operates on `SessionEntry` + PTY bytes,
   never the grid.
2. **LineMux sees raw PTY bytes.** Unchanged.
3. **Feature flag `session_stream` gates consumer side only.**
   Watchdog + rotation + budgets + launch profiles all live
   behind this flag on k2so-core.
4. **Sender's `Delivery` is load-bearing.** Budget denial is NOT a
   silent demotion to Inbox; it's a new outcome with its own
   activity_feed row.
5. **Audit always fires.** Every budget denial, every scheduler-
   wake, every watchdog escalation writes an activity_feed row.

---

## Manual smoke test (end of Phase 3.2)

Once all six commits land:

```bash
# Terminal 1 — daemon
cargo run --features session_stream -p k2so-daemon

# Terminal 2 — create an agent profile
cat > .k2so/agents/test-bot/agent.md <<EOF
---
name: test-bot
role: Integration test bot
launch:
  command: cat
  cwd: /tmp
coordination_level: moderate
---
EOF

# Terminal 3 — send to offline test-bot (scheduler-wake auto-launches)
k2so signal test-bot msg '{"text":"hi — auto-launch me"}'
# → daemon reads launch profile, spawns `cat` session, injects signal

# Terminal 4 — exhaust budget
for i in $(seq 1 6); do
  k2so signal test-bot msg "{\"text\":\"msg $i\"}"
done
# → msg 6 hits budget limit; activity_feed shows 'signal:budget-exceeded'

# Terminal 5 — check archive rotated
ls -la .k2so/sessions/*/archive*.ndjson*
# → multiple files, oldest gzipped after `k2so sessions compact`

# Terminal 6 — flip the Tauri toggle, see it persist
# (open K2SO app; toggle Session Stream off; reopen; still off)
```

---

## What gets documented at the end

- `.k2so/notes/phase-3.2-hardening-complete.md` — mirror of
  Phase 3/3.1 completion notes. Commit shas, test counts,
  remaining-deferred items.
- PRD amendment: mark Phase 3.2 SHIPPED; update the "What gets
  torn out" table if anything got deleted.
- Update `~/.claude/projects/.../memory/project_0.34.0_session_stream.md`
  with the new state.

---

## What's deliberately NOT in Phase 3.2

- **Scheduler-wake over cross-workspace** — Phase 4. G4 only
  handles same-workspace agents.
- **Watchdog per-agent idle threshold via frontmatter** — can be
  a Phase 3.2 follow-up if we actually need it. MVP uses global
  defaults + env override.
- **Budget windows beyond "per-session"** — no per-day, per-hour.
  If we ever need them, they build on G5's `BudgetState`.
- **Archive gzip on rotation** — only manual `k2so sessions
  compact` gzips. Automatic compression on rotation adds CPU
  pressure during normal operation; defer unless we measure a
  need.
- **Archive remote sync / S3 upload** — out of scope entirely.

---

## Rollback

Each commit is feature-flag-gated. Rollback paths:

- Flag off per project: `use_session_stream='off'` — all five
  hardening items become no-ops (because their modules are
  behind the flag).
- Flag off at compile time: `--no-default-features` —
  bit-for-bit v0.33.0.
- Individual revert: each commit is self-contained + idempotent.

---

## Before starting

Manual smoke test of Phase 3.1 still hasn't been done by Rosson.
G1 doesn't depend on it (watchdog operates on any session), but
G4's scheduler-wake integrates with the pending-live queue from
Phase 3.1 and Rosson should confirm the queue actually works
before we bolt auto-launch onto it.

Recommended first step: Rosson runs the Phase 3.1 three-terminal
demo (in `.k2so/notes/phase-3.1-live-inject-complete.md`), then
I start G1.
