# 0.38.0 — Daemon-authoritative tabs + multi-window sync

K2SO's biggest architectural shift since 0.37.0. Every Tauri window for
a given workspace now sees **the same tab set**, driven by the daemon's
`v2_session_map` rather than each window keeping its own private list.
Focus windows, "New Window", and (very soon) the mobile companion all
project the same canonical sessions instead of forking their own.

If you opened a focus window in 0.37.x and saw duplicate tabs, ghost
"forks of the same chat", or content drift between windows — those
classes of bug are gone. They were symptoms of two-source-of-truth.

## What you'll notice

- **Focus windows + "New Window" share PTYs cleanly.** Open the same
  workspace in two windows and you see the same N tabs, same content,
  typing in either window mirrors to the other.
- **Tabs propagate in real time.** Cmd+T in one window → tab appears
  in the other within ~100 ms. Cmd+W → tab disappears in both.
  Heartbeat-fire surfaces → both windows see it. Pinned-chat refresh
  → both windows remount.
- **Resize is clean.** When you switch focus between windows of
  different sizes, the terminal redraws onto a clean canvas instead of
  layering a new render on top of the old one. No more duplicated
  "Hello there!" prompts. Real conversation history (heartbeat
  summaries, your messages, acknowledgements) is preserved in
  scrollback — only the transient TUI chrome gets cleared.
- **TestingK2SO and every other workspace's saved layout migrated
  automatically** on daemon boot. Workspaces that had accumulated
  duplicate tab rows in `workspace_layouts.layout_json` (from the
  pre-0.38.0 sync race) self-heal at next launch.
- **Sidebar polish:** agent rows match pinned rows visually (no extra
  accent border); the Agents & Pinned section header has a touch more
  breathing room below the title when collapsed.

## What changed under the hood

Eleven commits stack into one architectural move. Each is independently
revertable; together they enforce one invariant: **N daemon sessions
⇒ exactly N tabs in every viewer.**

### 1+2 — Heal corrupt layouts + reconcile orphan PTYs

`workspace_layouts.layout_json` was accumulating duplicate tab rows
when non-main windows mount-time-broadcast their tab state. Added a
heal pass (renderer-side on every restore, daemon-side as a one-shot
boot migration over every workspace) that collapses tabs sharing
paneGroup-id sets. Generalised the non-main-window guard so focus
windows AND "New Window" both skip the destructive broadcast.

A separate reconcile pass queries the daemon's `v2_session_map` after
restore and adopts any `tab-<X>` PTY that isn't already surfaced in
a tab — fixing the "heartbeat fires while Tauri is closed, then I
can't find the session" class of orphan.

### 3 — Schema v2: terminal items are pointer-only

Terminal items in `layout_json` used to carry `cwd`/`command`/`args`/
`sessionId`/`renderer` alongside the canonical key. That was a
duplicate source of truth — drift was unavoidable. v2 keeps only the
canonical key (plus heartbeat metadata until the daemon's list
endpoint exposes it). The daemon owns the rest; reconcile refreshes
the in-memory copy from daemon data on every workspace open.

Migration runs on read (renderer side) AND as a one-shot boot pass
(daemon side, gated by `0.38.0-layout-v2-emit` marker). 76 of 77
workspace layouts converged automatically on the first build.

### 4 — Daemon session events + retire cross-window tab sync

New WS endpoint `/cli/sessions/events?path=<workspace>` streams
`session_added` / `session_removed` / `session_renamed` events.
Every viewer subscribes; tab adds and drops propagate in real time
via the daemon, not via Tauri's per-window event bus.

Retired: `sync:tabs-request`, `sync:tabs`, `broadcastAllTabs`,
`applyRemoteTabChange`. Cross-window sync for projects, settings,
presets, focus groups, and timer state stays on the Tauri event bus
(unchanged) — only the tab-sync paths moved.

**Mobile-companion parity: free.** Same WS endpoint, same wire
format, same hello-then-stream protocol.

### 5 — clearAllTabs stops killing daemon PTYs

`loadLayoutForWorkspace` used to call `clearAllTabs` which called
`closeTerminalForRenderer` per tab — and for v2 sessions that meant
unregistering the PTY from `v2_session_map`. So opening a focus
window for a workspace **killed the PTYs** the main window was
viewing; the main window kept WS handles pointed at dead session_ids
and looked frozen.

Under the new model the renderer's tab list is a view, not the
owner. `clearAllTabs` now just clears the view; daemon PTYs survive,
TerminalPane components close their WS gracefully on unmount, and
other viewers keep their subscriptions intact.

### 6 — Heartbeat minimize syncs across windows

