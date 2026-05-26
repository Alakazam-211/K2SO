# K2SO 0.39.3 — ConnectionGate: dynamic-import + black-screen fix

Patch release. Fixes the black-screen-on-install bug that 0.39.2's
ConnectionGate didn't fully address.

## What this fixes

0.39.2 added `ConnectionGate` to wait for daemon healthy before
mounting `<App />`. The render-gating worked — but the bug
persisted because `<App />` was still **imported** at startup by
`index.tsx` (`import App from './App'`).

`<App />` transitively imports a long list of Zustand stores
(`projects`, `tabs`, `settings`, `focus-groups`, `timer`, `assistant`,
`panels`, …). Several of those stores fire eager daemon fetches **at
module-init time**, NOT at component mount time. Order of events
in the install + relaunch scenario:

1. `index.tsx` runs → statically imports `App.tsx`
2. `App.tsx` module-init → imports stores
3. Stores fire `loadFromDaemon()` / `fetchProjects()` / `initFromSettings()` calls
4. Daemon is still kickstarting → all fetches fail with `TypeError: Load failed`
5. Stores end up in stuck/failed state, no retry
6. ConnectionGate is showing "Connecting…" through all of this
7. Eventually daemon comes up → gate dismisses → `<App />` mounts
8. App reads from stores → stores return empty/failed data → **nothing visible renders** → black screen

Confirmed via dev-mode devtools console: cascade of
`Load failed` errors from `tabs.ts:2384`, `projects.ts:122`,
`focus-groups.ts:170`, `settings.ts:231` (Unhandled Promise
Rejection), `timer.ts:122`, `assistant.ts:100` — all firing
before the gate ever transitioned.

## The fix

**Dynamic import** of `<App />`. `index.tsx` no longer imports
`App.tsx` at startup. ConnectionGate uses
`import('../App')` AFTER `/ping` succeeds — so App.tsx + stores
enter the JS context only when the daemon is verified healthy.
Stores fire their initial fetches against a known-healthy daemon,
no race, no stuck state.

### Code changes

- **`src/renderer/components/ConnectionGate.tsx`**: now drives both
  phases — Phase 1 polls `/ping` until reachable; Phase 2
  dynamically imports `./App` then mounts it. Gate keeps showing
  the "Connecting…" overlay through both phases.
- **`src/renderer/index.tsx`**: removed `import App from './App'`.
  Renders `<ConnectionGate />` only (no children prop — the gate
  imports + mounts App internally).

### UI tweaks

- Kept "Connecting…" overlay copy
- Removed attempt counter (no more "Attempt N+1" clutter)
- Reload button now appears after **20 attempts (~10s)** instead of
  30s — faster recovery if something's actually stuck
- Reload button copy now reads: "Your K2SO daemon may still be
  loading. If you're unsure, quit and relaunch the app, or try
  reloading with the button below."

## Tested

Reproduced the bug in dev mode (kill daemon → launch `bun run
tauri dev`). DevTools console captured the exact cascade of
`Load failed` errors from store-init fetches — verifying the
diagnosis. Applied the dynamic-import fix → reloaded → "Connecting…"
overlay showed cleanly with NO store-init errors in console → loaded
daemon → gate detected healthy → App imported + mounted cleanly →
stores fetched against healthy daemon → render succeeded.

## Why this matters beyond 0.39.x

The dynamic-import pattern is reusable for **K2 Connect** (0.40.0):
when the desktop connects to a remote daemon over a tunnel, the
same race exists (remote daemon might be momentarily unreachable).
ConnectionGate works there too — just parameterize the source URL.
The store-import gating remains correct.

## Upgrade notes

- Users on 0.39.2 → 0.39.3: clean update. First launch should
  render cleanly without the black-screen pause.
- Users on 0.39.1 → 0.39.3 (skipping 0.39.2): corrective auto-pin
  migration from 0.39.1 still fires on first boot (unpins
  over-pinned manager workspaces), then never again.
- Users on 0.38.x → 0.39.3: full migration sequence from 0.39.0 +
  0.39.1 fires on first boot.

## What else shipped in this release

Nothing else. See `release-notes-0.39.0.md`, `release-notes-0.39.1.md`,
and `release-notes-0.39.2.md` for prior content.
