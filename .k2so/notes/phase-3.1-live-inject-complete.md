# Phase 3.1 — Live Inject + Spawn + Durability — COMPLETE

**Branch:** `feat/session-stream` (continuing from Phase 3)
**Duration:** 2026-04-20 (same session as Phase 3)
**Status:** All F1–F3 commits landed and green. Phase 3's
"sender's Delivery intent is load-bearing" primitive is now
actually reachable from an external terminal.

## Commits

| # | Title | Commit |
|---|---|---|
| F1 | Daemon session_map + real Inject/Wake providers | `9852d70f` |
| F1.5 | Full signal JSON in activity_feed.metadata | `0e7a72e8` |
| F2 | POST /cli/sessions/spawn + CLI verb | `2d72f6ee` |
| F3 | Pending-live durability + Phase 3.1 doc | (this commit) |

## Test aggregate

| Crate | Tests | Failed |
|---|---|---|
| k2so-core (`session_stream test-util`) | 379 | 0 |
| k2so-daemon (unit + 4 integration files) | 32 | 0 |
| **Total** | **411** | **0** |

Flag-off workspace build: clean, bit-for-bit v0.33.0.

## What Phase 3.1 delivers

### Real-time peer-to-peer actually works end-to-end

Before Phase 3.1: `k2so signal bar msg '...'` wrote to
`activity_feed` and published to bus, but the InjectProvider wasn't
registered, so bar's terminal never saw anything.

After Phase 3.1: the daemon registers `DaemonInjectProvider` at
startup. When bar has been spawned through the daemon (via
`POST /cli/sessions/spawn`), the provider looks up bar's session
handle in `session_map` and calls `session.write(bytes)`. bar's
running TUI harness receives the bytes as if typed by a user.

### The `POST /cli/sessions/spawn` endpoint + CLI verb

Any external caller can now ask the daemon to spawn a Session
Stream session for a named agent:

```bash
k2so sessions spawn --agent bar --command cat
# → {"sessionId": "...", "agentName": "bar", "pendingDrained": 0}
```

The daemon:
1. Spawns via `spawn_session_stream()` in its tokio runtime
2. Tags the SessionEntry's `agent_name` so roster + liveness
   detection find bar
3. Registers `Arc<SessionStreamSession>` in session_map under bar
4. **Drains any pending-live signals queued while bar was offline**
   — injects them in order so the target sees them as its first
   input
5. Returns the session id + how many pending signals got drained

### Pending-live durability (F3)

`DaemonWakeProvider::wake` now persists signals to
`~/.k2so/daemon.pending-live/<agent>/<ts>-<uuid>.json` instead of
just logging. The signal survives daemon restart.

Three drain paths:
- **Spawn-time drain** (F2): when an agent's session spawns,
  `drain_for_agent(name)` reads + injects queued signals, files
  deleted after parse.
- **Boot-time replay** (F3, daemon startup): scans every agent's
  subdirectory, logs how many signals are queued. Files stay on
  disk until the agent's session spawns — replay re-enqueues
  them so the next spawn picks them up.
- **File lifecycle**: atomic-rename writes via `fs_atomic`;
  filenames use ns-precision timestamp + uuid so sorted-lex =
  sorted-by-time; concurrent writers never collide.

### Primitive audit (F1.5)

`activity_feed.metadata` now stores the full JSON AgentSignal on
every delivery. This makes activity_feed the complete audit source
— callers can reconstruct any message with:

```sql
SELECT json_extract(metadata, '$.kind.data.text') AS body
  FROM activity_feed
 WHERE event_type = 'signal:msg'
   AND json_extract(metadata, '$.from.name') = 'foo';
```

Higher-level views (conversation threads, reply chains,
per-agent message history) are SQL queries on top — no schema
migration needed.

## End-to-end demo

Three-terminal dance:

```bash
# Terminal 1 — daemon
cargo run --features session_stream -p k2so-daemon

# Terminal 2 — spawn bar under daemon control
k2so sessions spawn --agent bar --command bash
# bar's new session becomes visible

# Terminal 3 — send bar a real-time 1-on-1
k2so signal bar msg '{"text":"hello bar"}'
# → bar's terminal shows: [cli] hello bar

# Or: send bar a notice for later
k2so signal bar msg '{"text":"fyi scanning"}' --inbox
# bar's terminal unchanged; file lands in
# .k2so/awareness/inbox/bar/<ts>-<uuid>.json

# Or: send a signal to offline agent
k2so signal nobody msg '{"text":"wake up nobody"}'
# nothing injects (nobody's not live); daemon queues
# ~/.k2so/daemon.pending-live/nobody/<ts>-<uuid>.json
# Next `k2so sessions spawn --agent nobody` drains + injects.

# Full audit trail — every signal, full JSON preserved
sqlite3 ~/.k2so/k2so.db "
  SELECT from_agent, to_agent, event_type,
         json_extract(metadata, '$.kind.data.text') AS body,
         json_extract(metadata, '$.delivery') AS mode
  FROM activity_feed
  WHERE event_type LIKE 'signal:%'
  ORDER BY created_at DESC LIMIT 5;"
```

## Invariants (carried + preserved)

1. Subscribers never import alacritty types — holds.
2. LineMux sees raw PTY bytes — holds.
3. Feature flag gates consumer side only — holds.
4. Sender's Delivery choice is load-bearing — holds. Inbox never
   fallback for Live-to-offline; Live-to-offline wakes via the
   persistent queue.
5. Audit always fires — holds. Every signal writes activity_feed
   row with full JSON metadata.

## What's still deferred

- **Harness watchdog** — idle-session detection + SIGTERM/SIGKILL
  escalation. Phase 3.2 or its own small phase.
- **Real scheduler-wake** — the `DaemonWakeProvider` currently
  persists but doesn't actually LAUNCH the agent's session. A
  spawn via `k2so sessions spawn` or user action is required. A
  real scheduler-wake that launches the session automatically on
  signal arrival is follow-up work.
- **Archive rotation** — still MVP no-rotation (freeze at 500MB).
- **Cross-workspace peer-to-peer** — Phase 4.
- **Tauri UI consuming session stream** — Phase 5ish (the
  "user-visible wiring" milestone).

## Next recommended

Either:
- **Phase 4** — daemon-side consumption of the 14 stranded /cli/*
  routes + cross-workspace routing. Unblocks everything downstream.
- **Phase 5** — wire the Tauri React terminal pane to subscribe to
  Session Stream. First user-visible "terminals are now rendering
  from the new architecture" moment.
- **Phase 3.2** — harness watchdog, archive rotation, coordination
  budgets. Hardening before a user-visible release.

Full phase-3+3.1 summary on `feat/session-stream`:

```
F3  Pending-live durability + Phase 3.1 doc            ← just landed
F2  POST /cli/sessions/spawn endpoint + CLI verb
F1.5 Full signal JSON in activity_feed.metadata
F1  Daemon session_map + real Inject/Wake providers
E8  CLI verbs + archive wiring + Phase 3 docs
E7  daemon /cli/awareness/{publish,subscribe}
E6  session::archive NDJSON writer task
E5  APC ingress → bus (signals default to Live)
E4  awareness::egress composer
E3  Delivery enum + roster + routing
E2  awareness::inbox filesystem durable delivery
E1  awareness::bus ambient singleton
D1-D7  Phase 2 (SessionRegistry + dual-emit + subscribe)
C1-C6  Phase 1 (parser + Claude recognizer)
```

25 commits across three phases. Peer-to-peer agentic collaboration
works end-to-end via CLI + daemon + real PTYs, with full audit
and durability.
