# K2SO 0.37.2 — Canonical PTY ensurance on bot-mode transition

Closes a race window between "workspace transitions to bot mode"
and "first downstream caller addresses the agent" — visible in
high-throughput automation deployments (the `nsi-checkin` Scout
SMS bridge filed the original issue).

## What was happening

A fresh workspace's enrollment flow looks like:

```
k2so workspace open <path>
k2so mode custom
# write .k2so/agent/AGENT.md
<webhook fires k2so msg --wake within ~150ms>
```

The daemon knew about the primary agent the moment AGENT.md was
written, but no canonical PTY existed and no `workspace_sessions`
row was registered yet. The webhook's `--wake` would race ahead
into `fresh_fire`, spawn a session, and *implicitly* register the
SQL row — rather than the row being preceded by an explicit
canonical spawn.

Two symptoms:

1. The renderer's pinned-tab attach could subscribe to a different
   session than the one `--wake` injected into ("blank pinned tab"
   class).
2. The first inject's audit row bucketed to `_orphan` because the
   workspace+agent pair wasn't registered when the egress path ran.
   Self-corrected on next spawn but left an audit gap.

## The fix

New `ensure_canonical_session(project_path)` helper that's
idempotent + race-safe:

1. Resolve `project_id` + primary agent name (from AGENT.md).
2. Single-flight: if `<project_id>:<agent>` is already registered
   in v2_session_map, return `reused: true` without spawning.
3. Read AGENT.md's `launch:` profile (or the default).
4. Spawn under the canonical key — guaranteed to register before
   the function returns.
5. Persist the `workspace_sessions` row (terminal_id +
   status='running', owner='system').

Wired into three triggers:

- **`k2so mode <bot-mode>`** — when `agent_mode` flips to
  `custom`/`manager`/`k2so` AND AGENT.md exists, ensure runs
  immediately. The CLI response now includes the canonical IDs
  so operators/scripts can use them for follow-up calls.
- **New endpoint `/cli/workspace/ensure-canonical-session`** —
  explicit caller-driven ensurance. Replaces the SMS bridge's
  `k2so agents launch <name>` workaround. Returns
  `{session_id, agent, project_id, reused, pending_drained}`.
- **Daemon boot sweep** — walks every workspace whose
  `agent_mode` is bot-mode AND has AGENT.md, and ensures each.
  Recovers cleanly from a daemon restart that wiped
  v2_session_map.

## For SMS-bridge / automation deployments

You can now retire your post-AGENT.md `k2so agents launch <name>`
workaround. Either:

- Let `k2so mode custom` handle ensurance automatically (writes
  AGENT.md *before* setting mode if you want the proactive
  ensure to fire on the mode flip), or
- Call the explicit endpoint:
  ```
  curl -G "$DAEMON/cli/workspace/ensure-canonical-session" \
    -d "token=$TOKEN" --data-urlencode "project=$PROJECT_PATH"
  ```

Both end up at the same canonical state: live PTY under
`<project_id>:<agent>` in v2_session_map, `workspace_sessions`
row populated, ready for `k2so msg --wake` / inject / pinned-tab
attach to all converge on the same session.

## Tests

6 new integration tests in
`crates/k2so-daemon/tests/canonical_session_integration.rs`:

- `ensure_canonical_session_fresh_spawns_and_registers` — pin the
  fresh-spawn path: v2_session_map entry exists, workspace_sessions
  row populated with terminal_id + running status.
- `ensure_canonical_session_is_idempotent_when_session_alive` —
  two calls in a row, second returns `reused: true` with the
  same session_id.
- `ensure_canonical_session_errors_when_workspace_unregistered` —
  unregistered project_path → clean error message.
- `ensure_canonical_session_errors_when_no_agent_md` — missing
  AGENT.md → clean error.
- `boot_sweep_ensures_bot_mode_workspaces_with_agent_md` — sweep
  ensures bot-mode + AGENT.md, skips bot-mode without AGENT.md,
  skips off-mode workspaces even when AGENT.md exists.
- `ensure_then_wake_lands_in_same_session_no_duplicate_spawn` —
  the exact race from the issue: ensure → simulated wake-time
  ensure call → single v2_session_map entry, no duplicate.

Uses `cat` as the launch-profile command in tests so the suite
runs without a claude binary or API access.
