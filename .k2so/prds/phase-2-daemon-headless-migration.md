# Phase 2: Daemon-Headless Migration

**Status**: Active — Unit 1 first, Units 2–7 parallel after Unit 1 lands
**Internal version markers**: 0.39.0f and beyond (no public 0.39.0 release until Mobile Companion contract is hardened in Phase 3)
**Owner**: Rosson + pod-leader
**Updated**: 2026-05-23

---

## tl;dr

Move every workspace-coupled piece of logic from `src-tauri/` into `k2so-daemon` so the daemon serves clients — local Tauri, Mobile Companion via ngrok, and K2SO Connect from another laptop — without depending on Tauri being present.

After Phase 2:
- `src-tauri/` shrinks from ~18,800 LoC to ~3,500–4,000 LoC of legitimate host code (Tauri framework, native macOS chrome, host introspection, physical I/O).
- The daemon owns its own ngrok tunnel, its own scheduler, its own LLM subprocess, its own PTYs, its own DB writes.
- Every Tauri command that survives is either Tauri-framework-required or about the user's local machine (mic, keyboard, screen, clipboard, notifications, native dialogs).
- The `/cli/*` HTTP + WS contract becomes the **single surface** Mobile Companion and K2SO Connect harden against in Phase 3.

7 parallelizable migration units + Phase 2.1 CLI cleanup. ~12,500 LoC moved into the daemon, ~15,000 LoC deleted from `src-tauri/`, ~1,400 LoC deleted from `cli/k2so`.

---

## Architectural lens

K2SO has two real product shapes:

1. **Bundled K2SO** — Tauri thin client + `k2so-daemon` on the same machine, local socket (UDS or 127.0.0.1).
2. **K2SO Connect** — Tauri thin client on Machine A, `k2so-daemon` on Machine B, network transport via ngrok (same tunnel as Mobile Companion).

For every `.rs` file under `src-tauri/src/`, ask **one question**:

> "If the daemon is on a different machine than the Tauri client, does this code still belong on the Tauri side?"

- **YES** → `HOST` (stays Rust in `src-tauri/`).
- **NO** → `MIGRATE` to `k2so-daemon`, expose via `/cli/*`, renderer calls daemon directly.
- **BRIDGE** → Tauri `#[tauri::command]` shim that becomes ~0 LoC once the renderer hits the daemon directly. Delete after the renderer is migrated.

The corollary: "this uses libgit2 and libgit2 is Rust" is **not** a reason to keep code in `src-tauri/`. libgit2 lives in `k2so-core`, can be exposed as a daemon route, and the renderer calls it over HTTP. The only legitimate Rust-in-`src-tauri/` reasons are:

1. Tauri framework requirements (`tauri::Builder`, `setup()`, `#[tauri::command]` registration, AppHandle/AppState dependencies the framework injects).
2. Native macOS chrome the user sees and interacts with on their own machine.
3. Host-process introspection (memory watcher, this-process RSS).
4. Physical I/O devices attached to the user (mic, keyboard, mouse, clipboard, screen, speakers).

---

## HOST principles: where bytes enter or leave the user

Mental anchor: **the daemon is a remote computer; the Tauri client is the keyboard, screen, and ears.**

Anything that's about the keyboard/screen/ears stays Tauri. Anything that's about the remote computer goes daemon.

### Physical input devices (bytes entering from the user)
- **Microphone / voice → STT** — TCC mic permission, audio capture. Output goes over WS to the daemon as text + a "write to PTY" intent.
- **Keyboard** — global shortcuts (Cmd+T, Cmd+K, Cmd+L), key chord capture, IME composition.
- **Mouse / trackpad** — clicks, scrolls, drag-and-drop origin.
- **Touch ID / biometric** — local Secure Enclave; cannot be remoted.
- **Camera / screen capture** — TCC again; for face presence, screenshot-of-my-desktop, etc.
- **Clipboard reads** — pasteboard lives on the user's OS.
- **Drag-from-Finder** — file blob originates on the user's disk; the upload becomes a daemon WS send.

### Physical output devices (bytes leaving to the user)
- **Screen pixels** — the entire renderer.
- **Native dialogs** — file/folder picker for "pick a folder from MY computer", save-as, system alerts. Must overlay the user's window.
- **Notifications** — show on the user's screen, not the server's.
- **Tray icon, dock badge, menu bar** — user's menu bar.
- **Audio playback** — bells, TTS, alert sounds — user's speakers.
- **Updater** — replaces *this* `.app`, not the daemon.

### Host introspection (about the user's machine, not the workspace)
- **macOS permissions** (`AXIsProcessTrusted`, TCC checks for mic/cam/screen-recording, Full Disk Access) — about Tauri's own process.
- **Memory watcher** — this Tauri process's RSS.
- **Window state** — this window on this display.
- **System theme (dark/light), display geometry, DPI** — user's machine.
- **Idle detection** for active-hours — user's actual presence.
- **PATH enrichment for tools Tauri itself spawns** — only relevant in Bundled mode.

### Local network identity / connection management
- **K2SO Connect address book** — "which remote daemons have I trusted?" — personal to the user's device.
- **Per-daemon auth credentials** (TLS cert pinning, OAuth tokens) — must be unlockable by *this* user on *this* device.
- **LAN discovery (mDNS/Bonjour)** — finding K2SO daemons on the same LAN as the user.
- **Connection state machine** (online/offline indicator, retry/backoff) — about the network from the user's vantage point.

### Crash & diagnostics
- **Crash reporter** — must run from the crashing Tauri process.
- **Tauri app logs** — `~/Library/Logs/K2SO/`.

**What is NOT in this list and is genuinely daemon-side**: filesystem (it's the daemon's filesystem, not the user's), git (daemon's repos), themes (theme content is workspace; "which theme am I using" is a preference that follows the user, so daemon), chat history (lives where the IDE writes the JSONL — the workspace), settings (workspace-scoped), every SQLite table.

