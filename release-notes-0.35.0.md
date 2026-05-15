# K2SO 0.35.0 — Alacritty_v2 daemon-hosted terminal

This release introduces **Alacritty_v2**, a new terminal renderer
that runs on the K2SO daemon instead of inside the Tauri process.
Sessions survive app quit, heartbeats can target them naturally,
and the on-screen experience is byte-identical to the legacy
renderer. It ships as an opt-in choice in Settings — "Alacritty
(Legacy)" remains the default while we burn it in.

## Highlights

### Alacritty_v2 — daemon-hosted terminal renderer

Pick "Alacritty" in Settings → Terminal → Renderer to opt in.
Once selected, **new** terminals open against the daemon-hosted
path; existing tabs keep whichever renderer they were created
with.

What this gives you:

- **Sessions survive Tauri quit.** The daemon owns the PTY master;
  closing the K2SO window doesn't SIGHUP your shell.
- **Heartbeats work natively.** The daemon-hosted Term is a
  first-class session in `v2_session_map`, so wake-triggered
  signals can target it without any in-process coordination.
- **UX parity with legacy.** Scroll, reflow on resize, scrollback,
  Cmd-click links, paste from Finder, drag-and-drop, focus
  retention across split panes, Cmd+Shift+=/− zoom — all match
  the legacy renderer exactly.

The daemon serves a single Tauri-side viewer per session over
`/cli/sessions/grid` (WebSocket), streaming JSON snapshot + delta
payloads from the alacritty Term. No local Term, no local ANSI
parser, no APC coordination on the client.

### Daemon version-mismatch auto-restart

When you install a new K2SO over an old one, macOS lets the new
binary land on disk while launchd keeps the **old** daemon
process alive. Until now, a Settings → Restart Daemon click was
required to actually pick up the new binary. Now Tauri's startup
checks the daemon's reported version against its own and runs
`launchctl kickstart` automatically if they disagree. Look for
`[version-check] MISMATCH …` in the Tauri stderr the first time
this fires.

### Selection now tracks scroll

In Alacritty_v2: highlight some text, then mouse-wheel-scroll the
viewport. The highlight now rides along with the content
visually, instead of staying at the screen position the original
text used to occupy. Native gestures — double-click word,
shift-click extend, Cmd+A, Cmd+C copy — keep working.

### Cleaner WebSocket teardown

The daemon's grid-WS now sends a Close frame before tearing down
TCP, both on client-initiated close and on `child_exit`. WebKit
no longer logs spurious "The network connection was lost" / "ws
error" when a child process exits or a tab unmounts.

### `[v2-perf]` launch-time instrumentation

Every Alacritty_v2 spawn now emits `[v2-perf]` log lines at each
stage — `pty_open`, `term_new`, `event_loop_spawn`, `ws_accept`,
`first_snap` — on the daemon side, plus matching frontend stages
(`mount`, `creds`, `spawn_fetch`, `ws_open`, `first_snapshot`,
`first_render`, `tui_first_paint`) in DEV builds. A one-shot
`SUMMARY` line at first render gives the full breakdown.

Measured cold-spawn-to-first-render: **~50 ms**. Warm: **~25 ms**.
The remaining 1-second-or-so before your prompt appears is your
shell's startup, not K2SO.

## Internal — Kessel-T0 archived

The Kessel byte-stream reader path (where each Kessel pane hosted
a local mini-alacritty Term and consumed raw PTY bytes from the
daemon) was paused after 0.34.x — the byte-level approach turned
out not to be a viable foundation for the intended use cases.
Sixteen commits of stabilization work from that direction are
preserved on the `kessel-t0` branch and the `kessel-t0-archive`
tag, both reachable from main's merge graph. The two
"what we learned" docs were cherry-picked forward:

- `.k2so/prds/kessel-resize-architecture-notes.md`
- `.k2so/prds/kessel-instant-everywhere.md`

The latter's UX-feel principles still apply to any future
renderer work, including post-0.35.0 v2 perf.

## Notes

- Alacritty_v2 is **opt-in** in this release. The default stays
  on Alacritty (Legacy). We'll flip the default once v2 has
  burned in across more workflows.
- Kessel (renderer) sessions are unaffected by this release.
- CLI tools that spawn agent sessions still use the legacy
  renderer; routing them through v2 is queued as a follow-up.
