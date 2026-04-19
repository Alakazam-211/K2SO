## 0.32.9 — Resilience pass: atomic writes, shared SQLite, safer migrations

This release is focused entirely on resilience. No new features — instead, every critical write path, mutex, panic surface, and DB-access pattern got hardened against the classes of failure that would otherwise corrupt user state: power loss mid-write, parallel delegations dropping writes, panics poisoning locks, clock skew breaking drift detection, duplicate PTY spawns from TOCTOU races. The audit surfaced 40+ specific findings across five dimensions (atomicity, teardown reversibility, concurrency, panic surface, idempotency); this release lands the fixes and the tests that guard them.

Reference points used throughout the review: Zed's `crates/fs/src/fs.rs` (atomic_write via NamedTempFile + persist), `crates/workspace/src/persistence.rs` (shared connection + savepoints), and `crates/migrator/src/migrator.rs` (immutable versioned migrations). K2SO now mirrors the first two; the third is deferred.

### Added

- **`src-tauri/src/fs_atomic.rs`** — new module. Three entry points every critical write now routes through:
  - `atomic_write(path, bytes)` / `atomic_write_str(path, &str)` — writes to a sibling tempfile, fsyncs, then renames into place. POSIX rename is atomic, so a power loss or SIGKILL mid-syscall leaves either the old file in full or the new file in full — never a truncated intermediate. The pre-0.32.9 code used direct `fs::write`, which could corrupt canonical `SKILL.md` (losing USER_NOTES forever) if interrupted.
  - `atomic_symlink(source, target)` — creates the symlink at a sibling tempfile and renames over. No window where readers see a missing file between remove+create.
  - `unique_archive_path(dir, stem, ext)` — collision-free archive naming. Replaces the seconds-granularity `{name}-{unix_secs}.md` scheme that silently clobbered archives created in the same wall-clock second (a real risk during first-run harvest, which can archive 5+ harness files per project within milliseconds). New shape: `{stem}-{unix_nanos}-{seq:04}{ext}`, using a per-process `AtomicU64` counter for tiebreaks.
  - `log_if_err(op, path, result)` — safe stderr-writing wrapper for best-effort ops. Replaces the pervasive `let _ = fs::...` pattern that swallowed every failure silently, and uses `writeln!(stderr)` rather than `eprintln!` so it can't panic when K2SO runs without a tty (Finder launch).
  - 11 unit tests covering rapid-fire archive-name uniqueness (4096 archives, zero collisions), atomic replace of regular files with symlinks, tempfile cleanup on error, and survival across 256 back-to-back overwrites.

- **Shared SQLite handle** (`src-tauri/src/db/mod.rs`):
  - New `static SHARED: OnceLock<Arc<ReentrantMutex<Connection>>>` populated at startup by `init_database`.
  - `pub fn shared() -> Arc<ReentrantMutex<Connection>>` — the one supported way for any thread to acquire the DB handle. `AppState.db` and `db::shared()` point at the same Arc, so Tauri commands, HTTP endpoints, and background threads all serialize through the same in-memory write queue on a single connection.
  - `pub fn open_with_resilience(path)` helper applies K2SO's standard PRAGMAs (WAL, busy_timeout=5000ms, foreign_keys) to any future standalone-tool opens.
  - **Why `ReentrantMutex`, not plain `Mutex`:** the helper-calls-helper pattern is pervasive — a Tauri command takes the lock, then calls `find_primary_agent()`, which re-acquires. Plain `parking_lot::Mutex` is not reentrant, so this deadlocks the UI thread on first invocation (observed as a macOS beachball in dev). Reentrant semantics let the same thread re-enter without self-deadlocking while still serializing across threads.
  - Lazy-init-for-tests branch (`#[cfg(test)]`) so unit tests get an in-memory SQLite on first `shared()` call without wiring up full Tauri startup.

