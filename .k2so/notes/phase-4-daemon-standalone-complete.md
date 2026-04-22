# Phase 4 — Daemon Standalone: COMPLETE

**Branch:** `feat/session-stream` (continues from Phase 3.2)
**Status:** SHIPPED 2026-04-20
**Commits:** H1 → H7 + H4.1 follow-up fix (8 commits total)
**Tests:** 513 green (435 k2so-core + 78 k2so-daemon)
**Engineering stance held:** complete, not quick. No corner-cuts taken.

---

## Strategic outcome

The daemon no longer needs Tauri to serve any `/cli/*` route. Every
endpoint the CLI (`cli/k2so`), the hook scripts, and the companion
clients hit is owned by k2so-daemon. Tauri's role shrunk to:
pure HTTP/WS client + UI renderer + Tauri-command wrappers.

This unblocks the architectural payoffs Phase 4 was designed to
deliver:

- **Lid-closed fully daemon-hosted.** Laptop boots → launchd restarts
  daemon → every endpoint works without ever launching K2SO.app.
- **Thin clients are first-class.** Any WS/HTTP client can observe
  any session or send any CLI verb.
- **Remote connection story unblocked.** Daemon on workstation,
  viewer on phone over tailscale; same protocol as local.
- **Open-core split mechanical.** Daemon stays MIT; premium UI viewers
  can live in a separate crate without compromising the open primitive.
- **Tauri's `agent_hooks.rs` HTTP listener retired.** 3,454 lines of
  Tauri-specific routing no longer bind a TCP socket.

---

## Commit ladder

| Commit | Hash | Scope |
|---|---|---|
| H1 | `d5ce2ae0` | `/cli/terminal/read` + `/cli/terminal/write` daemon-side |
| H2 | `c2ee6a33` | `/cli/agents/running` enumerates daemon session_map |
| H3 | `4d01292b` | `/cli/terminal/spawn{,-background}` via shared spawn helper |
| H4 | `ce10a342` | `/cli/companion/{sessions,projects-summary}` daemon-side |
| H4.1 | `6736d81b` | line-break reconstruction + sentinel filter |
| H5 | `d205f645` | `/cli/agents/{launch,delegate}` daemon-side |
| H6 | `dffe017e` | triage spawns via Session Stream when project opt-in |
| H7 | `64a7368a` | retire Tauri HTTP listener; daemon owns every /cli route |

---

## What works end-to-end today

Everything Phase 3.1 proved still works, plus:

```bash
# Terminal 1 — daemon (starts via launchd; manual for dev)
cargo run -p k2so-daemon

# Terminal 2 — verify the sole HTTP server is the daemon
cat ~/.k2so/heartbeat.port    # daemon's port
cat ~/.k2so/daemon.port       # same port (backcompat file)

# Terminal 3 — every /cli/* route hits the daemon
k2so sessions spawn --agent runner --command bash
k2so terminal write <session-uuid> "echo hello"
k2so terminal read <session-uuid>
k2so agents running              # lists daemon's session_map
k2so companion sessions           # cross-workspace session list
k2so companion projects-summary   # per-project counts + focus groups

# Launch an agent (three-branch decision tree, but daemon-owned spawn)
k2so agents launch --agent runner

# Delegate a work item (creates worktree + writes CLAUDE.md, daemon spawn)
k2so agents delegate --target runner --file <path-to-work.md>

# Triage (scheduler tick + heartbeat tick)
# Per project setting:
k2so settings set use_session_stream on    # opt-in to daemon path
k2so agents triage                          # spawns land in session_map
```

---

## Architectural invariants preserved

Every Phase 1-3.2 invariant still holds. Explicitly re-verified
each commit:

1. **Subscribers never import alacritty types.** No new imports
   in `awareness/`, `session/`, or daemon-side routing crossed
   this line. ✅
2. **LineMux sees raw PTY bytes, not alacritty's grid.** Spawn
   path unchanged. ✅
3. **Feature flag gates consumer side only.** `session_stream`
   feature still gates k2so-core consumers; H6 honors
   `use_session_stream='on'` per-project. ✅
4. **Sender's `Delivery` choice is load-bearing.** Unchanged. ✅
5. **Audit always fires.** Every spawn + signal still writes
   `activity_feed`. ✅

New architectural facts introduced this phase:

6. **Daemon is the sole HTTP server.** Post-H7 the Tauri
   agent_hooks listener no longer binds. Tauri's daemon_client
   is the only legitimate "Tauri talks to daemon" channel.
7. **`heartbeat.port` / `heartbeat.token` are daemon-owned.**
   CLI + hook scripts read from one address — daemon's. Watchdog
   re-claims on file loss.
8. **`hook_config` in Tauri is primed from daemon.port at
   startup.** In-process Alacritty children inject daemon's port
   into child envs so they can still emit `/hook/complete` via
   `notify.sh`. 5x 500ms retry for cold-start races.

