# K2SO 0.36.12 — Hotfix: Chat re-surface dedup

Clicking a chat in the ChatHistory drawer used to open a duplicate tab every time, even if that exact session was already running in another tab. So you'd resume a chat, switch to another tab to check something, click the chat row again to come back — and end up with two tabs both running `claude --resume <same-session-id>` instead of being routed back to the one you already had open.

This release adds a dedup check to the click handler: if a tab is already running this session (same command + sessionId in args), it focuses that tab instead of spawning a duplicate.

## What changed

`handleSessionClick` in `ChatHistory.tsx` now scans every tab across all split groups before calling `addTabToGroup`. If it finds one whose terminal item's args contain the clicked session's ID, it focuses that tab (via `setActiveTab` for the main group or `setActiveTabInGroup` for splits) and returns early.

Cross-worktree resumes are exempt — when the workspace branch differs from the session's origin branch, K2SO appends `--fork-session` to create a *new* conversation branch off the original. That's a fresh conversation by design, so it gets its own tab even if the origin session is open elsewhere.

## Why this is its own system, not shared with heartbeat surfacing

0.36.11 added a similar "find existing tab" branch for heartbeat row clicks (in `tabs.ts::openHeartbeatTab`). The two flows look alike but solve different problems:

- **Heartbeat surfacing** has to reach into the daemon to ask "is there a live PTY for this heartbeat?" — daemon-spawned heartbeat PTYs aren't visible to the renderer until surfaced. Then it adopts the existing PTY through `k2so_session_set_surfaced` rather than re-resuming.
- **Chat re-surface** is purely a renderer concern. Chats live as files on disk (`~/.claude/projects/.../<uuid>.jsonl` etc.), and the renderer-spawned `claude --resume` tab is the only "live" surface. No daemon round-trip needed; just match against open tabs.

So the two systems share the matching shape (`command + sessionId in args`) but live in separate code paths. Keeping them separate means future changes to one — e.g., when the daemon-id-space unification lands and chats stop relying on args-scanning — don't ripple through the other.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
