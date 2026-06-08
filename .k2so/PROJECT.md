# K2SO

<!-- This file is shared context injected into every agent's CLAUDE.md at launch. -->
<!-- Keep it LEAN — high-signal, slow-changing knowledge every agent needs. -->
<!-- Each section ends with "Learn more" pointers to read on demand, not inline detail. -->

## About This Project

K2SO is an **AI workspace orchestration tool** (not an IDE): a headless server
with N possible viewers, where a *workspace is an agent* — it has hands (a
terminal + filesystem), memory, self-built tools, and scheduled heartbeats. A
"team" is connected workspaces, not sub-agents. MIT-licensed; ships no AI
harness — LLM integration is via the user's own CLI tools.

Learn more: memories `project_vision`, `project_workspace_equals_agent_foundational`, `project_agent_system`.

## Architecture

**One daemon, many front-ends.** The Rust **daemon** (`crates/k2so-daemon` +
`crates/k2so-core`) is the product; everything else is a thin front-end that
triggers it and renders its truth: the Tauri desktop client
(`src/renderer` + `src-tauri`), the `k2so` CLI (`cli/k2so`), K2 Connect (remote
access), and a future mobile/web/TUI. **The daemon also runs headless on Linux
with no client attached.**

- `src-tauri/` is **connection points + OS integration only** — no features.
- Clients talk to the daemon over `/cli/*` HTTP routes + WebSocket event buses
  (`session_events.rs` broadcast; grid-WS for terminal I/O).
- Readiness handshake: `/boot-status` (versioned) gates real routes until ready.

Learn more: memories `project_thin_client_is_connection_only`, `project_k2_product_taxonomy`, `project_daemon_handshake_contract`, `project_agent_terminal_architecture`.

## Key Directories

- `crates/k2so-core/` — daemon-shared logic (projects, sessions, settings, app_settings, skills, clone).
- `crates/k2so-daemon/` — the HTTP/WS daemon; routes in `routes/dispatcher.rs`, handlers in `*_routes.rs`.
- `src/renderer/` — React/TS thin client (`stores/`, `components/`).
- `src-tauri/` — Tauri shell: connection + OS integration only.
- `cli/k2so` — bash CLI; calls daemon routes directly via `cli_request`.
- `.k2so/prds/` — product/design docs (read the relevant PRD before large work).

## Conventions

**Daemon-first — canonical state & background loops live in the daemon.** The
daemon owns the truth and the loops that act on it; clients render truth and send
gestures. Before putting logic in `src/renderer/` or `src-tauri/`, check two
smells — if either is true, it belongs in `k2so-core`/`k2so-daemon`:
1. **Headless-breaking** — a timer/scheduler/sweep/cleanup/side-effect that only
   runs while the webview is open (never fires on the headless daemon).
2. **Multi-client divergence** — state derived or owned in the renderer that
   should be one canonical truth shared across all connected clients/users.
Cautionary tale: GH#22 — Active state + the session reaper ran in the renderer
keyed on the local client, so a remote client's session got reaped and a
headless daemon never reaped at all. Test headless (`k2so-daemon` + `curl
/cli/...`) before calling a feature done.

**Releases & versions.** Never hand-edit version strings — `scripts/release.sh
<version> [notes]` bumps tauri.conf.json + package.json + Cargo.toml ×3 +
Cargo.lock + `cli/k2so` in lockstep, builds/signs/notarizes, and publishes a
LIVE GitHub release. Requires a `## <version>` section in `WHATS_NEW.md`. Don't
publish a release without explicit user go.

**Commits.** No `Co-Authored-By`/model-attribution trailers.

**Tests.** Must fail loudly — no try/catch swallowing, no `unwrap_or` defaults in
assertions, no skip-if-missing fallbacks. Curl-test daemon endpoints in dev
before release. Mutating `/cli/*` routes need an `if !is_post { 405 }` guard.

**Subagent / worktree discipline.** Subagents work in isolated worktrees; never
build into main's `target/` or reload the production daemon — smoke-test in
foreground from the worktree. Integrate by **cherry-picking** the worktree's
commit onto current main (never merge the branch — main moves underneath).

**Renderer perf.** Never call `fetchProjects()` in render paths; use optimistic
updates.

Learn more: memories `feedback_daemon_first`, `feedback_no_version_bump`, `feedback_commit_attribution`, `feedback_test_discipline`, `feedback_test_before_release`, `feedback_post_only_route_guards`, `feedback_subagent_no_prod_reload`, `feedback_subagent_cherry_pick_pattern`, `feedback_dev_mode_performance`.

## External Systems

- **GitHub:** `Alakazam-211/K2SO` (private). Issues tracked there + in the task list.
- **k2.dev** — owned by Alakazam Labs; planned hosted-tier backbone + web (Vercel).
- **K2 Connect** — separate proprietary repo `../k2-connect`; control plane on Hetzner; Supabase (project "K2X") for accounts/billing.

Learn more (IDs, IPs, SSH paths, deploy commands kept out of shared context): memories `project_k2_dev_domain`, `project_k2_connect_repo`, `project_k2_connect_deploy`, `project_k2_connect_account_backend`, `reference_k2dev_web_deploy`.

## Build & Release

Build/sign/notarize uses Developer ID signing + the `K2SO-notarize` keychain
profile, driven by `scripts/release.sh`. Full credentials, exact commands, and
troubleshooting: memory `reference_build_sign_notarize`.
