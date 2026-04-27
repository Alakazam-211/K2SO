# K2SO 0.36.8 — Hotfix: Chat tab stops auto-triaging on app relaunch

The persistent-agent Chat tab no longer fires a fresh wake (with the agent's WAKEUP.md as the first user message and an optional `/compact` directive) every time the K2SO app relaunches. If you saw your workspace's pinned agent suddenly start triaging the inbox after upgrading K2SO to 0.36.5/0.36.6/0.36.7, this fixes it.

## What was happening

The pinned Chat tab in each workspace (the one that hosts the persistent agent's conversation) was using `k2so_agents_build_launch` to spawn its `claude` PTY. `build_launch` is the function the heartbeat scheduler and "Launch agent" button use to *wake an agent up with full context* — it injects the agent's `WAKEUP.md` content as the positional first user message and prefixes `/compact` every 20 wakes to keep history bounded.

That's the correct behavior for a deliberate wake (heartbeat fire, manual Launch click). It is NOT the right behavior for the Chat tab on app relaunch:

1. K2SO auto-update relaunches the daemon
2. Daemon restart kills the agent's PTY
3. Chat tab re-mounts, sees no live PTY, falls through to `build_launch`
4. `build_launch` constructs `claude --resume <id> --append-system-prompt <agent-skill> <WAKEUP.md body>` and spawns it
5. Agent reads the WAKEUP, sees inbox items, starts triaging — without the user ever clicking anything

Three K2SO releases (0.36.5, 0.36.6, 0.36.7) each triggered this on auto-update. 0.36.4's `triage_decide` gate didn't help because this path didn't go through `triage_decide` — it went straight through `build_launch`.

## What's fixed

A new lighter Tauri command, `k2so_agents_resume_chat_args`, returns a *bare resume* command for the Chat tab:

- If we have a saved session id for this agent and the session file exists on disk: `claude --dangerously-skip-permissions --resume <id>`
- Otherwise: `claude --dangerously-skip-permissions` (fresh)

No system-prompt injection. No positional WAKEUP body. No `/compact`. The Chat tab is for *chatting with* the agent — the agent should only triage when explicitly asked (heartbeat fire, manual button).

`AgentChatPane.tsx` now calls this command instead of `build_launch`. `build_launch` itself is unchanged and is still the right primitive for actual wake events.

## What's not affected

- **Scheduled heartbeats** — if you have an `agent_heartbeats` row enabled, it still fires the agent on schedule via `build_launch`. Same as before.
- **Manual "Launch" button** — still fires a wake with full WAKEUP context. Same as before.
- **`k2so heartbeat fire` CLI** — unchanged.
- **Existing chat history** — the saved session id is preserved; on first relaunch with 0.36.8 your Chat tab reattaches to the same conversation it had before, just without the surprise wake.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
