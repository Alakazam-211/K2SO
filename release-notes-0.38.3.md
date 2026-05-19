# 0.38.3 — System-wide Heartbeats settings panel

The Heartbeats settings page (Settings → Heartbeats) is now a
three-column command center for every heartbeat across every workspace,
not just the launchd plist mode toggle.

## What's new

### Middle column — All Heartbeats

Lists every active heartbeat in your install with:

- **Workspace / heartbeat name** + schedule summary ("Every day at 9 AM",
  "Every 60min 12 AM–11:59 PM", etc.)
- **Pinned chat** checkbox — when checked, this heartbeat's WAKEUP.md
  is delivered into the workspace's pinned chat session instead of its
  own. Mirrors the per-workspace toggle that's existed inline since
  0.37.8 — now reachable from one place.
- **Edit Wakeup** button — opens the same `WakeupEditor` (AIFileEditor
  takeover with live preview + Claude session) that the per-workspace
  flow uses. Edit any heartbeat's prompt template without navigating
  to its workspace first.
- **Enable/disable toggle** — flips `workspace_heartbeats.enabled` for
  this row.

### Right column — Recent Fires

Universal audit log of every scheduler decision across all workspaces:

- Live-updates every 5 seconds (no page reload needed to see new fires)
- Shows the most recent 100 decisions
- Color-coded one-character status: `●` fired (green), `!` error or
  missing-wakeup (red), `·` not-due (muted), `○` other skips
- Format: `[status] [time] [project / schedule] [decision]`
- Hover any row for the full skip reason

This is the diagnostic surface for "why isn't this heartbeat firing"
— you can spot a workspace's fires going `not_due → not_due → fired
→ not_due` or watch a stuck `error` row across multiple ticks
without grepping `daemon.stderr.log`.

## Why now

Before this release, you had to navigate to each workspace
individually to see/toggle its heartbeats. With 15+ workspaces and
hourly triage heartbeats firing across most of them, the per-workspace
view was the wrong UX for an operator who just wants to know
"is everything firing on schedule?" The 0.38.2 heartbeat-scheduler
refactor (replacing our hand-rolled deadline grace with `croner`)
made the fires-audit-log meaningful again — every row's decision now
carries real signal instead of the noise of `skipped_deadline` events
that meant nothing.

## Files touched

| Layer | File | Change |
|---|---|---|
| daemon | `crates/k2so-core/src/db/schema.rs` | `AgentHeartbeat::list_all_active_with_project` + `HeartbeatFire::list_all_recent_with_project` — cross-workspace queries with project name joined in |
| Tauri | `src-tauri/src/commands/k2so_agents.rs` | `k2so_heartbeat_list_all` + `k2so_heartbeat_fires_list_all` |
| Tauri | `src-tauri/src/lib.rs` | Register both commands |
| renderer | `src/renderer/components/Settings/sections/WakeSchedulerSection.tsx` | Two new columns (heartbeats list + fires audit log), `WakeupEditor` takeover wiring, 5-second polling for the fires feed |