---

## Why ngrok is the keystone (Unit 1 must land first)

Both K2SO Connect and Mobile Companion connect to the daemon **through the daemon's own ngrok tunnel**. Today, that tunnel is started by Tauri's setup hook (`src-tauri/src/lib.rs:942` calls `companion::start_companion()`). This is architecturally broken:

- **K2SO Connect**: user on Machine A connects to daemon on Machine B. There is **no Tauri on Machine B**. If Tauri owns the tunnel start, Machine B's daemon has no tunnel and Machine A can never reach it.
- **Mobile Companion**: phone connects to the public ngrok URL. If the user's Tauri is closed on the laptop, the daemon is unreachable. The "daemon-first" promise breaks the moment Tauri is the gatekeeper.

After Unit 1 lands:
- Daemon's first-boot hook reads `companion.auto_start` from its own SQLite and starts ngrok itself.
- Companion credentials, tunnel auth tokens, public hostname — already daemon-side in `k2so-core::companion`. Just need the daemon to call `start_companion()` instead of Tauri.
- `/cli/companion/{start,stop,status,set-password,disconnect-session}` becomes the **single contract surface** both Mobile Companion and K2SO Connect's tunnel-status indicator hit.
- Tauri's `lib.rs:942` start-on-setup call gets deleted. The `commands/companion.rs` shims (5 of them) become trivial daemon proxies, then deleted once the renderer calls `/cli/companion/*` directly.
- The Connect address book stays in Tauri per HOST principles — but the actual tunnel that those hostnames point to is daemon-owned.

**Implication for sequencing**: Unit 1 unblocks Phase 3 entirely. Mobile Companion and K2SO Connect can't be hardened against a contract that doesn't exist yet — and the contract is "the daemon serves its own tunnel, no Tauri required."

---

## Inventory: HOST / MIGRATE / BRIDGE

Current `src-tauri/src/` size: **18,794 LoC across 44 `.rs` files** (post-Phase-1, post-loose-ends).

### HOST (stays Rust in `src-tauri/`) — ~13 files, ~3,200 LoC

| File | Lines | Why HOST |
|---|---:|---|
| `main.rs` | 31 | Tauri entry; `--llm-worker` arm migrates with Unit 2 |
| `lib.rs` (residual after migrations land) | ~500 of 1,500 | Tauri builder, setup hook (slimmed), tray bootstrap, window event handlers |
| `menu.rs` | 227 | macOS menu, AppHandle-only |
| `tray.rs` | 509 | macOS tray; reads daemon status via `daemon_client` |
| `window.rs::position/size handlers` | partial | Window event subscriptions for THIS window; SQL writes go to daemon |
| `watcher.rs` | 135 | UI-only FS notifications for the renderer's file tree pane on Bundled mode (in Connect mode, daemon's watcher fires WS events instead) |
| `daemon_client.rs` | 218 | The bridge itself |
| `daemon_events.rs` | 179 | WS subscriber → Tauri event bus re-emit |
| `agent_hook_sink.rs` | 28 | Trait impl wiring core→Tauri event bus |
| `commands/daemon.rs` | 382 | launchctl bootstrap of the daemon's own plist (daemon can't manage itself) |
| `commands/updater.rs` | 102 | Tauri auto-updater for this `.app` |
| `commands/permissions.rs` | 233 | macOS TCC; must run in user session |
| `commands/memory_watcher.rs` | 145 | This process's `proc_pidinfo` RSS |
| `commands/mod.rs` | 28 | Module index |
| `commands/workspace_ops.rs` | 130 | Pure renderer-event re-emit; tied to native menu items |
| `commands/projects.rs::pick_folder` | partial of 1,026 | tauri-plugin-dialog folder picker only; rest of file migrates |
| `commands/agent_hooks.rs::script writers` | partial of 689 | Writes to `~/.claude/settings.json`, `~/.cursor/hooks.json` — needs Tauri's FDA-bound context |
| `commands/format.rs` | 128 | Optional HOST — pure subprocess that runs in user PATH; could go either way |

### MIGRATE (move to daemon) — ~18 files, ~9,500 LoC moved

See per-unit breakdowns below. Highlights:
- `commands/assistant.rs` (445) — LLM
- `commands/claude_auth.rs` (650) — auth scheduler
- `commands/chat_history.rs` (2,085) — JSONL parsing
- `commands/filesystem.rs` (728) — FS ops
- `commands/git.rs` (227) — git ops
- `commands/projects.rs` (1,026, minus pick_folder) — project lifecycle
- `commands/settings.rs` (785) — settings
- `commands/terminal.rs` (237) — PTY lifecycle

### BRIDGE → deleted — ~8 files, ~6,500 LoC eliminated

Files that are 1:1 `#[tauri::command]` re-exports of `k2so_core::*`. After the renderer switches to direct daemon HTTP calls, these files vanish.

- `commands/k2so_agents.rs` (6,237) — 80 thin re-exports of `k2so_core::agents::commands::*`
- `commands/companion.rs` (70)
- `commands/whats_new.rs` (66)
- `commands/states.rs` (68)
- `commands/workspaces.rs` (48)
- `commands/project_config.rs` (25)
- `commands/agents.rs` (133, presets)
- `commands/workspace_regen_provider.rs` (27)

---

## The 7 parallelizable units

Each unit is sized for one subagent session. Units 1–4 must respect the dependency chain; Units 5, 6, 7 are independent and can run alongside.

### Unit 1 — Companion + ngrok to daemon (Bucket A.1) **[FIRST — must land before others]**

**Files in scope:**
- `src-tauri/src/companion_host.rs` (105)
- `src-tauri/src/companion_settings_provider.rs` (35)
- `src-tauri/src/commands/companion.rs` (70)
- `src-tauri/src/lib.rs` — `start_companion()` call at line 942, bridge registrations at lines 261–263 and 322

