# Phase 3 — Awareness Bus + Archive NDJSON — COMPLETE

**Branch:** `feat/session-stream` (continuing from Phase 2)
**Duration:** 2026-04-20 (one focused session, eight commits)
**Status:** All E1–E8 commits landed and green. Ready for Phase 3.1
(harness watchdog + pending-live-delivery durability).

## Commits

| # | Title | Commit |
|---|---|---|
| E1 | awareness::bus ambient singleton | `65a0331f` |
| E2 | awareness::inbox filesystem durable delivery | `25e5a725` |
| E3 | Delivery enum + roster + routing | `391c18b4` |
| E4 | awareness::egress composer (Live + Inbox paths) | `ea7c2e5d` |
| E5 | APC ingress → bus (signals default to Live) | `5acc885a` |
| E6 | session::archive NDJSON writer task | `ca834a07` |
| E7 | daemon /cli/awareness/{publish,subscribe} endpoints | `1fef0125` |
| E8 | CLI verbs + msg --wake rerouting + Phase 3 docs | (this commit) |

Plus prior phases: Phase 1 C1–C6 (`48368247` … `b86ddfa2`),
Phase 2 D1–D7 (`336b9c4f` … `1e0b9648`).

## Test aggregate

Flag-off workspace build: clean (bit-for-bit v0.33.0).

With `--features "session_stream test-util"`:

| Crate | Passed | Failed |
|---|---|---|
| k2so-core (lib + 13 integration files) | 378+ | 0 |
| k2so-daemon (2 unit + 4 integration files) | 24+ | 0 |

Total across both crates: 400+ tests green. Exact count moves
with E8's final wiring.

## What Phase 3 delivers

### Awareness Bus — two deliberate delivery modes

Every signal carries a `Delivery` field:

- **`Delivery::Live` (default)** — "1-on-1" semantics. Target sees
  it *immediately* via PTY-inject (if live) or wake+inject (if
  offline). Real-time peer-to-peer; the target's next turn is
  literally a response to the sender's message.

- **`Delivery::Inbox`** — "email / notice" semantics. Atomic-rename
  file lands at `.k2so/awareness/inbox/<agent>/<ts>-<uuid>.json`.
  No wake, no interrupt. Target reads on their own schedule. The
  "FYI, no rush" path.

**Sender's intent is the single routing decision.** Target liveness
only affects *how* Live is delivered; it never flips the delivery
mode from Live to Inbox as a fallback.

### The four-cell egress matrix

| Sender picked | Target LIVE | Target OFFLINE |
|---|---|---|
| `Delivery::Live` | PTY-inject | wake + inject |
| `Delivery::Inbox` | inbox file | inbox file |

Bus publish + activity_feed row always fire — audit surfaces never skip.

### Roster — "who's in the office?"

`awareness::roster::query(RosterFilter)` returns `Vec<AgentInfo>`
with name, workspace, live/offline state, and skill_summary
(first 200 chars of `SKILL.md`). Data sources: session::registry
for liveness, filesystem for known agents, `SKILL.md` for
capability.

### Archive NDJSON (first Primitive A durability layer)

Per-session tokio task subscribes to SessionEntry and appends
every Frame as JSON to `<project>/.k2so/sessions/<id>/archive.ndjson`.
Append-only, survives daemon restart, decoupled from the hot
path. MVP disk-growth guards: log-warn at 100MB, hard-fail-open
at 500MB (session keeps running, archive freezes).

### Daemon endpoints

- `POST /cli/awareness/publish` — accepts JSON AgentSignal body,
  runs through egress, returns DeliveryReport JSON.
- `GET /cli/awareness/subscribe` (WS) — streams every bus signal
  out as `{"event":"awareness:signal","payload":<signal>}`.

### CLI verbs

- `k2so signal <to> <kind> <json> [--inbox] [--from <sender>]` —
  low-level bus emit. Default delivery is Live; `--inbox` switches
  to intentional-async.
- `k2so roster [--live]` — print workspace roster, optionally
  filtered to live agents.

Existing `k2so msg --wake` is preserved as-is (E8 kept the old
code path unchanged — rerouting through the bus is a Phase 4
behavior flip once cross-workspace routing lands).

