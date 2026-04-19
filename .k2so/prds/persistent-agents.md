---
title: Persistent agents via k2so-daemon
status: signed-off
created: 2026-04-19
signed_off: 2026-04-19
authors: [pod-leader, rosson]
---

# Persistent agents via k2so-daemon

## Problem

Today every piece of K2SO's runtime lives inside the Tauri app process:

- Cmd+Q kills the SQLite handle, the llama-cpp model, the ngrok tunnel, the `agent_hooks` HTTP server, and any in-flight agent sessions.
- `com.k2so.agent-heartbeat` launchd plist fires every 60s but HTTP-POSTs into the Tauri app — if the app isn't running, the tick no-ops.
- Closing the laptop lid suspends the whole system. When it wakes, the app's ngrok tunnel has to re-dial, the mobile companion is cut off, and the user's workspace feels like it hard-rebooted.

The practical consequence: **K2SO agents are "foreground-only."** Users who want overnight builds or lid-closed "keep working while I commute" behavior have to keep a laptop awake, plugged in, and the app in front. No open-source AI workspace tool ships a better answer today.

The 0.32.13 performance pass (seqno change-detection, SQLite pragma tuning, watcher batching, deferred SKILL regen) proved that **the UI event loop has massive headroom** — p99 of `terminal_poll_tick` is 13µs under 16ms gate, benches at `src-tauri/benches/perf.rs` show reflow cache-hit at 9.5ns and seqno compare at 372ps. Performance is not the bottleneck. **Architecture is.**

## Invariants we're preserving

- **No cloud infrastructure for the OSS core.** Everything that ships under the MIT license runs on the user's device. Alakazam Labs does not operate any servers as part of 0.33.0. (Locked-screen push is deferred to a future paid tier — see "Push notifications" below.)
- **One top-tier agent per workspace.** Multi-heartbeat PRD's invariant carries through. The daemon is a process, not a new product surface.
- **v0.32.13 is the permanent rollback target.** Tag + DMG stays live on GitHub. Any user can downgrade.
- **No feature flag.** When the branch ships, it ships as 0.33.0. No dual-mode codepath carrying the pre-daemon shape forward.
- **Tauri app stays visually identical.** UI commands move their backend, not their front-end surface. Zero UX regressions.
- **The CLI surface is unchanged.** `k2so` talks HTTP to whatever address is in `~/.k2so/heartbeat.port`. Users don't re-learn anything.

## Ratified design decisions

All seven resolved during the 0.33.0 kickoff on 2026-04-19. Implementors: do not re-open.

| # | Topic | Decision |
|---|---|---|
| 1 | Cloud dependency | None for v1. Alakazam-Labs-hosted push deferred to future paid tier ("K2SO Cloud Push"). |
| 2 | Daemon lifecycle | launchd `KeepAlive: true`. Always-on. ~30–50MB resident. |
| 3 | Wake cadence | User-configurable: `off` / `on_demand` / `heartbeat_every_N_minutes` (default 5). |
| 4 | IPC wire format | HTTP JSON, extending the existing `agent_hooks` pattern. No bincode, no Unix socket. |
| 5 | Companion tunnel | Paid ngrok reserved domain required for persistent mode. Free tier stays supported in non-persistent mode. |
| 6 | Mobile push | Three-tier pluggable `PushTarget`: `NoOp` (default), `Webhook`/`NtfySh` (v1 power-user), `K2soCloud` (v2 paid). |
| 7 | Branch / rollout | One long-lived `feat/persistent-agents` branch off `v0.32.13`. Commit-on-green. No incremental release. |

## Core design: k2so-daemon as an independent process

The Tauri app splits into three pieces:

