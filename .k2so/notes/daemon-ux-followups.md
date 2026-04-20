# Daemon UX follow-ups — deferred from 0.33.0

Items we identified during the 0.33.0 daemon lifecycle UX pass
(2026-04-19) that we chose NOT to land in 0.33.0 so we could ship.
Most are polish; a few are real gaps worth addressing soon.

Created 2026-04-19 after the Settings → K2SO Daemon row + Cmd+Q
prompt landed. Re-assess for 0.33.1 or 0.34.0 depending on priority.

---

## Real gaps (do these early in 0.33.1)

### 1. Stale plist path detection
- **Problem:** The launch-agent plist encodes the absolute path to
  the daemon binary. When a user moves K2SO.app (e.g., drag-installs
  into `/Applications` after first-launch from `~/Downloads`), the
  plist is stale. launchd keeps trying to exec a nonexistent path,
  the daemon never starts, and the Settings row silently shows "not
  installed."
- **Fix:** On Tauri launch, read the installed plist (if any),
  compare the encoded program path to the current bundled
  k2so-daemon path. Mismatch → regenerate the plist + reload. Cheap
  (single plutil read + string compare) and silently self-heals a
  class of "daemon won't start" bug reports.
- **File:** `src-tauri/src/lib.rs` near the `install_daemon_plist_v1`
  migration — add a sibling check that runs every launch (not just
  once per upgrade).
- **Scope:** ~1 hour.

### 2. First-run / first-heartbeat prompt
- **Problem:** Today the user has to discover the daemon exists by
  scrolling Settings. A power feature shouldn't be behind a
  scroll-to-find. When someone enables agent mode for the first time,
  or creates their first heartbeat, *that's* the moment to ask "do
  you want agents to keep running when the app is closed?"
- **Fix:** Zustand flag `daemonPromptShown` persisted to
  `app_settings`. On every agent-mode toggle and heartbeat creation,
  if flag is false AND daemon is not installed, show a one-time
  dialog with `[Enable daemon] [Not yet]`. Dialog body explains the
  tradeoff (power use, how to disable).
- **Files:** new component `src/renderer/components/DaemonEnablePrompt/`,
  triggered from `HeartbeatScheduleDialog.tsx` + `AgenticSystemsToggle.tsx`.
- **Scope:** ~1 hour.

### 3. Tauri-must-not-fork-daemon audit
- **Problem:** We *designed* the system so only launchd spawns the
  daemon — Tauri only ever `launchctl kickstart` or reads
  `daemon.port`. Haven't actually verified no code path forks the
  daemon directly.
- **Fix:** grep for `Command::new("k2so-daemon")` across `src-tauri/`
  and confirm no call sites. Add a `#[cfg(test)]` assertion at the
  `DaemonPlist::canonical` level that logs the single canonical
  spawn path so future audits have an anchor.
- **Scope:** ~30 min.

### 4. Daemon version mismatch detection
- **Problem:** User upgrades K2SO.app. The new bundle has
  `k2so-daemon v0.33.1`. launchd is still running `k2so-daemon
  v0.33.0` because `KeepAlive: true` means the old process just keeps
  respawning from the old binary path (until the plist is
  regenerated — which only happens if the path changed).
- **Fix:** Compare daemon's `version` field from `/status` to the
  bundled daemon's version string on every Tauri launch. Mismatch →
  silently `launchctl kickstart -k` to force a respawn against the
  new binary.
- **File:** `src-tauri/src/lib.rs` startup path.
- **Scope:** ~1 hour.

## Polish (0.34.0 or later)

### 5. Menubar icon with per-state rendering
- **Desired:** macOS menubar icon that shows daemon status at a
  glance (like Slack / 1Password / Discord). Four states: idle,
  agent active, pending review, error. Right-click menu: Show K2SO /
  Pause daemon / Quit / Settings. Badge for pending review count.
- **Why deferred:** This is ~3-4 hours of Tauri v2 `TrayIcon` API
  work + icon assets + menu wiring + the 4 state animations. Settings
  pane already covers the primary "how do I control this thing"
  question, so the menubar icon is pure ergonomic upgrade. Good
  sprint of its own.
- **Files:** `src-tauri/tauri.conf.json` has `trayIcon` configured
  but no handlers. New file `src-tauri/src/tray.rs` would own the
  menu + state transitions.

### 6. "Pause daemon" (vs full stop)
- **Desired:** A temporary "don't fire heartbeats for 2 hours" mode
  without unloading the launch agent. Useful when the user is on a
  call / in a meeting / battery-anxious.
