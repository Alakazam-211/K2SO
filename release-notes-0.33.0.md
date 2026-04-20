# 0.33.0 — Persistent agents: K2SO Server that outlives the window

> **tl;dr** K2SO now ships a second bundled binary — `k2so-daemon` — that runs as a launchd agent. Close the K2SO window and the agents, terminals, heartbeats, and mobile-companion server keep running. Open the window later and you reconnect to everything in progress. A menubar icon keeps the server visible while the window is hidden, and Cmd+Q / the red close button now do two distinct things (both user-configurable).

This is the architecture release. 0.32.13 proved the core was fast enough. 0.33.0 rebuilds how K2SO is *shaped*: a persistent device-local service with a thin Tauri UI client, not a single process whose lifetime is tied to an open window.

## The product headline

**Your agents keep running when the laptop lid is closed.**

Before 0.33.0, closing the K2SO window stopped every agent, every heartbeat, every companion session. The window *was* the product. Now:

- Close the K2SO window (red button) → the **K2SO Server** keeps running in the background. Agents continue. Heartbeats fire. Mobile companion stays connected.
- Open the K2SO window later → reconnects to the server. Agent state is live, not stale, not reloaded from scratch.
- Lock the Mac and walk away → the daemon keeps working. Close the lid, agents pause (macOS sleep); open the lid, agents resume.
- Cmd+Q → quits *everything* (window + server). Deliberate full shutdown.

No other open-source AI workspace tool ships this today.

## What's new in the UI

### Menubar icon

K2SO now installs a template icon in your menubar. It's visible whether the K2SO window is open or hidden. Click it to see:

- **Server Status** — Running (up Xm) with a green square, Unreachable with orange, or Not running with red.
- **Ngrok URL** — your mobile-companion tunnel address, if active.
- **Connected parties (N)** — live list of mobile companions currently subscribed.
- **Quit K2SO** — deliberate full shutdown (same as Cmd+Q).

### Settings → General → K2SO Server

A new pane shows the daemon's status at a glance: Running/Unreachable/Not installed, PID + uptime, and a "Restart" affordance when you need to recycle the process. The **View log** link tails `~/.k2so/daemon.stdout.log` inline — no terminal required for triage.

### "Keep server running when the window is closed" toggle

Also in Settings → General. Controls what the red close button does:

- **ON** (default) — red button hides the window and keeps the K2SO Server running. Menubar icon stays visible. Reopen K2SO from the Dock to come back.
- **OFF** — red button behaves like a full quit, same as Cmd+Q. Useful if you'd rather not have a background process after closing the window.

**Cmd+Q always quits everything** regardless of the toggle. Industry-standard behavior for the power-user shortcut.

### Auto-start on every launch

On every K2SO launch, the Tauri app checks whether the daemon plist is loaded in launchctl. If not (because you red-buttoned with the toggle OFF, or rebooted), it re-loads it automatically. You should never have to manually restart the server.

## What's new under the hood

### Cargo workspace + daemon crate split

The source tree splits into three Rust crates:

```
crates/
  k2so-core/     — pure library. db, llm, terminal, companion,
                   agent_hooks, scheduler, push, perf. No Tauri dep.
  k2so-daemon/   — tokio async binary. Owns the persistent-agent
                   runtime: scheduler, companion tunnel, push targets,
                   HTTP/WS server, launchd plist lifecycle.
src-tauri/       — thin Tauri client. Window + menus + UI commands.
                   Proxies state-mutating work through the daemon.
```

Both binaries link `k2so-core`. The daemon launches under launchd (`~/Library/LaunchAgents/com.k2so.k2so-daemon.plist`) with `RunAtLoad: true` + `KeepAlive: true` — crash-safe, login-start, survives Tauri process recycling.

### Device-local, no cloud dependencies

K2SO 0.33.0 does not depend on any Alakazam Labs servers. Full stop. The daemon runs entirely on your machine. The mobile companion connects through your own ngrok tunnel.

Push notifications use a pluggable `PushTarget` trait. v1 ships with three implementations:

- **NoOp** (default) — no push. Mobile companion sees updates when it's foregrounded.
- **Webhook** — you provide a URL; the daemon POSTs agent events there.
- **NtfySh** — self-hosted push via ntfy.sh on your own infrastructure.

