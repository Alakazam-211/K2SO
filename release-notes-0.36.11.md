# K2SO 0.36.11 — Heartbeat sessions: surface, don't resume

Clicking a heartbeat in the workspace tab drawer used to *resume* the chat into a brand-new PTY every time, even when the daemon already had a live session for that heartbeat running in the background. So you'd click Launch, watch the wakeup land in your tab, click the row to come back later, and end up looking at a fresh blank Claude Code session — not the conversation the heartbeat just had.

This release fixes the underlying daemon-vs-renderer PTY visibility gap that caused it. Heartbeat tabs now reliably surface the *existing* session instead of spawning a duplicate, and closing one minimizes the tab without killing the PTY (so you can re-summon it from the row click any time).

## What changed

### One PTY per heartbeat, surfaced on demand

Each heartbeat row now stores an `active_terminal_id` column pointing at its currently-live PTY's daemon session id. `smart_launch` stamps this synchronously alongside `last_session_id` whenever a heartbeat fires (fresh fire, inject into existing, or resume-and-fire). When you click the heartbeat row in the tab drawer:

1. Renderer asks the daemon "is there a live PTY for this heartbeat?"
2. If yes → daemon returns the session's id + agent_name; renderer attaches a new tab to that exact PTY (`/cli/sessions/v2/spawn` returns `reused: true`, no fresh resume).
3. If no → fall through to the legacy fresh-resume path (`claude --resume <session_id>` in a new tab).

No more two-PTYs-one-conversation. The tab attaches to whichever PTY the daemon is already running, regardless of how it got spawned (Launch button, scheduled fire, awareness-bus inject).

### Close button is now `–` (minimize), not `×` (kill)

The close button on heartbeat tabs renders as a minus glyph — and clicking it leaves the daemon-owned PTY running so the heartbeat keeps firing on schedule. The braille working-spinner still shows when the agent is doing something; hovering reveals the `–` glyph, same flow as regular tabs but with minimize-not-kill semantics. Tooltip: "Hide tab — heartbeat keeps running in the background."

Detection works two ways for backward compatibility:

- New surfaced tabs carry `heartbeatName` stamped on their data.
- Pre-existing tabs (from before this release) cross-reference: any tab whose `claude --resume <id>` args match any heartbeat row's `lastSessionId` is treated as a heartbeat tab.

So even tabs you opened before this release get the right close behavior the next time the heartbeat fires into them.

### Lazy cleanup + boot sweep keep the column honest

If the PTY exits (claude `--print` finishing, watchdog kill, daemon crash), the v2_session_map's child-exit observer nulls `active_terminal_id` automatically. Reading the column also lazy-cleans: if the recorded id no longer maps to a live session, the daemon nulls it inline so the next read reflects reality. And on daemon boot, any non-NULL `active_terminal_id` whose session isn't in the freshly-rehydrated map gets cleared — so a daemon restart doesn't leave stale pointers behind.

## Under the hood

- **DB migration 0036** — adds `agent_heartbeats.active_terminal_id` (NULLABLE TEXT) and `agent_sessions.surfaced` (INTEGER, defaults 0; existing user-owned sessions backfilled to 1).
- **New daemon HTTP route** `/cli/heartbeat/active-session` — walks both legacy + v2 session maps via `session_lookup::snapshot_all`, returns the live session's `agent_name` so the renderer can attach using the canonical key (heartbeat-spawned PTYs register under the workspace's primary agent name, not `tab-<rendererId>` — without the agent_name passthrough the renderer would never find them).
- **New Tauri commands** — `k2so_heartbeat_active_session`, `k2so_session_set_surfaced`. The latter emits `HookEvent::SessionSurfaced` so the renderer's listener creates a tab attached to the existing PTY, building the tab in a single `setState` so `attachAgentName` is on the data before `TerminalPane` mounts (otherwise `/cli/sessions/v2/spawn` fires before the override lands and the daemon spawns a duplicate).
- **`TerminalPane.attachAgentName` prop** — overrides the auto-derived `tab-${terminalId}` when surfacing a daemon-spawned session.
- **Heartbeat tabs force `alacritty-v2` renderer** — daemon-spawned PTYs only live in `v2_session_map`, so a tab attached to one must use the v2 path regardless of the workspace's renderer setting.
- **Child-exit observer in v2_spawn** — subscribes to `DaemonPtySession`'s alacritty event broadcast and unregisters on `ChildExit`, which triggers the active_terminal_id clear via `v2_session_map::unregister`'s hook.

## Filed for follow-up

- **Single-id-space cleanup** (`.k2so/prds/post-landing-cleanup.md`, new add-on section): renderer's tab id and daemon's session id should converge into one canonical UUID. Today they're bridged via `agent_name`; the bridge works but every consumer that wants to join the two has to remember it. Queued for the unification migration alongside the existing v1 retirement.
- **Single-agent workspace migration**: workspaces have one agent + many heartbeats, not many agents. Future migration will simplify addressing (`<workspace>:<agent>` becomes redundant, heartbeats become first-class addressable as `<workspace>:heartbeats:<name>`).
- **Scheduler test timezone flake** (`.k2so/work/inbox/bug-scheduler-test-timezone-flake.md`): pre-existing test fixture builds `DateTime<Local>` by converting from UTC, fails on machines west of UTC-2. Production behavior is correct (heartbeats follow OS local time, including across timezone changes); only the test is flaky.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
