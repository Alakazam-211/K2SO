# 0.38.11 — WhatsNewModal `mode` prop: auto vs button-only

Architecture fix to make the popup's two purposes explicit. Same modal
UI; cleaner separation under the hood.

## What changed

Before 0.38.11 the same `<WhatsNewModal />` instance handled both
auto-fire (on launch after update) and button-trigger (from Settings →
Read what's new). Every mount ran the initial `whats_new_check`, then
also listened for the `k2so:show-whats-new` event. Worked, but the
Settings instance could in principle auto-fire — e.g. if a user
opened Settings between dismissing the popup on launch and
mark_seen completing, or in any future state-machine edge case.

0.38.11 splits the behavior via a `mode` prop:

| Mount | Mode | Behavior |
|---|---|---|
| Main layout (default app view) | `auto` (default) | Runs `whats_new_check` on mount; opens if `has_new: true` |
| Focus-mode layout | `auto` (default) | Same as main |
| Settings layout | `button-only` | No mount-time check; only opens on `k2so:show-whats-new` event |

App.tsx mounts:

```tsx
<WhatsNewModal />                       {/* main layout */}
<WhatsNewModal />                       {/* focus mode */}
<WhatsNewModal mode="button-only" />    {/* Settings layout */}
```

## Files touched

| File | Change |
|---|---|
| `src/renderer/components/WhatsNewModal/WhatsNewModal.tsx` | NEW `mode` prop (`auto` \| `button-only`); the mount-time check is gated on `mode === 'auto'`; event listener stays unconditional so both modes can be button-triggered |
| `src/renderer/App.tsx` | Settings mount passes `mode="button-only"`; main + focus mounts unchanged (default `auto`) |
| `WHATS_NEW.md` | 0.38.11 entry |
| `release-notes-0.38.11.md` | (this file) |

## User-visible behavior

Auto popup on launch after update: unchanged.

"Read what's new" button in Settings → General: unchanged, opens
immediately on top of Settings.

The change is purely defensive — closes off a class of edge cases
where the Settings-mounted instance could fire in unexpected states.

## Smoke

`cargo build -p k2so`: clean (101 warnings, all pre-existing).
