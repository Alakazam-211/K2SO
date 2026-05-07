## Highlights

**Heartbeats finally have their own lane.** Pre-0.37.8, every heartbeat fire was silently colliding with the workspace's pinned chat session — the wakeup body landed in the chat tab regardless of intent, and `workspace_heartbeats.active_terminal_id` ended up pointing at the chat tab's PTY. This release fixes the underlying lane collapse AND adds an opt-in for the deliberate "send into pinned chat" case.

## Two changes shipped together

### 1. Lane separation (the bug fix)

Pre-fix, the daemon's idempotency check in `spawn_agent_session_v2_blocking` keyed on bare `<project_id>`. Both the chat tab AND every heartbeat fire registered under the same canonical_key. The first to spawn won; every subsequent caller's args (`--session-id <pinned>`, etc.) were silently discarded by idempotency.

Symptom: every heartbeat WAKEUP.md ended up dropped into the chat tab session. The heartbeat's pre-allocated session UUID was never used by claude. `workspace_heartbeats.active_terminal_id` got stamped to the chat tab's PTY id (a "ghost pointer" — pointing at the wrong PTY). Subsequent fires hit `smart_launch`'s "active_terminal_id alive → inject" branch and wrote directly into the chat tab forever.

Post-fix:

| Caller | canonical_key |
|---|---|
| Pinned chat tab (`ensure_canonical_session`) | `<project_id>` |
| `k2so msg --wake` (`workspace_msg::deliver_live`) | `<project_id>` |
| Heartbeat 'morning' fire | `<project_id>:hb:morning` |
| Heartbeat 'evening' fire | `<project_id>:hb:evening` |

Each heartbeat now has its own slot in `v2_session_map`, its own PTY, its own claude JSONL. They don't collide with each other or with the chat tab.

Plus: heartbeat fires no longer touch `workspace_sessions` (the chat tab's row). Pre-fix, `wake_headless` and `heartbeat_launch::run_resume_and_fire` both called `k2so_agents_lock` unconditionally — clobbering the chat tab's `terminal_id` with the heartbeat's PTY. Now that call is gated to chat-tab wakes only.

### 2. Per-heartbeat opt-in: deliver into the pinned chat

For users who actually want a heartbeat's WAKEUP.md to land in the workspace's pinned chat session (e.g., morning brief that should be visible alongside ongoing chat), there's a per-heartbeat checkbox in Settings → Heartbeats: **"Send into pinned chat"**.

When on, the heartbeat fire delegates to `workspace_msg::deliver_live(project_path, prompt)` — the same four-branch smart cascade `k2so msg <ws> --wake` uses. The pinned chat tab and the heartbeat fire converge on the same `workspace_sessions.session_id`, so the operator opens the chat tab and sees heartbeat-driven activity interleaved with their own messages.

When off (default), the heartbeat keeps its own dedicated session, addressed by the per-heartbeat canonical_key from change 1.

## What changed

### DB

| Migration | Effect |
|---|---|
| `0043_heartbeat_use_workspace_session.sql` | New column `workspace_heartbeats.use_workspace_session INTEGER NOT NULL DEFAULT 0` |
| `0044_clear_ghost_heartbeat_active_terminals.sql` | Clears `workspace_heartbeats.active_terminal_id` for any row whose value matches the workspace's `workspace_sessions.active_terminal_id` (ghost pointers from the pre-0.37.8 lane collapse). Next fire decides cleanly via the smart-launch cascade. |

### Code

- **`SpawnWorkspaceSessionRequest`** — new optional `canonical_key: Option<String>` field. When `Some`, overrides the default `canonical_key_for(project_id)`. Default `None` = unchanged behavior (chat-tab lane).
- **`wake_headless::spawn_wake_headless`** — when called for a heartbeat fire (`heartbeat_name = Some(_)`), passes `canonical_key = format!("{pid}:hb:{hb_name}")`. Chat-tab wakes (`heartbeat_name = None`) keep the default. The `k2so_agents_lock` call (which writes `workspace_sessions`) is gated to chat-tab path only.
- **`heartbeat_launch::run_resume_and_fire`** — same per-heartbeat canonical_key override. The redundant `k2so_agents_lock` call removed (heartbeat lane doesn't touch `workspace_sessions`).
- **`heartbeat_launch::smart_launch`** — when `hb.use_workspace_session = true`, delegates to `workspace_msg::deliver_live` instead of running the per-heartbeat cascade. Returns the same JSON shape (`success`, `branch`, `targetSessionId`).
- **`AgentHeartbeat`** — new `use_workspace_session: bool` field + `set_use_workspace_session` setter.
- **CLI** — `k2so heartbeat use-pinned-session [on|off] <name>`.
- **Tauri** — `k2so_heartbeat_set_use_workspace_session(project_path, name, enabled)` command.
- **UI** — inline "Send into pinned chat" checkbox under each heartbeat row in Settings → Heartbeats.

### What stays untouched on flag-on fires (the opt-in case)

- `agent_heartbeats.last_session_id` — historical session UUID stays in the DB
- `agent_heartbeats.active_terminal_id` — historical PTY pointer stays
- Cooldown logic, lease/lock, audit row to `heartbeat_fires` — all unchanged

Un-checking the flag restores the original per-heartbeat targeting with the historical session intact.

## Tests

759 passing (756 from 0.37.7 + 3 new) — every test before this release still passes.

New regression guards:

- **`use_workspace_session_routes_through_deliver_live_and_leaves_heartbeat_fields_untouched`** — flag-on fire converges on canonical `<workspace_id>` key in `v2_session_map` (chat-tab lane), populates `workspace_sessions.active_terminal_id`, leaves `agent_heartbeats.last_session_id` + `active_terminal_id` untouched at their seeded historical values.
- **`flag_off_uses_legacy_heartbeat_cascade_unchanged`** — default-off path produces non-`workspace_session:*` branch tags, registers the heartbeat under `<workspace_id>:hb:<name>` (NOT bare `<workspace_id>`), and stamps the heartbeat's own `last_session_id` + `active_terminal_id`. Hard regression guard for the lane collapse.
- **`chat_tab_and_heartbeat_register_under_separate_canonical_keys`** — the canonical lane-separation test. Spawns a chat tab session under bare `<workspace_id>`, fires a heartbeat, asserts `v2_session_map` holds two distinct entries with different PTY ids, `workspace_sessions.terminal_id` stays bonded to the chat tab's PTY (not clobbered by the heartbeat fire), and `workspace_heartbeats.active_terminal_id` points at the heartbeat's own PTY.

Updated existing test:

- **`wake_headless_v2_writes_heartbeat_fields_and_does_not_touch_workspace_sessions`** (was `wake_headless_v2_writes_workspace_sessions_row_and_heartbeat_fields`) — now asserts the inverse: heartbeat fires populate `workspace_heartbeats` only and do NOT clobber `workspace_sessions`.

## Reversibility

- The per-heartbeat opt-in is per-row, fully reversible — flip the checkbox and the next fire reverts to the heartbeat's own session.
- The lane separation is a pure code-side change. The 0044 migration is idempotent; re-running it on already-clean data is a no-op.
- No deprecation. No schema churn beyond the two new columns/migrations. Existing users see the bug fix on first launch, plus an unticked checkbox they can opt into.