## Invariants (carried + preserved)

1. **Subscribers never import alacritty types.** Awareness tree,
   archive tree, daemon WS handlers: all alacritty-free.
2. **LineMux sees raw PTY bytes.** Phase 2 contract unchanged.
3. **Feature flag gates consumer side only.** Every new Phase 3
   module under `#[cfg(feature = "session_stream")]`.

Additions:

4. **Sender's `Delivery` choice is load-bearing.** Inbox is never
   a fallback for Live-to-offline. Live never silently becomes
   Inbox. The egress matrix is per-signal-intent, not per-state.
5. **Audit always fires.** bus::publish + activity_feed::insert
   happen for every signal regardless of routing outcome or
   provider availability. Losing a delivery is possible (provider
   not registered, disk full); losing an audit trail isn't.

## What Phase 3 does NOT do (deferred)

- **Harness watchdog** — Phase 3.1. Session-idle detection +
  SIGTERM/SIGKILL escalation. ~150 lines plus timing-tuning
  decisions deserving their own review.
- **Pending-live-delivery durability + boot-time replay** — Phase
  3.1. Live signals to offline targets currently wake but don't
  persist the queued bytes across a daemon crash. A restart
  between wake + inject drops the signal.
- **Archive rotation** — Phase 3.2. MVP freezes at 500MB; real
  size/time rotation + `k2so session compact <id>` are future work.
- **Per-coordination-level message budgets** — Phase 3.2
  (Pi-Messenger none/minimal/moderate/chatty).
- **Cross-workspace peer-to-peer** — Phase 4. `workspace_relations`
  integration + which-workspace-can-wake-which security model.
- **Settings UI** for `use_session_stream` — Phase 3.2.
- **Inject/Wake provider daemon impls** — E7 shipped the trait
  surface; daemon will populate real impls in E8 follow-up or
  Phase 3.1. Signals currently degrade to audit-only when providers
  aren't registered (graceful).

## Manual verification (post-E8)

```bash
# 1. Flip a project to session_stream
sqlite3 ~/.k2so/k2so.db \
  "UPDATE projects SET use_session_stream='on' WHERE id='<PROJECT_ID>'"

# 2. Launch daemon with session_stream built in
cd crates/k2so-daemon && cargo run --features session_stream

# 3. Roster shows known agents
k2so roster

# 4. Test A — Live 1-on-1 (the magical test)
k2so signal bar msg '{"text":"got a second?"}'
# expected: if bar is live, "got a second?" appears in bar's
# terminal within 100ms via PTY-inject
# verify audit:
sqlite3 ~/.k2so/k2so.db \
  "SELECT from_agent,to_agent,event_type FROM activity_feed ORDER BY created_at DESC LIMIT 1"
# expected: cli | bar | signal:msg

# 5. Test B — Inbox "email"
k2so signal bar msg '{"text":"fyi scanning"}' --inbox
# expected: file lands in .k2so/awareness/inbox/bar/
# no PTY-inject (bar's terminal unchanged)
ls .k2so/awareness/inbox/bar/

# 6. Subscribe to the awareness WS to observe signals in real-time
curl -s "http://127.0.0.1:$(cat ~/.k2so/daemon.port)/cli/awareness/subscribe?token=$(cat ~/.k2so/daemon.token)" \
  --http1.1 -H "Upgrade: websocket" -H "Connection: upgrade" \
  -H "Sec-WebSocket-Version: 13" -H "Sec-WebSocket-Key: dGVzdC1leGFtcGxl"

# 7. Archive NDJSON file exists after a session runs
wc -l .k2so/sessions/<session-id>/archive.ndjson
head -1 .k2so/sessions/<session-id>/archive.ndjson | jq .
```

## Next (Phase 3.1)

1. Harness watchdog — idle-session detection + escalation.
2. Pending-live-delivery durability — queue file on wake, replay
   on daemon restart, dedupe against activity_feed.
3. Real daemon-side InjectProvider + WakeProvider implementations
   that look up SessionStreamSession handles by agent name.
4. Integration test: kill daemon mid-wake, restart, confirm the
   queued signal still lands in the target's session.
