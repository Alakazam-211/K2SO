# 0.38.4 — Heartbeats panel polish

Two small visual fixes to the system-wide Heartbeats settings page
that shipped in 0.38.3.

## Changes

- **Themed "Pinned chat" checkbox.** The native `<input type="checkbox">`
  rendered with the OS default blue chrome that clashed with K2SO's
  dark + accent palette. Replaced with a custom-styled square that
  uses `--color-border` / `--color-bg-elevated` for the unchecked state
  and `--color-accent` for the checked state, with an SVG checkmark
  inside. The native input is still the source of truth (sr-only)
  for accessibility + keyboard nav.
- **Case-insensitive alphabetical sort.** The middle column's heartbeat
  list was already ordered by the SQL `ORDER BY p.name, h.name`, but
  default SQLite collation is case-sensitive ASCII — uppercase-prefixed
  workspaces (`BIG-CRM`, `C3PO`, `K2SO`) clumped above lowercase ones
  (`alakazam-labs-website`, `nsi-plan01`, `peliguard-web`). Now sorted
  client-side via `localeCompare(..., toLowerCase())` so the list
  reads in natural alphabetical order regardless of capitalization.

## Files touched

`src/renderer/components/Settings/sections/WakeSchedulerSection.tsx` only.
