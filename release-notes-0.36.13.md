# K2SO 0.36.13 — Pinned chat tab: daemon-first, CLI-reachable, refresh-able

The workspace's pinned chat tab — the persistent Claude session you talk to when you click the agent's tab — was not playing well with the daemon. Three things were off, all visible to anyone using `k2so msg` from another terminal:

1. The chat tab registered its v2 PTY under a renderer-only key (`tab-agent-chat:<projId>:<agent>`), invisible to anything addressing the workspace agent by its actual name.
2. The awareness bus's liveness check (`is_agent_live`) only walked the legacy `session::registry`, so v2-only sessions looked offline → every Live signal hit the wake provider instead of injecting → duplicate sessions, missed messages.
3. Even when the inject path *did* find the session, it wrote the body and the `\r` (Enter) in one syscall — TUI input widgets like Claude Code's read that as a multi-line paste and the message landed typed-but-not-sent.

This release fixes all three plus adds a refresh button on the chat tab so you can recover when the underlying Claude process exits (`/exit`, crash, etc.) without quitting the whole app.

## What changed

### Chat tab attaches to the workspace agent's canonical session

`AgentChatPane` now passes `attachAgentName=<workspaceAgent>` to `TerminalPane`, so `/cli/sessions/v2/spawn` registers under the agent's real name (e.g. `manager`, `pod-leader`). On mount, the tab also queries a new daemon route to detect "is this agent already running headless?" — letting it attach to a live PTY across Tauri quit/reopen instead of orphaning it and spawning a duplicate. Same architectural shape as 0.36.11's heartbeat surfacing, applied to the persistent chat surface.

Concrete consequences:

- `k2so msg --wake <workspace-agent> "..."` reaches the chat tab's PTY. (Previously routed to a different session keyed under the renderer's tab UUID.)
- Quit Tauri → daemon keeps the PTY alive → reopen Tauri → tab re-attaches to the same conversation. (Previously: blank Claude session each time.)
- Heartbeat-spawned auto-launches and chat-tab spawns converge on one PTY per workspace agent (previously: two parallel sessions with no shared state).

### `is_agent_live` no longer blind to v2 sessions

`k2so_core::awareness::egress::is_agent_live` used to walk only the legacy `session::registry` — a holdover from the pre-A8 days when every session was Kessel-T0. Post-A8, every system-driven session is v2 and lives in `v2_session_map` (which the registry doesn't see), so every Live signal sent to a v2-only agent treated it as offline and routed straight to the wake provider, bypassing inject entirely.

The `InjectProvider` trait now has an optional `is_live(agent)` method (default returns false — back-compat). The daemon's `DaemonInjectProvider` overrides it with `session_lookup::lookup_any` so liveness checks see both maps. Egress consults the registry first (legacy path), then the provider (v2 path), then concludes offline.

This unblocks awareness-bus delivery to every v2 agent, not just the chat tab. Heartbeats benefit too.

### Inject does a two-phase write (body, settle, Enter)

Both inject paths — `DaemonInjectProvider::inject` and the `pending_live` drain in `v2_spawn::handle_v2_spawn` — now mirror what `heartbeat_launch::run_inject` was already doing for the Launch button:

```rust
session.write(body)?;
std::thread::sleep(std::time::Duration::from_millis(150));
session.write(b"\r")
```

A single combined `body+\r` write was being seen as a paste burst by the TUI input widget; raw-mode input widgets distinguish "fast-arriving bytes" (paste, internal newlines as literals) from "human-paced" (key by key, `\r` = submit). Splitting the write across two syscalls with a 150ms settle puts us on the human-paced side of that heuristic.

### Chat tab refresh button

A small refresh icon now lives on the right side of the chat tab header. Click it to:

1. POST `/cli/sessions/v2/close` to tear down the current PTY (clears the `agent_sessions.active_terminal_id` column via the existing unregister hook).
2. Reset the local mount state (`launchConfig=null`, `ready=false`).
3. Bump a key on `TerminalPane` so it fully unmounts/remounts and the next mount fresh-spawns via `/cli/sessions/v2/spawn`.

Useful when you've typed `exit`, the Claude process has crashed, or the session is just unresponsive — you don't have to quit-and-relaunch the whole app to get back in.

## Under the hood

- **DB migration 0037** — adds `agent_sessions.active_terminal_id` (NULLABLE TEXT). Stamped synchronously by `v2_spawn::handle_v2_spawn` when a workspace-agent-keyed session registers; cleared by the `v2_session_map::unregister` cleanup hook on PTY exit. Mirror of 0036's `agent_heartbeats.active_terminal_id`.
- **New daemon route** `/cli/sessions/lookup-by-agent?agent=<name>` — walks `session_lookup::lookup_any` (both maps) and returns `{agentName, sessionId, sessionAlive, isV2}`. Used by `AgentChatPane` on mount for diagnostic visibility; the actual attach happens via the `attachAgentName` prop passing through to `/cli/sessions/v2/spawn`'s find-or-spawn.
- **New Tauri command** — `k2so_session_lookup_by_agent(agent)` proxies to the daemon route.
- **`AgentSession` schema helpers** — `save_active_terminal_id`, `clear_active_terminal_id_by_terminal`, `clear_active_terminal_id`. Mirrors `AgentHeartbeat`'s helpers.
- **`InjectProvider::is_live`** — new optional trait method with default `false`. `DaemonInjectProvider` overrides it with `session_lookup::lookup_any(agent).is_some()`.
- **`signal_format::inject_bytes` / `egress::render_signal_for_inject`** — both now return body bytes without a trailing newline. The submit `\r` is the caller's responsibility (two-phase contract).

## Filed for follow-up

- **`Copy Session ID` on the pinned chat tab's right-click menu** — chat tab's daemon session UUID is the same kind of stable ID regular tabs already expose; should be addressable the same way.
- **`k2so workspaces` (yellow pages)** — single CLI command to list every known workspace + its primary agent + alive/asleep, so callers don't have to grep `.k2so/agents/<...>` to figure out who they're talking to.
- **`k2so msg <workspace> "..."` auto-resolve + auto-wake by default** — today's `--wake` is opt-in; should be the default since "send a message to the agent" usually means "land it in the live session, wake them if asleep." `--inbox` becomes the explicit opt-in for file-drop.
- **Surface `workspace`, `is_primary`, `isV2`, `spawned_by` fields on `/cli/agents/running`** — current response forces callers to filesystem-grep to attribute a session to a workspace.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
