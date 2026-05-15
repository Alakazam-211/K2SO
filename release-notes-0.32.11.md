## 0.32.11 — Code hygiene: zero warnings, ~1,100 lines of dead code retired

A cleanup pass on top of 0.32.10's testability work. No user-facing changes. All compiler warnings eliminated (111 → 0), about 1,100 lines of retired-or-experimental code removed, and two genuine architectural observations surfaced: the experimental GPU bitmap renderer was removed cleanly (with breadcrumbs for revival), and the `try_acquire_running` CAS fix from 0.32.9 turned out to never have been wired into production — logged as a TODO for follow-up.

### Retired experimental paths (documented, not forgotten)

- **Bitmap terminal renderer** (`src-tauri/src/terminal/bitmap_renderer.rs`, ~400 lines) — planned future work for smooth GPU scrolling with a DOM overlay for text selection, but the DOM overlay was never wired up, so the bitmap path had no live callers. Removed along with the bitmap emission loop (~355 lines) and full-bitmap render helpers in `alacritty_backend.rs`. Module doc comments in `alacritty_backend.rs`, `font_renderer.rs`, and `grid_types.rs` now record why this was pulled and **where to get it back** — `git show v0.32.10:src-tauri/src/terminal/bitmap_renderer.rs` recovers the full implementation. When the bitmap+DOM-overlay UX gets revived, reintroduce from that tag.
- **LLM-based triage** (`llm_triage_decide`, `TRIAGE_SYSTEM_PROMPT`, `parse_triage_response`, `safe_generate_for_triage`, ~130 lines) — replaced by scripted `scheduler_tick` triage in an earlier release. The LLM branch was still in the source but never called. Removed. Tier3 section 3.2 now asserts the opposite invariant: the LLM triage path must NOT reappear on the triage surface.
- **Cursor IDE conversation parser** (`parse_cursor_ide_sessions`, 146 lines) — exploratory code for surfacing Cursor's in-IDE sessions in K2SO's chat history panel. Never wired to the UI; deleted.

### Other dead-code removals

- `get_buffer`, `get_frame`, `get_active_count` methods on `TerminalManager` — bitmap-era reattach helpers.
- `RasterizedGlyph`, `GlyphKey` structs; `rasterize`, `rasterize_glyph` methods; `STYLE_REGULAR/BOLD/ITALIC/BOLD_ITALIC` constants; 3-font array — all from the bitmap path's glyph cache. `GlyphCache` now only holds `cell_width`, `cell_height`, and the one `Font` needed for metrics.
- `BitmapUpdate`, `BitmapPerfInfo`, `RowStripUpdate`, `TerminalCell`, `SelectionAction`, `SelectionRequest` structs in `grid_types.rs` — bitmap IPC types + an unused selection API stub.
- `format_response`, `handle_auth_inline` in `companion/mod.rs` — ngrok-inline-auth helpers that never got called from the live companion request path.
- `load_model_cpu` in `llm/mod.rs` — CPU-fallback model loader; the worker subprocess architecture that would have used it never shipped.
- `db_path()` in `db/mod.rs`, `k2so_db_path()` in `k2so_agents.rs` — leftover helpers from before the shared-connection refactor.
- `read_workspace_wakeup`, `ensure_workspace_wakeup` in `k2so_agents.rs` — workspace-scope `wakeup.md` was retired in commit `d51b525` (pre-0.32.7); these helpers were missed in that sweep.
- 21 stale `let db_path = dirs::home_dir()...` lines across `agent_hooks.rs` (20) and `k2so_agents.rs` (1) — dead leftovers from the 0.32.9 shared-SQLite refactor. The next line in every case already called `crate::db::shared()`.
- 7 unused variables (`content`, `filename`, `now`, `project_md5`, `last_updated_at`, etc.) from the same refactor-leftover class.
- 9 unused imports across `chat_history.rs`, `companion/mod.rs`, `companion/proxy.rs`, `terminal/alacritty_backend.rs`, `fs_abstract.rs`.

### Resilience gap surfaced during cleanup

- **`AgentSession::try_acquire_running` is never called from production code.** The `BEGIN IMMEDIATE` CAS helper shipped in 0.32.9's resilience pass is only invoked by its own unit tests — the production PTY-spawn path in `commands/k2so_agents.rs` still uses the pre-CAS `is_agent_locked → spawn → upsert` sequence, which has the TOCTOU race the CAS was supposed to close. Added `TODO(resilience-followup)` comment on the function and `#[allow(dead_code)]` with a pointer to the production site that should adopt it. **This should be wired in as a near-term follow-up** — the 0.32.9 release notes advertised the CAS as closing the race, which is currently not true at runtime.

### Compiler-hygiene fixes

- **Cocoa deprecations suppressed cleanly.** The `cocoa` crate is deprecated upstream in favor of `objc2-app-kit` + `objc2-foundation`. Full migration is its own project; until then, `#![allow(deprecated)]` at the top of `commands/settings.rs` and `commands/filesystem.rs` (the only two files using cocoa) silences the upstream-churn noise without masking any real code smell.
- **`cfg(cargo-clippy)` → expected.** The `objc::msg_send!` macro expands to `#[cfg(feature = "cargo-clippy")]` gates that newer Rust's `unexpected_cfgs` lint flags as unknown. Declaring `cargo-clippy` as an expected feature value in `Cargo.toml`'s `[lints.rust]` silences these without disabling the lint globally.

### API annotations: pending-adoption items marked, not hidden

Four items are legitimate public API surface that isn't wired from production callers yet. Rather than delete them (they're documented future work) or `allow` at the crate level (which would hide future regressions), each is tagged at the definition site with `#[allow(dead_code)]` and a one-line comment explaining why:

- `trait Fs`, `struct RealFs`, `struct FsMetadata` (`fs_abstract.rs`) — pending Phase E-bis: thread `&dyn Fs` through `agent_hooks.rs` triage path. The trait + `RealFs` will become live at that migration; `FakeFs` is already in active use by tests.
- `AgentSession::try_acquire_running`, `AgentSession::update_last_opened`, `HeartbeatFire::prune_before` — DB API helpers covered by tests, need production-caller wiring.
- `TerminalTab`, `TerminalPane` structs — schema scaffolds for the persisted-terminal-tabs feature.
- `RawCredentials.access_token` — stored for Keychain round-trip completeness; only `refresh_token` + `expires_at` are consumed.

### Tests

- **159 Rust unit tests still pass** — 0 regressions from the deletion sweep.
- **424 tier3 source assertions pass** — was 428 pre-cleanup; -5 from removing "assert that dead LLM-triage code still exists" checks that were testing code I just retired, +1 new assertion that LLM inference is not on the triage path (catches accidental reintroduction).
- **111 CLI integration tests still pass.**
- **Clean cargo build** produces 0 warnings end-to-end.

### Why this ships now

The testability work in 0.32.10 added a lot of new code (FakeFs, schema unit tests, concurrency tests). Hygiene drift is easier to fix while the code is fresh. Shipping this before moving onto the security audit keeps the diff surface narrow for that review.
