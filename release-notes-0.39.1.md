# K2SO 0.39.1 — Manager-pin fix

Patch release. One thing.

## What this fixes

0.39.0 shipped a one-shot migration that auto-pinned existing
agent-mode workspaces so they wouldn't visually disappear from the
top of the nav after the "AGENTS & PINNED" auto-promote behavior
retired. The migration's filter was too wide: it pinned `manager` /
`coordinator` / `pod` workspaces in addition to the correct `agent`
(K2SO Agent) and `custom` (Custom Agent) modes. Pre-0.39.0, the
sidebar's auto-promote only surfaced `agent` + `custom` — manager-
family workspaces never appeared in the Agents section. So 0.39.0
over-pinned them on first launch.

0.39.1 ships two changes:

1. **Source filter corrected** (`auto_pin_existing_agents_0_39_0`
   migration): the filter is now `('agent', 'custom')` only.
   Matters for users installing 0.39.1 fresh (skipping 0.39.0) —
   they get the right behavior from first launch.

2. **Corrective migration added** (`correct_auto_pin_filter_0_39_1`):
   one-shot daemon migration that unpins workspaces currently pinned
   with `agent_mode IN ('manager', 'coordinator', 'pod')`. Runs once
   per local DB on the first 0.39.1 boot, then never again. Trade-
   off: also unpins any manager-family workspace a user manually
   pinned pre-0.39.0; re-pin via right-click → Pin if you want them
   back.

**One-shot guarantee**: `code_migrations` gating means both migrations
run AT MOST ONCE per DB. Once 0.39.1 has applied the corrective
unpin, future boots — including 0.39.2, 0.40.x, and any version
beyond — leave your pin choices alone. If you pin a manager
workspace tomorrow, it stays pinned.

## Test coverage

- 8 inline tests on `auto_pin_existing_agents_0_39_0` (idempotent,
  filter-correct, mixed-mode partial-pin)
- 9 inline tests on `correct_auto_pin_filter_0_39_1` (each mode
  independently, mixed scenarios, second-run idempotent)
- 17 total new tests, all passing in `cargo test -p k2so-core --lib`

## Upgrade notes

- Users upgrading 0.39.0 → 0.39.1: corrective migration fires on
  first boot, unpins your manager-family workspaces, then writes the
  `correct_auto_pin_filter_0_39_1` marker. Re-pin anything you want
  to keep at the top.
- Users upgrading 0.38.x → 0.39.1 (skipping 0.39.0): both migrations
  fire, but the 0.39.0 migration's corrected filter only pins
  agent + custom from the start, so the corrective migration finds
  nothing to unpin and is a no-op.
- Users who already pinned the manager-family workspaces they want
  visible can re-pin them in seconds via right-click → Pin.

## What else shipped in this release

Nothing else — strictly the manager-pin fix. See `release-notes-0.39.0.md`
for the full 0.39.0 changelog.