**Daemon work:**
- Move `CompanionHost`/`TerminalProvider`/`AppEventSource` trait impls into daemon-side providers (daemon owns AppHandle-less impls).
- Add daemon first-boot hook: read `companion.auto_start` from SQLite; if true and credentials present, start ngrok tunnel.
- Add `/cli/companion/set-password`, `/cli/companion/disconnect-session`. Existing routes (`start`, `stop`, `status`, `presets`, `projects`, `sessions`, `projects-summary`) stay.

**Renderer TS changes:**
- Replace `invoke('companion_start' | 'companion_stop' | 'companion_status' | 'companion_set_password' | 'companion_disconnect_session')` → `daemon.cli('/cli/companion/...')` calls in the Companion settings UI.

**LoC**: ~210 moved, ~70 deleted from `src-tauri/`.

**Dependencies**: None. Lands first.

**Verify after merge**: With Tauri closed, restart daemon, confirm ngrok tunnel comes up by checking `~/.k2so/companion.url` (or equivalent state file). Mobile Companion should still reach the daemon.

---

### Unit 2 — LLM inference to daemon (Bucket A.2)

**Files in scope:**
- `src-tauri/src/commands/assistant.rs` (445)
- `src-tauri/src/main.rs:16` — `--llm-worker` dispatch arm
- `src-tauri/src/lib.rs::llm_worker_main` (~80 LoC)
- `AppState::llm_manager` field

**Daemon work:**
- Add `/cli/llm/{chat (streaming), status, load-model, download-default, check}`.
- Daemon spawns LLM subprocess via `k2so-daemon --llm-worker <payload>` (mirrors current pattern but daemon-parented).
- Daemon owns model auto-download on first boot.
- **Resilience requirement (per Rosson)**: subprocess supervisor with timeout (60s default), RSS watchdog, max-concurrency gate, crash isolation. LLM crash must not crash the daemon. Daemon autorestarts the LLM subprocess if it dies or exceeds memory.

**Renderer TS changes:**
- Sidebar chat hook switches `invoke('assistant_chat', ...)` → WS subscription on `/cli/llm/chat` for streaming tokens.

**LoC**: ~570 moved, ~520 deleted.

**Dependencies**: None for migration; blocks the AppState shutdown unload thread refactor (`lib.rs:699`).

---

### Unit 3 — Terminal PTY ownership to daemon (Bucket A.4 + A.5 partial)

**Files in scope:**
- `src-tauri/src/commands/terminal.rs` (237)
- `src-tauri/src/terminal_event_sink.rs` (53)
- `AppState::terminal_manager` field
- Terminal lifecycle in `lib.rs` close handler (~50 LoC at lib.rs:718–731)

**Daemon work:**
- Existing routes: `/cli/terminal/{read,write,spawn,spawn-background}`, `/cli/sessions/*` WS.
- Add: `/cli/terminal/{create, kill, resize, active-count, kill-foreground, get-grid, scroll, exists, log, foreground-cmd}`.
- Renderer subscribes to daemon's existing `/sessions/grid` + `/sessions/bytes` WS for output.

**Renderer TS changes:**
- Every PTY consumer (`useTerminal`, terminal panes) switches from `invoke('terminal_*')` + `listen('terminal:grid:<id>')` to daemon WS subscriptions.

**LoC**: ~290 moved, ~340 deleted.

**Dependencies**: Largest risk. **Must land after Unit 2** to avoid double-loading the AppState shutdown refactor. Best to land before Unit 4.

---

### Unit 4 — DB-direct writes to daemon (Bucket A.5)

**Files in scope:**
- `src-tauri/src/commands/states.rs` (68)
- `commands/workspaces.rs` (48)
- `commands/focus_groups.rs` (159)
- `commands/workspace_sections.rs` (90)
- `commands/workspace_layouts.rs` (110)
- `commands/timer.rs` (97)
- `commands/agents.rs` (133, presets)
- `commands/projects.rs` (1,026 — minus pick_folder)
- `commands/git.rs` (227) — worktree-DB couplings
- `src-tauri/src/window.rs::window_state writes` (partial of 130)
- `src-tauri/src/lib.rs:583–628` — SKILL regen DB write
- `src-tauri/src/lib.rs:1429–1499` — workspace_layouts migration

