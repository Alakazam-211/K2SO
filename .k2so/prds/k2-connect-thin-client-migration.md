---
title: "K2 Connect — Plan B: migrate the renderer off the local-only Tauri proxy"
status: draft
owner: Rosson
created: 2026-06-03
supersedes_consideration: "Plan A (host-aware DaemonClient), shipped in 0.39.18 as the pragmatic bridge"
---

# K2 Connect — Plan B: Thin-Client Migration to the Host-Aware HTTP Layer

## 1. Context & problem

When the desktop client connects to a **remote** daemon (a K2 Connect tunnel
host, e.g. `https://reggie.k2.dev`), only part of the UI follows it. Two data
paths exist in the renderer:

- **Host-aware (correct):** `src/renderer/lib/daemon-cli.ts`
  (`daemonCliGet`/`daemonCliPost`) + `src/renderer/kessel/daemon-ws.ts`
  (`getDaemonWs`), which read `useConnectHostStore.getState().activeHost` and
  build the request URL from the active host (local **or** remote). Used by the
  file tree, terminals/PTY streaming, settings, session-events WS, and
  `/boot-status`.
- **Host-unaware (the bug):** `renderer invoke('<cmd>')` → a Tauri command in
  `src-tauri/src/commands/*.rs` → `DaemonClient::try_connect()`, which is
  **hardcoded to `http://127.0.0.1:<local-port>`**. ~12–13 command modules
  (projects, git, workspaces, sections, focus_groups, agents/presets, states,
  workspace_layouts, timer, settings, k2so_agents partial, daemon partial)
  proxy daemon data this way, so the nav, git, agents, etc. always show **this**
  machine even when connected to a remote.

## 2. The two philosophies

- **Plan A — host-aware `DaemonClient` (chokepoint).** Teach the one Rust
  client to resolve the active host (remote base + session token) instead of
  always localhost. One place changes; all proxied routes follow. **Shipped in
  0.39.18** as the pragmatic bridge. Pros: tiny, fixes everything at once, low
  risk. Cons: keeps daemon data flowing *through* the thin client's Rust proxy;
  two HTTP paths (Rust `DaemonClient` + renderer `daemon-cli`) to keep in sync;
  adds a process-global "active daemon" in Rust.