```
┌─────────────────────┐  HTTP+token  ┌─────────────────────────────────────┐
│  K2SO.app (Tauri)   │  <──────────>│  k2so-daemon (launchd, KeepAlive)   │
│  - UI commands      │              │  - scheduler / heartbeat ticks       │
│  - proxy for        │              │  - agent_hooks HTTP                  │
│    state-mutating   │              │  - DB (SQLite, mutex shared)         │
│    commands         │              │  - llama-cpp (loaded once)           │
│  - renders React    │              │  - companion WS + ngrok tunnel       │
└─────────────────────┘              │  - wake-to-run scheduler             │
         ▲                           │  - PushTarget impl                   │
         │ HTTP+token                └─────────────────────────────────────┘
         │                                          ▲
┌─────────────────────┐  HTTP+token                 │
│  cli/k2so (bash)    │<────────────────────────────┘
│  unchanged          │
└─────────────────────┘
```

The **daemon owns state**. The Tauri app owns pixels. The CLI owns operator ergonomics.

### What moves to the daemon (state-mutating surface)

From the 230 `#[tauri::command]` audit in `src-tauri/src/commands/`:

| File | Count | Role | Where it lands |
|---|---|---|---|
| `k2so_agents.rs` | 56 | agent control, locking, scheduling, heartbeat admin | **daemon** |
| `projects.rs` | 24 | workspace/project CRUD | **daemon** |
| `git.rs` | 21 | version control | **daemon** |
| `terminal.rs` | 17 | terminal lifecycle + I/O | **daemon** |
| `filesystem.rs` | 16 | atomic writes, directory ops | **daemon** |
| `chat_history.rs` | 10 | persisted chat log | **daemon** |
| `settings.rs` | 9 | config read/write | **daemon** |
| `workspace_ops.rs` | 7 | workspace state | **daemon** |
| `agents.rs` | 6 | agent mode, detection | **daemon** |
| `workspace_sessions.rs` | 4 | session tracking | **daemon** |
| `states.rs` | 5 | workspace state | **daemon** |
| `assistant.rs` | 5 | LLM assistant calls | **daemon** |
| `companion.rs` | 5 | companion tunnel control | **daemon** |
| `claude_auth.rs` | 5 | Claude API auth | **daemon** |
| `updater.rs` | 3 | app updates | Tauri |
| `workspaces.rs` | 3 | workspace list | **daemon** |
| `project_config.rs` | 3 | project settings | **daemon** |
| `themes.rs` | 5 | UI theming | Tauri (UI-only) |
| `focus_groups.rs` | 6 | UI grouping | Tauri (UI-only) |
| `timer.rs` | 4 | UI stopwatch | Tauri (UI-only) |
| `review_checklist.rs` | 4 | UI checklist state | Tauri (UI-only) |
| `skill_layers.rs` | 4 | UI skill layers | Tauri (UI-only) |
| `format.rs` | 2 | UI text format | Tauri (UI-only) |

Total: ~199 to daemon, ~31 stay in Tauri app. The "UI-only" category is the one that touches only window-local state (theme preferences, panel arrangement, stopwatch UI). Everything else crosses the HTTP boundary.

### Proxy mechanics in the Tauri app

For every command that moves, the Tauri app keeps the `#[tauri::command]` wrapper, but the body becomes a thin HTTP client call:

```rust
#[tauri::command]
pub async fn k2so_agents_create(app: AppHandle, args: CreateArgs) -> Result<AgentId, String> {
    daemon_http_post("/cmd/k2so_agents_create", &args).await
}
```

The renderer-facing command signature stays identical. Frontend code doesn't change.

### Crate layout

```
crates/
  k2so-core/          # pure library. no tauri dep.
    src/
      db/             # moved from src-tauri/src/db
      llm/            # moved from src-tauri/src/llm
      terminal/       # moved from src-tauri/src/terminal
      companion/      # moved from src-tauri/src/companion
      agent_hooks.rs  # moved, now pub
      scheduler.rs    # extracted tick logic
      perf.rs         # moved from src-tauri/src/perf
      push/           # new: PushTarget trait + impls
      wake.rs         # new: launchd plist writer + wake semantics

  k2so-daemon/        # binary. links k2so-core + tokio.
    src/
      main.rs         # launchd entry; tokio runtime; HTTP server
      handlers/       # HTTP handler-per-command
      lifecycle.rs    # plist install/uninstall, port publishing

src-tauri/            # unchanged public surface; commands proxy to daemon
  src/
    commands/         # wrappers become HTTP clients
    daemon_client.rs  # new: HTTP client + token loading

cli/
  k2so                # unchanged
```

