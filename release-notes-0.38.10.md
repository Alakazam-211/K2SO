# 0.38.10 — Heartbeat add/remove/rename trusts `projects.agent_mode`

Hotfix: workspaces that had been mode-flipped to Custom / Workspace
Manager / K2SO Agent couldn't add heartbeats until `.k2so/agent/AGENT.md`
existed with a `name:` field. The DB knew the workspace was an agent;
the disk hadn't caught up yet.

## What changed

Three heartbeat code paths in `crates/k2so-core/src/agents/heartbeat.rs`
were using `find_primary_agent` (which probes AGENT.md's `name:`
frontmatter) as a proxy for "is this an agent workspace?" — even though
none of them actually USED the returned agent name (`let _agent_name =
...`).

- **Add** (the user-facing error): replaced with a `projects.agent_mode`
  column check. Mode in `{custom, manager, k2so-agent}` → allow.
- **Remove**: dropped the probe entirely; remove is a row + folder
  cleanup, agent identity isn't part of it.
- **Rename**: same — rename only touches the heartbeat row's name +
  its wakeup folder.

The fire path (`k2so_agents_heartbeat_tick`) still uses
`find_primary_agent` legitimately — that path actually needs the
agent name to fire the heartbeat against.

## Why the old check broke

After 0.37.0 made heartbeats workspace-level (no per-agent
heartbeat dirs), the validation comment was honest about its
half-finished state:

```
// 0.37.0: heartbeats are workspace-level (.k2so/heartbeats/<sched>/),
// independent of which agent owns them. find_primary_agent is
// still required for validation (a workspace must have an agent
// to schedule against) but the path no longer routes through it.
```

The intent — "validate that this is an agent workspace" — was right.
The implementation — "probe disk for AGENT.md and parse its frontmatter"
— was wrong, because `projects.agent_mode` is the canonical declaration.
Mode-flips can run ahead of file writes.

## Smoke

`cargo test -p k2so-core --lib agents::`: 127 passed / 0 failed.

## Files touched

| File | Change |
|---|---|
| `crates/k2so-core/src/agents/heartbeat.rs` | Replace `find_primary_agent` probe in add/remove/rename with `projects.agent_mode` check; comments explain rationale |
| `WHATS_NEW.md` | 0.38.10 entry |
| `release-notes-0.38.10.md` | (this file) |
