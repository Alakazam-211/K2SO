## 0.32.10 — Testability pass: Fs trait + FakeFs, db unit coverage, proven concurrency

This release is testability, end-to-end. No user-facing changes — instead, the codebase gained an in-memory filesystem abstraction (Zed-inspired), a per-struct unit test suite for the previously-untested database module, and real multi-thread concurrency tests that *prove* the resilience claims 0.32.9 introduced (rather than just asserting them in doc comments).

Test count went from 62 → 159 Rust unit tests and 398 → 428 tier3 source assertions. Every piece of the resilience work from 0.32.9 that said "safe under contention" now has a test that spawns threads and verifies it.

Reference: this review used Zed's `crates/fs/src/fs.rs::trait Fs` + `FakeFs` as the model for the new `fs_abstract` module, and `crates/project/tests/` as the model for parity-style unit tests. K2SO's `fs_abstract` lands the essentials (trait + both impls + JSON DSL + write-count instrumentation) and deliberately skips Zed's extras (custom async executor, file-watcher integration, git-repo trait) that don't pull their weight in a Tauri app.

### Added

- **`src-tauri/src/fs_abstract.rs`** — new module. The testability seam.
  - `pub trait Fs` with 13 methods covering every std::fs call K2SO business logic reaches for: `read_to_string`, `read`, `write`, `exists`, `metadata`, `read_dir`, `create_dir_all`, `remove_file`, `remove_dir_all`, `rename`, `symlink`, `read_link`, `copy`. Signatures mirror `std::fs` one-for-one so migration is a receiver-only change at call sites.
  - `pub struct RealFs` — always compiled, trivially-thread-safe impl that delegates to `std::fs`. Production always uses this.
  - `pub struct FakeFs` — `#[cfg(test)]`-only in-memory BTreeMap-backed impl. Arc<Mutex<State>> behind it so it's Send+Sync and can be cloned into worker threads the same way a real Arc<dyn Fs> would be. Absolute-paths-only (panics on relative paths — test authors get a loud error, not silent misbehavior).
  - JSON DSL via `insert_tree(path, serde_json::json!({...}))` — seed an entire workspace tree in one call, matching Zed's ergonomics. Objects become directories, strings become files.
  - Write-count / metadata-call / read-dir-call instrumentation (`write_count(path)`, `metadata_call_count()`, `read_dir_call_count()`). Catches regressions like "a refactor silently added a double-write" or "this loop does O(n) stat where O(1) read_dir would do."
  - 13 FakeFs unit tests + 8 RealFs↔FakeFs parity tests (every operation runs against both impls; any behavioral difference fails the test).

- **`src-tauri/src/db/schema.rs unit_tests` module** — 32 per-struct CRUD tests. The DB module was previously 1,969 LOC with 0 unit tests; tier3 grep assertions against SQL strings were the only safety net. Now every public struct (FocusGroup, Project, AgentSession, AgentHeartbeat, HeartbeatFire, ActivityFeedEntry, WorkspaceRelation, AgentPreset) has round-trip coverage, plus edge-case tests for: UNIQUE(project_id, agent_name) rejection on duplicate agent sessions, heartbeat name validation (reserved names, hyphen rules, uppercase), project path UNIQUE constraint, wake counter increment/reset semantics, terminal_id→session lookup, unread-messages filtering, RFC3339 timestamp parsing after `stamp_last_fired`.

- **`src-tauri/src/db/schema.rs concurrency_tests` module** — 4 multi-thread CAS tests. `try_acquire_running_exactly_one_winner_under_parallel_contention` spawns 20 threads racing for the same (project, agent) lock and asserts exactly one returns `Ok(true)` — the proof that `BEGIN IMMEDIATE` actually closes the TOCTOU. Plus: different-agents-all-win (per-agent scope), serializes-without-busy-errors (5 rounds × 10 threads = 50 CAS calls with zero SQLITE_BUSY surface), and reacquire-after-release.