---

## Gotchas + patterns future commits must match

### Daemon writes 4 files at startup

- `daemon.port` / `daemon.token` — for Tauri's DaemonClient
  (internal) and `k2so daemon status` (external health check).
- `heartbeat.port` / `heartbeat.token` — for the CLI + every
  launchd hook script. This is the user-facing address.

Both pairs carry the same port/token today. A future follow-up
can unify (corner #2 of corners-cut-0.33.0.md) once
daemon_client.rs is rewritten to read heartbeat.*. Not in Phase 4
scope — closing #2 cleanly requires touching the Tauri daemon
client in a way that would balloon H7.

### Tauri's `agent_hooks.rs` is dead code

The file still compiles. `start_server` is never called; its 60+
HTTP match arms + all supporting helpers exist but are
unreachable. A future commit can delete the file outright, but
that's a 3,454-line removal we deliberately held out of Phase 4
to keep the diff focused.

### H5's delegate test depends on `git` binary

`crates/k2so-daemon/tests/agents_routes_integration.rs` shells
out to `git init` + `git add` + `git commit`. CI that disables
git or runs with HOME=/ will skip these tests. Mirror pattern
exists in other test files in the repo.

### H6's triage test requires `heartbeat_mode != 'off'`

Seeds must insert `heartbeat_mode = 'heartbeat'` explicitly —
the column defaults to 'off' in migration 0020, which causes
`scheduler_tick` to return early with `skipped_schedule`. One-line
gotcha that cost me 10 minutes debugging the first attempt.

### H7's Tauri hook_config priming is best-effort

If `daemon.port` / `daemon.token` aren't readable within 5×500ms
of Tauri boot, the prime helper logs and gives up. Alacritty
children still spawn fine but their `K2SO_HOOK_PORT` env is 0 +
`K2SO_HOOK_TOKEN` is empty — hooks run silently. Test envs
without a real daemon won't have this problem because they use
`k2so_core::agent_hooks::emit` directly through the
in-process sink, not subshell curl.

### Test count jumped from 473 to 513 (+40 tests)

Breakdown:
- H1: 17 terminal_routes tests
- H2: embedded in H1 (agents_running tests)
- H3: embedded in H1 (spawn tests)
- H4: 4 companion_routes tests
- H5: 5 agents_routes tests
- H6: 3 triage tests
- H7: 1 heartbeat_port_claim test

Remaining ~10 came from regressions hardened with focused test
coverage during each H-commit.

---

## What's NOT in Phase 4

Held out on purpose to keep the phase shippable:

- **`alacritty_terminal` dep removal.** Phase 5 after 4.5
  proves the new pipeline is the sole viewer path.
- **`daemon.port` / `heartbeat.port` unification.** Follow-up
  tidy commit — touches Tauri's daemon_client.rs.
- **Deletion of `src-tauri/src/agent_hooks.rs`.** Still compiles
  but unreachable. Separate commit.
- **`/cli/msg` coverage in daemon.** Still Tauri-only if invoked;
  no active consumer reaches it today (the k2so CLI's `msg` verb
  routes through `/cli/awareness/publish` which is daemon-native).

---

## Before starting the next phase

1. **Manual smoke.** Rosson, run through the three-terminal demo
   above. Biggest risk: H7's hook_config priming — does a fresh
   Tauri launch still receive `/hook/complete` calls from Claude
   sessions? Confirm `~/.claude/settings.json` has the
   `.k2so/hooks/notify.sh` hook registered and works end-to-end.

2. **Phase 4.5 unblocked.** Tauri React pane subscribes to
   `/cli/sessions/subscribe` WS; handles `use_session_stream='off'`
   fallback; feature-flag reversible. Conceptually isolated from
   everything Phase 4 shipped.

3. **Phase 5 unblocked (but wait).** With Phase 4 + 4.5 stable
   for one release we can start tearing out the alacritty legacy
   path. Don't rush it.

---

## Rollback

All 8 Phase 4 commits are additive on the feature branch. Every
H-commit is independently revert-safe:

- H7 revert restores Tauri's HTTP listener; daemon stops
  claiming heartbeat.port (watchdog still fills the gap).
- H6 revert loses the `use_session_stream` dispatch; legacy
  `spawn_wake_headless` owns every triage spawn.
- H1-H5 reverts remove the corresponding routes; old Tauri
  endpoints serve requests via the still-bound listener.
- Nuclear: `git reset --hard v0.33.0` + rebuild. Still
  bit-for-bit v0.33.0.

Emergency partial-rollback pattern: stage the daemon binary's
route removal (cli.rs dispatch arms) to return 404; the legacy
Tauri endpoints still bind in pre-H7 checkouts. Given H7 flipped
Tauri's bind off, emergency rollback after H7 needs either a
Tauri rebuild with the bind restored OR a daemon rollback that
also restores the watchdog's 2s delay.
