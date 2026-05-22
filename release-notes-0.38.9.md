# 0.38.9 — "Read what's new" popup works while Settings is open

Hotfix to 0.38.8's new Settings button.

## What changed

The "Read what's new" button in Settings → General appeared to do
nothing when clicked. The popup actually opened, but it was rendered
behind the Settings layout — only became visible after closing
Settings.

**Cause:** `App.tsx` has three render paths (Settings open / focus
mode / default). The default and focus-mode paths both mount
`<WhatsNewModal />`, but the Settings-open path didn't. The button's
`k2so:show-whats-new` event had no listener mounted in the Settings
view, so the popup state machine never advanced. Closing Settings
swapped to the default layout, which mounted the modal and ran the
initial `whats_new_check` — finding `has_new: true` (the button had
reset the state earlier), it opened the popup. That's why it
appeared "delayed."

**Fix:** mount `<WhatsNewModal />` in the Settings-open layout too,
right after `<ConfirmDialog />` to match the existing pattern. Now
the popup opens immediately, layered above Settings via its
zIndex 99998/99999 stacking.

## Files touched

| File | Change |
|---|---|
| `src/renderer/App.tsx` | Add `<WhatsNewModal />` to Settings-open return path (line 701) |
| `WHATS_NEW.md` | 0.38.9 entry |
| `release-notes-0.38.9.md` | (this file) |

## Why this slipped past 0.38.8 smoke

The 0.38.8 visual test for the popup happened in the default layout
(not Settings). The button only exists inside Settings, so the
test-from-button path requires Settings to be open to even click it.
Adding a "open Settings → click Read what's new → verify popup
appears immediately" step to the manual smoke for any future
popup-affecting release.
