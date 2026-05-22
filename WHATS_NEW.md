# K2SO — What's New

User-facing highlights of recent updates. See `release-notes-X.Y.Z.md`
files in the repo root for the full developer-facing changelog.

## 0.38.7 — Update notes when K2SO updates

You're seeing this because K2SO now shows a small "what's new" popup
the first time you open the app after an update. It rolls up everything
you missed if you skipped a version or two — no more wondering what
changed.

- Friendly per-update highlights
- Catches you up across multiple versions if you skipped a few
- `k2so whatsnew` reprints them anytime from the terminal
- `k2so whatsnew --reset` makes the popup show again next launch
  (good for sharing with a teammate)

## 0.38.6 — Inter-agent messages just work

`k2so msg <workspace> "text"` now delivers reliably on the first try.
The "send it twice and pray" workaround that agents were using is no
longer needed.

- One canonical JSON response shape every call — no more guessing
  whether `injected_to_pty: true` actually meant delivered.
- When delivery fails, you get a specific `reason` and an actionable
  `hint` instead of a silent inbox fallback.
- Recipients see `[from <sender>]` prefixed on every message, so they
  always know who's talking.
- `--wake` is no longer needed — `msg` is always live. Use
  `k2so work send` when you actually want to queue something for later.
- `k2so msg --help` finally works.

## 0.38.5 — Cmd+T tabs survive app updates

Your terminal tabs (including pinned chat) keep their `claude` sessions
through app updates and daemon restarts.

Before: a tab opened with `claude` would become a plain shell after the
next K2SO update. Now it comes back as `claude` — same command, same
working directory, same args. Subsequent updates won't reset your tabs
back to a shell.

## 0.38.4 — Heartbeats panel polish

The Heartbeats settings panel now matches the rest of the app's theme.
Heartbeat list is sorted alphabetically (case-insensitive — workspaces
named `alakazam-labs-website` and `BIG-CRM` no longer cluster apart).
Cosmetic only; no behavior change.

## 0.38.3 — System-wide Heartbeats settings page

Added a right-hand panel to the Heartbeats settings showing every
heartbeat across every workspace with toggles for enable/disable,
pinned-chat opt-in, and edit-wakeup. Plus a third column for a running
audit log of every fire system-wide — so you can finally see at a
glance which heartbeats are firing and which are dark.

## 0.38.2 — Heartbeats finally fire reliably

If you had heartbeats configured but they hadn't been firing for a
while (sometimes weeks), 0.38.2 fixes it. We replaced our hand-rolled
scheduler with the well-tested `croner` crate. Heartbeats now recover
cleanly from any pause and fire on schedule.

## 0.38.0 — Daemon-authoritative tabs + multi-window sync

Terminal tabs, including the pinned chat, now persist correctly when
the Tauri app closes and reopens — the daemon owns the sessions, and
the renderer attaches to whatever's already running. Cross-window
state (heartbeats minimized, pinned chat refresh, etc.) syncs
automatically.
