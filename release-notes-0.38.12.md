# 0.38.12 — Phase A diagnostics: memory watcher + heartbeat auto-disable

Phase A response to C3PO ticket `c9b0d9a9` (Tauri WebView memory leak
crash on 2026-05-21). Adds renderer-side telemetry so we can see leak
growth in real time, and silences the recurring heartbeat audit-log
noise the same ticket flagged.

Phase B (actual leak triage — heap snapshots, useEffect cleanup
audit, store eviction verification) follows once the watcher has been
in production a few days.

## What changed

### Renderer memory watcher

New `<MemoryWatcher />` component mounted at app root in all three
render paths (main, focus mode, Settings). Polls a new
`renderer_memory_status` Tauri command every 5 minutes; logs the
Tauri process's RSS to console as `[k2so/memory] pid=N rss=NMB
vsize=NMB`. If RSS crosses **800 MB** it surfaces an error toast with
a 1-hour cooldown so a chronic leak doesn't drown the user in
notifications.

Why not `performance.memory.usedJSHeapSize`? That API is
Chromium-only; Tauri uses WebKit on macOS, which doesn't expose JS
heap size to web pages. The next-best signal is the Tauri process's
own resident memory via `proc_pidinfo` on Darwin. Returns
`pti_resident_size` + `pti_virtual_size` + pid for stable logging.

Where this helps: the 2026-05-21 crash was diagnosed post-mortem by
mining `JetsamEvent-*.ips` files in `/Library/Logs/DiagnosticReports/`.
With the watcher in place, the same trend appears in the renderer
console every 5 min — no autopsy required.

### Heartbeat auto-disable on missing WAKEUP.md

`wake::compose_wake_prompt_from_path` returns `None` when WAKEUP.md is
missing or unreadable. Three call sites in `heartbeat_launch.rs` were
writing the same `failed to compose wake prompt` audit entry on every
tick and retrying immediately — producing the chronic log spam noted
in the C3PO ticket.

Now: the first miss writes one `auto_disabled` audit entry (with the
specific path it couldn't read) and flips `workspace_heartbeats.enabled
= false`. The next tick won't include this row in
`AgentHeartbeat::list_enabled`, so the heartbeat stays silent until
the user fixes WAKEUP.md and re-enables it from Settings → Heartbeats.

Shared via a new `auto_disable_missing_wakeup` helper so all three
fire paths (fresh-fire, workspace-session, resume+print) get the same
behavior without duplicated logic.

## Files touched

| File | Change |
|---|---|
| `src-tauri/src/commands/memory_watcher.rs` | NEW — `renderer_memory_status` Tauri command + `proc_pidinfo`-backed RSS read |
| `src-tauri/src/commands/mod.rs` | Register `pub mod memory_watcher` |
| `src-tauri/src/lib.rs` | Register `renderer_memory_status` in invoke_handler |
| `src/renderer/components/MemoryWatcher/MemoryWatcher.tsx` | NEW — 5-min poll loop, console log, 800 MB toast warning |
| `src/renderer/App.tsx` | Mount `<MemoryWatcher />` in all three render paths |
| `crates/k2so-daemon/src/heartbeat_launch.rs` | NEW `auto_disable_missing_wakeup` helper; three call sites collapsed to one line each |
| `WHATS_NEW.md` | 0.38.12 entry |
| `release-notes-0.38.12.md` | (this file) |

## Out of scope (Phase B)

The actual leak fix. Likely candidates per the C3PO triage:
session_grid_ws / session_events_ws subscriber cleanup, Zustand store
slice eviction on tab close, AppKit state-restoration churn from the
`NSPersistentUIManager flushAllChanges` loop. Will be informed by the
data the watcher collects over the next few days.

## Smoke

- `cargo build -p k2so-daemon` and `cargo build -p k2so`: both clean.
- `cargo test -p k2so commands::memory_watcher`: 1 passed / 0 failed.
- All three heartbeat call sites replaced; only doc-comment reference
  to the old error string remains.

## Bonus: how to triage the popup loop without releases

Tested today: `cli/k2so whatsnew --reset` → quit K2SO → relaunch.
Popup auto-fires correctly. State at `~/.k2so/whats-new.state`. No
release needed to verify popup-affecting changes. Same loop will work
for the memory watcher: open dev console, run the app for 5+
minutes, look for `[k2so/memory]` lines.
