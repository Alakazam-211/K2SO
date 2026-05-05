# K2SO 0.36.15 — Hotfix: `k2so msg` workspace-aware routing

`k2so msg <agent> "..." --wake` was ignoring `K2SO_PROJECT_PATH` when multiple workspaces had the same agent name. All `k2so msg scout` invocations from any workspace context landed in whichever workspace's pinned chat tab happened to register the bare agent name in the daemon registry last — completely independent of which workspace the sender was running from. 0.36.14 fixed half of this (the registry assignment, so each chat tab is properly bound to its own workspace's PTY); this release fixes the other half (the routing path that decides which session a `--wake` signal lands in).

## What was happening

The pinned chat tab fix in 0.36.14 made `v2_session_map` register sessions under project-namespaced keys (`<projectId>:<agentName>`), with a bare-name mirror for back-compat. That fixed cross-workspace cross-wiring of the chat tabs themselves.

But the awareness bus's egress path — the code that turns `k2so msg` into PTY writes — was still doing bare-name lookups:

- `egress::is_agent_live(agent)` consulted the inject provider's `is_live(agent)` with bare names. With two workspaces both registering bare `scout`, the bare slot resolved to whichever spawned last.
- `egress::try_inject(agent, signal)` injected via `provider.inject(agent, bytes)` with the bare name. Even though the signal envelope carried `signal.to.workspace = <project_id>`, that workspace context was never consulted at the inject site.

So the reported scenario, with two workspaces both running an agent named `agent-X`:

```
K2SO_PROJECT_PATH=<workspace-A>  k2so msg agent-X "..." --wake  →  delivered to whichever workspace's chat registered last
K2SO_PROJECT_PATH=<workspace-B>  k2so msg agent-X "..." --wake  →  same destination — sender path ignored
K2SO_PROJECT_PATH=<workspace-A>  k2so msg agent-X "..." --wake  →  same destination — sender path ignored
```

The CLI correctly stamped each signal with its sender's workspace ID in `signal.to.workspace`, but egress threw that information away before deciding where to deliver.

## What's fixed

Three functions, all on the awareness bus's inject/wake side:

1. **`egress::is_agent_live(agent, signal)`** — now extracts `signal.to.workspace` and checks the prefixed key (`<workspace_id>:<agent>`) first via `provider.is_live`. When the signal targets a specific workspace, that's the *only* key checked — no bare fallback, because the bare slot points at "whichever workspace registered last," which would re-introduce the cross-wiring bug.
2. **`egress::try_inject(agent, signal)`** — same prefixed-only lookup for workspace-targeted signals. Returns `NotFound` (offline) when the target workspace's session isn't running, which correctly routes the signal to the wake path.
3. **`DaemonWakeProvider::try_auto_launch`** — the auto-spawn path for offline workspace agents. Now uses `signal.to.workspace` to resolve which workspace to spawn into (was `signal.from.workspace` — a legacy assumption that messages target the sender's workspace), checks the prefixed key for single-flight (so a live session in workspace A doesn't suppress an auto-spawn in workspace B), and registers the spawned session under `<workspace_id>:<agent>` so the next inject finds it.

The bare-name lookup path is preserved for callers that don't supply a workspace context (Workspace/Broadcast addresses, heartbeat-surfaced sessions registered under `tab-<id>` keys, worktree chats under `wt-<id>` keys).

## What's not affected

- **Heartbeats.** The heartbeat scheduler runs on a separate spawn path (`heartbeat_launch::run_inject` + `smart_launch`). Heartbeat-surfaced session keys (`tab-<rendererId>`) don't have colons and fall through to the legacy bare logic unchanged. Heartbeat firing, surfacing, dedup, active-session tracking — all untouched.
- **Inbox delivery (`k2so msg --inbox`).** Filesystem write path, doesn't go through inject/wake. Unchanged.
- **Worktree chats.** Register under `wt-<worktreeId>` keys, don't match the chat-tab `<projectId>:<agentName>` pattern. Unchanged.
- **Cross-workspace work delivery (`k2so work send`).** Different code path. Unchanged.
- **Activity feed audit, bus pub/sub.** Both fire unconditionally for every signal regardless of inject success or failure. Unchanged.
- **Pinned chat tab renderer (`AgentChatPane.tsx`).** No renderer changes — 0.36.14 already shipped the prefixed `attachAgentName` and the `key={projectId}` remount. This release just completes the daemon-side routing to match.
- **DB schema.** No migrations. The fix is pure routing logic.

## Behavior change worth noting

The wake provider previously resolved its target workspace from `signal.from.workspace` (a legacy assumption: "messages target the sender's workspace"). Now it prefers `signal.to.workspace`, falling back to `signal.from` only when `to` doesn't carry a workspace identifier. For same-workspace messaging — `K2SO_PROJECT_PATH=A k2so msg agent` — sender and target are the same, so behavior is unchanged. For future cross-workspace messaging (`signal.from.workspace = A, signal.to.workspace = B`), this is the correct semantic.

## Combined effect of 0.36.14 + 0.36.15

| Scenario | Pre-0.36.14 | 0.36.14 only | 0.36.15 |
|---|---|---|---|
| Two workspaces both running `scout` chat tabs | Chat tabs cross-wired (same PTY) | Chat tabs isolated ✓ | Chat tabs isolated ✓ |
| `K2SO_PROJECT_PATH=A k2so msg scout` (A's chat alive) | Lands in last-registered workspace's PTY | Lands in last-registered workspace's PTY | Lands in A's PTY ✓ |
| `K2SO_PROJECT_PATH=B k2so msg scout` (B's chat offline) | Lands in last-registered workspace's PTY | Lands in last-registered workspace's PTY | Routes to wake; auto-spawns B's scout; signal drains into the new B session ✓ |
| `k2so msg scout` (no `K2SO_PROJECT_PATH`) | Bare-name lookup (legacy) | Bare-name lookup (legacy) | Bare-name lookup (legacy) — unchanged for unscoped callers |

## Forward compatibility

The 0.37.0 workspace–agent unification will replace the prefixed-key workaround with workspace-keyed addressing throughout the daemon (`workspace_sessions` PK on `project_id`, `lookup_by_workspace`, etc.). The bare-name mirror retires when every caller carries workspace context; until then, the prefixed/bare dual-key pattern is the bridge.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
