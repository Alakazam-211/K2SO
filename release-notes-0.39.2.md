# K2SO 0.39.2 — ConnectionGate: render after daemon healthy

Patch release. One thing.

## What this fixes

On auto-update, the old daemon process stays in memory until launchctl
kickstart cycles it. The new K2SO.app launches, React mounts, and the
renderer's initial fetches race the daemon restart — fetches fail
silently and React mounts against empty/stale data. Visually presents
as a blank app until the user right-clicks → Reload.

0.39.2 fixes this with a renderer-side `ConnectionGate` component
that wraps the app's mount path. It polls the daemon's `/ping`
endpoint with a 500ms retry loop and a 2s per-attempt timeout. While
waiting, it shows a "Connecting…" overlay. Once `/ping` succeeds,
it unmounts the overlay and renders the actual app — every store's
initial fetch then runs against a healthy daemon, no race.

## Why this design (instead of Rust-side wait + reload)

Tried the Rust-side approach first (wait for daemon-healthy, eval-
reload webview), but the renderer-side gate is architecturally
better:

1. **Doesn't assume daemon will come up.** If something genuinely
   broken keeps the daemon from starting (corrupt config, port
   conflict, launchctl misconfiguration), the gate surfaces a
   `Reload` button after ~30s instead of leaving Tauri spinning
   forever waiting for an event that never fires.

2. **User-visible feedback.** "Connecting…" overlay tells the user
   the app is working on it; blank screen feels broken.

3. **Reusable for K2 Connect.** When the desktop connects to a
   remote daemon over a tunnel (planned for 0.40.0), the same
   "retry until reachable, show progress while we wait" pattern
   applies — transient network blips, tunnel reconnects, daemon
   restarts on the remote machine. ConnectionGate becomes the
   single primitive for "we want to mount, but the data source
   isn't there yet."

4. **Survives runtime daemon crashes.** A future iteration can
   detect mid-session connection loss and re-engage the gate
   instead of leaving the app in a stuck-and-doesn't-know-it
   state.

## Behaviour summary

- **Happy path** (daemon already healthy on first launch): `/ping`
  succeeds on first try, overlay never paints (or paints for
  <100ms). Imperceptible to user.
- **Auto-update path** (daemon restarting): `/ping` fails for 1-3s,
  gate shows "Connecting…", mounts the app the moment `/ping`
  succeeds. No blank screen, no manual reload.
- **Permanent-failure path** (daemon won't come up): after ~30s of
  failed polls, a `Reload` button appears. User can recover instead
  of staring at an infinite spinner.

## Implementation

- **New file**: `src/renderer/components/ConnectionGate.tsx` (~140
  lines including overlay UI + retry loop)
- **Modified**: `src/renderer/index.tsx` — wraps `<App />` in
  `<ConnectionGate>`
- **Reverted from earlier 0.39.2 attempt**: `src-tauri/src/lib.rs`
  back to its pre-attempt state. The Rust-side `check_daemon_
  version_and_restart` still triggers the daemon kickstart on
  version mismatch (unchanged from 0.39.0); the gate just handles
  the "wait until daemon's back" UX in React.

## Upgrade notes

- Users on 0.39.1 → 0.39.2: clean update; no migration needed.
- Users on 0.39.0 → 0.39.2: corrective auto-pin migration from
  0.39.1 fires on first boot (unpins over-pinned manager workspaces),
  then never again.
- Users on 0.38.x → 0.39.2: full migration sequence from 0.39.0 +
  0.39.1 fires on first boot.

## What else shipped in this release

Nothing else. See `release-notes-0.39.0.md` and `release-notes-0.39.1.md`
for those versions.
