---
title: "Bug: Companion API drops after first request, restart fails with muxado error"
priority: high
assigned_by: K2SO:manager
created: 2026-04-10
type: bug
source: manual
---

## Description

Two related issues with the companion API after the crash fix (v0.28.3):

### 1. Tunnel drops after first HTTP request

Auth (POST /companion/auth) succeeds and returns a token. But the next request (GET /companion/agents) returns ngrok 503 (ERR_NGROK_3004 — invalid/incomplete response). The tunnel goes offline after handling one request.

**Likely cause:** The async accept loop in `run_ngrok_listener` reads one connection, writes a response with `stream.shutdown().await`, and the ngrok tunnel interprets the stream shutdown as the endpoint going offline. The `try_next().await` then returns `None` (tunnel closed) or errors on the next iteration.

**The `stream.shutdown()` call may be the culprit** — it closes the underlying TCP connection which ngrok interprets as the endpoint dying. Need to either:
- Not call shutdown (let ngrok handle connection lifecycle)
- Or handle each connection in a spawned task so the main accept loop keeps running

### 2. Restart fails with "muxado stream" error

After stop → start, the reused ngrok Session fails with "failed to open muxado stream". The session object is stale after the listener thread exits.

**Fix:** Don't reuse sessions across stop/start cycles. Create a fresh session on each start. Handle the one-session-per-account limit by ensuring the old session is fully closed before creating a new one.

## Steps to Reproduce

```bash
k2so companion start                    # Works
curl -X POST $URL/companion/auth ...    # Works (200)
curl $URL/companion/agents?token=...    # Fails (503)
k2so companion stop                     # Works
k2so companion start                    # Fails (muxado error)
```

## Acceptance Criteria

- [ ] Multiple sequential HTTP requests work through the tunnel
- [ ] Auth → agents list → status → terminal read all succeed in sequence
- [ ] Stop → start cycle works reliably
- [ ] App doesn't crash under any companion failure scenario
