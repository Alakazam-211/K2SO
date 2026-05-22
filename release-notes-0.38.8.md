# 0.38.8 — Cmd+T session continuity + popup launch-race fix

Two follow-ups: closes the conversation-continuity gap for Cmd+T tabs
(0.38.5 known-follow-up), and fixes the launch-race that caused the
"What's new" popup to miss on first launch after 0.38.7.

## What changed

### Cmd+T `claude` tabs now resume conversations across daemon restarts

The 0.38.5 work brought Cmd+T tabs back as `claude` (not `shell`)
after app updates — but as **fresh** sessions with no conversation
history. 0.38.8 closes that gap: tabs resume **the same** claude
conversation across daemon restarts, just like pinned chat already
does.

**How it works:**

- `v2_spawn::handle_v2_spawn` auto-injects `--session-id <new-uuid>`
  when spawning `claude` with no session-id / resume flag (the common
  Cmd+T-from-Tauri-renderer shape).
- `v2_session_map::register` parses the injected flag and stamps
  `workspace_tab_sessions.session_id` with the same UUID. The upsert
  uses `COALESCE` so subsequent re-registers without the flag never
  clobber a stamped value.
- On daemon restart, `v2_spawn`'s restart-recovery branch strips any
  prior `--session-id` from saved args and splices in `--resume <uuid>`.
  Net: `claude --dangerously-skip-permissions --resume <stored-uuid>`
  resumes the exact same JSONL conversation.

**Existing tabs** (created before 0.38.8) don't have a stamped
session_id, so their first post-update restart still spawns a fresh
session — but the auto-inject fires on that first new spawn, and
**from that point forward** they restore properly. Forward-looking
fix; no migration script needed.

Smoke-verified end-to-end against the dev daemon: spawn → kickstart
→ second spawn with empty command → daemon log confirms `--resume
<original-uuid>` spliced in.

### "What's new" popup now survives the launch race

A handful of users reported they didn't see the popup after the
0.38.7 update. Root cause: the Tauri renderer's mount-time
`whats_new_check` call fired before the launchd-managed daemon had
finished writing `~/.k2so/daemon.port` + `daemon.token`. With no
credentials, `DaemonClient::try_connect` failed and the renderer
silently skipped the popup.

Fixed by adding a 10-attempt × 500ms retry loop to the
`whats_new_check` Tauri command — same pattern as the existing
`check_daemon_version_and_restart`. Total worst-case wait before the
popup gives up: 5 seconds. Works without changes to the daemon side
(daemon was always reachable post-credential-write; the renderer
just gives up too early before this fix).

### New "Read what's new" button in Settings → General

Sits under the CLI version row. Clicking it resets the daemon-side
"last seen" marker and dispatches a `k2so:show-whats-new` window
event the modal listens for — opens the popup with the current
version's content even after you've already dismissed it.

Useful for re-reading what shipped in the most recent update, or
showing a teammate what's new.

## Files touched

| Layer | File | Change |
|---|---|---|
| Daemon | `crates/k2so-daemon/src/v2_spawn.rs` | Auto-inject `--session-id <uuid>` for claude spawns missing one; restart-recovery now strips prior `--session-id` before splicing `--resume` |
| Daemon | `crates/k2so-daemon/src/v2_session_map.rs` | Parse args for `--session-id` / `--resume` at register, stamp `workspace_tab_sessions.session_id` |
| Tauri | `src-tauri/src/commands/whats_new.rs` | 10× / 500ms retry loop in `whats_new_check` to survive launch race |
| Renderer | `src/renderer/components/Settings/sections/GeneralSection.tsx` | NEW `WhatsNewRow` component under `CLIVersionRow` |
| Renderer | `src/renderer/components/WhatsNewModal/WhatsNewModal.tsx` | Listen for `k2so:show-whats-new` window event + force-open path |
| Content | `WHATS_NEW.md` | 0.38.8 entry |
| Notes | `release-notes-0.38.8.md` | (this file) |

## Smoke tested

- Spawn `claude` via `/cli/sessions/v2/spawn` with no session-id arg →
  daemon log shows `auto-injected --session-id=<uuid>` →
  `workspace_tab_sessions.session_id` populated with same uuid. ✓
- Kickstart daemon → re-spawn with empty command → daemon log shows
  `restart-recovery: ... args=["--dangerously-skip-permissions",
  "--resume", "<uuid>"]`. Same conversation resumed. ✓
- `cargo test -p k2so-core --lib whats_new`: 18 passed / 0 failed. ✓
- Tauri + daemon crates build clean. ✓
