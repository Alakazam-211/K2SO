# Resilience & Lean-up — 0.37.x

A consolidated kill list of code, data, and dev-loop friction surfaced during the 0.36.2 release work and the per-agent-heartbeat investigation that followed. Items are grouped by risk + impact so you can knock out a tier in one sitting without opening a rabbit hole. Each item names exact paths so the cleanup diff writes itself.

The premise: the new system (DB-backed `agent_heartbeats` + daemon-driven scheduler) has stabilized. Several legacy paths that ran in parallel during the migration are still wired up but no longer pulling their weight. Some are actively harmful (autoloop firing wakes against opted-out workspaces — fixed in this session). The rest are dead weight that adds compile time, noise, and surface area.

## Severity legend

- 🔴 **Active leak** — code is wrong or producing user-visible misbehavior right now
- 🟠 **Mothballed but live** — code path still wired up; nothing legitimate calls it; removal is mechanical
- 🟡 **Lean-up** — works correctly but adds noise, footprint, or duplication; removal is a quality-of-life win
- 🟢 **Dev-tooling gap** — friction in the dev loop that's worth one-time investment

---

## Tier 1 — finish what 0.36.2 started: per-agent heartbeats

The autoloop was killed in `active-agents.ts` this session. Everything below was on its dependency chain. With the loop gone, none of these have legitimate callers; removal is purely deletion.

### 1.1 🟠 Legacy heartbeat config struct + readers

Already marked `#[deprecated(note = "Per-agent heartbeats — superseded by AgentHeartbeat (DB-backed). Planned for removal in 0.37.x. See `legacy-per-agent-heartbeat` tag.")]` — the compiler currently emits ~24 warnings, one per call site. Search the build output for `legacy-per-agent-heartbeat` to enumerate.

- `crates/k2so-core/src/agents/scheduler.rs::AgentHeartbeatConfig` (the struct)
- `crates/k2so-core/src/agents/scheduler.rs::read_heartbeat_config` (the reader, currently has a backtrace logger gated on `K2SO_TRACE_HEARTBEAT_JSON`)
- `crates/k2so-core/src/agents/scheduler.rs::write_heartbeat_config`
- `crates/k2so-core/src/agents/scheduler.rs` lines 480–560 — the custom-agent branch inside `k2so_agents_scheduler_tick` that reads `next_wake` / `interval_seconds` from the legacy file
- `crates/k2so-core/src/agents/commands.rs::heartbeat_noop` and `heartbeat_action` (callers of `write_heartbeat_config`)
- The default-value helpers in scheduler.rs (`default_heartbeat_mode`, `default_interval`, etc.) once nothing references the struct

### 1.2 🟠 Legacy CLI subcommands

`cli/k2so` has a stack of subcommands that only operate on `heartbeat.json`. With 1.1 gone, these have nothing to read or write.

