# K2SO 0.36.4 — Hotfix: gate inbox-driven triage on heartbeat mode

Second hotfix in the same evening as 0.36.3, closing the last leak in the legacy "decide whether to wake an agent based on inbox contents" path that 0.36.3 partially fixed.

## What was still happening after 0.36.3

0.36.3 retired the renderer's auto-retriage loop, which was the most visible firing path. But the underlying decision function — `k2so_agents_triage_decide` — had no project-level gate of its own. So any caller (the `/cli/heartbeat` HTTP route, the renderer's launch-failure-retry, manual `k2so heartbeat` CLI invocations) would still return launchable agents whenever a workspace inbox had items, regardless of whether that workspace had its heartbeats disabled.

In practice that meant a workspace with `agent_heartbeats` rows all set to `enabled=0` and `projects.heartbeat_mode='off'` could still fire wakes if anything triggered a triage decision against it. We saw this happen on the K2SO workspace itself: agents got auto-spawned and made commits to feature branches (which were rolled back as part of this release).

## What's fixed

`k2so_agents_triage_decide` now reads `projects.heartbeat_mode` and returns an empty launchable list when it's `'off'`. Same gate the new `scheduler_tick` already uses; now both code paths agree.

The function is also flagged `#[deprecated]` with the `legacy-per-agent-heartbeat` tag, so the compiler emits a warning at every remaining call site. That's the kill list for the broader cleanup planned in 0.37.x — by then we expect to remove `triage_decide`, `read_heartbeat_config`, `AgentHeartbeatConfig`, and the on-disk `.k2so/agents/<name>/heartbeat.json` files entirely.

## Local LLM cleanup

While investigating, we noticed `src-tauri/src/commands/assistant.rs::safe_generate_for_triage` — a public wrapper from the original "local on-device LLM reads inboxes and decides whether to wake papa-Claude" automation — had zero callers anywhere in the codebase. The LLM-driven decision path was retired earlier this year in favor of the script-based path; the wrapper was just leftover scaffolding. Removed.

The local LLM remains in active use for the Workspace Assistant (Cmd+L) feature; only the orphan triage wrapper was removed.

## Rollback of unauthorized commits

Three commits made on top of 0.36.3 by an auto-fired agent (companion API background-terminal-spawn endpoint and cumulative display_offset in reflowed grid events) were reverted. The original work stays in `git log`/`git show` for future reference if we decide to ship those features properly later, but their effects are undone in main.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below. After upgrade, workspaces with all heartbeats disabled stay silent — no more auto-spawned chats.
