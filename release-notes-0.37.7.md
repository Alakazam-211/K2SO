## What changed

Smooths the install-relaunch race that surfaced "ws error" on the user's terminal until they manually right-click → reload.

## The race

1. Auto-update / installer completes, K2SO relaunches.
2. Renderer mounts and starts opening WebSockets to the daemon.
3. Daemon process hasn't yet finished binding its port (or hasn't written the credentials file the renderer reads on first connect).
4. WS open fails → renderer flips the tab into `kind: 'error'` with message `'ws error'`.
5. Daemon comes up a moment later (typically 200-700ms after the renderer's first attempt).
6. User had to know to right-click the tab and pick "Reload" to recover.

## The fix

The v2 grid WebSocket open is now wrapped in a retry-with-backoff loop, mirroring the existing pattern the spawn-fetch path uses:

```
WS_BOOT_DEADLINE_MS = 8_000

attempt 1 → fail → wait 250ms
attempt 2 → fail → wait 500ms
attempt 3 → fail → wait 1000ms
attempt 4 → fail → wait 2000ms
attempt 5 → fail → wait 2000ms (capped)
... up to the 8s deadline
```

Most install-relaunch races resolve in 1-2 retries (~250-750ms total — invisible to the user; no error flash before the connection settles). If the daemon really is unreachable past the deadline, the existing `ws error` surface fires so the user knows something's actually wrong — but a transient race no longer bubbles up.

## Implementation note

The `ws.onopen` perf log moved into the connect-retry loop's success branch — setting `onopen` on an already-open socket would never fire because the browser already dispatched the event during the connect race. The new perf log line `ws_open` includes the `attempts` count so we can see in the perf trace whether the user hit a retry on first launch or got through cleanly on attempt 1.

## Scope

v2 only. Kessel-side WebSockets (legacy renderer) keep their existing single-attempt connect; users on Kessel can still right-click reload if the race bites them. Migrating Kessel to v2 is the long-term path; once that's done this fix carries over.

## Tests

756 passing — same as 0.37.6. The change is renderer-only; existing Rust test surface unaffected.