- **Mechanism:** DB flag `daemon_paused_until DATETIME`. Scheduler
  tick checks it before firing. Menubar icon shows a pause badge.
- **Scope:** ~1-2 hours.

### 7. System notification when heartbeat fires in background
- **Desired:** macOS notification center delivers "Agent X woke up
  and finished a task" when Tauri is closed. Mirrors what the mobile
  push target delivers to phone.
- **Mechanism:** `PushTarget::NotificationCenter` impl alongside the
  existing Webhook/NtfySh adapters.
- **Scope:** ~1-2 hours. The trait is designed for this.

### 8. Dock badge count for pending reviews
- **Desired:** When K2SO is in the dock (window closed), dock icon
  shows a red badge with the pending-review count. Same pattern as
  Mail.app / Messages.
- **Mechanism:** `app_handle.set_dock_badge(count)` on every review
  queue poll result.
- **Scope:** ~30 min.

### 9. Daemon log rotation
- **Problem:** `~/.k2so/daemon.stdout.log` grows unbounded. Over
  months of running, could eat gigabytes.
- **Fix:** Rotate on startup if > 50 MB. Keep last 2 rotated files.
  Could use `tracing-appender` with daily rotation if we want
  structured.
- **Scope:** ~1 hour.

### 10. Daemon resource-limit throttling when idle
- **Desired:** When no heartbeat has fired in N minutes and no
  companion clients are connected, drop the daemon's tokio runtime
  to minimum worker count or switch to event-driven wake. Save
  battery.
- **Mechanism:** Activity counter + timer. When idle, park the scheduler's
  select loop on a condvar the next wake request poke. Exit parked mode
  on scheduler tick or WS client connect.
- **Scope:** ~2-4 hours. Measure actual idle cost first — may not
  be worth it.

### 11. Activity Monitor display name polish
- **Problem:** Daemon appears as "k2so-daemon" in Activity Monitor
  / ps output. Should arguably be "K2SO Helper" or "K2SO Background
  Agent" to match user-visible product naming.
- **Fix:** Use the bundle's `CFBundleName` + renaming the launchd
  `Label` won't help — needs `LSBackgroundOnly` plist entry + an
  actual bundle wrapper, which is a bigger change than it sounds.
- **Scope:** ~2 hours including the bundle plumbing. Low priority.

### 12. "Don't ask again" on Cmd+Q prompt
- **Problem:** The Cmd+Q prompt (0.33.0) fires every time the user
  quits while the daemon is running. Power users will want to
  suppress it.
- **Fix:** Settings toggle "Ask before quitting when the daemon is
  running" with default `true`. When `false`, Cmd+Q goes through
  with the "keep daemon running" behavior silently.
- **Scope:** ~30 min. Just didn't want the scope creep in 0.33.0.

### 13. Uninstall when K2SO.app is dragged to Trash
- **Problem:** macOS doesn't notify apps when they're moved to
  Trash. The launch agent stays orphan, tries to exec missing
  binary, fills daemon.stderr.log with errors on every login.
- **Fix:** Either (a) ship an Uninstall.app helper that runs
  `k2so daemon uninstall`, or (b) have the daemon itself detect a
  missing parent bundle on startup and self-uninstall. (b) is
  cleaner — daemon reads its own path, verifies the containing .app
  still exists, exits + removes its plist if not.
- **Scope:** ~1 hour.

## Open questions (hypothesis-level)

- **Should the daemon auto-install on first launch for every user, or
  only after opt-in?** Currently auto-installs in release builds via
  the `install_daemon_plist_v1` migration. That's aggressive — new
  users get a background process without asking. Consider gating
  behind the first-heartbeat prompt (#2 above) and NOT auto-installing
  until the user signals they want persistent-agent behavior. Right
  now the behavior is: install on first launch, even if the user
  never touches agent mode. Defensible ("opt-out, not opt-in") but
  worth revisiting if we see pushback.

- **Do we need multi-user support on shared Macs?** Each user has
  their own `~/Library/LaunchAgents/`, so daemon is per-user
  naturally. Open question: if one user installs K2SO and creates
  agents, then another user logs in and runs the same app bundle,
  does the second user get a clean install or does something cross-
  contaminate? Haven't tested.

- **What's the right granularity for the Cmd+Q prompt's "Stop
  everything" button?** Currently it `launchctl unload`s the plist,
  which is a durable stop — daemon won't come back until the user
  clicks "Start daemon" in Settings. Should "Stop" be a transient
  one-time stop (plist stays loaded, user gets daemon back next
  login)? Decision deferred to first user feedback.
