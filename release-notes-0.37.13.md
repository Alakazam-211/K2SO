# 0.37.13 — Sidebar collapse + Active section expansion

Small UX update that makes the left sidebar behave better when you have a lot of workspaces.

## Changes

- **Agents & Pinned section is now collapsible.** New chevron arrow in the section header toggles the entire group hidden/visible; state persists across launches via `localStorage`. Header also shows a count badge (matches the Active section's pattern).
- **Agents & Pinned scrolls when > 10 items.** When the combined Agents + Pinned count exceeds 10, the section caps at ~360 px tall with its own scrollbar — instead of growing unbounded and pushing Focus Groups + Active off-screen.
- **Active section now includes pinned + agent-mode workspaces.** Previously it filtered them out because they're already visible at the top of the sidebar. After this change, your most-used pinned/agent workspaces also appear in Active so the `⌘ 1-0` (or `⌥⌘ 1-0` depending on layout) shortcuts target what you're actually using right now, not just the unpinned set.
- **Active section grows to fit all 10 shortcut-bound rows.** Max height bumped from 200 px → 320 px so the workspaces bound to `1, 2, 3, 4, 5, 6, 7, 8, 9, 0` are all visible without clipping. Anything beyond 10 rolls off the end (the keyboard surface doesn't extend past 0, so there's nothing to address there).

## Files touched

| File | Change |
|---|---|
| `src/renderer/components/Sidebar/Sidebar.tsx` | Collapse toggle, count badge, inner scroll for Agents & Pinned section |
| `src/renderer/components/Sidebar/ActiveBar.tsx` | Removed pinned + agent-mode filters from both Active membership paths; max-height 200 → 320 |
