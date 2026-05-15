# 0.37.12 — Pinned chat & heartbeat sessions survive close/crash

## TL;DR

- **Pinned chat tab resumes the same Claude session** across K2SO quit + relaunch + kernel panic. The session id is now persisted in the workspace layout, not just trusted from the daemon's DB.
- **Heartbeat tabs survive close/reopen** with the same daemon-side PTY. Heartbeat tab metadata (`heartbeatName`, `attachAgentName`, `surfacedAgentName`) now flows through `serializeTab` → `restoreLayout` so the restored tab reattaches to the canonical heartbeat session via agent_name idempotency instead of spawning a duplicate `claude --resume <X>`.
- **`k2so msg <workspace> "text" --wake`** correctly targets the user's chosen Claude session via `deliver_live` → Branch 1 (`active_terminal_id`) fast-path. No more cascading into duplicate-resume races.
- **New chat-history dropdown** on the pinned Chat tab header (left of the refresh icon). Lists every Claude session for the workspace and lets the user **switch the pinned chat to a different past session** — escape hatch for cases where the canonical pointer drifted (orphaned PTY, deleted JSONL, manual recovery).
- **Daemon stays the source of truth.** New write goes through `/cli/workspace/set-chat-session`, not direct Tauri-side DB access. Tauri remains a thin client per the daemon-first architecture.

## The problem we were fixing

The pinned Chat tab and heartbeat tabs each had their own "lane" in SQLite (`workspace_sessions.session_id`, `agent_heartbeats.last_session_id`) — but the lanes didn't coordinate with each other or with the workspace_layouts blob. Symptoms users hit:

- **Pinned chat forgets its Claude session.** Send a message, close K2SO, reopen → fresh tab, no history.
- **`k2so msg <workspace>` targets the wrong session.** The CLI's `deliver_live` cascade saw a stale `active_terminal_id` after restart and spawned a duplicate `claude --resume`, forking the conversation.
- **Heartbeats reconnect to the wrong session on restart.** The renderer rebuilt heartbeat tabs from `agent_heartbeats` rows alone — no memory of which heartbeat had a surfaced tab before close, no way to reattach to the live PTY by canonical agent_name.

Architecturally these were variations on the same problem: state modelled per-lane in SQL, but no coordination between renderer state, daemon's in-memory `v2_session_map`, and the auto-stamp hook that's supposed to converge them. See `.k2so/prds/canonical-lane-restore.md` for the full design.

## What changed

### Pinned chat tab: session id lives in the renderer's serialized layout

`SerializedItem` for agent items now carries a `sessionId` field (parallel to the one terminal items already had). `serializeTab` captures it on save; `restoreLayout` plumbs it back into `AgentItemData.sessionId`. PaneGroupView forwards it to AgentPane → AgentChatPane as a prop.

On AgentChatPane mount, if `restoredSessionId` is set, a new **fast-path** builds the launch config directly:

```typescript
{ command: 'claude', args: ['--dangerously-skip-permissions', '--resume', restoredSessionId], cwd: projectPath }
```

— bypassing the `k2so_agents_resume_chat_args` daemon roundtrip that could race with auto-stamp writes or land on the wrong workspace_sessions row. The renderer holds the canonical record; the daemon's v2_spawn auto-stamp hook reconciles `workspace_sessions.active_terminal_id` and `.session_id` on the next register.

### Stamp-back: renderer mirror always converges

New tabs store action `stampAgentSessionId(agentName, projectPath, sessionId)` writes the resolved session id onto `AgentItemData` after AgentChatPane resolves its launch config (either from the fast-path or from `resume_chat_args`). Next `serializeTab` captures it, so the layout JSON always matches what's actually running. Tightened to `section === 'chat'` so Inbox pinned tabs don't get a meaningless sessionId stamped.