No `crates/k2so-relay`. Explicitly dropped per decision #1.

## Lifecycle

### Daemon launchd plist

Installed at `~/Library/LaunchAgents/com.k2so.k2so-daemon.plist` on first post-upgrade launch of the Tauri app. Shape:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>                        <string>com.k2so.k2so-daemon</string>
  <key>ProgramArguments</key>             <array><string>/path/to/k2so-daemon</string></array>
  <key>RunAtLoad</key>                    <true/>
  <key>KeepAlive</key>                    <true/>
  <key>StandardErrorPath</key>            <string>~/.k2so/daemon.stderr.log</string>
  <key>StandardOutPath</key>              <string>~/.k2so/daemon.stdout.log</string>
  <key>ProcessType</key>                  <string>Background</string>
</dict>
</plist>
```

- `RunAtLoad: true` — starts on user login.
- `KeepAlive: true` — launchd restarts the daemon if it crashes.
- `ProcessType: Background` — macOS scheduler treats it as low-priority, not interactive.

### Tauri app launch sequence

1. App starts. Reads `~/.k2so/heartbeat.port`.
2. Attempts HTTP GET `/ping` with the token from `~/.k2so/heartbeat-token`.
3. On success: proceed, render UI.
4. On connect failure (no port file / connection refused):
   - Shell `launchctl load -w ~/Library/LaunchAgents/com.k2so.k2so-daemon.plist`.
   - Poll `/ping` for up to 5s with 100ms backoff.
   - If still failed: show error dialog, offer "Install daemon" button that runs a fresh `launchctl load`.

### Tauri app close

Quit via Cmd+Q: Tauri process exits. Daemon keeps running (distinct PID, launchd-managed). Heartbeats continue firing. Mobile companion tunnel stays up if it was already enabled.

### Existing `com.k2so.agent-heartbeat.plist`

Unchanged. Its bash script HTTP-POSTs to whatever port `~/.k2so/heartbeat.port` contains. Under 0.32.13 that's the Tauri app's port; under 0.33.0 that's the daemon's port. The plist itself doesn't know the difference.

## Wake-to-run

### Settings UI

New section in **Settings > K2SO Daemon**:

```
┌──────────────────────────────────────────────────────────────────┐
│ Persistent Agents                                                │
├──────────────────────────────────────────────────────────────────┤
│                                                                  │
│  Run agents when the app is closed                               │
│  ◯ Off — agents stop when you quit the app                       │
│  ◯ While app is open only                                        │
│  ● Heartbeat every  [ 5  ] minutes  (default)                    │
│                                                                  │
│  ☑ Wake the laptop from sleep to fire heartbeats                 │
│     (uses launchd StartCalendarInterval with Wake:true)          │
│                                                                  │
│  Companion tunnel persistence:                                   │
│    ngrok reserved domain:  [ user.ngrok.app           ]          │
│    ngrok authtoken:         [ ••••••••••••           ] [ Test ]  │
│    ⚠ Free-tier ngrok URLs rotate and break persistence.          │
│                                                                  │
│  Push notifications (optional):                                  │
│    ◉ None (default)                                              │
│    ◯ Webhook — URL:  [                              ]            │
│    ◯ ntfy.sh — Topic: [                              ]            │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
```

### Wake cadence → launchd

User-supplied cadence translates to plist XML:

| Mode | Plist shape |
|---|---|
| `off` | Daemon plist unloaded; no heartbeat. |
| `on_demand` | Daemon plist loaded; heartbeat plist `StartInterval=60` but `WakeSystem` absent (doesn't wake sleeping laptop). |
| `heartbeat_every_N_minutes` | Daemon plist loaded; heartbeat plist rewritten as `StartCalendarInterval` with minute entries `0, N, 2N, …, 60-N` and `WakeSystem: true`. Writing multiple minute entries is the standard macOS idiom for "every N minutes." |

The wake plist writer lives in `k2so-core/src/wake.rs`. Exposed via daemon HTTP endpoint `POST /daemon/wake/reconfigure` with body `{mode, interval_minutes, wake_system}`. Settings UI calls it on Apply.

### Verifying wake actually happens

Manual: `pmset -g log | grep 'Wake from'` after a lid-closed test run.
Automated: the daemon's `wake_observer` compares local time at tick T+1 against T to detect gap > (interval × 2). If gap large, logs a "missed wake" warning.

## Companion tunnel persistence

ngrok tunnels reconnect with a new URL by default. Persistent mode requires a stable URL so the mobile companion doesn't lose its base address after every laptop sleep/wake cycle.

**Paid ngrok reserved domain is required** for persistent mode. The daemon reads `companion.ngrok_reserved_domain` from settings and binds the tunnel to that domain across reconnects.

Free-tier users keep everything that works today (tunnels work while the app is open; URL rotates on reconnect). Settings UI flags this with a warning when the user enables persistent mode without a reserved domain.

### Reconnect policy

Daemon's tunnel supervisor (in `k2so-core/src/companion/tunnel.rs`):

1. On network blip: reconnect immediately with exponential backoff (100ms → 30s cap).
2. On ngrok auth failure: log + disable persistence until user re-validates in Settings.
3. On successful reconnect to the same reserved domain: emit `tunnel:reconnected` on WS so mobile knows to refresh.

Mobile app's base URL does not change after initial pairing. That's the whole point of the reserved domain.

## Push notifications

### PushTarget trait

```rust
pub trait PushTarget: Send + Sync {
    fn send(&self, event: PushEvent) -> Result<(), PushError>;
}

