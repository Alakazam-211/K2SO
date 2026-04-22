# Kessel Launch Perf — Diagnosis + Plan

**Problem:** Kessel terminal launch (Cmd+T, Cmd+Shift+T) is 3-5x
slower than Alacritty, end-to-end from keypress to "claude is
running." The renderer once mounted feels fine; the gap is entirely
in the boot path. Rosson: *"Rust is a C++ level-type of language, so
we should be able to make it VERY fast."* Correct — and the current
slowness is structural, not fundamental.

---

## Why it's slow — full critical path

### Alacritty (baseline, ~20-30ms total)

| # | Step | File | Est |
|---|---|---|---|
| 1 | Cell metrics measure | `AlacrittyTerminalView.tsx` | <1ms |
| 2 | `invoke('terminal_exists')` | Tauri IPC | 1-3ms |
| 3 | `invoke('terminal_create')` | Tauri IPC | 10-30ms |
| 3a |   Shell detect + resolve cwd | `alacritty_backend.rs:280` | 1-3ms (sync file I/O) |
| 3b |   `portable_pty::openpty` + `spawn_command` | `alacritty_backend.rs:412` | **5-15ms (fork+exec)** |
| 3c |   `EventLoop::spawn` (alacritty's own reader thread) | `alacritty_backend.rs:431` | 1-2ms |
| 3d |   `GlyphCache::new` + emission thread | `alacritty_backend.rs:439` | 1-2ms |
| 4 | `invoke('terminal_get_grid')` | Tauri IPC | 2-5ms |
| 5 | `listen('terminal:...')` subscriptions | Tauri | <1ms |

Everything is **in-process** with the Tauri app. One fork+exec, two
thread spawns, no HTTP, no WebSocket, no JSON.

### Kessel (~100-150ms)

| # | Step | File | Est |
|---|---|---|---|
| 1 | `invoke('daemon_ws_url')` (cached after `b1aebde4`) | `daemon-ws.ts` | 0ms after first call |
| 2 | Browser `fetch(POST /cli/sessions/spawn)` | `KesselTerminal.tsx:82` | 5-15ms |
| 3 | Daemon: TCP accept + HTTP parse + JSON body parse | `awareness_ws.rs:55` | 2-5ms |
| 4 | `spawn_agent_session` (`spawn.rs:77`): |  | **~30-50ms synchronous** |
| 4a |   `SessionId::new()` | `session/types.rs` | <1ms |
| 4b |   `spawn_session_stream`: resolve_cwd, detect_shell | `session_stream_pty.rs:236` | 1-3ms |
| 4c |   `openpty` + `spawn_command` | `session_stream_pty.rs:247` | **5-15ms (fork+exec)** |
| 4d |   `Term::new` + `FairMutex` | `session_stream_pty.rs:320` | 1-2ms |
| 4e |   `registry::register` (HashMap insert) | `session_stream_pty.rs:326` | <1ms |
| 4f |   `archive::spawn` (tokio task, async but setup has sync parts) | `session_stream_pty.rs:336` | **5-15ms (file create + dir walk)** |
| 4g |   `std::thread::spawn(reader_loop)` | `session_stream_pty.rs:354` | 1-2ms |
| 4h |   `session_map::register` | `spawn.rs:106` | <1ms |
| 4i |   **`pending_live::drain_for_agent`** → `fs::read_dir` + `fs::read` per file | `pending_live.rs:79` | **5-10ms (always pays disk I/O even when empty)** |
| 5 | Daemon: JSON response serialize + HTTP write | `awareness_ws.rs:119` | 1-3ms |
| 6 | Browser: `await res.json()` | `KesselTerminal.tsx:112` | 1-2ms |
| 7 | React setState → SessionStreamView mount | React reconciliation | 2-5ms |
| 8 | `KesselClient.connect()` → WS handshake | `client.ts:121` | **10-20ms** |
| 9 | Daemon: WS accept + subscribe to `SessionEntry` broadcast | `sessions_ws.rs` | 2-5ms |
| 10 | Daemon: replay burst (initial shell prompt frames) | `sessions_ws.rs` | 5-15ms |
| 11 | Browser: decode replay + apply to grid + first paint | `SessionStreamView.tsx` | 3-8ms |

**The extra ~70-120ms vs Alacritty is concentrated in:**

1. **Two IPC hops** (Tauri + HTTP) instead of one (Tauri).
2. **`pending_live::drain_for_agent`** touches disk on every spawn
   even when the pending directory is empty.
3. **Archive task setup** does `create_dir_all` + `metadata` +
   `compute_aggregate_bytes` (directory walk).
4. **WebSocket handshake** on top of the spawn round trip.
5. **Replay burst** has to arrive before the user sees anything —
   serialized JSON frames vs. Alacritty's direct grid access.

---

## The plan — 5 levels, roughly in order of ROI

### Level 1 — Zero-risk hot-path cleanups (target: -20ms)

Each is a small, self-contained change. Ship as individual commits.

**L1.1 — Skip `pending_live::drain_for_agent` when the dir is empty.**
Replace per-spawn `fs::read_dir` with a daemon-lifetime
`AtomicBool`-per-agent "has pending" flag populated at boot from one
`replay_all` scan. Every subsequent spawn is a pure atomic read;
only spawns for agents with queued signals pay the disk read.
**Impact:** ~5ms per spawn. O(1) instead of O(k) where k is files
in the directory.
File: `crates/k2so-daemon/src/pending_live.rs`.

**L1.2 — Eagerly return the `sessionId` from the spawn handler;
defer archive setup to a fully-async task.**
Today `archive::spawn()` happens inside the synchronous spawn path
but the tokio task doesn't block. However the task's *setup*
(`create_dir_all` + `metadata` + `compute_aggregate_bytes`) blocks
the first few frames from being archived — not the spawn itself.
Audit + confirm archive setup is not on the spawn-response path.
If anything blocks, move it to the task body after first `rx.recv()`.
**Impact:** 0-5ms depending on current ordering.
File: `crates/k2so-core/src/session/archive.rs`.

**L1.3 — Replace `fork+exec` with `posix_spawnp` on macOS.**
`portable-pty` defaults to `fork+exec` on Unix. macOS's
`posix_spawnp` is 1-3ms faster because it skips the full
`fork()` memory-copy (CoW or not, there's still kernel bookkeeping).
Requires a small patch to `portable-pty` OR a local shim using
`std::os::unix::process::CommandExt`. Alacritty uses whatever
portable-pty provides so this helps both paths equally.
**Impact:** ~1-3ms per spawn.
File: new `crates/k2so-core/src/terminal/posix_spawn.rs` (shim).

**L1.4 — Single Tauri command `kessel_spawn` that does the HTTP
POST server-side.**
Eliminates the browser `fetch` overhead (TLS setup even on
localhost, CORS preflight checks, JSON body pipelining). Rust does
the POST with a persistent `reqwest::Client` keep-alive connection
to the daemon.
**Impact:** ~3-8ms per spawn on the frontend side.
File: new `src-tauri/src/commands/kessel.rs` + update
`KesselTerminal.tsx` to `invoke('kessel_spawn', ...)`.

**L1.5 — Optimistic pane mount.**
Refactor `SessionStreamView` to accept `sessionId: string | null`
and render the grid shell immediately; `KesselClient.connect()`
becomes lazy. `KesselTerminal` mounts `SessionStreamView` right
away, the pane renders its structure (cursor, bg color, cell
metrics), and the WS connects while the user's eye is still
settling on the new tab. Doesn't reduce total wall-clock time but
eliminates the "blank pane → pop in" visual delay.
**Impact:** ~20-50ms perceived.
Files: `SessionStreamView.tsx`, `KesselTerminal.tsx`, `client.ts`.

### Level 2 — Medium-risk structural changes (target: -30ms)

**L2.1 — Persistent shell pool.**
Pre-spawn N bare shells at daemon startup (default 2-3). When a
Kessel spawn arrives, check the pool: if a shell is available with
a matching command profile (bare `zsh`, `bash --noprofile`, etc.),
claim it and `cd` to the requested cwd via a synthetic write
instead of forking a new one. Refill the pool async after claim.

Complications:
- Commands with args (`claude`, `htop`) can't reuse a pool shell —
  they need their own spawn. But bare shells are the common case
  for Cmd+T.
- Pool shells must not leak env-var state from one user to the next
  (in a single-user app, not an issue).
- `cwd` change is a `chdir` via shell command, not a real process
  cwd change. Behavior should be indistinguishable in practice.

**Impact:** ~5-15ms per bare-shell spawn.

**L2.2 — Combined spawn-and-subscribe endpoint.**
Add a variant of `/cli/sessions/spawn` that upgrades to WebSocket
on the same request. Frontend sends one HTTP+WS upgrade; daemon
spawns the session synchronously and starts streaming on the same
socket. Saves one TCP+HTTP handshake (~5-10ms) AND eliminates the
gap between spawn response and WS subscribe where the shell might
print its prompt before anyone's listening.

Requires protocol change + careful WS upgrade handling.

**Impact:** ~5-10ms per spawn.

**L2.3 — Unix domain sockets for daemon IPC.**
TCP loopback has ~100-500µs per round-trip on macOS. UDS has
~20-100µs. For a ~2 round-trip handshake this is a 1-3ms save.
Low-risk but touches both daemon (accept-loop) and frontend (Tauri
command wrapping reqwest). Ship behind a feature flag first.

**Impact:** ~1-3ms per IPC round trip.

### Level 3 — Big-O improvements

**L3.1 — `compute_aggregate_bytes` is O(N) in segments.**
On archive-writer startup for a long-running session, walks every
`.ndjson` file in the session dir. For Kessel this rarely matters
(new session = empty dir) but regressions during replay or session
re-open would pay. Cache the aggregate in
`archive_dir/.aggregate` (single u64 written after each rotation).
On startup, read the cached value and verify by stating the active
segment only. **Impact: O(N) → O(1) on replay.**

**L3.2 — `session_map::register` contends on a global RwLock.**
Under load (many concurrent spawns, not a current issue) the lock
serializes. Swap to `DashMap` for shard-striped concurrent inserts.
**Impact: amortized O(1) but concurrent-friendly.** Phase-it-when-we-
need-it.

**L3.3 — `registry::lookup` on every frame publish.**
Check if the `Arc<SessionEntry>` is passed down to publishers
(`reader_loop`) or if there's a lookup per frame. If the latter —
clear win to pass by reference. Already done via `entry_for_reader`
clone into the closure, so no change needed.

### Level 4 — Benchmarking infrastructure

None of the above should ship without numbers. Add `criterion`
benches so we can track regressions and demonstrate wins.

**L4.1 — Rust micro-benchmarks (`criterion`):**
- `crates/k2so-core/benches/spawn_session_stream.rs` — measures
  fork+exec + registry insert + archive setup. Target: drive to
  parity with alacritty's `TerminalManager::create`.
- `crates/k2so-core/benches/line_mux_feed.rs` — bytes/sec
  throughput of LineMux parsing. Baseline existing performance +
  catch regressions.
- `crates/k2so-core/benches/frame_broadcast.rs` — publish-to-
  subscribe latency across the broadcast channel.

**L4.2 — Daemon HTTP integration benches:**
- `crates/k2so-daemon/benches/sessions_spawn_handler.rs` — full
  HTTP handler from body-parse to response, using a test TCP
  connection. Measures ceiling for an L1.4 `kessel_spawn` improvement.

**L4.3 — Frontend timing dashboard (dev-only):**
We already have `performance.mark` + `performance.measure` in
`KesselTerminal.tsx` (commit `b1aebde4`). Add a tiny dev-overlay
panel that shows the last N spawn times + the breakdown between
stages. Lets Rosson see "before / after" visually during iteration.

**L4.4 — Flamegraph script:**
Add `scripts/flamegraph-daemon.sh` that launches `cargo flamegraph`
against the daemon under synthetic spawn load (50 sequential
spawns, then 10 concurrent). Required to find the *actual* hot
functions rather than the ones we suspect.

### Level 5 — Rust perf micro-wins (take once benches exist)

**L5.1 — String clone reduction in spawn path.**
`spawn_session_stream` clones `cwd`, `command`, `agent_name`
multiple times. Audit for `&str` / `Cow<str>` opportunities.
**Impact:** ~0.5ms per spawn; cumulative win.

**L5.2 — `Arc` counting in the hot reader loop.**
`reader_loop` clones `Arc<SessionEntry>` into the closure and uses
it every iteration. No regression — but worth auditing that no
per-byte `Arc::clone` happens.

**L5.3 — JSON serialization uses `to_vec` not `to_string`.**
`awareness_ws::handle_sessions_spawn` currently builds a JSON value
then calls `.to_string()`. `serde_json::to_vec` directly into the
response buffer skips one allocation. Tiny but free.

---

## Recommended execution order

**Phase A (this week — ~20ms visible win):**
1. L1.1 pending_live skip (biggest single quick win)
2. L1.4 `kessel_spawn` Tauri command
3. L1.5 optimistic pane mount (perceptual win even if wall-clock
   is unchanged)
4. L4.3 frontend timing overlay so we can SEE the before/after

**Phase B (next week — validate + close the rest):**
5. L4.1–L4.2 criterion benches (must-have before bigger changes)
6. L1.3 posix_spawnp if portable-pty cooperates
7. L2.2 spawn-and-subscribe combined endpoint (needs the benches
   to justify the protocol change)

**Phase C (opportunistic):**
8. L2.1 shell pool — biggest remaining win but also biggest risk
   surface; ship only after benches prove it's worth it
9. L3.1 `compute_aggregate_bytes` O(1) cache — when long-session
   re-opens start showing up as a real pattern
10. L5.1–L5.3 Rust micro-wins — measurable improvements only via
    criterion; skip if flat

---

## Success criteria

**Primary:** Kessel `Cmd+T` → first keystroke ready matches
Alacritty within 10%, measured via the frontend timing overlay
(L4.3) across 10 consecutive spawns.

**Secondary:** Kessel `Cmd+Shift+T` → Claude ready matches
Alacritty within 10%.

**Benchmarks green:**
- `spawn_session_stream` < 25ms p95 (currently likely ~40-60ms)
- `sessions_spawn_handler` < 30ms p95 (currently likely ~50-80ms)
- No regression on `line_mux_feed` throughput after any change

**What this plan does NOT do:**
- Change the Frame / LineMux / SessionStream architecture. All
  the wins are hot-path cleanup + IPC reduction + instrumentation.
- Break the awareness-bus integration. pending_live + archive + bus
  all continue working; we just stop paying for them when they're
  not needed.
- Require a protocol break. L2.2 adds a new endpoint; the existing
  spawn + subscribe endpoints keep working.

---

## One-paragraph exec summary for later context

Kessel's launch gap vs Alacritty decomposes into (a) an extra IPC
hop (browser fetch + daemon HTTP) ~10ms, (b) always-on disk I/O
for pending-signal drain ~5ms, (c) archive task setup I/O ~10ms,
(d) WebSocket handshake ~15ms, (e) replay burst serialization ~8ms.
Eliminating (b) is free (an atomic bool), moving (a) inside a
Tauri command saves another ~5-8ms, and perceptually overlapping
(d)+(e) with an optimistic pane mount hides most of the remainder.
Adding `criterion` benches is mandatory before L2+ so we can
measure instead of guess. Full plan: L1.1 + L1.4 + L1.5 + L4.3
shipped this week, benches + posix_spawnp + combined-endpoint next,
shell pool only if benches justify it.