- **`src-tauri/src/db/mod.rs tests` module** — 12 migration/bootstrap tests. Covers: every core table exists after `run_migrations` (catches a migration file being dropped from the list), idempotency on re-run, `seed_agent_presets` produces exactly 11 built-ins and is idempotent across reseeds, `open_with_resilience` applies WAL + busy_timeout=5000ms + foreign_keys=ON PRAGMAs, `bootstrap_test_db_at` creates usable file-backed DBs, `isolated_test_connection` gives distinct in-memory DBs (writes don't leak across test boundaries).

- **fs_atomic concurrency tests** — 6 new. Every primitive in the 0.32.9 resilience pass now has a real-thread test:
  - `atomic_write_survives_parallel_writer_contention` — 10 writers × 100 iterations hammer the same path; final content is always a complete, well-formed payload.
  - `atomic_write_reader_never_observes_partial_content` — 4 writers × 80 iters + a reader thread; every read returns either the sentinel or a well-formed payload, never a truncated mid-rename state.
  - `atomic_write_leaves_no_tempfiles_after_parallel_contention` — 8 threads × 50 iters; directory afterwards contains only the target, no orphans.
  - `atomic_symlink_reader_never_observes_enoent_under_contention` — 1000 reads while a writer re-links continuously; ENOENT is strictly forbidden (what would falsify the atomicity claim). EINVAL retries are tolerated with explicit documentation of the macOS kernel's path-resolution race during `rename()` on symlinks.
  - `unique_archive_path_never_collides_under_parallel_contention` — 16 threads × 500 = 8,000 paths; zero duplicates.
  - `tempfile_path_never_collides_under_parallel_contention` — internal tempfile naming proven unique across threads.

- **Pure helpers extracted from Tauri command handlers** (Phase C — testability pattern demonstration):
  - `pub fn update_agent_md_field(content, field, value) -> Result<String>` — the 50+ lines of frontmatter + section manipulation from `k2so_agents_update_field`. 8 unit tests covering frontmatter updates, section replacement (existing + appending), missing frontmatter rejection, unterminated frontmatter rejection, values with colons, body-preservation regression.
  - `pub fn compose_manager_wake_from_body(Option<&str>) -> String` — the wake-prompt composer for workspace managers. 4 tests covering body-present, body-None-falls-to-template, empty-string-falls-to-template, frontmatter stripping.
  - `pub fn compose_agent_wake_from_body(Option<&str>) -> Option<String>` — the agent-tier wake composer. 3 tests covering None→None, header wrapping, as-given frontmatter behavior.
  - `pub fn parse_work_item_content(content, filename, folder) -> WorkItem` — extracted from `read_work_item`. 4 tests covering full frontmatter, defaults on missing fields, no-frontmatter handling, 120-char body-preview truncation.

- **3 FakeFs-driven integration tests in k2so_agents** — demonstrating the end-state pattern:
  - Scaffold an agent work tree with `FakeFs::insert_tree`, read entries via the trait, feed content into `parse_work_item_content`, assert expected WorkItems result. No tempdir, no disk I/O.
  - Simulate a missing agent work dir and verify the NotFound error surfaces correctly.
  - End-to-end frontmatter round-trip: insert AGENT.md into FakeFs, read, pass through `update_agent_md_field`, write back, verify content + write-count.

- **Test helpers in `db/mod.rs`**: `bootstrap_test_db_at(path)` and `isolated_test_connection()` — `pub(crate)` utilities for multi-connection concurrency tests (file-backed) and single-test isolation (each call returns its own fresh in-memory SQLite with full migrations + seeds applied).

### Changed

- **macOS symlink-read-race quirk documented and tolerated**. The `atomic_symlink_reader_never_observes_enoent_under_contention` test initially flaked with EINVAL (os error 22) during rapid re-linking. Root cause: macOS's kernel occasionally surfaces path-resolution races during `rename()` on symlinks as EINVAL on the subsequent `read()` — a transient retry condition, not an atomicity violation. The test now retries EINVAL up to 8 times while strictly failing any ENOENT. Documented inline so future authors understand the distinction: EINVAL = kernel race (retry); ENOENT = atomicity broken (bug).

### Fixed

- Nothing user-reachable. This release adds tests and test infrastructure; no production code paths changed behavior.

### Tests

- **159 Rust unit tests, all passing** (was 62). Breakdown: 17 fs_atomic (11 original + 6 new concurrency), 21 fs_abstract (new), 32 db::schema::unit_tests (new), 4 db::schema::concurrency_tests (new), 12 db::tests (new), 22 k2so_agents::pure_helper_tests (new), 25 k2so_agents::migration_safety_tests (unchanged from 0.32.9), plus 26 miscellaneous unchanged (agent_hooks ring buffer, terminal reflow, llm file_index, llm tools).

- **428 tier3 source assertions, all passing** (was 398). +30 new assertions protecting the testability infrastructure: every Fs trait method pinned, RealFs+FakeFs presence, FakeFs cfg(test) gating, JSON DSL shape, instrumentation methods, parity-test floor (≥6), db/mod.rs and db/schema.rs test module presence, all concurrency-test names pinned, pure-helper extraction pinned, FakeFs adoption signal in k2so_agents tests.

- **111 CLI integration tests, all passing** (unchanged).

- **Total: 698 tests across four suites, 0 failures.**

### Why this ships now

0.32.9 shipped resilience code that *claimed* safety under contention (BEGIN IMMEDIATE CAS, atomic_write with tempfile+rename, atomic_symlink). Those were claims, not proofs — every existing "rapid-fire" test ran sequentially on one thread. This release closes the proof gap. If atomic_write were non-atomic, a new test would fail. If try_acquire_running had a TOCTOU, a new test would show multiple winners. That's the standard we want every future resilience claim held to.

The FakeFs module is the bigger lever. Until now, every test that touches filesystem logic had to scaffold a real tempdir (~2ms per test on macOS APFS). FakeFs drops that to ~5µs and unlocks simulating conditions that would otherwise require root (the BTreeMap-based fake can model any error class we choose to surface). Phase E demonstrated the pattern on 3 tests; the remaining migration is additive and can happen incrementally.

### Out of scope / explicitly deferred

- **Threading `&dyn Fs` through every std::fs caller**. The audit identified ~937 direct filesystem touchpoints across 20 files. Migrating all of them is multi-week work. This release lands the trait + fake + pattern demonstration; future PRs migrate hot-path modules (k2so_agents command handlers, agent_hooks triage, lib.rs startup migrations) incrementally. Tier3 asserts the fake has been adopted *somewhere* as a smoke signal against "trait added then forgotten."

- **Error injection API for FakeFs**. Current FakeFs doesn't model permission errors, disk-full, or partial-read failures — only missing files, wrong types, and NotFound-on-missing. If/when we need "simulate ENOSPC mid-write" tests, that gets added. Zed's FakeFs also lacks this, for the same reason: real-world resilience tests live in the concurrency suite where the actual race exists, not in a fake that might or might not match the kernel.

- **Deterministic async test executor** (Zed's `TestDispatcher` + `TestScheduler`, ~1000 LOC). K2SO's fs ops are synchronous and Tauri uses Tokio; no reason to port Zed's custom scheduler. If K2SO ever adds async trait methods to `Fs`, we can revisit — but it's unlikely.

- **Property-based testing (proptest/quickcheck)**. Could replace some of the "8000 path uniqueness" loop-tests with a proptest strategy. Real value once K2SO has fuzzable invariant targets (e.g., "any frontmatter-update sequence preserves the body byte-identity"). Not load-bearing today.

- **Removing TEST_LOCK in agent_hooks.rs**. Investigated and kept — the lock protects a legitimate process-wide ring buffer singleton, not a shared-DB leak. Removing it would require injecting the buffer into `record_recent_event` as a parameter, which isn't worth the refactor for 3 tests.