pub enum PushEvent {
    AgentNeedsAttention { agent: String, summary: String, action_url: String },
    HeartbeatFailed     { heartbeat: String, reason: String },
    // more as daemon grows
}
```

### v1 implementations (0.33.0)

| Impl | Behavior | Config |
|---|---|---|
| `NoOp` | Discards events. | Default on fresh install. |
| `Webhook` | HTTP POST to user's URL with the event JSON. | URL in settings. |
| `NtfySh` | POST to `https://ntfy.sh/<topic>` OR user-specified self-hosted ntfy server. | Topic + optional server URL. |

Zero Alakazam Labs endpoints in any v1 impl.

### v2 implementation (future paid tier, NOT in 0.33.0)

`K2soCloud` implements the same trait. Daemon POSTs push intents to `https://push.k2so.io` (or wherever we end up hosting). The cloud service holds our APNs `.p8` and FCM service account and fans out to the user's paired device tokens.

**Why device-local can't do this**: iOS/Android locked-screen push routes through APNs/FCM by design. Apple does not permit alternative push pipelines. The best we can do device-locally is `PushTarget::Webhook` pointing at the user's own iPhone Shortcut, Apple Mail account, or ntfy.sh app. That works for power users; it's not a default experience.

The v2 impl drops in as a new `PushTarget::K2soCloud` variant without core or daemon changes. The trait surface is forward-compatible by design.

## Security model

### Threat model

The daemon exposes a TCP listener on `127.0.0.1:<random>`. Local unauthenticated access would allow arbitrary agent control, file writes, git ops. Unacceptable. Hence token auth.

### Carry-forward from `agent_hooks`

The existing pattern at `src-tauri/src/agent_hooks.rs:534` already:

