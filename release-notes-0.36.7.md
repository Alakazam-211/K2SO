# K2SO 0.36.7 — Alacritty (v2) is the new default for fresh installs

The daemon-hosted Alacritty renderer (the one that survives Tauri quit and supports heartbeat continuity) is now the default for fresh installs. The legacy in-Tauri renderer remains available — labeled "Alacritty (Legacy)" in Settings → Terminal — for users who relied on the older lifecycle, but it's no longer what new users land on out of the box.

## What's new

### Default renderer is now `alacritty-v2`

Fresh installs of 0.36.7+ open new terminal tabs against the daemon-hosted v2 renderer by default. Practical effects:

- Terminal sessions survive K2SO quit. The next time you open the app, the daemon already has the PTY running; the tab just reattaches.
- Heartbeats can target the session — wake-injects work the same way they do for Claude Code chats today.
- The PTY is shared between Tauri and the mobile companion (when that ships), so you can pick up a session on another device.

The A1–A5 phase plan from `.k2so/prds/alacritty-v2.md` landed across 0.34–0.36; v2 is now production-hardened.

### Existing users keep their choice

Zustand's persist middleware means existing users' renderer preference is preserved on upgrade. If you previously picked "Alacritty (Legacy)" or "Kessel (BETA)" — or never touched the dropdown and were on the old default — your selection sticks. Only installs with no prior K2SO state see the new default.

### UI labels and hints refreshed

The Settings → Terminal → "Terminal Renderer" dropdown still shows all three options ("Alacritty", "Alacritty (Legacy)", "Kessel (BETA)"), but the surrounding tooltip and search-manifest description now reflect v2's status as the new default. The "while v2 finishes baking" wording is gone.

## Internals

- `src/renderer/stores/terminal-settings.ts` — default flipped + comment block refreshed.
- `src/renderer/stores/tabs.ts` — `SerializedItem.renderer` type extended to include `'alacritty-v2'`. This was a long-standing TS warning where the runtime value (which K2SO already wrote to disk for users who opted in) didn't match the static type. With v2 now the default, every new install will be persisting this value, so the type needed to be honest.
- `src/renderer/components/Settings/sections/TerminalSection.tsx` — comment, tooltip, and search keywords aligned with the new default.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
