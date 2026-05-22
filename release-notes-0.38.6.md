# 0.38.6 — `k2so msg`: deliver-or-loudly-fail

The chronic "first call returns success but doesn't land; re-fire works"
UX is gone. `k2so msg` now has one delivery code path, one canonical
JSON response shape, an internal retry window, and a `[from <name>]`
prefix on every recipient's PTY so agents always know who sent what.

## The bug we're closing

Every C3PO ticket about `msg` told the same story:

- First `--wake` returned `{injected_to_pty: true, published_to_bus: true}`
  — recipient never saw it.
- Second identical call returned `{branch: "active_terminal_id",
  delivery: "live", success: true, targetSessionId: "..."}` — landed.

Two response shapes from two delivery code paths, with state mutating
between calls so the daemon picked a different branch the second time.
`injected_to_pty: true` didn't mean what the field name suggested.
Agents learned to "always send twice" — a superstition that masked a
real protocol bug.

## What changed

### Canonical response shape — same every call

```json
// Success
{ "success": true, "target_session_id": "<uuid>", "attempts": 1 }

// Failure
{ "success": false, "target_session_id": null, "attempts": 1,
  "reason": "<code>", "hint": "<actionable next step>" }
```

No more `injected_to_pty`, `published_to_bus`, `branch`, `delivery`,
`targetSessionId`, `activity_feed_row_id`, `inbox_path`,
`woke_offline_target`. Internal cascade branch is tracked
daemon-side for debug logs but **never** appears in the public JSON.

### Four failure reasons, each with a hint

| `reason` | When | `hint` |
|---|---|---|
| `workspace_not_found` | `<workspace>` arg didn't resolve | "Run `k2so connections list` to see available workspaces." |
| `no_agent_mode` | Workspace exists but has no AGENT.md | "Workspace has no agent. Use `k2so work send` to queue, or `k2so mode custom` to set up an agent." |
| `spawn_failed` | Cascade tried fresh-fire, claude binary failed | "Spawn failed. Verify `claude` is on PATH for the daemon." |
| `pty_died` | PTY existed but child exited during write | "Target session crashed during delivery. Check `~/.k2so/daemon.stderr.log`." |

Exit code is `0` on success, `2` on any failure. `success: true` means
the bytes have **landed** in a PTY whose child is alive — not "we wrote
to something, hope for the best."

### Internal retry window — the cure for re-fire superstition

The empirical observation across every report: re-fire almost always
succeeds. Root cause is timing — `active_terminal_id` populates as a
side-effect of the first call's spawn, so the second call picks the
fast path. 0.38.6 absorbs that race inside the daemon:

- Max 3 attempts (1 initial + 2 retries)
- Backoffs: 200ms, then 400ms (≤ ~600ms worst case)
- Permanent reasons (`workspace_not_found`, `no_agent_mode`)
  short-circuit immediately — waiting won't help them.
- Transient reasons (`spawn_failed`, `pty_died`) get retried.

The `attempts` field surfaces the retry count for telemetry, but the
caller still makes one CLI invocation. The re-fire superstition is no
longer load-bearing.

### `[from <name>]` prefix on every delivered message

Recipients always see who sent the message:

```
[from scout_v3] hello world
```

`--from` resolution priority:

1. Explicit `--from <name>` (e.g. `--from sms-bridge`).
2. Auto-derive from sender's workspace name (CWD or `K2SO_PROJECT_PATH`).
3. Fallback `external` — for webhook scripts and shells outside any
   workspace. Recipient still gets a usable sender identity.

The daemon defensively substitutes `external` if it ever sees an empty
`from` value, so the prefix contract holds even if a future CLI edit
breaks the auto-derive.

### `--wake` is now the default; flag retired

`k2so msg <workspace> "text"` always tries live delivery — there's no
silent inbox fallback for the `msg` verb. The `--wake` flag is accepted
as a silent no-op for one release (warns to stderr); it'll be removed
in 0.39.x. Use `k2so work send` for queued delivery (recipient reads
on their own schedule).

### Help text rewrite

- `k2so msg --help` and `k2so help msg` now work (closes the
  "errors with 'message text required'" bug).
- `k2so help` (general) leads with the new msg form + cross-references
  `work send` and `terminal write`.
- `k2so help --advanced` documents `terminal write <id>` as the
  power-user escape hatch for raw PTY injection.

### Generated CLAUDE.md / AGENT.md / SKILL.md templates rewritten

Every onboarding agent now reads the correct contract:

- Drops the deprecated `:inbox` suffix.
- Drops the deprecated `--wake` flag.
- Drops `msg --agent <name>` references.
- Adds the `work send` vs `msg` distinction in skill protocol docs.

## Smoke tested end-to-end against dev daemon

- `workspace_not_found` reproducible → canonical shape ✓
- `no_agent_mode` (mode=off workspace) reproducible → canonical shape ✓
- CLI exit code 2 on failure ✓
- CLI `--help`, `--wake` deprecation, missing-args usage all behave ✓
- No legacy fields (`injected_to_pty`, `published_to_bus`, `branch`,
  `delivery`, `targetSessionId`) leak into any response ✓

16 new daemon-side unit tests + 127 existing k2so-core agent-template
tests all green.

## What's NOT in 0.38.6 (deferred to 0.38.7)

- **Single canonical-session writer.** `ensure_canonical_session`, the
  chat-picker dropdown, and the renderer's pinned-tab open path are
  still separate writers to `workspace_sessions`.
- **Pinned-tab bootstrap.** A freshly-created workspace's first `msg`
  still hits the fresh-spawn branch (now reliable thanks to retry +
  liveness checks, but no auto-bind of `active_terminal_id` to the
  spawned session until first turn completes).
- **First-turn `session_id` auto-stamp** for `claude --resume`
  continuity across daemon restarts.
- **"Pinned" naming collision** between `projects.pinned` (sidebar) and
  `workspace_sessions.active_terminal_id` (msg target) — kept; will
  resolve via documentation rather than rename.

## Files touched

| Layer | File | Change |
|---|---|---|
| Daemon | `crates/k2so-daemon/src/workspace_msg.rs` | Full rewrite: `MsgResponse` struct, `MsgReason` enum, retry-wrapped `deliver_live`, `attempt_delivery` cascade, `format_message` prefix helper, liveness check, 16 new unit tests |
| Daemon | `crates/k2so-daemon/src/cli.rs` | `/cli/workspace/msg` endpoint serializes `MsgResponse`; removed `delivery=inbox` branching |
| Daemon | `crates/k2so-daemon/src/heartbeat_launch.rs` | Audit consumer reads `MsgResponse` fields directly; updated audit string format |
| Core | `crates/k2so-core/src/agents/skill_content.rs` | 3 skill templates updated — drops `:inbox` / `--wake` / `msg --agent <n>`; explains msg-vs-work-send |
| Core | `crates/k2so-core/src/agents/events.rs` | Doc-comment updated to current canonical msg form |
| CLI | `cli/k2so` | `cmd_msg` rewrite: `--from` parsing + auto-derive, retired `--wake` (silent no-op), retired `--agent <n>`, `--help` handled first, exit code 2 on failure |
| CLI | `cli/k2so` | New `cmd_help_msg` verb-help; general + advanced help reflect new msg + cross-references |
| Notes | `release-notes-0.38.6.md` | (this file) |
