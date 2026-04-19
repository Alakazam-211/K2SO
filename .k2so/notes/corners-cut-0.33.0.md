# Corners cut during the 0.33.0 daemon pass

Honest log of shortcuts taken on `feat/persistent-agents`. Each item
includes the rationale, the "do it right" version, and — where
obvious — the specific file/line to revisit.

Created 2026-04-19 during the P3+P4 execution. Updated as more are
found.

---

## Architecture

### 1. Daemon uses `std::net::TcpListener`, not tokio — **CLOSED 2026-04-19**
- **Where:** `crates/k2so-daemon/src/main.rs`.
- **Resolution:** switched to `#[tokio::main(flavor = "multi_thread")]`
  + `tokio::net::TcpListener` + `tokio::spawn` per connection. Graceful
  shutdown via `tokio::signal::ctrl_c` feeding a broadcast channel that
  both the accept loop and per-connection handlers `select!` on. Commit
  `075ef534`. Verified 230 tests green + manual smoke of /ping and
  /status. Concurrent connections now safe for the upcoming /hook/*
  and /cli/* migration.

### 2. `daemon.port` / `daemon.token` are parallel to the existing
       `heartbeat.port` / `heartbeat-token`
- **Where:** `crates/k2so-daemon/src/main.rs`,
  `src-tauri/src/agent_hooks.rs`.
- **Why cut:** during dev we want the Tauri app's agent_hooks server
  AND the daemon to coexist. Sharing one port file would make one of
  them silently hijack the other.
- **Do it right:** after agent_hooks fully migrates into k2so-core
  and runs inside the daemon, reconcile to a single port file
  (`heartbeat.port`) so the CLI discovery path doesn't fork. Delete
  the `daemon.port` / `daemon.token` files and update
  `crate::daemon_client::DaemonClient` to read `heartbeat.port` /
  `heartbeat-token`.

### 3. `agent_hooks` still in src-tauri
- **Where:** `src-tauri/src/agent_hooks.rs` (3,454 lines).
- **Why cut:** deeply entwined with `AppState::try_state`, the Tauri
  command framework, and the src-tauri-resident companion module. A
  wholesale move requires a trait-based refactor of every
  `app_handle.emit(...)` (17 sites) AND every `app_handle.try_state(...)`
  (dozens more) and drags companion along.
- **Do it right:** in-progress. Pattern same as terminal migration:
  introduce `AgentHookEventSink` trait in k2so-core; have src-tauri
  provide a `TauriAgentHookEventSink`. State access should be passed
  in via trait parameters, not `try_state`. This IS the next
  substantial commit; user has explicitly OK'd multi-day scope.

### 4. `companion` still in src-tauri — **CLOSED 2026-04-19**
- **Where:** was `src-tauri/src/companion/`, now
  `crates/k2so-core/src/companion/`.
- **Resolution:** full module migrated. Four bridges
  (`settings_bridge`, `terminal_bridge`, `event_sink`,
  `app_event_source`) decouple mod.rs from Tauri completely. Tauri
  app registers impls in `setup()` via `companion_host::register()`
  + `TauriCompanionSettingsProvider`.

### 5. `watcher.rs` stayed in src-tauri
- **Where:** `src-tauri/src/watcher.rs` (135 lines).
- **Why cut:** it's a `#[tauri::command]` that reaches into `AppState`
  to store its watcher handles. Cleanly moving it requires
  abstracting AppState access, which is larger than the module.
- **Do it right:** when we do the command-proxy sweep, either move it
  into core with an `Arc<Mutex<Watchers>>` injected, or keep it as a
  Tauri command that calls into a core helper. Low priority — it's
  idempotent and doesn't block daemon functionality.

### 6. `editors`/`menu`/`window`/`state` status mixed
- **Where:** `src-tauri/src/{editors,menu,window,state}.rs` —
  editors migrated, the rest not.
- **Why cut:** menu + window are Tauri API-specific
  (`tauri::Manager`, `tauri::Menu`, `tauri::Window`); state wraps
  `tauri::Manager`-managed state. These legitimately belong in
  src-tauri.
- **Do it right:** intentional. Not a cut corner — documenting for
  completeness.

## Release / build

### 7. Daemon bundling path-cp, not `externalBin`
- **Where:** `scripts/release.sh` Step 2.5.
- **Why cut:** Tauri v2's `externalBin` mechanism expects
  target-triple-suffixed filenames (`k2so-daemon-aarch64-apple-darwin`)
  and a matching config entry in `tauri.conf.json`. A shell cp is
  simpler and easier to audit.
- **Do it right:** migrate to `externalBin` if we ever need Tauri to
  spawn the daemon itself (currently launchd spawns it, so we don't).
  Otherwise leave as-is — `cp` works and the codesign step is
  explicit.

### 8. Daemon bundling not end-to-end tested
- **Where:** `scripts/release.sh`.
- **Why cut:** running a full signed + notarized release from this
  session would burn hours of context on CI-ish work that belongs on
  the user's release machine.
- **Do it right:** next time `./scripts/release.sh 0.33.0-dev …` runs,
  watch for:
  - `cargo build --release -p k2so-daemon` exits 0
  - `K2SO.app/Contents/MacOS/k2so-daemon` exists post-copy
  - The third codesign call succeeds
  - On first launch of the signed build, `launchctl list | grep k2so-daemon`
    shows the daemon loaded

### 9. No plist-conflict handling on install — **CLOSED 2026-04-19**
- **Where:** `k2so_core::wake::install`.
- **Resolution:** `install()` now calls `launchctl_unload` as a
  best-effort prelude; the unload function already treats
  "not loaded" as Ok. Fresh installs + upgrades both work.

## Schema / migrations

### 10. drizzle_sql duplicated at repo root — **CLOSED 2026-04-19**
- **Where:** was `drizzle/` at the repo root vs
  `crates/k2so-core/drizzle_sql/`.
- **Resolution:** `drizzle.config.ts` now writes directly to
  `./crates/k2so-core/drizzle_sql`. Root `drizzle/` directory deleted
  (its 4 .sql files were stale — drizzle-kit hadn't been re-run
  against it since migration 0003). `.gitignore` updated so
  `crates/k2so-core/drizzle_sql/meta/` (drizzle-kit's snapshot
  metadata) is excluded instead of the old `drizzle/meta/`.

### 11. `db::init_for_tests` no longer `#[cfg(test)]`-gated — **CLOSED 2026-04-19**
- **Where:** `crates/k2so-core/src/db/mod.rs`.
- **Resolution:** re-gated behind `cfg(any(test, feature = "test-util"))`.
  Production builds compile it out again; src-tauri's test binary
  still reaches it via the existing dev-dependency features entry.

## Dead code / hygiene

### 12. `bitmap_renderer.rs` dead code — **CLOSED 2026-04-19**
- **Where:** `crates/k2so-core/src/terminal/bitmap_renderer.rs`.
- **Resolution:** deleted. `mod bitmap_renderer;` removed from
  `terminal/mod.rs`. The 414-line file plus ~15 dead-code warnings
  are gone.

### 13. `fs_abstract.rs` was added as an untracked file
- **Where:** `src-tauri/src/fs_abstract.rs` at v0.32.13, then moved
  to `crates/k2so-core/src/fs_abstract.rs`.
- **Why cut:** the file existed on disk at session start but was
  never tracked. Bundled into my `db` migration commit via
  `git add -A`. Unrelated but benign.
- **Do it right:** not actionable — the file is legit and tracked
  now. Documenting so the bundling is understood in git archaeology.

## Frontend / UX

### 14. Wake-scheduler Settings UI not built
- **Where:** task #220 (pending).
- **Why cut:** frontend work is more UX-design-heavy than the
  backend refactor. Wanted the backend model solid first.
- **Do it right:** a Settings panel with three radio buttons (off /
  on-demand / heartbeat-every-N), an integer picker for N, a
  "Wake system from sleep" checkbox. On Apply, call a new
  Tauri command that builds a `DaemonPlist` and calls
  `k2so_core::wake::install()` — or tears down the current one if
  mode=off.

### 15. Companion App team not notified of daemon-side tunnel URL
         rotation
- **Where:** product comms, not code.
- **Why cut:** none of the companion changes are live yet — mobile
  clients still see the existing Tauri-app-owned tunnel.
- **Do it right:** when companion migrates into the daemon (item #4
  above), send a memo: tunnel URL may rotate on daemon restart;
  paid-ngrok reserved domain is the stable-URL answer.

## Validation

### 16. No end-to-end lid-closed walkthrough yet
- **Where:** task #222 (pending); acceptance checklist in the plan
  file.
- **Why cut:** the 9-step walkthrough requires a build with daemon +
  plist installed + launchd actually running it on a real Mac,
  overnight, on battery. That's the final signoff, not an in-session
  validation.
- **Do it right:** after agent_hooks + companion migrations + release
  bundle, cut a dev DMG, install it, run the walkthrough, record a
  90-second video.

### 17. Daemon `/status` tested only by curl smoke, not integration
- **Where:** `crates/k2so-daemon/src/main.rs` routing logic.
- **Why cut:** cargo test can't spawn the daemon binary inside a
  hermetic harness without racing with the user's real running
  daemon (shared file paths).
- **Do it right:** add a `crates/k2so-daemon/tests/` integration
  test that uses a per-test `K2SO_STATE_DIR` override env var to
  redirect the port/token files, then spawns the binary under test.
  Small scope change — add the env var, then wire the harness.

---

---

## Infrastructure shipped since 2026-04-19 compaction checkpoint

These aren't "corners" — they're affirmative build-outs that close
prerequisites for remaining corners.

### Tokio runtime on daemon (closes corner #1)

Commit `075ef534`. Details folded into corner #1 above.

### Heartbeat slice migrated (9 fns + 6 daemon routes)

Commit `cdf20a34`. Moved from `src-tauri/src/commands/k2so_agents.rs`
into `k2so_core::agents`:

- Helpers: `resolve_project_id`, `agents_dir`, `agent_dir`,
  `parse_frontmatter`, `agent_type_for`, `find_primary_agent`.
- Heartbeat CRUD + tick: `k2so_heartbeat_{add,list,remove,
  set_enabled,edit,rename,fires_list}`, `k2so_agents_heartbeat_tick`,
  `stamp_heartbeat_fired` + `HeartbeatFireCandidate` struct.

src-tauri re-exports the helpers at their old paths so the 170+
local call sites stay intact (no rename churn). The 7
`#[tauri::command]` wrappers are now three-line forwards.

Daemon picks up 6 `/cli/heartbeat/*` routes (add/list/remove/enable/
edit/rename) sharing common auth + project_path extraction via new
`parse_params` + `handle_cli_heartbeat` helpers. E2E verified against
the live K2SO project: /cli/heartbeat/list returns the same JSON the
Tauri app would.

**This is the minimum surface the launchd heartbeat plist needs.**
When the lid is closed and the Tauri app is quit, launchd wakes the
laptop, fires `com.k2so.agent-heartbeat`, the CLI POSTs
`/cli/heartbeat/list` + `/cli/heartbeat/tick` (tick route still
pending) to whichever process owns the port file, and heartbeats
fire from the daemon's process.

### First route migrated: /hook/complete

Commit `a3136d74`. `k2so_core::agent_hooks::handle_hook_complete`
takes a pre-parsed params map, fires `HookEvent::AgentLifecycle`
through the registered sink, and syncs `agent_sessions.status` via
`db::shared()`. Both the daemon and src-tauri's existing server now
call this one function so their behavior can't drift — verified with
a Python WS subscriber that watches a real `/hook/complete` fired via
curl to the daemon.

Pattern for the remaining routes: each handler moves into a
`k2so_core::agent_hooks::handle_*` function that takes a
`&HashMap<String, String>` of params and returns a response body
string; the daemon + src-tauri HTTP layers each keep their own token
validation and serialization but delegate the actual work. Routes
that currently call into `commands::k2so_agents::*` are blocked on
that module moving into k2so-core (or bridge traits if the move is
deferred). See "Remaining for corner #3" below.

### Daemon -> Tauri WS event channel (/events)

Commit `385d0977`. New module `crates/k2so-daemon/src/events.rs`
(`DaemonBroadcastSink` + `serve_events_connection`) plus new module
`src-tauri/src/daemon_events.rs` (reconnecting subscriber thread).
Wire format is `WireEvent { event: &'static str, payload: Value }`
JSON-encoded text frames; event names match `HookEvent::event_name()`
so Tauri can emit them through the existing AppHandle bus without a
second mapping table.

This is the prerequisite that unblocks **incremental** migration of
`/hook/*` + `/cli/*` routes (corner #3): handlers can now move to the
daemon one at a time, each emitting `HookEvent` frames that reach the
UI identically to src-tauri's existing server. No need for a big-bang
migration to make the daemon's routes useful.

Auth is the same 32-hex token as /ping and /status; validated pre-WS-
upgrade so unauthenticated clients see a 403 rather than a dangling
close. Reconnect backoff is 2/4/8/16/30s capped; daemon-not-installed
case is treated the same as "daemon transient error" — no error noise
when running against the pre-0.33.0 binary.

---

---

## Remaining for corner #3 ("daemon serves /hook/* + /cli/*")

**Routes still owned exclusively by src-tauri's server:**

- `/hook/*` — **all migrated.** There's only one hook endpoint
  (`/hook/complete`).
- `/cli/*` — **73 branches remain in src-tauri.** Most delegate to
  `crate::commands::k2so_agents::*` (~9k lines). The blocker isn't
  the HTTP layer; it's that `commands::k2so_agents` has to either
  move into k2so-core or grow a `CommandDispatcher` bridge trait.

**Recommended next slice (heartbeat-critical for the persistent-agents
feature):** migrate these 9 public functions from
`src-tauri/src/commands/k2so_agents.rs` into `k2so-core`:
`k2so_heartbeat_{add,list,remove,set_enabled,edit,rename,fires_list}`,
`k2so_agents_heartbeat_tick`, `stamp_heartbeat_fired`. All are already
Tauri-free internally (uses `db::shared()` + pure helpers from the
same file). Pull the shared pure helpers (`resolve_project_id`,
`agents_dir`, `agent_dir`, `find_primary_agent`, `agent_wakeup_path`,
etc.) alongside them.

After that slice lands, the daemon's `/cli/heartbeat{,/add,/list,…}`
routes can migrate as thin 3-line wrappers around the extracted core
fns — the same pattern `/hook/complete` uses.

**Full scope beyond heartbeat:** the remaining ~64 /cli/* routes
require the rest of `commands::k2so_agents.rs` (and smaller chunks
of `commands::{projects, git, filesystem, settings, terminal}`) to
move into k2so-core. Estimate: 2–3 focused days if done carefully,
more if caught by circular deps. Not required for the "agents run
when lid is closed" demo — only the heartbeat slice is.

---

**Last updated:** 2026-04-19 post-compaction. Review on merge of
`feat/persistent-agents` into `main` — any items still open become their
own work-item files under `.k2so/work/inbox/`.