- Binds `TcpListener::bind("127.0.0.1:0")` (random port, localhost only).
- Publishes the port at `~/.k2so/heartbeat.port` (0600 permissions).
- Publishes a 32-byte random token at `~/.k2so/heartbeat-token` (0600 permissions).
- Rejects any request whose `token=<value>` query param doesn't match.

The daemon reuses this verbatim. No new primitives.

### Hardening additions

- **Refuse non-loopback binds.** If somehow the listener gets a non-127.0.0.1 address (e.g. a bug introducing `0.0.0.0`), abort startup with a loud error.
- **Rotate token on daemon restart.** launchd restart = fresh token. Prevents token leakage from persisting across crashes.
- **Token file mode 0600 enforced.** Check mode on read; refuse to use a world-readable token file.

### Push target spoofing

`PushTarget::Webhook` POSTs to a user-configured URL. If the URL is attacker-controlled, the attacker learns agent events. This is accepted — the user chose the URL. The settings UI warns that webhook URLs see the same data the companion sees.

## Data model

Settings keys (in the user-wide settings store, same mechanism as today's `settings.rs`):

| Key | Type | Default |
|---|---|---|
| `daemon.wake_mode` | `off \| on_demand \| heartbeat` | `on_demand` on upgrade, `heartbeat` on fresh install |
| `daemon.wake_interval_minutes` | int (1–60) | 5 |
| `daemon.wake_system_on_heartbeat` | bool | false on upgrade, true on fresh install |
| `companion.ngrok_reserved_domain` | string | empty |
| `companion.ngrok_authtoken` | string (stored encrypted via keychain) | existing |
| `daemon.push_target_type` | `none \| webhook \| ntfy_sh \| k2so_cloud` | `none` |
| `daemon.push_target_config` | JSON | `{}` |

No new tables. Daemon state is already in the existing DB at `~/.k2so/k2so.db`.

## Migration

### Schema migrations

- **`0032_daemon_settings.sql`** — inserts the default rows for the keys above via `INSERT OR IGNORE`.

### Code migrations (via `code_migrations` marker, shipped in 0.32.13)

- **`install_daemon_plist_v1`** — writes `com.k2so.k2so-daemon.plist` and loads it via `launchctl load`. Records completion in `code_migrations`. Idempotent — skipped on subsequent launches.
- **`migrate_heartbeat_plist_target_v1`** — nothing to do if the port file is already being read at runtime. The existing plist references `~/.k2so/heartbeat.port` verbatim; when the daemon boots it overwrites that file with its own port. Transparent.

### Upgrade flow (user on 0.32.13 → 0.33.0)

1. User downloads 0.33.0 DMG, replaces `/Applications/K2SO.app`.
2. First launch: the Tauri app runs `install_daemon_plist_v1` code migration (requires no admin rights — user-level LaunchAgent).
3. Daemon boots via launchd; takes ownership of `~/.k2so/heartbeat.port`.
4. Tauri app connects to daemon, proceeds as normal.
5. Existing heartbeat plist continues to fire every 60s, now hitting the daemon's port.

### Downgrade flow (user reverts to 0.32.13)

1. User drags 0.33.0 K2SO.app to trash, installs 0.32.13 DMG.
2. Daemon is still running via launchd, but its binary path is gone.
3. First launch of 0.32.13 Tauri app: connects to `~/.k2so/heartbeat.port`. If daemon is somehow still alive, port works but version mismatch might cause issues — not our problem.
4. User runs `launchctl unload ~/Library/LaunchAgents/com.k2so.k2so-daemon.plist` per the downgrade doc, then deletes the plist. State returns to pre-0.33.0.

A `scripts/uninstall-daemon.sh` ships in the 0.33.0 DMG for one-command downgrade prep.

## Edge cases

| Case | Resolution |
|---|---|
| Daemon crashes | launchd restart (KeepAlive). Token rotates on restart, so active mobile WS sessions get 401 on next request and reconnect with fresh handshake. |
| Tauri app launched before daemon | Tauri polls `/ping`, fails, shells `launchctl load`, retries. |
| Two Tauri apps open same DB | Existing behavior (SQLite WAL + busy_timeout). Daemon is sole mutator, Tauri apps are readers + proxies. Concurrent reads are fine. |
| Port file stale (daemon died uncleanly) | Tauri gets connection refused, reruns `launchctl load` which starts a fresh daemon, fresh port, fresh token. |
| ngrok auth failure mid-persistence | Tunnel supervisor logs, disables persistent mode, writes `companion.tunnel_error` setting; Settings UI surfaces. |
| Free-tier ngrok user enables persistence | Settings UI warns "Reserved domain required"; feature enables but tunnel URL rotates on each reconnect; mobile app has to rescan QR code each cycle. Ugly but not broken. |
| User's Webhook target returns 5xx | Daemon logs, retries up to 3× with backoff, then drops the event. No retry queue — push is fire-and-forget. |
| User configures `ntfy_sh` with invalid topic | First event returns error; daemon disables push target; Settings UI surfaces via `daemon.push_target_error` setting. |
| Heartbeat fires while Tauri app is quit | Daemon executes normally. Activity feed persists. When user relaunches Tauri, the app fetches recent activity and the agents pane reflects the work done while the app was closed. |
| Heartbeat fires while system is sleeping | If `wake_system_on_heartbeat=true`: system wakes, daemon fires, system returns to sleep. If false: plist fires on next wake instead; `heartbeat_fires` records `decision='deferred_sleep'`. |
| Daemon and Tauri app disagree on DB schema version | Daemon migrations run on daemon startup. Tauri reads the same DB. If Tauri is older than daemon's schema, reads might fail — this is handled by Tauri refusing to start if DB schema is ahead of its known version (existing behavior from 0.32.13's code_migrations pattern). |
| CLI (`k2so …`) after reboot before Tauri app opens | Daemon is already running via launchd (`RunAtLoad: true`). CLI hits the port, works. |
| User disables daemon entirely (`wake_mode=off`) | `launchctl unload` runs on Apply. Daemon exits. Tauri app reverts to pre-daemon behavior: HTTP server starts inside Tauri setup, owns the port file. Full equivalence with 0.32.13 when daemon is off. |
| User enables daemon later | `launchctl load` runs on Apply. Tauri app releases its in-process HTTP server, hands off to daemon. |
| Multiple user accounts on same Mac | LaunchAgents are per-user. Each account gets its own daemon + port. No conflict. |
| Daemon consumes too much memory | Monitor via `log show --predicate 'processImagePath contains "k2so-daemon"'`. If steady-state > 150MB, file a bug — something is leaking. v1 target is 30–50MB resident. |

