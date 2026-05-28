# K2SO 0.39.7 — Daemon HTTP keep-alive: end fd-exhaustion lockups

Closes Issue #2 — the "K2SO progressively slows down over ~50 minutes,
loading indicators lengthen, terminals freeze, then it comes back to
life on its own" class of bug. Reported with a full live-CPU + `lsof`
+ `sample` profile by an external user; that diagnosis nailed the
root cause exactly.

## Root cause

The daemon's HTTP dispatcher had hard-coded
`Connection: close` on every response since the 0.39.0 routes refactor.
Every renderer fetch was therefore a fresh TCP socket — no HTTP
keep-alive, no connection reuse. Combined with the renderer's ~12
periodic pollers (the most active at 2-s intervals) that's a steady
trickle of fresh sockets through macOS's WKWebView Networking process.

WKWebView Networking is Apple-managed and has a default soft
`RLIMIT_NOFILE = 256`. It releases `close()`'d sockets slowly. The
reporter's `lsof -nP -iTCP:<daemon-port>` at ~49 min uptime showed:

```
 228 (CLOSE_WAIT)
  29 (ESTABLISHED)
```

228 + 29 = 257, right at the 256 wall. After that, every new
connection had to wait for the kernel to time out an old `CLOSE_WAIT`
— which matches the symptom progression exactly:

| Uptime | Symptom |
|---|---|
| 0–40 min | Brief, transient `loading…` indicators |
| ~40–50 min | `loading…` periods lengthen visibly |
| ~50 min+ | UI / terminals freeze in stretches |
| After kernel timeout | "Comes back to life out of nowhere" |
| Quit + relaunch | Instant full recovery (fresh WKWebView Networking process) |

## The fix

Daemon now does proper HTTP/1.1 keep-alive:

1. **`crates/k2so-daemon/src/routes/http.rs`** — `send_response` +
   `send_cors_preflight` no longer emit `Connection: close`. HTTP/1.1's
   default is keep-alive; let the connection live.

2. **`crates/k2so-daemon/src/routes/dispatcher.rs`** — split into two
   functions:
   - `pub async fn dispatch(stream, state)` — loops, calling
     `handle_one_request` until it returns `Done`.
   - `async fn handle_one_request(stream, state) -> DispatchOutcome` —
     the existing dispatch body, now taking `&mut TcpStream` and
     returning `DispatchOutcome::{KeepAlive, Done}`.
   - **Idle timeout**: 60 s on the "wait for next request" peek. Once
     a request arrives, the handler is untimed (LLM inference et al.
     can take tens of seconds legitimately).
   - **Max-requests-per-connection**: 10 000. Safety cap. At 2 s/poll
     that's ~5.5 hours of nonstop polling on one socket before recycle.
   - **`Connection: close` request-header detection** — if a client
     opts out of keep-alive, the daemon serves the request normally
     and closes after responding.

3. **WS handler signatures** — `serve_events_connection`,
   `serve_session_subscribe_connection`,
   `serve_session_bytes_connection`,
   `serve_session_grid_connection`,
   `serve_session_events_connection`,
   `serve_awareness_subscribe_connection` (+ the 4 private
   `send_error_then_close` helpers in the per-WS modules) now take
   `stream: &mut TcpStream` instead of `stream: TcpStream`. Bodies
   unchanged — `tokio_tungstenite::accept_async` works fine on
   `&mut TcpStream` via the blanket `AsyncRead`/`AsyncWrite` impls.
   This lets the dispatcher keep ownership of the socket across loop
   iterations.

## Behavior summary

- **Happy path:** renderer's many periodic pollers re-use the same
  socket. WebKit Networking process socket count stays flat under
  100 instead of climbing toward 256.
- **Idle connection:** if a client opens a connection and never
  sends a request, the daemon closes it after 60 s.
- **Explicit close request:** `Connection: close` header → daemon
  serves the request normally, then closes.
- **WS upgrade:** unchanged user-visibly. WS handlers now borrow the
  stream; after a WS handoff the dispatch loop exits because the
  upgraded connection has no notion of a "next request."
- **Auth failure / 4xx / 5xx:** dispatcher closes the socket (the
  safe default — if a client is sending broken requests we don't
  amplify the problem by keeping the connection alive).

## Tested

- **`request_wants_close` parser** — 7 new inline unit tests in
  `http.rs` covering header-absence, explicit close, keep-alive,
  case-insensitive matching (RFC 9110 §5.6), multi-token
  `Connection: keep-alive, close` lists, unrelated-header false
  matches, and empty-value handling.
- **193 existing daemon tests still pass** — verifies the dispatcher
  refactor + WS handler signature changes didn't regress anything.
- **Live curl smoke test** confirms keep-alive end-to-end:
  ```
  req 1: status=200 num_connects=1   ← new TCP socket
  req 2: status=200 num_connects=0   ← REUSED
  req 3: status=200 num_connects=0   ← REUSED
  ```
  Pre-0.39.7 each line would have shown `num_connects=1`.

## What's NOT in this release (deferred to 0.40.x)

The reporter's `Out of scope` follow-ups stand on their own:

- Daemon `[perf] terminal_poll_tick` logs ~3×/sec → unbounded
  `daemon.stderr.log` growth. Should be gated behind a debug flag.
- Phantom `nobody` pending-live signals re-queued every boot but
  never drained.
- Push more renderer polls into the existing `session-events` WS
  channel. Even with keep-alive shipping, fewer HTTP polls is
  always better — and the WS channel is the K2 Connect direction
  anyway.

## Acceptance criteria status (from the issue)

> A user can run K2SO continuously for 4+ hours with active agent
> terminals, without observing `loading…` indicators visibly
> lengthen, and `lsof -nP -iTCP:<daemon-port>` stays well below 100
> sockets total.

The architectural change directly addresses the leak source. The
4-hour wall-clock validation will come from real-user reports on
the released build; the curl smoke proves connection reuse works
under the actual dispatcher.

## Upgrade notes

- Any 0.39.x → 0.39.7: clean update. No migrations. ConnectionGate
  + boot-status handshake from 0.39.5 cover the upgrade race.
- Users on 0.38.x → 0.39.7: full 0.39.0 + 0.39.1 migration sequence
  fires on first boot, gated behind the 0.39.5 readiness handshake.

## Credit

PR-equivalent diagnosis from an external user (filed as Issue #2)
who profiled the live process with `sample`, `lsof`, and CPU
attribution — confirmed the daemon was idle (a victim of
backpressure) and that the renderer's `WebKit::ResourceRequest`
churn matched the periodic fetch fan-out. Methodology to emulate
for future renderer-perf bugs.
