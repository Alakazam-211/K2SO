# K2SO 0.35.5 — Hotfix: v2 panes survive auto-update relaunch

Quick follow-up to **0.35.4**: every "install and relaunch" was
breaking your v2 terminals.

## What you saw

After clicking "install and relaunch" on the auto-updater, every
v2 (Alacritty) pane that was open before the update would remount
into an error state:

> Alacritty v2: spawn fetch failed: TypeError: Load failed

Legacy panes were unaffected. Closing + reopening the tab worked
around it, but it was a real disruption to flow — exactly the
opposite of what we want from auto-update.

## Why it happened

v2 panes spawn over HTTP against the daemon's
`/cli/sessions/v2/spawn` endpoint. Legacy panes spawn in-process
via Tauri IPC and don't talk to the daemon over a socket at all.

When the auto-updater installs and relaunches:

1. The new Tauri binary boots immediately.
2. The renderer mounts → v2 panes fire spawn fetches **right away**.
3. The k2so-daemon process is still running the **old** binary.
4. Tauri's version-mismatch handshake (landed in 0.35.0) detects
   the drift and kicks the daemon to restart with the new binary.
5. There's a **~2–5 second window** where the daemon's HTTP socket
   is closed (it's launching). The spawn fetches in step 2 land
   in that window and fail with `TypeError: Load failed` — the
   browser's standard signal for "connection refused."

Legacy panes never hit step 2; they boot through Tauri commands
locally and survive transparently.

## Fix

`src/renderer/terminal-v2/TerminalPane.tsx` now retries the boot
sequence (creds resolve + spawn fetch) for up to 10 seconds with
exponential backoff (250 ms → 500 ms → 1 s → 2 s, capped at 2 s).

- **Network errors** (`TypeError: Load failed`, connection
  refused) → retry, daemon is restarting.
- **HTTP 5xx** → retry, daemon answered but is mid-init.
- **HTTP 4xx** → surface immediately, this is a real request
  error (missing field, bad body) and won't get better.
- **Deadline exceeded** → surface the error after 10 s with the
  elapsed time in the message so it's clear why the wait was long.

Each retry invalidates the daemon-creds cache (so a daemon that
restarted on a new port is picked up fresh) and updates the
perf-log breadcrumb (`spawn_retry attempt=N delay_ms=X
elapsed_ms=Y`) so future debugging has a trail.

The legacy renderer's behavior is unchanged (it doesn't share
this code path).

## Why this couldn't be unit-tested

The bug only reproduces during the live update flow: a fresh
binary boots, the daemon is mid-restart, the renderer's
useEffect fires within the gap. There's no good way to simulate
that without a real daemon-restart cycle. The fix has been hand-
checked against the failure modes by inspection, but its real
test is the next install — which is exactly the situation it's
trying to fix.

## Upgrade

This is the first install after 0.35.4 where the new behavior
applies. Future updates should be transparent (loading spinner
for ~3-5s while the daemon restarts, then the panes attach).
