---
title: "Bug: Companion ngrok tunnel drops connection, toggle stays enabled"
priority: high
assigned_by: external
created: 2026-04-09
type: bug
source: manual
---

## Description

Two related issues with the Mobile Companion feature:

### 1. ngrok tunnel drops shortly after connecting

When enabling the companion, the status shows "Connected" briefly but then the tunnel goes offline (ngrok returns `ERR_NGROK_3200 — endpoint is offline`). The local `TcpListener` that the companion proxy binds is not receiving forwarded traffic from ngrok.

**Likely cause:** In `companion/mod.rs`, the `start_ngrok_tunnel()` function uses a tokio runtime to create the ngrok session and tunnel. The tunnel is kept alive by a spawned tokio task that just sleeps in a loop. However, the `tokio::runtime::Runtime` is created inside `start_ngrok_tunnel()` with `rt.block_on()` — once `block_on` returns with the URL, the runtime may be getting dropped, which kills the spawned task and closes the ngrok connection.

```rust
// companion/mod.rs lines 174-209
fn start_ngrok_tunnel(local_port: u16, ngrok_token: &str) -> Result<String, String> {
    let rt = tokio::runtime::Runtime::new()...;
    let url = rt.block_on(async move {
        // ... connect, create tunnel ...
        tokio::spawn(async move {
            loop { tokio::time::sleep(...).await; }  // keeps tunnel alive
        });
        Ok(url)
    })?;
    Ok(url)
    // ← rt is dropped here, killing the spawned keep-alive task
}
```

**Fix:** The `Runtime` (or at least the tunnel handle) needs to be stored in `CompanionState` so it lives as long as the companion is running. Alternatively, use Tauri's existing async runtime instead of creating a new one.

Additionally, the ngrok SDK's `forwards_to()` only sets metadata — it doesn't actually proxy traffic. The ngrok Rust SDK requires you to `accept()` incoming connections from the tunnel and manually forward them to the local port. The current code creates the tunnel but never accepts connections from it — the `TcpListener` on the local port never receives traffic because ngrok has no way to reach it.

**The fix should either:**
- Accept connections from the ngrok tunnel object and forward bytes to/from the local `TcpListener`, OR
- Skip the separate `TcpListener` entirely and handle requests directly from the ngrok tunnel's accepted connections

### 2. Toggle stays enabled when tunnel disconnects

When the companion tunnel drops or fails, the "Enable Companion" toggle in the settings UI stays in the "on" position. The status text changes (no longer shows "Connected") but the toggle doesn't reflect the actual state.

**Expected behavior:** If the tunnel drops unexpectedly, either:
- Auto-reconnect and show a "Reconnecting..." status, OR
- Turn off the toggle and show the disconnected state clearly

**Current behavior:** Toggle stays on, status shows stale/blank state, user has to manually toggle off then on to retry.

## Steps to Reproduce

1. Open K2SO Settings → Mobile Companion
2. Set username, password, ngrok auth token
3. Enable the toggle
4. Observe: status shows "Connected (0 clients)" briefly
5. Wait a few seconds — the tunnel goes offline
6. Try to curl the ngrok URL: returns `ERR_NGROK_3200` (endpoint offline)
7. The toggle in the UI still shows enabled

## Environment

- K2SO version: current main branch
- ngrok free tier account
- Tested on different network connections — same behavior

## Acceptance Criteria

- [ ] Companion tunnel stays connected persistently after enabling
- [ ] Companion API endpoints respond through the ngrok URL
- [ ] `POST /companion/auth` with valid Basic Auth credentials returns a session token
- [ ] Toggle reflects actual tunnel state — turns off if tunnel drops
- [ ] Auto-reconnect on transient network failures with "Reconnecting..." status
- [ ] The tokio runtime / ngrok tunnel handle is stored for the lifetime of the companion session

## Related

- The K2SO-companion mobile app is ready and waiting to connect: `/Users/z3thon/DevProjects/Alakazam Labs/K2SO-companion/`
- Server-side companion API spec: `.k2so/work/inbox/feature-mobile-companion-server-side-support.md`
