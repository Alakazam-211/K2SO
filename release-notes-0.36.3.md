# K2SO 0.36.3 — Hotfix: retire legacy auto-retriage loop

Hotfix for a legacy code path that was firing wakes against workspaces with no scheduled heartbeats. If you saw chats unexpectedly appear in workspaces you hadn't configured for autonomous agent activity, this release stops it.

## What was happening

A renderer-side loop, originally written for the pre-0.30s "agent always re-fires after stopping" model, was still wired up. Whenever any Claude session in a heartbeat-enabled project ended, the loop ran a separate triage path that read the legacy `.k2so/agents/<name>/heartbeat.json` config (the per-agent adaptive heartbeat from before workspace-scoped scheduled heartbeats existed) and immediately re-spawned the agent in a new tab. The new spawn ran its WAKEUP, ended, fired the loop again, and so on.

The loop was self-perpetuating — autoBackoff slowed it but never stopped it — and it bypassed the `projects.heartbeat_mode='off'` gate that the new system uses, because it called `triage_decide` (legacy gate, reads `heartbeat.json` directly) instead of `scheduler_tick` (DB-gated). That's why opting a workspace out of heartbeats via the UI didn't stop the wakes.

## What's fixed

The renderer's `'stop'`-event auto-retriage block is removed. Sessions now end and stay ended until their next scheduled fire from `agent_heartbeats` (the DB-backed system shipped in 0.36.0+). Workspaces with no scheduled heartbeats are silent.

The legacy `heartbeat.json` files on disk are preserved for now — a follow-up release will sweep them along with the rest of the per-agent heartbeat code (currently flagged as deprecated; the compiler emits warnings at every call site so the cleanup diff writes itself).

## Diagnostic instrumentation added

A few opt-in traces landed alongside the fix, off by default:

- `K2SO_TRACE_HEARTBEAT_JSON=1` — prints a backtrace whenever any code reads a `heartbeat.json` file. Useful if a similar leak surfaces in the future.
- `K2SO_TRACE_WAKE_SPAWN=1` — prints a backtrace at every wake-spawn entry point (daemon-side and Tauri-side). Confirms what's firing wakes.
- `K2SO_PERF=1` — opt-in for the `[perf] *_tick` histogram lines in the dev console. Was always-on in debug builds, drowning out other tracing; now opt-in.
- `localStorage.K2SO_V2_ACTIVITY_VERBOSE='1'` — opt-in for the per-title-change `[v2-activity] TITLE` line in the renderer console. Was always-on in dev mode, ~1 line/sec per active agent; now opt-in.

## What didn't change

Everything else from 0.36.2 ships unchanged: default presets, LLM provider icons, Heartbeats settings page, Reset Built-ins, mobile companion deprecation notice. Scheduled heartbeats (the new DB-backed system) continue to fire on their configured cadence.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download from the GitHub release page below.
