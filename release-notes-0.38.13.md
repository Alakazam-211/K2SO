# 0.38.13 — Launch perf cleanup + smarter memory threshold

Three small fixes addressing the user-reported regressions in 0.38.12:

## What changed

### Faster launch (popup retry stops blocking Tauri worker thread)

The 0.38.7–0.38.12 `whats_new_check` Tauri command had a 10×500ms
synchronous retry loop inside the Rust function for the daemon-launch
race. That worked, but it consumed a Tauri worker thread for up to
5 seconds at startup, contending with workspace hydration, daemon
version-check, agent state loading, and every other mount-time
`invoke()`. Even on a fast machine where the daemon was ready
immediately, the worker-pool contention was real.

0.38.13:

- Rust `whats_new_check` is now single-shot. Returns instantly if the
  daemon is reachable; returns Err otherwise.
- The renderer (`WhatsNewModal`) handles retry via `setTimeout`, which
  yields to the React event loop between attempts. Same 10×500ms
  budget, zero worker-thread blocking.
- The popup's first check is **deferred by 2 seconds** after mount
  so it no longer competes with other boot-time work. The popup
  appears as a clear post-boot event rather than a stampede contributor.

Net effect: cold-launch perceived latency drops, the rest of the
renderer paints faster, and the popup still shows reliably (just
2 seconds later than before — past the boot window).

### Smarter memory watcher threshold

0.38.12's 800 MB absolute threshold was firing immediately on launch
because the local LLM loads ~1+ GB of weights into the Tauri process
address space at boot. The watcher correctly measured RSS, but the
threshold incorrectly flagged "LLM steady-state" as "leak."

0.38.13 changes the watcher to be **growth-aware**:

1. **Skip sample #1** (LLM may still be loading early in boot).
2. **Capture baseline at sample #2** (after the LLM has settled into
   steady-state).
3. **Warn on +800 MB above baseline** OR **3 GB absolute ceiling**
   (whichever fires first). Either signals a real leak; LLM
   steady-state is silent.

Console log line gains a `baseline=... growth=...` suffix so the
trend is visible even when no toast fires:

```
[k2so/memory] pid=12345 rss=1452MB vsize=...MB baseline=1340MB growth=112MB
```

## Files touched

| File | Change |
|---|---|
| `src-tauri/src/commands/whats_new.rs` | Drop 5s retry loop; single-shot check. Renderer owns retry. |
| `src/renderer/components/WhatsNewModal/WhatsNewModal.tsx` | JS-side 10×500ms retry; 2s mount-time defer for `auto` mode |
| `src/renderer/components/MemoryWatcher/MemoryWatcher.tsx` | Baseline-aware growth detection; absolute ceiling at 3 GB |
| `WHATS_NEW.md` | 0.38.13 entry |
| `release-notes-0.38.13.md` | (this file) |

## Architecture note (sanity check during this work)

K2SO remains daemon-first. All business logic (sessions, agents, msg
delivery, scheduling, popup state, memory measurement, file watchers,
heartbeats) lives in `k2so-daemon` and `k2so-core`. The renderer is
genuinely thin — it reads state via Tauri commands that proxy to the
daemon's HTTP API, paints UI, and sends user actions back.

The boot-perf regression isn't "the renderer is doing daemon work."
It's "the renderer has accumulated more UI work" — bigger component
tree, more dialogs at root, more useEffects, more parallel
`invoke()` calls on mount. The fix isn't to push work to the daemon
— it's renderer hygiene: defer non-essential boot work, lazy-load
heavy components, stagger initial `invoke()` calls.

0.38.13 is the immediate-impact slice of that hygiene work. A broader
renderer boot-perf audit (lazy-loading react-markdown / codemirror /
monaco, splitting the WhatsNewModal bundle, stagger of mount-time
`invoke()`s) belongs in a future release.

## Smoke

`cargo build -p k2so`: clean (101 pre-existing warnings).
The Rust `whats_new_check` simplification removes 5 lines of retry
logic. The renderer-side retry is functionally equivalent in
worst-case behavior (10 attempts × 500ms = 5 seconds), but uses
`setTimeout` which yields the event loop instead of blocking a
worker thread.