- `cmd_heartbeat` (no-arg form — calls `/cli/heartbeat`, which we're also retiring below)
- `cmd_heartbeat_noop`
- `cmd_heartbeat_action`
- `cmd_heartbeat_force` (around line 2451)
- `cmd_heartbeat_set` (around line 2468)
- `cmd_heartbeat_get` (around line 2507)

Keep: every `k2so heartbeat <subcommand>` that operates on the new `agent_heartbeats` table — `add`, `list`, `remove`, `unarchive`, `rename`, `edit`, `show`, `fire`, `launch`, `status`, `enable`, `disable`, `seed`, `wakeup`, `panel`, `clear`, `log`, `preview`. Those map to `/cli/heartbeat/*` (note the slash) and the new system.

### 1.3 🟠 Legacy HTTP route + Tauri command

- `src-tauri/src/agent_hooks.rs:705` — the `/cli/heartbeat` route handler (the one that calls `triage_decide` then `spawn_wake_pty`). This was the dispatch site behind the renderer autoloop.
- `src-tauri/src/commands/k2so_agents.rs:2140::k2so_agents_triage_decide` — the Tauri command. Three call sites total, two of which were in `active-agents.ts` (one already retired this session, the other at `:302` is a launch-failure retry that should also be retired since it operates on the legacy gating logic).
- The `'launch-failure-retry'` block at `active-agents.ts:286-316` — once `triage_decide` is gone this path can fall back to a simple "log and surface a toast"; the auto-respawn at `:302` is the same legacy autoloop in miniature.

### 1.4 🟠 On-disk `heartbeat.json` files

Existing workspaces (Cortana, anything else with a custom agent) carry a `.k2so/agents/<name>/heartbeat.json` file from the old model. Add a one-shot migration to delete them on first 0.37.x launch — same shape as the Code Puppy `DELETE FROM agent_presets` we shipped in 0.36.2:

```rust
// In db/mod.rs or a new migration site:
for project in projects { for agent in project.agents() { fs::remove_file(agent_dir(project.path, agent).join("heartbeat.json")).ok(); } }
```

### 1.5 🟠 DB columns on `projects`

`projects.heartbeat_mode`, `heartbeat_schedule`, `heartbeat_last_fire` are the workspace-level legacy gates. With the per-agent path gone they only serve the now-defunct `k2so_agents_scheduler_tick`. Drop in a schema migration alongside 1.1.

Sanity check before dropping: confirm no agent_hooks route or renderer code reads those columns directly. (Earlier grep showed only `scheduler.rs` reading them.)

### 1.6 🟠 Agent-level `agent.log`

`.k2so/agents/<name>/agent.log` is written by `heartbeat_noop` (the auto-backoff warning we found in Cortana). With 1.1 gone, nothing writes to these files. Add to the same migration that nukes `heartbeat.json`.

### 1.7 🟢 Dead TS catalog

`src/shared/agent-catalog.ts` exports `builtInAgentPresets` — never imported anywhere. We cleaned it up to match canonical IDs in 0.36.2 but it's pure dead code. Delete the file; remove from any tsconfig include glob if necessary.

---

## Tier 2 — old-school workflow leftovers

Pre-0.30s K2SO was built around per-agent local-LLM triage on a 30–60min cadence. The shape of that system survives in a few corners that are now technically unreachable but compile-included.

### 2.1 🟠 The two-HTTP-server split

There are two HTTP servers each handling `/cli/*` routes:

- **Daemon** at `~/.k2so/daemon.port` (also written to `~/.k2so/heartbeat.port`) — `crates/k2so-daemon/src/cli.rs`
- **Tauri's agent_hooks** at its own dynamically-allocated port — `src-tauri/src/agent_hooks.rs`

Some routes overlap (`/cli/scheduler-tick` exists on both). Daemon-first principle (see `feedback_daemon_first.md` memory) says routes should consolidate into the daemon. Audit which routes are still served only by Tauri and migrate them to the daemon, then drop the Tauri HTTP server entirely.

Caveat: routes that genuinely need the Tauri AppHandle (window control, native dialog, etc.) stay on the Tauri side. Most agent/heartbeat/triage routes don't need it.

### 2.2 🟠 `find_primary_agent` semantic mismatch

`crates/k2so-core/src/agents/mod.rs:116::find_primary_agent` assumes one primary agent per workspace and returns its name. This fits the legacy "the workspace's primary fires on heartbeat" model. The new model has multiple `agent_heartbeats` rows, each carrying its own agent target. Fourteen call sites still use this — review each one to decide whether it should switch to a heartbeat-row-based lookup or stay primary-based for legitimate reasons (e.g., the main Chat tab's default agent).

### 2.3 🟠 Workspace-level `wakeup.md`

Some workspaces have a top-level `.k2so/wakeup.md` (e.g., Cortana). The new model puts WAKEUP files only inside `.k2so/agents/<name>/heartbeats/<schedule-name>/WAKEUP.md`. The top-level file isn't read by any current code path. Add to the 1.4 migration.

### 2.4 🟡 `active-projects` endpoint

`/cli/heartbeat/active-projects` (handler at `triage.rs:75::handle_active_projects`) returns workspaces with at least one enabled, non-archived `agent_heartbeats` row. Currently consumed only by `~/.k2so/heartbeat.sh`, which iterates them and calls `/cli/scheduler-tick` for each. After Tier 1 + 2.1 land, the daemon's heartbeat tick can fan out internally without `heartbeat.sh` mediating. At that point the endpoint and the script become deletable.

### 2.5 🟡 `KNOWN_AGENT_COMMANDS` static set

`src/shared/constants.ts:48` is a hand-maintained set of CLI binary names used for the "are you sure you want to close?" warning when a tab is foregrounded on a known LLM CLI. The list will rot every time a new tool ships. Since 0.36.2 the same data lives in `agent_presets` table rows (each preset has a `command` field). Replace the static set with a derived selector over `usePresetsStore`. Adds Pi/Goose/Copilot/Interpreter/Ollama coverage automatically and removes the maintenance footgun.

---

## Tier 3 — debug instrumentation

Partially cleaned this session. Listed for completeness so the next pass doesn't re-discover them.

### 3.1 ✅ Already gated this session

- `[v2-activity] TITLE` (per-second per-agent) — opt-in via `localStorage.K2SO_V2_ACTIVITY_VERBOSE='1'`
- `[perf] *_tick` histograms — opt-in via `K2SO_PERF=1` env (was auto-on in debug builds)

### 3.2 🟡 Still always-on, candidates for the same treatment

- `[v2-perf] SPAWN_SUMMARY`, `CONNECT-SUMMARY`, `TUI_SUMMARY` in `src/renderer/terminal-v2/TerminalPane.tsx` — fires on every PTY spawn / WS connect / tab teardown. Useful for perf hunts, noise the rest of the time. Same `localStorage.K2SO_V2_PERF='1'` treatment.
- `[daemon-events] connect refused` warnings during daemon-restart races. Either silence the first N occurrences or downgrade to debug-level.
- `[daemon/sessions_grid_ws] subscriber attached/detached` in the daemon — once per tab open/close, useful for connection debugging. Gate on `K2SO_DEBUG_WS=1`.
- `[v2-activity] WIRED` and `FLIP` — keep always-on (infrequent, load-bearing for spinner debugging).

### 3.3 🟡 `[wake-spawn-trace]` instrumentation we added

The `eprintln!`s in `crates/k2so-core/src/agents/wake.rs::spawn_wake_headless` and `src-tauri/src/agent_hooks.rs::spawn_wake_pty` were added during the Cortana investigation. After Tier 1 lands, decide: keep them (gated on `K2SO_TRACE_WAKE_SPAWN`) for future spawn-path debugging, or remove them entirely. They're cheap (one env-var check + one optional backtrace) so keeping is defensible. Same goes for the `read_heartbeat_config` trace — once that function is deleted, the trace goes with it.

### 3.4 🟡 General `println!` / `console.log` audit

265 total bare-print sites across the codebase (per session-time grep). Most are legitimate (settings persistence, error reporting, UI lifecycle). Worth one focused hour to sort each into:

- **Keep always-on** (errors, recoverable warnings)
- **Gate behind env or localStorage** (perf metrics, verbose state dumps)
- **Delete** (orphans from earlier debugging sessions — search for `// TODO remove` / `// debug` comments)

---

## Tier 4 — release notes formatting

Past `release-notes-*.md` files pre-0.36.2 are hard-wrapped at ~64 cols. GitHub Releases honors single `\n` as `<br>` in some surfaces (the release-page sidebar, embed cards, auto-generated changelog), so paragraphs render as choppy fragments.

### 4.1 🟡 Unwrap past notes

Files: `release-notes-0.35.0.md` through `release-notes-0.36.1.md` (and earlier if anyone cares about archive consistency). Mechanical fix: for each paragraph (run of non-empty lines that aren't a heading, list bullet, or fenced code), join into a single long line. Empty lines, headings, lists, and code blocks stay as-is.

A 30-line awk/python script handles this in one pass; the diff is "obvious," reviewable, and zero functional risk.

### 4.2 🟡 Document the format going forward

Add a one-line note at the top of `scripts/release.sh` (or in a sibling `RELEASE.md`) saying "release notes use one paragraph per line; markdown handles wrapping at render time." Otherwise the next release we hand-edit will repeat the mistake.

---

## Tier 5 — dev-tooling gaps

Surfaced during this session because the lack of these turned a 5-minute debug into a 30-minute one.

### 5.1 🟢 `k2so app stop` / `k2so app status` / `k2so daemon swap`

Three small CLI subcommands that compress the dev loop:

- `k2so app stop` — sends SIGTERM to bundled K2SO.app + its daemon, waits for clean exit. Used before swapping the daemon binary.
- `k2so app status` — lists running K2SO processes (app, daemon, dev tauri) with PIDs, uptime, and which binary path each is. Replaces the `ps -eo … | grep k2so` ritual.
- `k2so daemon swap` — `cargo build --release -p k2so-daemon`, sign with the K2SO Dev ID, replace `/Applications/K2SO.app/Contents/MacOS/k2so-daemon`, kickstart launchd. Compresses the four-step dance we did this session into one command.

The third one specifically caught us — the daemon binary needs to be signed with the K2SO Developer ID after a cargo build, otherwise macOS hardened-runtime kills it on launch. The script should embed that signing step so a fresh checkout doesn't trip on it.

### 5.2 🟢 Pending-live queue cruft

`~/.k2so/daemon.pending-live/nobody/` carries 46 stale signals from awareness-bus testing. `testbot/` and `rust-eng/` also have leftovers. Add a `k2so awareness purge` command (or just a startup scan that drops queued signals older than N days). Or a `k2so debug purge` that wipes test artifacts in one go.

### 5.3 🟢 `.k2so/agents/.archive/` accumulation

`.k2so/agents/.archive/` collects every retired agent run. K2SO already auto-archives on agent deletion. There's no auto-prune — workspaces that have been around since 0.32 carry hundreds of these directories. Add age-based pruning (`> 30 days` → delete) to the same `purge` command, or to the daemon's startup scan.

---

## Out of scope (intentional non-goals for 0.37.x)

- **Kessel-T0 retirement** — the `SessionStreamSession` legacy session_map is still referenced when a user explicitly picks Kessel renderer in Settings. That stays alive until Kessel-T1 work begins; see `kessel-t1.md` for the plan.
- **Mobile companion code** — companion HTTP/WS endpoints stay in the daemon. 0.29.x is the last with mobile-app pairing per the deprecation notice we shipped in 0.36.2; companion is coming back in a future version, so the daemon-side plumbing is paused, not retired.
- **Cargo.lock churn** — the release script regenerates lockfiles on every bump. Don't try to factor that out without understanding the current notarization flow.

---

## Suggested execution order

1. **Tier 1.1–1.7** in one branch — finishes the per-agent heartbeat retirement. Compile warnings drop from 24 to 0. Migration cleans up on-disk state. Test: open Cortana, kill any agent session, confirm no auto-respawn.
2. **Tier 2.1** (daemon-only HTTP) in a separate branch — bigger lift, more risk surface, but unblocks every other "is this called from the daemon or Tauri?" question.
3. **Tier 2.2–2.5** can interleave with 2.1.
4. **Tier 3 + 4** are quality-of-life, batch them whenever convenient.
5. **Tier 5** dev-tooling is a one-afternoon investment that pays off on every subsequent debugging session — earlier the better.

## Definition of done

- `cargo build` produces zero `legacy-per-agent-heartbeat` deprecation warnings.
- A grep for `heartbeat.json`, `read_heartbeat_config`, `triage_decide`, `AgentHeartbeatConfig` returns zero hits in `crates/`, `src-tauri/`, `src/renderer/`, `cli/`.
- A fresh K2SO install on a blank workspace produces zero `.k2so/agents/<name>/heartbeat.json` files.
- Closing a Claude session in any workspace does not spawn a follow-up wake unless that workspace has an explicit `agent_heartbeats` row scheduled to fire now.
- The dev console at idle shows fewer than ~20 lines per minute (versus the current ~600 lines/min from the perf and v2-activity streams).