- **Crash-detection marker for skill regen** — `write_workspace_skill_file_with_body` stamps `.k2so/.regen-in-flight` on entry and clears it on successful completion. New `detect_interrupted_regen()` runs in the startup migration loop and surfaces a one-shot stderr warning if the marker is stale from a previous crash (doesn't auto-repair — the next regen is idempotent and overwrites any partial state — but lets the user check `.k2so/migration/` for stale archives).

- **Content-hash drift adoption**. The `.k2so/.last-skill-regen` stamp is no longer an empty sentinel — it's now a JSON map of source-file content hashes (`{"project_md": "fnvhex", "agent_md::sarah": "fnvhex"}`). `adopt_workspace_skill_drift` compares stored hashes against current source-file hashes, not mtime, so drift detection is immune to clock skew, NTP jumps, and rsync mtime coercion. Mtime comparison remains as a fallback for workspaces upgrading from pre-0.32.9 stamps. User edits to `PROJECT.md` / `AGENT.md` always win: downstream SKILL.md + every harness symlink (CLAUDE.md, GEMINI.md, AGENT.md, .goosehints, .cursor/rules/k2so.mdc) get rebuilt with the user's new content on the next regen.

- **CAS-based agent session acquisition** (`AgentSession::try_acquire_running` in `src-tauri/src/db/schema.rs`). Replaces the pre-0.32.9 `is_agent_locked() → spawn PTY → upsert` sequence, which had a TOCTOU race: two heartbeats firing within a few milliseconds could both observe `is_locked=false` and both spawn, producing duplicate PTYs and a stale DB row. The new function wraps the check+insert in `BEGIN IMMEDIATE`, so concurrent callers serialize at the database level and only one returns `Ok(true)`.

- **`parking_lot::Mutex` everywhere**. Swapped `std::sync::Mutex` → `parking_lot::Mutex` in:
  - `companion/{mod,types,auth,proxy,websocket}.rs` (5 files, ~20 call sites)
  - `commands/companion.rs`
  - `terminal/alacritty_backend.rs` (6 `.lock().unwrap()` sites in the rendering hot path)
  - `agent_hooks.rs` (15 sites including ring-buffer + triage lock + port-file watchdog)
  - `editors.rs`
  
  `std::sync::Mutex` poisons on panic — a single `.unwrap()` inside a locked section cascades into every future lock attempt failing. `parking_lot::Mutex` doesn't poison; a panic releases the lock cleanly. Removes 12 Tier-B panic surfaces identified in the audit. `parking_lot::Mutex::try_lock` returns `Option` (not `Result`); three call sites in `companion/mod.rs` were updated accordingly.

- **Agent Skills page rework** (`src/renderer/components/Settings/sections/AgentSkillsSection.tsx`):
  - Tab order reshuffled to **Custom Agent → K2SO Agent → Workspace Manager → Agent Template** (default tab is now Custom Agent, the most common case for solo workspaces).
  - Right-side preview panel replaced with **inline collapsible rows**. Clicking a layer rotates a `▸` chevron and expands the content in place. Multiple rows can be open at once for side-by-side comparison. User-layer content is fetched lazily on first open and cached per tab.
  - New **"context stack" explanation block** above the list. Per-tab copy explaining what kind of agent the stack gets injected into (Custom Agent → autonomous single-agent workspaces; K2SO Agent → the planner; Manager → top-of-stack triage; Agent Template → sub-agents the manager delegates to). Replaces the old "click a layer to preview" tooltip that didn't explain what users were actually configuring.
  - "Context stack" as the user-facing term replaces the internal-test-script nickname "hamburger" — which stays in tier3 shell assertions (where it's descriptive of what the assertion is checking) but never surfaces to users.

### Changed

- **Every critical-path `fs::write` converted to atomic writes.** Rewired: `archive_claude_md_file`, `strip_workspace_skill_tail`, `append_workspace_source_regions`, `import_claude_md_into_user_notes`, `migrate_and_symlink_root_claude_md`, `force_symlink`, `safe_symlink_harness_file`, `scaffold_aider_conf`, `write_cursor_rules_mdc`, `upsert_k2so_section`, `harvest_per_agent_claude_md_files` sentinel, migration banner, `.last-skill-regen` stamp, plus the Tauri-facing `ensure_skill_up_to_date` / `ensure_agent_wakeup` / `ensure_workspace_wakeup` / manager + K2SO agent scaffolding / `promote_legacy_heartbeat` template writes / `migrate_or_scaffold_lead_heartbeat` wakeup write / all teardown write-backs. The existing `atomic_write` helper inside `k2so_agents.rs` became a thin wrapper around `fs_atomic::atomic_write_str` so its ~15 existing callers inherit the new tempfile naming + fsync guarantees without needing per-site changes.

- **Teardown `keep_current` mode now atomic.** Previously: `fs::remove_file(path)` → `fs::write(path, body)`. If the write failed, the user was left with neither a symlink nor a real file — unrecoverable without manual intervention. Now: `atomic_write_str(path, body)` does the replace in one atomic rename, so a write failure leaves the original symlink intact. Same fix applied to `restore_original` mode.

- **Harvest sentinel now gated on full success.** `harvest_per_agent_claude_md_files` tracks `any_failure` across all agent CLAUDE.md archives. Only stamps `.harvest-0.32.7-done` if every archive + remove succeeded. Before: sentinel was stamped unconditionally, so a single failure mid-loop permanently stranded orphan pre-0.32.7 CLAUDE.md files (they'd be skipped on every future boot). Now: partial failure retries on next launch.

- **60 ad-hoc `rusqlite::Connection::open(...)` sites consolidated** onto `crate::db::shared().lock()`. Files touched: `agent_hooks.rs` (22 sites), `commands/k2so_agents.rs` (35 sites), `lib.rs` (1 site). `chat_history.rs` opens against third-party SQLite files (Claude/Cursor chat histories) and is exempt. The Zed lesson here: one physical connection per process means one in-memory write queue, so WAL-mode serialization actually serializes. Before, 60 transient connections each hit the BUSY handler independently — under parallel delegations this produced silent write drops.

- **`find_latest_archive` extension-matching preserved** across the new nanosecond naming format. Backward-compat: falls back to parsing old `<stem>-<unix_secs>{ext}` archives alongside the new `<stem>-<unix_nanos>-<seq>{ext}` format, so existing `.k2so/migration/` archives still restore cleanly on `restore_original` teardown.

- **Adoption log wording improved.** When the user edits `PROJECT.md` or `AGENT.md` directly, the regen no longer logs "CONFLICT" (which was misleading when only the user changed things). New wording: *"user edit detected — downstream SKILL.md + harness files will pick up the new content on this regen."* The behavior is unchanged; only the log message is clearer.

### Fixed

- **Tier-A panic surface — 3 startup/HTTP sites that would lock users out of the app:**
  - `agent_hooks.rs:531` — `TcpListener::bind("127.0.0.1:0").expect(...)`: the notification HTTP server used to panic on bind failure (port exhaustion, sandbox denial). Now returns `Result<u16, String>`; the caller in `lib.rs` logs the diagnostic and emits a `hook-injection-failed` frontend event so the UI can still render without the HTTP endpoint.
  - `agent_hooks.rs:532` — `listener.local_addr().unwrap()` now propagates via `?`.
  - `lib.rs:911` — `.expect("error while building K2SO")` on Tauri's `build()` replaced with `.unwrap_or_else(|e| { writeln!(stderr, "..."); process::exit(1) })`. We can't show a GUI error pre-webview, but at least the crash message lands in Console.app instead of a silent abort.

- **Dev-mode startup beachball** introduced during the Batch 6 shared-SQLite refactor. `migrate_or_scaffold_lead_heartbeat` held `db::shared().lock()` across a call to `k2so_heartbeat_add`, which also locks. Plain `parking_lot::Mutex` is not reentrant, so this self-deadlocked the main thread on first boot with a manager-mode workspace. Root cause fixed by switching the shared handle to `ReentrantMutex`; a tier3 assertion guards against accidental regression back to plain `Mutex`.

- **`log_if_err` uses raw `writeln!(stderr)` instead of `eprintln!`.** `eprintln!` panics on write failure, which can cascade to SIGABRT when K2SO runs with no tty attached (Finder-launched builds). The new helper matches the existing `log_debug!` macro's behavior — silently drops the write rather than crashing the process over a failed log line.

### Tests

- **62 Rust unit tests** (was 42) — 20 new tests covering: atomic-write cleanup on error, tempfile non-orphaning, rapid-fire archive uniqueness (4096-way test), symlink replace of regular files, regen-in-flight marker lifecycle (stamped on entry, cleared on success, one-shot warning on stale marker), collision-free harvest under tight-loop pressure, tight-retry idempotency of `teardown_keep_current`, content-hash-based drift detection (identical content ignored despite mtime changes; real content changes detected), `AgentSession::try_acquire_running` CAS semantics across three-round acquire/release/reacquire.

- **390 tier3 source assertions** (was 329) — 56 new assertions covering: fs_atomic module presence + API shape + fsync-before-rename invariant, force_symlink using atomic_symlink, all scaffolding writes routed through `log_if_err` + `atomic_write_str`, harvest sentinel gated on full-success, per-file-no-std-sync-Mutex invariant across companion/terminal/agent_hooks, zero `.lock().unwrap()` in converted files, Tier-A panic sites gone, shared SQLite handle type shape (`OnceLock<Arc<ReentrantMutex<Connection>>>`), zero ad-hoc `Connection::open` in runtime paths, CAS via `BEGIN IMMEDIATE`, content-hash drift helper set, Agent Skills tab order (Custom first), default tab is `custom_agent`, no right-side preview panel leaked back in, "context stack" explanation present, per-tier blurbs present.

- **19 tier1** + **111 CLI integration** — both unchanged from baseline, all passing against the live rebuilt app.

- **582 tests total across all four suites, 0 failures.** Clean-build dev server comes up without the beachball (verified end-to-end: tauri dev → app launches → `/health` responds → still responds 5 seconds later, confirming no hang).

### Why this ships now

The 0.32.x line has been landing user-facing features fast (Phase 7b harness fan-out, Phase 7c drift adoption, Phase 7d generalized ingest, Phase 7e teardown modes, Phase 7f add/remove dialogs). Each landed cleanly, but an end-of-line audit against Zed's resilience patterns surfaced that the underpinning is fragile — specifically around atomicity, concurrency under parallel delegations, and the panic-poison-cascade class. This release closes that gap so 0.33.x can introduce new features (Ollama Modelfile auto-generation, savepoint-based transactional multi-step ops, proper schema migrations with dependency resolution) on top of a foundation that doesn't corrupt user state under failure. Nothing user-visible changes except the Agent Skills tab rework; the rest is invisible until something goes wrong, which is exactly how this kind of work is supposed to feel.

### Out of scope / explicitly deferred

- **Full `Fs` trait abstraction** (Zed's `crates/fs/src/fs.rs::trait Fs` + `FakeFs`). Would unlock deterministic mid-write-failure tests, but requires threading the trait through every filesystem caller — multi-week work whose immediate value is testability we achieve today via tempdirs + the new atomic helpers. Planned for a later refactor.
- **Savepoint-based transactional multi-step ops** (Zed's `with_savepoint("name", || { ... })` pattern). K2SO now has a shared connection that could support this, but no call site currently benefits enough to justify the API work. Adoption, skill regen, and harvest are all step-atomic via `atomic_write` today — true multi-step rollback becomes useful when we have multi-row DB writes that need all-or-nothing semantics.
- **Immutable versioned settings migrations** (Zed's `crates/migrator/src/migrator.rs`). K2SO's current startup migration loop uses filesystem sentinels (`.harvest-0.32.7-done`, etc.) per-version. Works fine; upgrading to a chain-based migrator matters only when we have user-editable JSON settings that need schema evolution.
- **Backporting recent Tauri-Alacritty improvements to the open-source `tauri-plugin-terminal` repo.** Worth doing — the copy-paste reliability, tab-swap persistence, and Rust-side state ownership work is general-purpose. Tracked as a follow-up; scoped as a diff-and-cherry-pick pass when that repo is the active target.