## Out of scope for v1 (0.33.0)

- **Alakazam-Labs-hosted K2SO Cloud Push** — first paid tier; not this release.
- **Windows service / Linux systemd user unit** — macOS-first. Follow-up work.
- **Proactive locked-screen push as default** — not technically deliverable device-local.
- **Daemon self-update** — daemon binary version is pinned to the Tauri app's bundled resource copy. Updates ship as a single DMG; user drags new app in; Tauri app reinstalls plist referencing the new binary path.
- **Remote daemon attach** — CLI binding to a daemon on another machine over SSH / mesh net. Possible but deferred.
- **Multi-daemon federation** — multiple K2SO installs on one user's devices sharing state. Deferred.
- **Web client** — mobile companion covers mobile; a browser-based client is its own product.
- **Persistence guarantees under kernel panic** — if the Mac panics, in-flight agent state depends on SQLite's WAL durability. We don't add a separate crash-consistency layer.

## Test plan

### Unit

- `k2so-core/src/push/` — each impl (`NoOp`, `Webhook`, `NtfySh`) tested against mock HTTP with 200 / 4xx / 5xx / timeout branches.
- `k2so-core/src/wake.rs` — plist writer produces valid XML for each `wake_mode`, XML round-trips through `plutil -lint` in a fixture test.
- `k2so-daemon/src/handlers/` — one handler per command, tested against a golden request/response fixture.