A future "K2SO Cloud Push" (paid subscription, Alakazam Labs APNs sender) is designed as a fourth `PushTarget::K2soCloud` impl that drops in without core or daemon changes. Not part of 0.33.0.

### HTTP IPC between Tauri and the daemon

The daemon serves a token-authed HTTP server on `127.0.0.1:<random>`. Tauri commands proxy state-mutating work to the daemon through this endpoint. Same auth pattern as the existing `agent_hooks` bridge in 0.32.x — extended, not reinvented.

**60 `/cli/*` routes ported to the daemon** (see the persistent-agents PRD at `.k2so/prds/persistent-agents.md` for the full list). Every CLI verb (`k2so work create`, `k2so delegate`, `k2so review approve`, `k2so agents running`, etc.) now talks to the daemon. The CLI surface and behavior are unchanged — only the process that handles the request has moved.

**`/hook/*` routes** (agent lifecycle webhooks) also moved to the daemon, so Claude Code sessions can fire hooks whether or not the Tauri app is open.

### Daemon → Tauri WebSocket event channel

The daemon broadcasts events (agent state changes, heartbeat fires, companion session join/leave) to any connected Tauri client via a WebSocket. The UI updates in real time. Multiple Tauri clients *can* connect simultaneously (groundwork for future multi-window scenarios).

### launchctl lifecycle model

- **First install** — Tauri runs the `install_daemon_plist_v2` code migration. Writes the plist under `~/Library/LaunchAgents/` and loads it. Idempotent.
- **Every launch** — Tauri runs `ensure_loaded()`: checks `launchctl list com.k2so.k2so-daemon`, loads the plist if not already loaded. Safe no-op when the daemon is alive.
- **Uninstall** — handled automatically when you remove K2SO.app. No separate uninstall flow needed in-product.

### macOS signing + notarization

Both `k2so` (Tauri app) and `k2so-daemon` are signed with hardened runtime and notarized via Apple. The `scripts/release.sh` pipeline bundles, signs, and notarizes both binaries.

## Tests & verification

All suites pass on `feat/persistent-agents` prior to release:

| Suite | Count | Result |
|---|---|---|
| `cargo test --workspace --lib` | 291 (62 src-tauri + 229 k2so-core) | 0 failed |
| `bunx tsc --noEmit` | — | clean |
| `tests/behavior-test-tier3.sh` | 385 passed, 0 failed, 4 skipped | skips are retired LLM-triage paths |
| `tests/cli-integration-test.sh` | 111 passed, 0 failed | — |

Per-release performance envelope from 0.32.13 still applies — no regressions measured.

## Breaking changes

**None for end users.** CLI behavior is preserved. Settings format is additive (new `keep_daemon_on_quit` key defaults to `true`). The `agent_hooks` HTTP token/port files (`~/.k2so/heartbeat.port`, `~/.k2so/heartbeat-token`) continue to work; the daemon now also publishes `~/.k2so/daemon.port` and `~/.k2so/daemon.token` for HTTP IPC.

## Known limitations (deferred to 0.33.1+)

- `heartbeat.port` can drift out of sync with the live daemon when the daemon restarts (port rotates). CLI tooling that reads `heartbeat.port` may hit stale ports until the next heartbeat-port sync. Documented in `.k2so/notes/daemon-ux-followups.md`.
- Locked-screen push notifications — platform constraint (iOS blocks non-APNs delivery to a locked device). True "phone rings in your pocket" delivery requires APNs, which is out of scope for the device-local v1. Power users can wire up `PushTarget::Webhook` or `NtfySh` for their own push flow today.
- Windows / Linux daemons — macOS-first release. `systemd --user` (Linux) and Windows service equivalents are follow-on work.

## Rollback

v0.32.13 (tag + DMG) remains the permanent rollback target on GitHub. If you hit a blocker in 0.33.0, downgrade to 0.32.13 — schema migrations in 0.33.0 are additive (new columns only), so the old binary still reads the database.

To fully revert: uninstall `~/Library/LaunchAgents/com.k2so.k2so-daemon.plist` via `launchctl unload` + `rm`, then reinstall 0.32.13.

## Next

0.34.0 is queued up: **Session Stream + Awareness Bus** — device-local primitives that let multiple clients subscribe to the same PTY session without reflow fighting, and let agents discover each other's work without polling. PRD draft at `.k2so/prds/session-stream-and-awareness-bus.md`.
