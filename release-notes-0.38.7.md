# 0.38.7 — What's New popup + release-script contract

When K2SO updates, the app now shows a friendly popup on first launch
explaining what changed — in plain language, scoped to the versions
the user actually skipped. Backed by a daemon-side parser, a single
canonical state file, and a release-script gate that requires every
new version to ship with a user-facing entry.

## What changed

### User-facing
- **Popup on first launch after an update.** Shows the markdown slice
  for every version newer than the user's last-seen marker. Catches you
  up if you skipped a few versions in a row.
- **`k2so whatsnew` CLI verb.** Reprints the same content anytime from
  the terminal. `--reset` clears the dismissal marker (the popup will
  reappear on next K2SO launch). `--all` shows the full changelog
  regardless of state. `--mark-seen` marks the current version dismissed.

### Daemon
- New `k2so_core::whats_new` module with a markdown parser, semver-lite
  version compare, slice computation, and atomic state-file I/O.
  18 unit tests cover parser, comparator, slicer, and state roundtrip.
- New `/cli/whats_new`, `/cli/whats_new/mark_seen`, `/cli/whats_new/reset`
  routes. Backed by the embedded `WHATS_NEW.md` (`include_str!`) so the
  content travels with the binary; no asset bundling needed.
- State file at `~/.k2so/whats-new.state` — single line containing the
  last-seen version. Atomic write via temp+rename. Absent = "never
  dismissed."

### Release-script gate
- **`./scripts/release.sh <version>` now fails fast** in step 1.5 if
  `WHATS_NEW.md` has no `## <version> — title` section header. The
  failure message includes the exact format to add. This is intentional:
  every released version must ship with user-facing notes, separate
  from the developer-facing `release-notes-X.Y.Z.md` (which goes to
  the GitHub release body).
- Two-audience model: `release-notes-X.Y.Z.md` for the engineering
  audit trail on GitHub, `WHATS_NEW.md` for the popup an end user
  sees. Same release, two different writeups, both required.

### Tauri
- New `WhatsNewModal` component mounted at the app root. Calls
  `whats_new_check` on mount; if `has_new`, renders a modal with the
  markdown content (via the existing `Markdown` wrapper) and a single
  "Got it" button. Esc dismisses; click-outside dismisses; backdrop
  + content layout match existing `ConfirmDialog` chrome.
- Three new Tauri commands (`whats_new_check`, `whats_new_mark_seen`,
  `whats_new_reset`) — all thin wrappers over the daemon HTTP routes.

## Smoke-tested end-to-end

- Daemon: `/cli/whats_new` → returns canonical `WhatsNewCheck` JSON
  with `has_new: true` from a clean state; content includes 0.38.0
  through 0.38.6 (correctly capped at the daemon's embedded version).
- `mark_seen` → writes `~/.k2so/whats-new.state`. Re-check returns
  `has_new: false` and empty content.
- `reset` → deletes the state file. Re-check returns `has_new: true`
  again.
- CLI: `k2so whatsnew`, `--reset`, `--all`, `--mark-seen`, `--help`
  all behave correctly; output passes through to the daemon, exit
  codes mirror success.
- Modal: visually verified in `bun tauri dev` — appears on first
  launch with cleared state, dismissable via "Got it" button + Esc +
  backdrop click, doesn't re-appear after marking seen.
- Release-script gate: tested against present (0.38.7), absent
  (0.99.99), and prefix-collision (0.38) version strings — gate
  correctly proceeds on present and halts on absent without
  false-positive prefix matches.

## Files touched

| Layer | File | Change |
|---|---|---|
| Content | `WHATS_NEW.md` | NEW — user-facing changelog, embedded into the daemon binary at build time |
| Core | `crates/k2so-core/src/whats_new.rs` | NEW — parser, comparator, slicer, state I/O, 18 unit tests |
| Core | `crates/k2so-core/src/lib.rs` | Register `pub mod whats_new` |
| Daemon | `crates/k2so-daemon/src/cli.rs` | Three `/cli/whats_new*` routes |
| Tauri | `src-tauri/src/commands/whats_new.rs` | NEW — thin daemon proxies |
| Tauri | `src-tauri/src/commands/mod.rs` | Register `pub mod whats_new` |
| Tauri | `src-tauri/src/lib.rs` | Register Tauri commands in invoke_handler |
| Renderer | `src/renderer/components/WhatsNewModal/WhatsNewModal.tsx` | NEW — modal component |
| Renderer | `src/renderer/App.tsx` | Mount `<WhatsNewModal />` |
| CLI | `cli/k2so` | New `cmd_whatsnew` + verb dispatcher; `whatsnew --reset` / `--all` / `--mark-seen` / `--help` |
| Release | `scripts/release.sh` | NEW step 1.5 — verify `## <version>` section exists in `WHATS_NEW.md` before proceeding |
| Notes | `release-notes-0.38.7.md` | (this file) |
