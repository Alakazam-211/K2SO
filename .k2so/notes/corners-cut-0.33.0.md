# Corners cut during the 0.33.0 daemon pass

Honest log of shortcuts taken on `feat/persistent-agents`. Each item
includes the rationale, the "do it right" version, and — where
obvious — the specific file/line to revisit.

Created 2026-04-19 during the P3+P4 execution. Updated as more are
found.

---

## Architecture

### 1. Daemon uses `std::net::TcpListener`, not tokio
- **Where:** `crates/k2so-daemon/src/main.rs`
- **Why cut:** keeps the first working daemon small. Tokio-ization was
  scoped as its own phase (P3) but the plan let it fold into P4 so we
  didn't duplicate effort. Getting a synchronous boot path working
  first unblocked everything else.
- **Do it right:** switch the daemon's accept loop to
  `tokio::net::TcpListener` + `tokio::spawn` per connection, with a
  shared `tokio::runtime::Runtime` on the binary. Needed before the
  daemon can handle concurrent long-lived connections
  (companion WS, scheduled wakes, heartbeats).

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

**Last updated:** 2026-04-19. Review on merge of `feat/persistent-agents`
into `main` — any items still open become their own work-item files
under `.k2so/work/inbox/`.