### Integration

- **Daemon boot + Tauri connect** — spawn daemon, spawn Tauri app, verify `/ping` handshake, verify token match, verify port file atomic write.
- **230-command audit** — every command listed in the "What moves to the daemon" table has a proxy test: Tauri-side call with known args → HTTP crosses the boundary → daemon handler runs → response matches pre-split baseline.
- **Heartbeat fire with Tauri app quit** — quit Tauri, trigger scheduler tick via `curl`, verify `heartbeat_fires` row written, verify agent session spawned.
- **ngrok reserved-domain reconnect** — simulate network blip (kill tunnel), verify reconnect attempt within 100ms, verify same URL on reconnect (against a test ngrok account).

### Manual (P4 acceptance checklist from the plan)

The full 9-step lid-closed walkthrough from `happy-hatching-locket.md`. Run 3× on M-series Mac. Pass = no missed heartbeat, no dropped mobile connection, Tauri reopens with current state.

### Tier 3 (structural)

- `launchctl list | grep k2so-daemon` post-install
- Port file at `~/.k2so/heartbeat.port` exists + 0600 permissions
- Token file at `~/.k2so/heartbeat-token` exists + 0600 permissions
- Daemon plist XML validates via `plutil -lint`
- `cargo build --workspace` clean in release mode
- `cargo test --workspace --all-features` all passing (existing 188 + ~30 new)

## Sign-off trail

- 2026-04-19 `pod-leader` initial draft folding in the seven user-ratified decisions from the 0.33.0 kickoff session + the architecture findings from the pre-plan Explore pass.
- 2026-04-19 `rosson` approves the plan (ExitPlanMode) with these decisions locked: no cloud in v1, KeepAlive daemon, user-configurable wake cadence, HTTP JSON IPC, paid-ngrok required for persistence, pluggable PushTarget (NoOp default, Webhook + NtfySh v1 power-user, K2soCloud v2 paid), single long-lived branch with v0.32.13 rollback.
- 2026-04-19 `rosson` adds: push notifications that can't be served from the laptop alone become the first feature of K2SO Cloud Push (paid). Alakazam Labs does not operate any infrastructure for 0.33.0.

## Open questions

None after 2026-04-19 alignment. Green-lit for implementation.

## Implementation status — 2026-04-19 (pre-compaction checkpoint)

Branch: `feat/persistent-agents`, 32 commits off `v0.32.13`.
Test suite: 62 src-tauri + 168 k2so-core = 230 lib tests green.

### Shipped in this branch

**Infrastructure.**
- Cargo workspace: root `Cargo.toml` + `crates/k2so-core` + `crates/k2so-daemon`.
- k2so-core crate hosts 14 modules (every module this PRD said would move).
- k2so-daemon binary boots, writes `~/.k2so/daemon.{port,token}` 0600,
  serves token-authed `/ping` + `/status` (JSON).
- Release pipeline bundles the daemon into `K2SO.app/Contents/MacOS/`
  with hardened-runtime codesign (`scripts/release.sh` Step 2.5).
- `install_daemon_plist_v1` code migration auto-installs the launchd
  plist on first 0.33.0 launch (release-build or `K2SO_INSTALL_DAEMON=1`).
  Uses `wake::install()` which is idempotent (best-effort unload before
  load — handles upgrade-over-install + rollback-then-reinstall).

**Decoupling work.**
- Four companion bridges in k2so-core let `companion::mod` live in core
  with zero Tauri dep: `settings_bridge`, `terminal_bridge`, `event_sink`,
  `app_event_source`. Tauri-side providers registered in `setup()` via
  `companion_host::register()`.
