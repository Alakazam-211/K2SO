# K2SO 0.39.11 — Webview liveness watchdog (Issue #6) + "Download" button label

Bundles the two commits on `main` since 0.39.10: `977a58c5`
(launch-only watchdog) and `0b2f4cac` (persistent heartbeat redesign +
button label). No daemon-protocol changes; the watchdog is entirely in
the Tauri shell.

## 1. Webview liveness watchdog — recover the black-screen window (Issue #6)

### The bug
K2SO could land in a black, unresponsive window where the renderer JS
isn't running, in two situations:
1. **At launch** (esp. after an auto-update over the running process):
   WKWebView loads `index.html` but never executes the JS bundle.
2. **Mid-session**: the WKWebView content process dies and respawns
   blank — most commonly when the laptop **sleeps and wakes** (the
   case a user actually reported: "renderer crashed after my computer
   took a nap").

In both, right-click → Reload fixes it — but ordinary users won't
know to do that, so the app reads as broken.

### Why nothing already shipped caught it
This is **upstream** of every 0.39.2–0.39.5 black-screen defense
(ConnectionGate, dynamic-import, the `/boot-status` handshake,
LocalPaired policy) — they all live in the renderer JS and cannot fire
when the renderer JS itself isn't running. Recovery has to come from
Rust.

### The fix — a persistent renderer heartbeat
- **`src/renderer/index.tsx`**: calls a `renderer_heartbeat` command
  the instant the bundle executes, then every ~3s.
- **`src-tauri/src/lib.rs`**: tracks the last heartbeat in
  `LAST_HEARTBEAT_MS` (wall-clock `SystemTime`, so a long sleep counts
  as elapsed time). A persistent watchdog thread ticks every 3s; if no
  heartbeat within `STALE_MS` (9s — tolerates ~2 missed beats of
  ordinary jank) for `MIN_STALE_STREAK` (2) consecutive ticks (which
  kills the sleep/wake race where a surviving renderer resumes a beat
  late), it reloads the webview (`win.eval("location.reload()")` — the
  programmatic equivalent of the proven manual reload) up to
  `MAX_RELOADS` (3), then shows a native error sheet +
  `~/.k2so/webview-watchdog.log` breadcrumb **once per episode**.
  Whenever a heartbeat resumes it **re-arms** — so it recovers launch
  failures AND later mid-session crashes, and never reloads a healthy
  window.
- The decision logic is a pure `watchdog_decision()` helper
  (`Healthy` / `Watch` / `Reload` / `GiveUp`) so the staleness +
  confirm-streak + retry-cap is unit-testable without a real WKWebView.

### Stays a thin client (K2 Connect-aligned)
This is pure OS/webview-shell integration — per-client and
daemon-agnostic. The heartbeat is renderer→its-own-shell, never a
daemon round-trip. A Tauri client pointed at a *remote* daemon guards
its own window the same way; the remote daemon neither knows nor cares.
Same client-side shape as the ConnectionGate work.

## 2. "Download" button label

Settings → General update button: **"Download & Install" → "Download"**.
The install is the separate "Install & Relaunch" button, so the old
label was misleading.

## Tested

- **6 unit tests** on `watchdog_decision` (healthy-resets, watch-below-
  streak / sleep-wake guard, reload-with-budget, give-up-at-cap, the
  full `Watch×2 → Reload×3 → GiveUp` episode, and recovery-reset).
- **k2so (tauri) lib: 55 passed, 0 failed.** `cargo check`: 0 errors.
- **Full workspace sweep** (pre-release): k2so-core 665/0, k2so-daemon
  199/0, renderer vitest 42/42, renderer typecheck baseline unchanged
  (49), renderer build succeeds.
- **Real-build validation**: ran the packaged build with stderr
  captured across **two genuine system sleeps** — the app stayed
  healthy, the watchdog never gave up (no breadcrumb, no error sheet),
  and it did not spuriously reload a working window. The short (~30s,
  on-AC) sleeps did not reproduce the intermittent content-process
  crash, so the recovery path itself was exercised by the unit tests
  rather than the live nap; the design is low-risk by construction
  (launch-only side effects are bounded, happy path is a no-op).

## Upgrade notes

- Any 0.39.x → 0.39.11: clean update. No migrations.
- Users on 0.38.x → 0.39.11: full 0.39.0 + 0.39.1 migration sequence
  fires on first boot, gated behind the 0.39.5 `/boot-status` handshake.

## What else shipped in this release

Nothing else. See `release-notes-0.39.0.md` through
`release-notes-0.39.10.md` for prior content.
