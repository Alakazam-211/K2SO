# K2SO 0.36.14 — Hotfix: pinned chat tabs cross-wired across workspaces

If you had two K2SO workspaces both configured in the same agent mode (e.g., both Workspace Manager → both running an agent named `manager`, or both K2SO Manager → both `k2so-agent`), opening the second workspace would silently take over the pinned chat tab of the first. Both workspaces' chat tabs ended up bound to the same daemon-side PTY, sharing scrollback, sharing input, and impossible to separate without a daemon restart. This release fixes that.

## What was happening

The daemon's v2 session registry (`v2_session_map`) keys by agent name only — a flat `HashMap<String, Arc<DaemonPtySession>>`. When the chat tab in 0.36.13 started passing `attachAgentName=<workspaceAgent>` to `/cli/sessions/v2/spawn` (so external `k2so msg` callers could find the session by the agent's canonical name), it accidentally exposed a name-collision path:

1. **Workspace A's chat tab** mounts → spawn registers `manager → SessionA` in the map.
2. **User opens Workspace B**, also in Workspace Manager mode → its chat tab mounts → spawn registers `manager → SessionB`, **replacing** Workspace A's entry.
3. Both workspaces' chat tabs now resolve `manager` to the same session. Cross-wired.

The cleanup path made it worse: closing one workspace's chat ran `UPDATE agent_sessions SET surfaced=0, status='sleeping' WHERE agent_name='manager'` with no `project_id` filter, sleeping every workspace's row for that agent name, not just the one that closed.

## What's fixed

The daemon registry now supports project-namespaced keys: when the chat tab spawns, it passes `attachAgentName=<projectId>:<agentName>` instead of bare `<agentName>`. The register function inserts the prefixed key AND mirrors to the bare key (last-write-wins) so legacy bare-keyed callers like `k2so msg --wake manager` still find the most-recently-active workspace's session.

Cleanup is now project-scoped when the key is prefixed: closing Workspace A's chat tab only updates Workspace A's `agent_sessions` row. Workspace B's row stays exactly as it was.

## What's not affected

- **Heartbeat-surfaced sessions** — register under names like `tab-<rendererId>` (no colon), take the legacy bare-key path, behave identically.
- **Worktree chat tabs** — register under `wt-<worktreeId>` (no colon), unaffected.
- **`k2so msg --wake <agent-name>`** — still works against the bare-name mirror. Behavior unchanged from 0.36.13.
- **Existing single-workspace setups** — projectId prefix is invisible until a second workspace using the same agent name is added; transparent for users who never hit the collision.

## Under the hood

| File | Change |
|---|---|
| `crates/k2so-daemon/src/v2_session_map.rs` | `register()` mirrors prefixed keys to bare names; `unregister()` removes both slots when bare still points at the same session, scopes `agent_sessions` UPDATE by `project_id` when the key is prefixed. |
| `crates/k2so-daemon/src/v2_spawn.rs` | `save_active_terminal_id` strips the prefix to land the stamp on the right (project_id, bare_agent_name) row. `pending_live::drain_for_agent` drains under both prefixed and bare keys so awareness-bus signals queued under bare names still reach a chat-tab spawn under the prefixed key. |
| `src/renderer/components/AgentPane/AgentChatPane.tsx` | `attachAgentName` is now `${projectId}:${agentName}`. Refresh button's `/cli/sessions/v2/close` POST uses the prefixed form. Lookup-by-agent diagnostic uses the prefixed form. `<AgentChatTerminal>` gets `key={projectId}:${agentName}` to force a clean remount on workspace switch (defense-in-depth against a stale `useRef` retaining the previous workspace's terminal id). |

## Forward compatibility

The 0.37.0 workspace–agent unification (single-agent-per-workspace refactor, currently in design) replaces the prefixed-key workaround with workspace-keyed addressing throughout the daemon. The bare-name mirror retires when `lookup_any` and the awareness bus take a `project_id` parameter directly; until then, the back-compat mirror is the bridge.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
