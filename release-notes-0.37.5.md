## What changed

**Drops the vestigial `:<agent_name>` suffix from the canonical workspace+agent session key everywhere.** Closes the SMS Bridge regression (C3PO `5c80bef1`) where opening the pinned chat tab spawned a duplicate `__lead__` PTY and orphaned the canonical scout session.

## The bug this closes

On a 0.37.4 workspace running `mode custom` with AGENT.md `name: scout`:

1. `ensure_canonical_session` registers the scout PTY in `v2_session_map` under key `<pid>:scout`.
2. Webhook → `msg --wake` injects into scout's PTY. SMS Bridge replies. Works.
3. Operator clicks the pinned chat tab in K2SO's sidebar.
4. **Bug:** the renderer's mode→name mapping hardcoded `__lead__` for `mode === 'manager' | 'coordinator' | 'pod' | 'custom'`. The spawn body landed at `<pid>:__lead__` — a different key — so `lookup_by_agent_name` missed the existing slot, spawned a SECOND PTY, and rewrote `workspace_sessions.terminal_id` to point at the wrong agent.
5. Result: scout PTY orphaned. Subsequent `msg --wake` lands at `__lead__`, which has no scout persona. SMS replies break permanently from a single click.

## The fix

Post-0.37.0 unification, every workspace has at most one agent. The agent's name is metadata about the session — display label, persona file owner, launch profile owner — **not part of its address**. The vestigial suffix only created opportunities for the renderer to compute the wrong key.

In 0.37.5:

| Layer | Pre-0.37.5 | Post-0.37.5 |
|---|---|---|
| `v2_session_map` key | `<project_id>:<agent_name>` | `<project_id>` |
| `workspace_sessions.terminal_id` | `agent-chat:<pid>:<name>` | `agent-chat:<pid>` |
| `pending_live` queue dir | `<sanitized_pid>_<name>/` | `<sanitized_pid>/` |
| Renderer pinned-tab `attachAgentName` | `${projectId}:${agentName}` | `projectId` |
| egress workspace-aware inject key | `<workspace_id>:<agent>` | `<workspace_id>` |

The renderer no longer composes an agent-name suffix anywhere it talks to the daemon — it just hands over `projectId` and the daemon resolves to the canonical session via SQL → `v2_session_map`.

## Architectural note

`canonical_session::canonical_key_for(project_id) -> String` is the single chokepoint for canonical-key construction. Every workspace+agent session goes through it. Forms like `<pid>:something` for canonical sessions can no longer enter the system from any direction.

Heartbeat-fire sessions and worktree-scoped chats keep their existing keying — they're separate session classes that legitimately need a discriminator.

## Migration (boot-time, idempotent)

When the 0.37.5 daemon boots:

1. **SQL migration `0042_canonical_key_drop_agent_suffix.sql`** — rewrites every `agent-chat:<pid>:<name>` row in `workspace_sessions` to `agent-chat:<pid>`. Heartbeat-shaped (`...:hb:<schedule>`) and worktree-shaped (`agent-chat:wt:<wid>`) rows untouched.
2. **`pending_live` dir migration** — merges legacy `<sanitized_pid>_<agent>/` queue dirs into bare `<sanitized_pid>/`. Lossless: per-file `mv` (atomic on same filesystem). Runs BEFORE `replay_all` so the in-memory counter is built from post-migration shape.
3. **`v2_session_map` in-memory migration** — defensive sweep that re-keys any lingering `<pid>:<agent>` entries to bare `<pid>`. No-op on cold boot (map is empty); meaningful only when the daemon binary upgrades without a restart.

**Daemon restart advisory:** the auto-update path triggers a daemon restart (Tauri relaunches, daemon respawns from the new binary), so the in-memory migration is largely defensive. Operators upgrading via DMG should restart the daemon manually if they suspect mid-conversation orphan PTYs after upgrade.

## Cross-version safety

0.37.5 daemon + 0.37.4 renderer: the daemon's spawn helper accepts the legacy `<pid>:<agent>` shape from a 0.37.4 renderer and parses out the project_id. The pre-Phase-B fallback at `spawn.rs:228` keeps things working during the transition window.

## Tests

754 tests passing across the workspace. Test cleanup discipline (the user's headline ask) included a **revert-and-fail audit**: after Phase F's test updates landed, I reverted `egress::try_inject` back to the pre-0.37.5 prefix shape and confirmed `live_live_injects_and_audits_no_inbox_no_wake` failed loudly with `left: "test-ws", right: "test-ws:bar"`. Same for the canonical_session tests under the helper revert. Tests pin the new invariant; no test silently passes via fallback.

Specific test files re-targeted:
- `session_stream_egress.rs` — bare-pid inject assertion + comment block
- `session_stream_ingress.rs` — same
- `canonical_session_integration.rs` — bare-pid lookup + regression guards
- `heartbeat_fire_v2_integration.rs` — bare-pid spawn key
- `triage_integration.rs` — bare-pid scheduler-fire spawn
- `agents_routes_integration.rs` — bare-pid + regression guard for legacy shape
- `pending_live_durability.rs` — bare-pid queue dir + collapsed-by-workspace replay assertion
- `scheduler_wake_integration.rs` — bare-pid auto-launch + queue dir
- `spawn_to_signal_e2e.rs` — bare-pid Kessel spawn key
- `providers_inject_integration.rs` — bare-pid canonical key

## Acceptance test (manual)

1. Enroll a workspace: `mkdir foo-checkin && cd foo-checkin && k2so workspace open . && k2so mode custom`.
2. Write `.k2so/agent/AGENT.md` with `name: scout` and a `launch:` block.
3. Confirm `workspace_sessions.terminal_id` is `agent-chat:<pid>` (no `:scout`).
4. `k2so msg foo-checkin "hello" --wake` → injects in canonical session.
5. Open Tauri, click the pinned chat tab. Network: spawn body is `agent_name: <pid>` (raw UUID) — no `:__lead__` or `:scout` suffix.
6. `lookup_by_agent_name(<pid>)` returns `reused: true`.
7. `k2so agents running` shows ONE claude process, not two.
8. Subsequent `msg --wake` lands in the SAME PTY.
9. SMS replies continue through subsequent webhook fires.