A one-time **scrub** in `restoreLayout` drops `sessionId` from agent items whose `section !== 'chat'` so any pre-fix layouts (which leaked the chat's sessionId onto Inbox tabs via the broader match) self-heal on the next save.

### Heartbeat tab metadata in the serialized layout

`SerializedItem` for terminal items now carries `heartbeatName`, `projectPath`, `surfacedAgentName`, and `attachAgentName`. Without these, a restored heartbeat tab would spawn with the renderer's default `tab-<terminalId>` agent_name — missing the canonical heartbeat key `tab-<terminalId>` the daemon's idempotency check needs to find the existing PTY. Result: duplicate `claude --resume <X>` and the closeTerminalForRenderer cross-reference logic couldn't recognize the tab as heartbeat-bonded.

With these fields persisted: restore reattaches to the exact same daemon-side PTY via canonical agent_name idempotency. No respawn, no race, no duplicate sessions.

### Chat-history dropdown — escape hatch

New UI element in `AgentChatPane`'s header (left of the refresh icon):

- Title display showing the currently-active chat's title (from `chat_history_list_for_project`) — confirms what the live PTY is actually running
- Click → popover listing every Claude session for the workspace, sorted by recency, with message counts and a checkmark on the current selection
- Selecting a different session: updates `workspace_sessions.session_id` via the **new daemon route** `/cli/workspace/set-chat-session`, stamps the new id on the AgentItemData, then triggers the existing refresh flow to swap the live PTY

The dropdown is the user-facing escape hatch when the canonical pointer has drifted — orphaned PTYs from older bug states, deleted JSONLs, manual recovery from a known-broken workspace. Switching takes effect instantly: DB row updates, renderer mirror updates, PTY swaps in one user action.

### Daemon-first: new write goes through the daemon

Per the daemon-first architecture (Tauri is one of N viewers, daemon owns the writes), the new `workspace_session_set_session_id` Tauri command is a thin facade over the daemon HTTP route `/cli/workspace/set-chat-session`. No direct DB access from Tauri-side. Same pattern as the existing `k2so_session_lookup_by_agent` / `k2so_heartbeat_active_session` commands.

## Verified end-to-end

| Scenario | Result |
|---|---|
| Pinned chat: send message → quit → relaunch → message + history intact | ✅ |
| Daemon-side PTY survives Tauri close (same daemon SessionId before and after relaunch) | ✅ |
| Three-way sync: workspace_sessions.session_id ↔ workspace_layouts.sessionId ↔ live PTY args | ✅ |
| Heartbeat tab: open → fire completes → quit → relaunch → same conversation visible | ✅ |
| `k2so msg <workspace> "text" --wake` → injects into pinned chat via Branch 1 fast-path | ✅ |
| Cmd+T tab session resumes across close/reopen (regression check) | ✅ |
| Dropdown escape hatch: switch to past session → DB + layout + live PTY all converge on new session_id | ✅ |
| Inbox tab `sessionId` scrub on restore (no longer carries leaked chat id) | ✅ |

## Files touched

| Layer | File | Change |
|---|---|---|
| renderer | `src/renderer/stores/tabs.ts` | `SerializedItem.sessionId` for agent items; `heartbeatName/projectPath/surfacedAgentName/attachAgentName` for terminal items; `stampAgentSessionId` action; scrub on restore |
| renderer | `src/renderer/components/AgentPane/AgentChatPane.tsx` | `restoredSessionId` prop fast-path; chat-history dropdown UI + switch logic |
| renderer | `src/renderer/components/AgentPane/AgentPane.tsx` | Forwards `restoredSessionId` to AgentChatPane |
| renderer | `src/renderer/components/PaneLayout/PaneGroupView.tsx` | Reads `ad.sessionId` and passes through to AgentPane |
| daemon | `crates/k2so-daemon/src/cli.rs` | New route `/cli/workspace/set-chat-session` |
| Tauri | `src-tauri/src/commands/k2so_agents.rs` | `workspace_session_set_session_id` thin facade over daemon route |
| Tauri | `src-tauri/src/lib.rs` | Registers the new command |
| docs | `.k2so/prds/canonical-lane-restore.md` | Design doc |

Plus the existing `find_live_for_resume` in `heartbeat_launch.rs` and the Branch 1b argv-scan in `workspace_msg::deliver_live` (already shipped pre-0.37.12) — verified working as the safety net behind the renderer-side fixes.

## What's still alpha

Focus Windows still don't fully match the rest of the architecture. The active-viewer protocol (0.37.11) handles resize coordination, but cross-window workspace-switch propagation and a couple of smaller rough edges aren't fully polished. Their architecture is sound; the polish is queued for a later release.