- `AgentHookEventSink` trait + `HookEvent` enum cover the 7 wire events
  `src-tauri/agent_hooks.rs` emits. All 17 emit sites migrated to the
  bridge. Tauri sink registered in setup.
- Pure helpers extracted from agent_hooks to k2so-core: `map_event_type`,
  `parse_query_params`, `urldecode`, ring buffer, `AgentLifecycleEvent`.
  Daemon can import and reuse.
- `TerminalManager`, `LlmManager`, DB all exposed as singletons
  (`*::shared()` accessors). AppState holds Arc clones; any core module
  can reach the same instances.

**Ratified decisions, implemented.**
- Decision #1 (no cloud): confirmed; push is opt-in via pluggable
  `PushTarget` (NoOp default, Webhook + NtfySh wired with reqwest).
- Decision #2 (`KeepAlive: true`): `DaemonPlist::canonical` emits this.
- Decision #4 (HTTP JSON IPC): daemon serves HTTP on 127.0.0.1; Tauri
  `DaemonClient` uses reqwest::blocking.
- Decision #6 (`PushTarget` trait with `K2soCloud` deferred to v2): shape
  ratified, `NoOp`/`Webhook`/`NtfySh` impls shipped.
- Decision #7 (long-lived branch, commit-on-green): 32 commits in; every
  one passes the lib test suite.

### Intentionally deferred — tracked in `.k2so/notes/corners-cut-0.33.0.md`

| Item | Status | Next-step pointer |
|---|---|---|
| Tokio-ize daemon runtime | open | Replace `std::net::TcpListener` in `crates/k2so-daemon/src/main.rs` with `tokio::net`; add shared `tokio::runtime::Runtime`. Prereq for concurrent connections. ~2h. |
| Daemon binds `heartbeat.port` | open | Swap the write paths in `k2so-daemon/src/main.rs` from `daemon.port`/`daemon.token` to the canonical `heartbeat.port`/`heartbeat-token`. Requires coordinating with src-tauri's `agent_hooks::start_server` (either gate the Tauri server off when daemon is running, or delete it). ~2h. |
| Daemon serves `/hook/*` + `/cli/*` routes | open | Port the route matching + body handling from `src-tauri/src/agent_hooks.rs:538-3200` into daemon. Business logic is now reusable via `k2so_core::agent_hooks::*`, `k2so_core::db::shared()`, `k2so_core::terminal::shared()`. ~1d focused. |
| Daemon → Tauri UI event channel | open | Daemon hosts a WS endpoint (reuse companion's tokio/tungstenite stack); Tauri connects at startup; incoming events feed `AppHandle::emit`. ~4h. |
| Wake-scheduler Settings UI | open | React Settings panel + `#[tauri::command] daemon_configure_wake(mode, minutes, wake_system)` → calls `wake::install()` with a customized `DaemonPlist`. ~4h (~2h React, ~2h Rust). |
| Full tokio + Tauri-command proxy | open | Each state-mutating Tauri command forwards via `DaemonClient::post_cmd`. 199 commands total; realistic to ship 0.33.0 with this NOT done — the Tauri app already shares state with the daemon via the core singletons. Deferrable to 0.33.x+. |
| 9-step lid-closed walkthrough | open | Manual run-through per the plan's acceptance criteria, on a real Mac, overnight on battery. Record 90-second demo. |
| DMG-level daemon bundling verification | open | Cut a signed release; verify `launchctl list | grep k2so-daemon` shows loaded, daemon survives Tauri quit. |

### Pattern reference (what's worked this branch)

See `happy-hatching-locket.md` → "Strategies that have worked" for the
9 proven patterns: ambient-singleton bridges, test-isolation locks,
test-util feature, refactor-then-relocate, callback instead of
AppHandle, git hygiene post-mess, `/target` gitignore gotcha,
commit-on-green cadence, corners-cut-as-forcing-function. Copy these
verbatim for the remaining work — they compressed weeks of theoretical
refactor into 32 green commits.