- **Plan B — migrate the renderer to `daemon-cli` (this PRD).** Stop proxying
  daemon data through Tauri commands; the renderer calls the host-aware HTTP
  layer directly for everything, and the redundant proxy commands are deleted.
  This is the architectural north star: **thin client = connection points + OS
  integration only; the renderer talks to the daemon.** Pros: one canonical HTTP
  path (single place for token/retry/TLS/host logic), slims the thin client
  (advances [[project_thin_client_is_connection_only]] and the slim-thin-client
  work, task #574), no shared mutable Rust state. Cons: large sweep across many
  call sites (easy to miss one → silent local fallback), higher one-shot
  regression risk, slower.

**Decision:** A is the bridge (live now). B is the complete fix and the fallback
if A's global-state ordering / token plumbing proves fragile. A does **not**
block B — B can land incrementally on top of A and then remove A's chokepoint.

## 3. Goal

Every daemon-data read/write in the renderer goes through `daemonCliGet` /
`daemonCliPost` (host-aware). The Tauri command surface retains **only**
genuine local / OS-integration commands. After migration, both the local app
and a K2 Connect remote call `/cli/*` identically, with host + token derived
from `useConnectHostStore`.

## 4. Scope

### 4.1 Migratable (daemon-routed) — move to `daemonCliGet/Post`

~90 commands across these modules map 1:1 to `/cli/*` routes (verified by
grepping `DaemonClient` + `cli_get*/cli_post*` in `src-tauri/src/commands/`):

| Module | Routes | Notes |
|---|---|---|
| `projects.rs` | `/cli/projects/*`, `/cli/workspaces/set-nav-visible` | ~21 cmds; **partial** — keep local file/window ops (below) |
| `git.rs` | `/cli/git/*` | ~21 cmds; candidate for full deletion |
| `workspace_sections.rs` | `/cli/sections/*` | 6 cmds; full deletion candidate |
| `workspaces.rs` | `/cli/workspaces/*` | 3 cmds; full deletion candidate |
| `focus_groups.rs` | `/cli/focus-groups/*` | 6 cmds; full deletion candidate |
| `agents.rs` | `/cli/presets/*` | 6 cmds; full deletion candidate |
| `states.rs` | `/cli/states/*` | 5 cmds; full deletion candidate |
| `workspace_layouts.rs` | `/cli/workspace-layouts/*` | 4 cmds; full deletion candidate |
| `timer.rs` | `/cli/timer/*` | 4 cmds; full deletion candidate (good pilot) |
| `settings.rs` | `/cli/settings/{get,update,reset}` | **partial** — `daemon-settings.ts` already wraps these |
| `k2so_agents.rs` | `/cli/workspace/{agent-display-name,set-agent-display-name,resume-chat-args}` | **partial**; rest is local/in-process |
| `daemon.rs` | `/cli/settings/{get,update}` (keep-daemon-on-quit shims) | **partial** |

~95% of call sites are **pass-through** (daemon JSON → type assertion →
return); no response massaging to relocate. A handful fire Tauri
`app.emit('sync:*')` events post-mutation — see Risk 5.

### 4.2 Do-NOT-migrate (stay `invoke()` — local / OS integration)

Keep these; they are why the thin client still exists:
- **File/window/dialog:** `projects_pick_folder`, `projects_upload_icon`,
  `projects_open_focus_window`.
- **Local FS watch:** `fs_watch_dir`, `fs_unwatch_dir` (FileTree live updates).
- **Keychain:** `k2_secret_{set,get,delete}`.
- **Connect address book:** `connect_hosts_{read,write}` (client-side config,
  needed *before* a daemon is chosen).
- **Daemon lifecycle / discovery:** `daemon_ws_url` (bootstraps the local creds),
  `daemon_install/uninstall/restart`, `daemon_log_path/tail`, `daemon_status`.
- **App/process/OS:** `set_document_edited`, `set_relaunch_mode`,
  `relaunch_via_open`, `cli_install*`, `permissions_*`, `check_for_update`,
  `get_current_version`, `renderer_heartbeat`, `memory_status`.
- **Verify case-by-case:** `format_file*`, `workspace_ops.rs` (pane/tab ops —
  mostly daemon-routed but confirm no local side-effect), `worktree.rs`,
  `skills.rs`, `inbox.rs`.

⚠️ **Hybrids** (local side-effect *and* daemon call) need explicit handling:
`projects_open_in_finder` / `_in_editor` / `_in_terminal` actually run `open`
**daemon-side**, so they can migrate as-is (the *remote* would open on the
remote host — confirm that's the desired semantic; if "open on my machine" is
wanted, they must stay local). `workspace_arrange` reads renderer state then
persists via daemon — fine if stateless.

### 4.3 WebSockets — already host-aware (verify the assumed ones)

Confirmed host-aware via `getDaemonWs()` + `daemonWsBase(creds)`:
`session-events.ts` (`/cli/sessions/events`), terminal grid
(`/cli/sessions/grid`). **Verification item:** confirm awareness/CRDT,
channels/messaging, and chat-streaming WS all resolve through
`getDaemonWs()` and not a local-only URL. No WS migration expected beyond
confirmation.

### 4.4 Token model — no change needed

`daemonCliGet/Post` already attach `?token=<creds.token>` where `creds` come
from `getDaemonWs()` (local: `daemon_ws_url`; remote: `activeHost.token`).
`handleRemoteUnauthorized` (401 → `expireSession` → re-sign-in) and the
connection-retry wrapper already cover remote token expiry + daemon restarts.
Migrated routes inherit all of this for free.

### 4.5 Dead-code removal (the payoff)

After migration, fully deletable modules (all-daemon, no local work):
`git.rs`, `workspace_sections.rs`, `workspaces.rs`, `focus_groups.rs`,
`agents.rs`, `states.rs`, `workspace_layouts.rs`, `timer.rs`. Partially
trimmed: `projects.rs` (keep ~50 lines of file/window ops), `settings.rs`
(keep `cli_install*`, relaunch, document-edited), `k2so_agents.rs`,
`daemon.rs`. Remove ~60 entries from the `generate_handler!` macro in
`src-tauri/src/lib.rs`. Estimated ~900 lines of Rust deleted. **Once B fully
lands, remove Plan A's `set_active_daemon` chokepoint + the global `ACTIVE`
override** (no longer needed — nothing routes through `DaemonClient` for data).

## 5. Risks & mitigations

1. **Response-shape drift** (~~med~~ → **VERIFIED LOW**): a parity audit of 25
   representative routes (projects/git/settings/states/sections/workspaces/
   timer/presets/focus-groups/layouts) found **100% match** — daemon structs all
   use `#[serde(rename_all="camelCase")]` (incl. `Workspace.type_` →
   `#[serde(rename="type")]`), responses are bare `serde_json::to_string` with no
   envelope, and the Tauri shims are pure pass-through EXCEPT one transform:
   `workspace_layouts.rs:70` maps `layouts.into_iter().map(Into::into)` into a
   Tauri wrapper struct — that conversion must move into the renderer call site.
   Residual risk: confirm the ~unaudited routes follow the same camelCase rule.
2. **Boot-time race** (handled): stores must `await getDaemonWs()` before
   firing; `ConnectionGate` already blocks the app until `/boot-status`. No new
   risk vs. the existing WS pattern.
3. **Error-path parity** (low): `parseDaemonResponse` normalizes to
   `Error{message}`, so catch blocks are unchanged. Grep for any `instanceof`
   checks on invoke results and replace with message checks.
4. **No in-process fallback** (low/accepted): a few commands (e.g.
   `k2so_workspace_agent_display_name`) had a Rust in-process fallback; the
   renderer has none. Acceptable under daemon-first (ConnectionGate guarantees a
   daemon). Document as a hard dependency.
5. **Event-emit timing** (~~med~~ → **VERIFIED LOW**): audit of all 43 `sync:*`
   emits found **zero** stores that depend on the broadcast alone — every store
   either applies the HTTP response directly or its `useWindowSync.ts` listener
   re-fetches (`fetchProjects`/`fetchPresets`/`fetchEntries`/…). The emits are
   fail-safe cross-window sync + audit, not the primary update path. Migration
   action is just an optional `emit` delete per command; **no store refactor**.
   Listeners stay as the cross-window safety net.

## 6. Phased plan

**Effort (refined after recon):** ~88 migratable commands — **82 trivial
pass-through swaps**, **1 real transform** (`workspace_layouts`), **11 optional
emit-deletes**, **5 host-only stay**. With both swing factors (shape parity,
event dependency) verified clean, this is now a **LOW-risk, mostly mechanical**
migration: ~5–7 days single-dev human, compressing to **~2–3 focused days
agent-driven** with parallel per-module subagents + a two-device verification
pass. Incremental & independently shippable per module.


1. **Foundation:** harden `daemon-cli.ts` tests (GET+params, POST+body, 401,
   retry, token attach); type-audit daemon routes ↔ renderer types; confirm the
   `daemon-settings.ts` pattern is the template.
2. **Pilot:** migrate `timer.*` (4 simple CRUD cmds) end-to-end incl. test
   mock swap (`invoke` → `daemonCli*`); then `focus_groups.*`. Validate no
   regressions locally + against the live remote.
3. **Bulk:** `projects.*` (split local ops out), `git.*` (delete), then
   `workspace_sections`, `workspaces`, `agents`, `states`, `workspace_layouts`.
4. **Settings & edge:** route `settings_*` via `daemon-settings.ts`; line-audit
   `k2so_agents.rs`, `workspace_ops.rs`, `daemon.rs` shims.
5. **Cleanup:** delete emptied modules + `generate_handler!` entries; remove
   Plan A's `set_active_daemon`/`ACTIVE` chokepoint; full `cargo test` +
   `vitest`; update/remove affected tests (Rust inline blocks in deleted files;
   vitest component mocks of `invoke` → `daemonCli*`).

## 7. Success criteria

- Connecting to a remote host shows the **remote's** projects, git, agents,
  states, layouts, sections, settings, timer — verified on a two-device test.
- `grep -rl DaemonClient src-tauri/src/commands` returns only modules with a
  documented local reason (or is empty).
- No daemon-data `invoke()` remains in `src/renderer` (only the
  documented local/OS set).
- Full suite green (cargo + vitest + tsc); two-device e2e passes.

## 7b. Remote-completeness gaps — the SEPARATE next-fix list (post-A/B)

A recon pass confirmed the encouraging part: **all WebSocket paths
(`/cli/sessions/events`, grid, chat) and the sampled daemon-HTTP surfaces
(reviews, chat history, heartbeats) are already host-aware**, and there are
**no direct `invoke('daemon_ws_url')` leaks**. Heartbeats now follow the remote
*because of Plan A's `set_active_daemon` proxy*. So the remaining remote
breakage is NOT the data plane — it's **local-machine integration points**.
These are independent of Plan B (they'd remain after the migration) and form
their own fix list, mostly "detect `activeHost !== 'local'` → disable / relabel
/ route-to-daemon":

**Tier 1 (breaks core remote UX):**
- **Path semantics** — `projects_pick_folder` returns a CLIENT path, then
  `projects_add_from_path` / `init_git_and_open` / `add_without_git` send it to
  the REMOTE daemon, which can't see the client FS. `Sidebar.tsx:919-935`,
  `GitInitDialog.tsx:85,98`. Fix: on remote, switch to remote-path text entry /
  a remote directory picker.
- **`fs_watch_dir`/`fs_unwatch_dir`** (`FileTree.tsx:699,741`) are local-only
  Tauri watchers → remote file changes (agent writes, git checkout) don't push;
  FileTree goes stale. Fix: daemon-driven `/cli/fs/watch` WS or polling refresh.
- **Wrong-machine actions** — `projects_open_in_finder` / `_in_editor` /
  `_in_terminal` (`Sidebar.tsx:541,1000,1005`, `IconRail.tsx:226,231`,
  `TabBar.tsx:312`) act on the daemon HOST, useless to the client. Fix:
  disable/hide on remote with a tooltip.

**Tier 2:** editor list (`projects_get_editors`/`_all_editors`/`refresh`) shows
the REMOTE host's installed editors, not the client's — relabel "Editors on
&lt;host&gt;:" or fetch client-local. `Sidebar.tsx:500,946`,
`EditorsAgentsSection.tsx:99,111`.

**Verified OK:** drag-drop (paths originate remote, stay remote), icon upload
(base64 transferred via `daemonCliPost`), all WS, all sampled data surfaces.

This list should become its own task/PRD section — it's "make remote *complete*",
distinct from Plan B's "make the data plane host-aware".

## 8. Open verification items (do before/within Phase 1)

- Confirm `format.rs`, `workspace_ops.rs`, `inbox.rs`, `worktree.rs`,
  `skills.rs` local-vs-daemon classification by reading each.
- Confirm awareness/channels/chat WS are host-aware.
- Confirm the desired "open in Finder/editor" semantic for a remote host
  (local machine vs. remote machine).
- Exact renderer call-site list per command (the Explore pass sampled; Phase 1
  should enumerate fully, e.g. `rg "invoke\('(projects_|git_|...)"`).
