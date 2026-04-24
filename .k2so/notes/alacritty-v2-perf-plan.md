# A7.5 — Alacritty_v2 Launch-Time Instrumentation + Optimization Plan

**Status:** Planned. To execute after context compaction.
**Goal:** Measure every stage from Cmd+T → "terminal is live on screen," then shrink the total.
**Baseline:** `alacritty-v2-a7-complete` tag.

## The full critical path (what we need to measure)

Cmd+T → tab created → TerminalPane mount → HTTP spawn → WS open → first snapshot → DOM paint.
Everything should log a tagged line so `grep '\[v2-perf\]'` in devtools + `tail -f ~/.k2so/*.log` tells the whole story.

## Frontend timing (TerminalPane.tsx)

Capture a `mountT0` at the first render, then log deltas:

| Stage | Log line | What it measures |
|---|---|---|
| `mount` | `[v2-perf] t=0 stage=mount pane=<tabId>` | React reached component body |
| `creds_start` | `[v2-perf] t=Nms stage=creds_start` | before `getDaemonWs()` |
| `creds_end` | `[v2-perf] t=Nms stage=creds_end cached=<bool>` | after port+token resolved (indicates cache warmth) |
| `spawn_fetch_start` | `[v2-perf] t=Nms stage=spawn_fetch_start` | POST /cli/sessions/v2/spawn |
| `spawn_fetch_end` | `[v2-perf] t=Nms stage=spawn_fetch_end reused=<bool> sid=<uuid>` | got response |
| `ws_opening` | `[v2-perf] t=Nms stage=ws_opening` | `new WebSocket(...)` |
| `ws_open` | `[v2-perf] t=Nms stage=ws_open` | `ws.onopen` fired |
| `first_snapshot` | `[v2-perf] t=Nms stage=first_snapshot rows=N cols=N empty=<bool>` | initial snapshot message received; flag whether the grid had any non-blank cells |
| `first_render` | `[v2-perf] t=Nms stage=first_render` | `useEffect` after `setSnapshot` fired (DOM committed — Term is "live" but may still be blank because the child hasn't painted yet) |
| `tui_first_paint` | `[v2-perf] t=Nms stage=tui_first_paint` | first subsequent snapshot/delta where the child process has actually written content (e.g., bash prompt appears, Claude banner renders). Separate from `first_render` because the child's startup latency (~20-100ms) is a different category. |
| `summary` | `[v2-perf] t=Nms stage=summary total=Nms breakdown={...}` | one-shot summary at first_render |
| `tui_summary` | `[v2-perf] t=Nms stage=tui_summary` | one-shot summary at tui_first_paint, with the child-startup stage broken out |

Add a `spawnedAt` prop capture at Cmd+T (already exists in props) so we can also log the **user-perceived** time from keystroke to on-screen.

## Daemon timing (already has `log_debug!` helpers)

Emit matching `[v2-perf] side=daemon ...` lines so the two streams interleave cleanly.

### `v2_spawn.rs`
- `spawn_handler_enter` — POST arrived.
- `spawn_map_lookup_done` — session_map checked (existing vs spawn).
- `spawn_pty_new_start` / `spawn_pty_new_end` — `DaemonPtySession::spawn()` duration.
- `spawn_response_sent`

### `daemon_pty.rs::spawn`
- `pty_open_start` / `pty_open_end` — `tty::new()` cost.
- `term_new_done` — `Term::new()`.
- `event_loop_start` — `event_loop.spawn()`.

### `sessions_grid_ws.rs`
- `ws_accept_start` / `ws_accept_end` — WS handshake.
- `ws_events_subscribed`
- `ws_first_snapshot_start` / `ws_first_snapshot_end size=N` — initial snapshot build + serialize.
- `ws_first_snapshot_sent`

## Summary format

On first-render in Tauri, print ONE line with the full breakdown:
```
[v2-perf] SUMMARY total=247ms | mount=0 creds=4 spawn_fetch=82 ws_open=12 first_snap=38 render=16
```

And in the daemon log, the matching server-side breakdown for the spawn handler.

## Likely win candidates (review list)

Once we see real numbers, the usual suspects:

1. **Cold `reqwest` / HTTP client pool** — does the first POST pay setup cost? Check `kessel_warm_http` usage + whether v2's fetch-based path benefits from it.
2. **`getDaemonWs()` cache miss** — first call hits disk for heartbeat.port / heartbeat.token. `prewarmDaemonWs()` is already called at app mount; verify it's actually running for v2 users.
3. **`tty::new()` fork cost** — posix_spawn + setsid + setctty. Not much to do here except measure.
4. **`Term::new()` + scrollback allocation** — `SCROLLBACK_CAP=5000` rows preallocated. Could lazy-init.
5. **WS handshake round trip** — localhost should be <5ms but verify.
6. **First snapshot serialization** — empty Term = cheap. Resumed-session reattach = larger payload. Measure both paths.
7. **React state + DOM paint** — the first `setSnapshot` triggers a big render. RAF throttle from the original A7.5 scope may help more on delta path than first render.
8. **Child process startup** — bash/zsh takes ~20-50ms to print its prompt. This is outside our control but worth noting in the summary so we don't chase it.

## Three scenarios to measure (each has its own critical path)

### Scenario A — Cold spawn (fresh Cmd+T, no existing session)
Path: mount → creds → spawn → pty-fork → child-start → ws → first_snapshot (empty) → first_render → **child paints prompt** → tui_first_paint.
Dominant stages likely: pty-fork, child startup, first HTTP (if daemon was cold).

### Scenario B — Warm spawn (second Cmd+T right after the first)
Path: same as A, but creds cached + reqwest/HTTP pool warm + daemon already running.
Dominant stages likely: pty-fork + child startup. Everything else should be <10ms.

### Scenario C — Reattach (workspace swap or Tauri window reopen)
Path: mount → creds → spawn (find-or-spawn returns reused session) → ws → first_snapshot (populated with existing grid + scrollback) → first_render (TUI content visible immediately because daemon already had it).
Dominant stages likely: snapshot serialization if scrollback is large, WS handshake.

**Important distinction: Scenario C has no `tui_first_paint` gap** — the daemon's Term already has content, so `first_render` IS the TUI paint. Log both timings and expect them to collapse to ~0 apart in this case. If they don't, something else is wrong.

Separate "time-to-connect" log for Scenario C so we can see reattach as its own number:
```
[v2-perf] CONNECT-SUMMARY scenario=reattach connect_ms=Nms snapshot_size=Nbytes
```

## Execution order

1. **Add the instrumentation** (no behavior change).
2. **Collect Scenario A baseline**: kill the daemon, restart it, open one tab.
3. **Collect Scenario B baseline**: close that tab, open another immediately.
4. **Collect Scenario C baseline**: workspace-swap away + back.
5. **For each scenario**, identify the top 1-2 dominant stages.
6. **Implement fixes one at a time**, measure after each.
7. **Stop when we hit diminishing returns**. Target envelopes:
   - Scenario A (cold): total < 150ms to first_render, < 250ms to tui_first_paint.
   - Scenario B (warm): total < 50ms to first_render, < 120ms to tui_first_paint.
   - Scenario C (reattach): total < 30ms to first_render (no tui_first_paint gap).

Don't pre-optimize — measure first. Every fix should be gated on "this stage was > 20ms in the baseline."

## Constraints

- No behavior regressions. The UX parity checklist from A7 still passes at every step.
- Instrumentation stays off hot-path in release — gate behind `cfg!(debug_assertions)` on the daemon side and behind `import.meta.env.DEV` on the frontend.
- One optimization per commit so bisect works if something regresses later.