Pre-fix: minimize a heartbeat tab in one window → the other window
still showed it. `k2so_session_set_surfaced(false)` flipped the DB
flag but emitted no event. Added a symmetric `SessionUnsurfaced`
broadcast that every window's listener picks up to drop the tab.

### 7 — Pinned-chat refresh syncs across windows

Click the refresh button on the pinned Chat tab in one window →
the other window kept its WS handle on the now-dead session_id.
Added `ChatRefreshed` Tauri broadcast; every window's `AgentChatPane`
listens and remounts together.

### 8 + 9 — Clear-on-resize without scrollback pollution

Claude's TUI does in-place SIGWINCH redraws — it re-emits the prompt
at a new row position without first clearing the canvas. Result:
old chrome stays in the visible grid while new chrome paints below
it. Daemon now clears the visible grid before reflowing the
alacritty Term.

Subtle gotcha caught during testing: alacritty's `ClearMode::All`
in non-alt-screen mode (Claude is not alt-screen) calls
`grid.clear_viewport()` which **scrolls the visible content into
scrollback** before clearing — so every resize was appending a stale
prompt frame to history. Fix: use `goto(0,0) + ClearMode::Below`,
which discards via `reset_region` without growing scrollback. Real
history (heartbeat summaries, conversation) is untouched.

### 10 + 11 — Sidebar polish

Agent rows lost their 2px accent left-border so the Agents and
Pinned groups have matching visual weight (the section divider
already conveys the grouping). The Agents & Pinned header gets a
bit more bottom padding (`pb-1` → `pb-2`) so the collapsed state
sits visually centered.

## Architecture invariant

After this release, no code path in the renderer can produce a tab
without a matching daemon session — structurally enforced by the
reconcile pass and the new event subscription, both keyed by the
daemon's canonical `agent_name`. If you ever see a tab that doesn't
correspond to a live `v2_session_map` entry, that's a bug.

`workspace_layouts.layout_json` is now metadata-only for daemon-
backed tabs: it stores positioning, ordering, custom titles, and
split structure. Tab **existence** lives in the daemon.

## Migrations applied on first boot

| Migration | What |
|---|---|
| `0.38.0-layout-dedup` | One-shot heal pass over every `workspace_layouts` row; collapses duplicate tab entries. |
| `0.38.0-layout-v2-emit` | Converts pre-v2 layouts to schema v2 (terminal items pointer-only). |

Both gated by `code_migrations` markers so they run exactly once.

## Mobile companion roadmap

Everything daemon-side that the mobile companion needs to mirror the
desktop UX is now in place: list sessions, subscribe to lifecycle
events, claim active viewer for grid sizing, drive resize. The next
release in this line focuses on the mobile-app side.

## Verification before ship

- `tsc --noEmit` clean
- `cargo test -p k2so-daemon` 35 lib tests + 17 integration binaries all green
- End-to-end manual test against TestingK2SO and C3PO covering: focus
  window open, tab spawn cross-window propagation, heartbeat
  minimize/open sync, pinned-chat refresh sync, `k2so msg --wake`
  Branch 1 + Branch resume_and_fire delivery, daemon-boot layout
  migrations across 76/77 workspaces.

## Files touched

| Layer | File | Role |
|---|---|---|
| renderer | `src/renderer/stores/tabs.ts` | Schema v2 types, dedup, reconcile, session-events subscribe |
| renderer | `src/renderer/stores/session-events.ts` | WS subscription helper |
| renderer | `src/renderer/stores/active-agents.ts` | `session:unsurfaced` listener |
| renderer | `src/renderer/hooks/useWindowSync.ts` | Removed tab-sync paths |
| renderer | `src/renderer/components/AgentPane/AgentChatPane.tsx` | `chat:refreshed` listener |
| renderer | `src/renderer/components/Sidebar/Sidebar.tsx` | Agent-row border drop, header padding |
| daemon | `crates/k2so-daemon/src/session_events.rs` | Event bus + types |
| daemon | `crates/k2so-daemon/src/session_events_ws.rs` | WS handler |
| daemon | `crates/k2so-daemon/src/workspace_layouts_dedup.rs` | Boot heal + v2 emit |
| daemon | `crates/k2so-daemon/src/v2_session_map.rs` | Emit `session_added` / `_removed` on register/unregister |
| core | `crates/k2so-core/src/agent_hooks.rs` | `SessionUnsurfaced` + `ChatRefreshed` events |
| core | `crates/k2so-core/src/terminal/daemon_pty.rs` | Clear-before-resize |
| tauri | `src-tauri/src/commands/k2so_agents.rs` | `surfaced=false` emit + `chat_refresh_broadcast` |
| docs | `.k2so/prds/daemon-authoritative-tabs.{md,html}` | PRD + HTML reference |