**Daemon work:**
- New routes: `/cli/states/{create,update,delete}`, `/cli/workspaces/{list,create,delete}`, `/cli/focus-groups/*`, `/cli/sections/*`, `/cli/workspace-layouts/*`, `/cli/timer/*`, `/cli/presets/*`, `/cli/projects/*` (~20 endpoints), `/cli/window-state/{get,set}`.
- Daemon runs the SKILL regen background pass + workspace_layouts migration on its own startup (not Tauri's).

**Renderer TS changes**: Replace ~40 `invoke(...)` calls across renderer state/projects/workspaces/focus/sections/timer/presets/layouts hooks.

**LoC**: ~2,000 moved, ~1,900 deleted.

**Dependencies**: Land after Unit 3 (shared `state.db` field removed in same series). Largest by LoC.

---

### Unit 5 — Claude Auth scheduler to daemon (Bucket A.3)

**Files in scope**:
- `src-tauri/src/commands/claude_auth.rs` (650)

**Daemon work:**
- New routes: `/cli/claude-auth/{status, install-scheduler, uninstall-scheduler, refresh-now, credentials-get}`.
- Daemon writes `com.k2so.claude-auth-refresh.plist` on its own first boot (mirrors daemon.plist pattern).

**Renderer TS changes**: Settings → Claude Auth section swaps `invoke('claude_auth_*')` → daemon CLI calls.

**LoC**: ~650 moved, ~650 deleted.

**Dependencies**: Independent of Units 1–4. **Spike daemon-side macOS Keychain access first** — open question whether the daemon LaunchAgent process can read `security find-generic-password -s 'Claude Code-credentials'` cleanly. If it fails, the read-side stays in Tauri and the daemon reads via a small Tauri-mediated cache file.

---

### Unit 6 — Filesystem + Chat + Themes + Skill layers + Review checklist + ProjectConfig + WhatsNew (renderer-only) + Format (optional)

**Files in scope:**
- `commands/filesystem.rs` (728)
- `commands/chat_history.rs` (2,085)
- `commands/themes.rs` (160)
- `commands/skill_layers.rs` (125)
- `commands/review_checklist.rs` (178)
- `commands/project_config.rs` (25)
- `commands/whats_new.rs` (66) — daemon already has the routes; just swap renderer invokes
- `commands/format.rs` (128) — optional; consider HOST until daemon's PATH parity is verified

**Daemon work:**
- New routes: `/cli/fs/*` (~13 routes), `/cli/chat/*` (~10 routes), `/cli/themes/*` (~5 routes), `/cli/skill-layers/*` (~4 routes), `/cli/review-checklist/*` (~4 routes), `/cli/project-config/*` (~3 routes).
- Daemon may need to expose larger payloads (e.g. read-binary) — consider WS-streamed or chunked HTTP.

**Renderer TS changes**: File browser, chat history sidebar, theme manager, review pane — all switch invokes to daemon CLI.

**LoC**: ~3,500 moved, ~3,400 deleted.

**Dependencies**: Independent of Units 1–5. Largest single landing.

---

### Unit 7 — split into 7a / 7b / 7c

The original framing — "delete 6,237 LoC of thin re-exports + move 1,200 LoC of SKILL scaffolding" — materially undercounts the work. A scope audit on 2026-05-23 found that `k2so_agents.rs` contains: ~1,500 LoC of one-shot migration helpers called from `lib.rs` setup hooks, ~3,500 LoC of SKILL scaffolding (not 1,200), ~440 LoC of heartbeat-launchd installer, ~200 LoC of triage logic, and real BRIDGE re-exports on top — plus 75 renderer `invoke()` call sites and cross-file references from `lib.rs`, `projects.rs`, `agent_hooks.rs`, and `daemon.rs`. Attempting all of it in one session risks a half-finished merge.

The unit is split into three landings:

#### Unit 7a — App Settings to daemon + F3 close [BOUNDED — Wave A]

**Files in scope:**
- `src-tauri/src/commands/settings.rs` (785 — `settings_update`, `settings_reset`, related accessors; `read_settings`/`write_settings` stay until 7c since `daemon.rs` references them)
- `crates/k2so-core/src/app_settings.rs` (NEW — global `AppSettings` struct + tmp+rename atomic writes)
- `crates/k2so-daemon/src/main.rs` (POST allowlist + handler branches for new routes)
- ~25 renderer `invoke` call sites in `src/renderer/components/Settings/sections/*`

**Daemon work:**
- New `k2so_core::app_settings` module: `pub fn load()`, `pub fn save()`, `pub fn update(partial)`.
- New routes: `GET /cli/settings/get`, `POST /cli/settings/update`, `POST /cli/settings/reset`.
- **F3 closure**: `app_settings::update()` detects when companion-affecting fields change (username, password, port) and triggers daemon-side companion invalidation directly. No more two-writer race on `~/.k2so/settings.json`.

**Tauri side:**
- Replace `commands/settings.rs::settings_update` body with a daemon proxy (don't delete the file yet — `daemon.rs::{get,set}_keep_daemon_on_quit` still references it; 7c will delete).
- Resolve the `TODO Phase 2 Unit 7` markers.

**Renderer:**
- Migrate ~25 Settings UI `invoke()` calls to daemon CLI.

**LoC**: ~400 moved/refactored, ~30 LoC F3 fix.

**Dependencies**: None. Wave A.

#### Unit 7b — SKILL scaffolding + migration helpers to k2so-core [LARGE, NO TAURI CHURN]

**Files in scope:**
- `src-tauri/src/commands/k2so_agents.rs:1443–4964` — SKILL scaffolding (~3,500 LoC): `write_workspace_skill_file_with_body`, `adopt_workspace_skill_drift`, `strip_workspace_skill_tail`, `append_workspace_source_regions`, `migrate_and_symlink_root_claude_md`, `import_claude_md_into_user_notes`, `harvest_per_agent_claude_md_files`, `archive_claude_md_file`, `inject_first_migration_banner`, `safe_symlink_harness_file`, `write_workspace_harness_discovery_targets`, `write_cursor_rules_mdc`, `scaffold_aider_conf`, `teardown_workspace_harness_files`, `find_latest_archive`, `k2so_agents_teardown_workspace`, `k2so_agents_preview_workspace_ingest`, `k2so_agents_run_workspace_ingest`, `ensure_all_skills_up_to_date`
- `src-tauri/src/commands/k2so_agents.rs:609–1251` — one-shot migration helpers (~1,500 LoC): `archive_orphan_top_tier_agents`, `repair_mismigrated_heartbeats`, `promote_legacy_heartbeat`, `ensure_workspace_wakeups`, `migrate_filenames_to_uppercase`, `migrate_or_scaffold_lead_heartbeat`, `detect_interrupted_regen`
- New module: `crates/k2so-core/src/agents/workspace.rs` (or similar — propose at landing time)
- Constants and section markers (`K2SO_SECTION_BEGIN/END`, `SKILL_VERSION_WORKSPACE`) get a public home alongside the scaffolding.

**Goal:** Move bodies into k2so-core. The Tauri `#[tauri::command]` wrappers in `k2so_agents.rs` become trivial `pub use` re-exports — they STAY in `k2so_agents.rs` until 7c. **No renderer changes**, **no `lib.rs::invoke_handler!` changes**. Tauri-side compile must still pass at unit-merge time.

**Daemon work:** Daemon's first-boot hook calls the now-public k2so-core functions directly (replacing Tauri's `lib.rs` setup-hook calls for these migrations — the daemon should run them on its own boot, not Tauri's).

**LoC**: ~5,000 moved into k2so-core; same LoC reduced from bodies in `k2so_agents.rs` (becomes `pub use` re-exports until 7c finishes the job).

**Dependencies**: None. Can land alongside 7a or any other Wave A unit.

#### Unit 7c — `k2so_agents.rs` final BRIDGE deletion + heartbeat-launchd MIGRATE

**Files in scope:**
- `src-tauri/src/commands/k2so_agents.rs` (entire file — fully deletable after 7b)
- `src-tauri/src/commands/settings.rs` (entire file — last accessors `read_settings`/`write_settings` move to daemon)
- `src-tauri/src/workspace_regen_provider.rs` (27)
- `src-tauri/src/commands/daemon.rs` (refactor `get_keep_daemon_on_quit`/`set_keep_daemon_on_quit` to call daemon HTTP)
- `src-tauri/src/lib.rs` (~70 `invoke_handler!` entries removed, 13 setup-hook calls cleaned up)
- `src-tauri/src/commands/projects.rs:369` (replace `archive_orphan_top_tier_agents` call with daemon HTTP)
- `src-tauri/src/commands/mod.rs` (remove module decls)
- ~45 remaining renderer invoke call sites

**Heartbeat-launchd MIGRATE** (`k2so_agents.rs:2700–3138`, ~440 LoC):
- Move `install_heartbeat_launchd`, `install_heartbeat_cron`, `generate_heartbeat_script`, `k2so_agents_install_heartbeat`, `k2so_agents_apply_wake_scheduler`, `k2so_agents_uninstall_heartbeat` into daemon-side `crates/k2so-daemon/src/heartbeat_launch.rs` (already partially exists).
- New routes: `/cli/heartbeat/install-launchd`, `/cli/heartbeat/uninstall-launchd`, `/cli/heartbeat/apply-wake-scheduler`.
- **Rationale**: required for K2SO Connect — a remote daemon must install its own scheduler plist without depending on Tauri being present. Daemon already manages its own `com.k2so.daemon` plist via Unit 1's pattern; the heartbeat is the next plist to come under daemon ownership.

**LoC**: ~6,600 deleted from src-tauri, ~440 moved to daemon (heartbeat installer).

**Dependencies**: Must land after 7b (which empties the body bodies). Other Wave units can land any time.

---

## Phase 2.1 — CLI verb redesign + headless-daemon simplification

**Status**: Carries over the deferred work from 0.37.1 (originally Phase 4.1 — see commit `1bdeb528` for the pattern established by Phase 4). Reframed under the daemon-headless lens.

**Goal**: Every `k2so <verb>` should be a thin shell over a single `/cli/*` daemon route. **No business logic in the CLI itself.** This is the same architectural argument as Tauri: the `cli/k2so` shell script is another thin client, and its job is parsing args + formatting output, not doing work.

**Current state**: `cli/k2so` is **3,854 lines** with **96 `cmd_*` functions**. Many of those functions still do work that should be in the daemon (filesystem checks, git operations, JSON wrangling), and the verb taxonomy still reflects the pre-workspace-unification design.

### Scope (per task #433 / workspace-agent-unification PRD)

**New verbs:**
- `k2so workspaces list` — yellow pages: every registered workspace + agent + status (alive/sleeping/cold) + last activity. Thin shell over new `/cli/workspaces/list`.
- `k2so workspaces running` — replaces `k2so agents running`, listed by workspace.
- `k2so workspace launch [--workspace <path>]` — spawn-or-attach smart cascade.
- `k2so workspace profile [--workspace <path>]` — reads `.k2so/agent/AGENT.md`.
- `k2so workspace update --field <f> --value <v>` — edits workspace's primary agent.
- `k2so signal --workspace <path> <kind> <payload>` — workspace-keyed signal addressing (parallel to `msg`).
- `k2so template {list, create, delete}` — replaces `k2so agent template *`, manages `.k2so/agent-templates/`.

**Behavior changes:**
- `k2so work` (no `--agent` flag) — workspace-implicit.
- Remove (or deprecate-with-warning) `k2so agents create` / `k2so agents delete` — workspaces own their primary agent; bare-agent CRUD is gone.
- `k2so help-deprecated` aggregator listing every retired verb + the new equivalent.

**Architectural cleanup under the daemon-headless lens:**
- Audit every `cmd_*` function. For each, ensure the body is no more than: parse args → call `cli_get`/`cli_post` against a `/cli/*` route → format output. Any function with conditionals, SQL, filesystem reads, or git logic is doing work the daemon should own.
- The CLI gets shorter (target: ≤ 2,500 LoC). The daemon `/cli/*` surface gets richer.
- Side benefit: `k2so` works against a **remote daemon** (K2SO Connect mode) with no code changes — every verb is already an HTTP call. This is the CLI equivalent of "Tauri is just a thin client."

**New tests:**
- `msg_no_agent_flag` — exit non-zero with deprecation message.
- `workspaces_list_yellow_pages` — register 3 workspaces, assert all 3 in output with correct status.
- `cli_works_against_remote_daemon` — point `K2SO_DAEMON_URL` at a remote daemon, run a representative slice of verbs, assert success.

**LoC**: ~1,400 deleted from `cli/k2so` (3,854 → ~2,500); ~600 added to `crates/k2so-daemon/src/cli.rs` for new routes; net workspace shrinkage ~800 LoC.

**Dependencies**: Independent of Units 1–7 in source, but **best landed after Units 2/5/6** so the new verbs target the post-migration `/cli/*` surface rather than chasing a moving target.

**Verify after merge**:
1. `K2SO_DAEMON_URL=https://remote-host.ngrok.app k2so workspaces list` — works end-to-end against a remote daemon. CLI on Machine A, daemon on Machine B, no Tauri involvement.
2. `wc -l cli/k2so` — ≤ 2,500.
3. No `cmd_*` function contains direct filesystem/git/SQL code.

---

## Daemon contract surface (post-Phase-2)

The full `/cli/*` route table that Mobile Companion and K2SO Connect will harden against in Phase 3.

**EXISTING** = currently in `crates/k2so-daemon/src/cli.rs`/`main.rs`.
**NEW** = added by this phase.

### Filesystem (all NEW — Unit 6)
`/cli/fs/{read-dir, search-tree, read-file, read-binary, write-file, move, copy, delete, rename, create, duplicate, open-finder, open-external, clipboard-paths}`

### Git (all NEW — Unit 4)
`/cli/git/{info, branches, worktrees, create-worktree, remove-worktree, reopen-worktree, status, diff-file, diff-summary, diff-between, file-at-ref, stage, unstage, stage-all, commit, merge-branch, merge-status, abort-merge, resolve, delete-branch, prune-worktrees}`

### Projects / Workspaces (mostly NEW — Unit 4)
- NEW: `/cli/projects/*` (~21 routes), `/cli/workspaces/{list,create,delete}`, `/cli/focus-groups/*` (~6), `/cli/sections/*` (~6), `/cli/workspace-layouts/*` (~4)
- EXISTING: `/cli/states/{list,get,set}` — add NEW `/cli/states/{create,update,delete}`
- NEW: `/cli/window-state/{get,set}`

### Settings (extend EXISTING — Unit 7a + Unit 1)
- EXISTING: `/cli/settings`
- NEW: `/cli/settings/{get, update, reset}`, `/cli/companion/{settings, set-password, disconnect-session}`

### Heartbeat-launchd (NEW — Unit 7c)
`/cli/heartbeat/{install-launchd, uninstall-launchd, apply-wake-scheduler}` — daemon writes/uninstalls the heartbeat plist on its own; required for K2SO Connect.

### Agents / Work / Reviews / Heartbeats (mostly EXIST — Unit 7)
EXISTING (incomplete list): `/cli/agents/*` (list, create, delete, profile, work, heartbeat, lock, unlock, triage, launch, delegate, running, reap, generate-claude-md), `/cli/agent/{update, reply, complete}`, `/cli/work/*`, `/cli/reviews`, `/cli/review/*`, `/cli/onboarding/*`, `/cli/heartbeat/*`, `/cli/scheduler-tick`

Audit for missing edges as part of Unit 7.

### Terminal (mostly EXIST, extend — Unit 3)
- EXISTING: `/cli/terminal/{read, write, spawn, spawn-background}`, `/cli/sessions/{resize, label, subscribe, bytes, grid, events, v2/spawn, v2/close, diagnose, lookup-by-agent, list-for-workspace}`
- NEW: `/cli/terminal/{create, kill, resize, active-count, kill-foreground, foreground-cmd, exists, get-grid, scroll, log}`

### Chat history (all NEW — Unit 6)
`/cli/chat/{list, storage-paths, custom-names, rename, pinned, toggle-pin, detect-active, discover-ide, migrate-ide, session-exists}`

### LLM (all NEW — Unit 2)
`/cli/llm/{chat (streaming WS), status, load-model, download-default, check}`

### Companion (extend EXISTING — Unit 1)
- EXISTING: `/cli/companion/{start, stop, status, presets, projects, sessions, projects-summary}`
- NEW: `/cli/companion/{set-password, disconnect-session, settings}`

### Claude Auth (all NEW — Unit 5)
`/cli/claude-auth/{status, install-scheduler, uninstall-scheduler, refresh-now}`

### Themes / Skill layers / Review checklist / Project config / Timer / Presets / Whats New
- NEW: `/cli/themes/*` (~5), `/cli/skill-layers/*` (~4), `/cli/review-checklist/*` (~4), `/cli/project-config/*` (~3), `/cli/timer/*` (~4), `/cli/presets/*` (~6)
- EXISTING: `/cli/whats_new`, `/cli/whats_new/{mark_seen, reset}`

### Events / Awareness / Hooks (all EXIST)
EXISTING: `/cli/events`, `/cli/awareness/{publish, subscribe}`, `/cli/hooks/status`, `/cli/feed`, `/cli/checkin`, `/cli/done`, `/cli/reserve`, `/cli/release`, `/cli/status`, `/cli/commit`, `/cli/commit-merge`, `/cli/connections`, `/cli/agentic`, `/cli/mode`

### WS contract (all EXIST)
`/events`, `/sessions/subscribe`, `/sessions/bytes`, `/sessions/grid`, `/sessions/events`, `/awareness/subscribe` — these are already the channels K2SO Connect and Companion use; no Phase 2 changes required.

---

## Findings from Unit 1 (apply to subsequent units)

These surfaced during Unit 1 execution — not anticipated in the original plan, important for the other agents to know:

### F1. Rustls 0.23 needs `CryptoProvider::install_default()` at daemon boot

Tauri already does this; the daemon hadn't because nothing in the pre-Phase-2 daemon used TLS directly. Unit 1 wired the call into `crates/k2so-daemon/src/main.rs` before any HTTPS code path runs. **Implication for Unit 5 (Claude Auth)** and any unit that brings up a `reqwest`/HTTPS path: this is already done — no need to add it again, just be aware that TLS will Just Work.

### F2. `tokio::runtime::Handle` capture pattern for non-tokio threads

`companion::start_companion()` runs on a `std::thread::spawn`'d worker. From inside that worker, calling `app_event_source::subscribe()` triggers a `tokio::spawn` which panics with "no reactor running" because the worker thread isn't inside a tokio runtime. Unit 1's fix: capture a `tokio::runtime::Handle` at provider register time (when we're still on the runtime), then use `handle.spawn(...)` from inside the worker.

**Implication for Unit 3 (Terminal PTY)**: `TerminalEventSink` migration will hit the identical pattern — PTY reader threads are `std::thread::spawn` workers that need to publish events into the tokio runtime. Reuse the handle-capture pattern. See `crates/k2so-daemon/src/companion_host.rs` for the reference impl.

### F3. Two writers to `~/.k2so/settings.json` until Unit 7 consolidates

After Unit 1, both Tauri's `settings_update` and the daemon's `companion_host::persist_companion_password_fields` write to the same file. Both use tmp+rename so reads are never torn, but there's no merge-on-write protection — the race window is single-digit milliseconds. **Tauri's `crate::companion::invalidate_all_sessions()` in `src-tauri/src/commands/settings.rs::settings_update` is now a no-op** because Tauri's in-process `STATE` is empty post-Unit-1; it's marked `TODO Phase 2 Unit 7`.

**Implication for Unit 7 (Settings to daemon)**: the consolidation work is non-optional — keep `TODO Phase 2 Unit 7` markers visible so the agent doesn't miss them. Renderer password rotates go through `/cli/companion/set-password` which DOES invalidate daemon-side, so the practical gap is only "username changes via Tauri settings UI without a password rotate" — narrow, but a real correctness gap that must close in Unit 7.

---

## Cross-cutting concerns

### 1. `lib.rs` is a sequence of startup-time migrations

The 1,500-line file is mostly setup-hook code: 8+ one-shot migrations (legacy agent types, daemon plist install, daemon autostart, SKILL regen, window state migration, workspace_layouts migration, wakeup scaffolds, hook script writing).

**At least half of these belong in the daemon's own first-boot** (skill regen, layouts migration, agent type migration, wakeup scaffolds). After Unit 4 lands, `lib.rs` should shrink to ~500 LoC: Tauri-Builder + window event + tray + hook script registration + daemon kickstart.

### 2. Tauri-side event emission becomes daemon WS frames

Files like `agent_hook_sink.rs`, `terminal_event_sink.rs`, `companion_host.rs::TauriCompanionEventSink` are trait impls that take an `AppHandle` and forward to Tauri's event bus. After daemon migration, the daemon already has `crates/k2so-daemon/src/events.rs` doing this via WireEvent JSON over WS.

The renderer's `listen('agent:lifecycle', ...)` becomes a WS frame subscription via `daemon_events.rs`. The trait impls in `src-tauri/` become noops once daemon owns the emit source. `daemon_events.rs` already re-emits daemon events back onto Tauri's event bus so the renderer doesn't have to change. **Preserve this pattern.**

### 3. `projects_pick_folder` is the only `commands/projects.rs` HOST carve-out

The folder picker is a native dialog — must stay HOST. Carve it out when migrating projects.rs; the other 20 project commands move.

### 4. `workspace_regen_provider.rs` exists because of a circular dep

It exists so `k2so_core::agents::build_launch` can call the heavy SKILL.md scaffolding orchestrator in `commands/k2so_agents.rs`. Once Unit 7 moves the scaffolding into `k2so-core`, this bridge dies. **Order**: Unit 7's scaffolding migration must precede deleting the regen provider.

### 5. AppState ownership model needs explicit unwinding

After Units 2/3/4 land, `AppState` has only `watchers: Mutex<HashMap<...>>` left. Inline it into `watcher.rs` as a module-level static and delete `state.rs`. Watch for `try_state::<AppState>()` calls in `daemon_events.rs`, `window.rs`, `tray.rs`, `companion_host.rs` — those need alternate paths during the transition.

### 6. macOS Keychain access from a LaunchAgent daemon — open question

Tauri runs in the user GUI session; the daemon is a LaunchAgent. Both *should* have user-session keychain access (LaunchAgents run in user session, unlike LaunchDaemons), but the Claude Auth `security find-generic-password -s 'Claude Code-credentials'` call should be smoke-tested from the daemon process before Unit 5 commits. If it fails, the read-side stays in Tauri and the daemon reads via a small Tauri-mediated cache file.

### 7. `format.rs` PATH question

It spawns `prettier`/`rustfmt`/`gofmt` from the user's PATH. Daemon-side works if the daemon launches with the same shell-enriched PATH (`enrich_path_from_login_shell` at `lib.rs:251`). If so, MIGRATE. Otherwise HOST. Default to HOST since "format on save" already targets the local file in the user's environment.

### 8. `menu.rs` + `tray.rs` keep their daemon-status dependency

Tray reads daemon uptime via `daemon_client::status()`. They're host UI surfaces that *display* daemon state. No migration. Ensure they handle K2SO Connect mode (where daemon is on a different machine — tray currently assumes 127.0.0.1).

### 9. K2SO Connect implication for Unit 3 (terminal)

When daemon is on Machine B, Tauri on Machine A subscribes to WS terminal events from Machine B. This is exactly the architecture Unit 3 produces — the migration *is* the K2SO Connect enablement for terminals.

---

## Sequencing

```
                                       ┌────────────────────────────┐
                                       │ Unit 1: Companion + ngrok │  ← lands first (keystone)
                                       │ (no AppState changes)      │
                                       └─────────────┬──────────────┘
                                                     │
                  ┌──────────────────────────────────┼──────────────────────────────────┐
                  ▼                                  ▼                                  ▼
   ┌────────────────────────┐    ┌────────────────────────┐    ┌────────────────────────────────┐
   │ Unit 2: LLM            │    │ Unit 5: Claude Auth     │    │ Unit 6: FS + Chat + Themes +   │
   │ subprocess to daemon   │    │ scheduler to daemon     │    │ Skills + Review + Format       │
   │                        │    │ (Keychain spike first)  │    │ (largest LoC)                  │
   └─────────────┬──────────┘    └────────────────────────┘    └────────────────────────────────┘
                 │
                 ▼
   ┌────────────────────────┐
   │ Unit 3: Terminal PTY    │  ← shared AppState shutdown refactor with Unit 2
   │ ownership to daemon     │
   └─────────────┬──────────┘
                 │
                 ▼
   ┌────────────────────────┐
   │ Unit 4: DB writes +     │  ← shared state.db field removal with Unit 3
   │ window state + lib.rs   │
   │ migration moves         │
   └────────────────────────┘
                                                     │
                                                     ▼
   ┌────────────────────────────────────────────────────────────────────────────────────────┐
   │ Unit 7: k2so_agents BRIDGE deletion + Settings to daemon                                │
   │ (Independent of other units; can run alongside 2/5/6 or after 3/4)                      │
   └────────────────────────────────────────────────────────────────────────────────────────┘
```

**Rationale**:
- **Unit 1 first**: keystone for K2SO Connect + Mobile Companion. Unblocks Phase 3.
- **Units 2, 5, 6 parallel after Unit 1**: independent file sets, no shared state.
- **Unit 3 after Unit 2**: both touch the AppState shutdown refactor — serialize them to avoid merge thrash.
- **Unit 4 after Unit 3**: shares `state.db` removal — serialize.
- **Unit 7 floats**: independent of the above; can land any time after Unit 1.

---

## Definition of done (Phase 2 → Phase 3 gate)

Phase 2 is complete when:

1. ✅ Tauri can be force-killed and the daemon continues to serve Mobile Companion + K2SO Connect requests with no degradation.
2. ✅ `cargo build -p k2so-daemon && ./target/debug/k2so-daemon` — runs end-to-end without Tauri ever being launched. Ngrok tunnel comes up. LLM responds. Terminals spawn. Heartbeats fire. DB writes work.
3. ✅ `src-tauri/src/` is ≤ 4,000 LoC of Rust.
4. ✅ Every remaining `#[tauri::command]` in `src-tauri/` is one of: (a) Tauri framework requirement, (b) native macOS chrome, (c) host introspection, (d) physical I/O.
5. ✅ The `/cli/*` route table above is fully implemented and exercised by integration tests.
6. ✅ Renderer has zero `invoke('...')` calls for workspace state — only HOST concerns invoke Rust.

After done: enter Phase 3 (contract hardening — TLS, auth, OpenAPI export, Mobile Companion contract update, K2SO Connect thin-client-only build).

---

## Out of scope (explicit non-goals)

- **TLS on the ngrok tunnel** — Phase 3 hardening.
- **OpenAPI codegen from `/cli/*`** — Phase 3; will replace handwritten TS daemon-client shims.
- **TS rewrite of any HOST code** — Phase 3.1 (K2SO Connect thin-client-only build).
- **Removing `k2so-core` as a direct dep of `src-tauri`** — HOST files may legitimately still import core types; the constraint is no `core::*` *side-effects* in Tauri (no SQL writes, no LLM start, no PTY spawn, no ngrok start, no scheduler tick).
- **Daemon HA / multi-daemon failover** — single daemon per machine remains the model.
- **WebSocket-first companion protocol redesign** — already a separate planned project; this phase only ensures the daemon owns the existing protocol.

---

## Open questions

1. **Daemon Keychain access from LaunchAgent** — must spike before Unit 5 commits.
2. **`format.rs` PATH parity** — verify daemon's PATH matches user shell's PATH before deciding HOST vs MIGRATE.
3. **K2SO Connect address book schema** — what's the on-disk format? Probably JSON in `~/.k2so/connect-hosts.json` with TLS cert pins. Out of Phase 2 scope but flag for Phase 3.1.
4. **LLM subprocess supervision policy** — exact RSS threshold (start with 3GB?), exact concurrency cap (1?), exact restart cooldown.

---

## References

- Prior audit: `/tmp/k2so-thin-client-reaudit-post-phase-1.md`
- Loose-ends work: completed 2026-05-23 (uncommitted in current branch — `session_stream` feature flag retired, `tabs.ts` Kessel refs purged, `agent_hooks.rs` dead direct-DB helpers deleted)
- Related PRD: `.k2so/prds/kessel-research-archive.md` (Kessel v2 future vision; out of Phase 2 scope)
- Memory: `project_websocket_companion_plan.md`, `feedback_daemon_first.md`, `feedback_subagent_no_prod_reload.md`

---

## Execution log

| Unit | Status | Branch / Commit | Notes |
|---|---|---|---|
| 1 — Companion + ngrok | **Merged to main** 2026-05-23 | `02efb165` | Keystone landed. Headless smoke verified: daemon-only run brings up ngrok at `https://k2.ngrok.app` without Tauri. See findings F1–F3. |
| 2 — LLM subprocess | pending | — | Wave A |
| 3 — Terminal PTY | pending | — | Wave B (after Unit 2 merges) — use F2 tokio-handle pattern |
| 4 — DB-direct writes | pending | — | Wave C (after Unit 3 merges) |
| 5 — Claude Auth | pending | — | Wave A — Keychain spike first |
| 6 — FS + Chat + Themes + Skills + Review + ProjectConfig + WhatsNew | pending | — | Wave A — largest LoC |
| 7a — App Settings + F3 close | pending | — | Wave A — bounded; closes the correctness gap Phase 3 needs |
| 7b — SKILL scaffolding + migration helpers → k2so-core | pending | — | Can land alongside 7a; no Tauri-command churn |
| 7c — `k2so_agents` BRIDGE deletion + heartbeat-launchd MIGRATE | pending | — | After 7b empties the bodies; daemon owns its own heartbeat plist |
| 2.1 — CLI verb redesign | pending | — | After Units 2/5/6 merge |
