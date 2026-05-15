# 0.34.1 — Kessel goes from BETA to "feels like a real terminal"

> **tl;dr** 0.34.0 shipped the Session Stream pipeline and the new **Kessel** React terminal renderer as an opt-in BETA, with a documented list of fidelity gaps. 0.34.1 closes almost all of those gaps and pushes Kessel's launch latency to parity with Alacritty. It's still BETA, still opt-in — but picking Kessel no longer means trading correctness or speed for the chance to try something new.

0.34.0's release notes ended with: *"All of these are fidelity issues, not correctness bugs. Users who need them today pick Alacritty. 0.34.N is where the polish lands."* This is that release.

## What the user feels

### 1. Launches at parity with Alacritty

Real-numbers goalpost, measured on an M-series Mac with Claude Code as the worst-case TUI:

| Metric | 0.34.0 Kessel | 0.34.1 Kessel | Alacritty |
|---|---|---|---|
| First-tab spawn → cursor visible | 3000–10000 ms | 80–120 ms (warm) | 80–120 ms |
| Cmd+T → Claude `tui-ready` | ~3600 ms | 463–510 ms | 402–405 ms |
| Typing latency | visibly laggy | at parity | baseline |

The fix was roughly 10 different things, not one: a persistent `reqwest::blocking::Client` in Tauri (kills the 600ms tokio-cold-start on first spawn), an `http_client()` pool-warm at Tauri `main()`, cached daemon creds that skip the disk read after the first call, an O(1) in-memory counter for pending-live queue drains (avoiding a directory scan on every spawn), an optimistic pane mount that draws the grid + cursor before the daemon round-trip returns, opt-in Alacritty `Term` dual-parse so production sessions skip a 4.6× system-time tax, and a fix to the macOS launchd `ProcessType` that had been silently pinning Claude to low-priority scheduling (this alone was the 3-second Claude lag).

### 2. Vim and Claude Code both render correctly now

Alt-screen buffer (DECSET ?1049 / ?47) is wired. `vim`, `htop`, `less`, `tmux`'s alt-screen mode, and Claude Code's interactive panel all switch buffers cleanly and restore on exit. Scrollback is suppressed inside alt-screen (matches every real terminal).

### 3. Paste works

Bracketed-paste mode (DECSET ?2004) is honored. Paste into Claude Code, into `zsh`, into `vim` insert mode — each gets the right `\e[200~` / `\e[201~` framing so the receiving program knows it's paste, not typing. Natural text-editing chords (Cmd+Backspace, Option+Left/Right, Cmd+Arrow) now produce the right sequences.

### 4. Cursor behaves

- **Always visible.** Claude Code and other TUIs hide the cursor with DECTCEM so they can paint their own — but our DOM pane had no way to render that custom glyph, so users "lost the cursor in Claude Code." Kessel now ignores DECTCEM and always shows its own caret. Solid when the tab has focus, hollow outline when it doesn't (matches the native macOS text-input caret convention).
- **No more blink, no more hopping.** Rosson specifically asked for a stable solid caret; that's what you get. The resting-cursor state machine defers rendering intermediate positions when a TUI repaint bursts a cursor through several locations (Claude's bottom-border refresh used to make the caret visibly shoot to row 0 and back). Small moves (typing, Enter, line wrap) commit immediately; large moves wait 60ms for the burst to settle. Same-row snaps to col=0 also settle — that was the "cursor jumps onto the `>` prompt for a frame" pattern in Claude Code's input repaint.
- **DECSCUSR shape.** `CSI Ps SP q` is honored, so vim's mode indicator (block in normal mode, bar in insert) works.
- **DECSC/DECRC + ESC 7 / ESC 8.** Legacy save/restore cursor sequences wired.

### 5. Trackpad scroll isn't insane

The old handler treated every wheel event as a discrete tick advancing 3 lines. On macOS trackpads, which fire 30-60 pixel-delta events per swipe, that meant one swipe scrolled 90-180 lines. Fixed by porting Alacritty's approach: accumulate pixel deltas, flush every 50ms, convert to lines at `1 line per cellHeight`. A 100px swipe on a 20px cell now scrolls 5 lines — matching every native macOS text view. The `scrolling.multiplier` config is now a sensitivity scalar (1.0 = Alacritty-equivalent, default).

### 6. Scrollback doesn't render at the wrong width

Rows pushed to scrollback while the grid was narrower (e.g., during the pre-first-ResizeObserver window at 80 cols) used to keep their narrow cell count forever — scrollback rendered as a short-line block until enough new content rolled through to replace them. `grid.resize()` now also pads/trims every scrollback row to the current cols, so any resize event makes the entire viewport consistent.

*(True soft-wrap re-flow — re-joining a logical line that was split across two narrow rows — is a harder problem parked for the Phase 4.7 word-editor pass.)*

### 7. Bell, focus reporting, synchronized output, autowrap

Small but real:

- **Bell (`\x07`).** Configurable: visual flash, audio chirp, or both. No more silent bells.
- **Focus reporting (DECSET ?1004).** When neovim or tmux asks for it, focus/blur events write `CSI I` / `CSI O` so the TUI can dim its UI while unfocused.
- **Synchronized output (DECSET ?2026).** `BEGIN_SYNCHRONIZED_UPDATE` / `END_SYNCHRONIZED_UPDATE` sequences buffer writes at the grid layer so a full repaint commits atomically to the DOM. Kills mid-repaint flicker.
- **Autowrap (DECSET ?7)** + **application cursor keys (DECSET ?1)** both honored.

### 8. Keyboard bugs that were silently broken

- **Backspace.** `LineMux` was popping bytes from `pending_text` on `\x08`, which ate the BS before it reached `TerminalGrid` — so the shell saw the keystroke but the pane didn't. Fix: pass `\x08` through to the grid's `writeChar`. Fixes up-arrow history replay as well.
- **Ctrl+Backspace.** Sends `ESC+DEL` (backward-kill-word), matching Alacritty. Previously sent plain `\x08` which shells interpret as single-char delete.
- **Shell was in canonical mode.** Bare-shell spawn now uses `-il` so zsh loads `.zshrc` and ZLE activates — backspace, history, word-motion all work as expected.

### 9. FD leak plugged

Each Kessel tab held a PTY master FD + reader thread + archive handle. Closing the tab wasn't telling the daemon to tear down the session, so resources piled up. At ~14 open tabs the per-process FD limit (`ulimit -n` default 256) hit and every further spawn failed with `dup of fd 255 failed`. Fix: `kessel_close` IPC on tab unmount → `/cli/sessions/close` → session teardown.

### 10. Config surface for customization

New `KesselConfig` + React context:

```ts
{
  font:       { family, lineHeightMultiplier, ... }
  colors:     { foreground, background, palette, cursor, selection }
  scrolling:  { cap, multiplier }
  cursor:     { defaultShape, settleMs, blinkIntervalMs, thickness, ... }
  bell:       { mode: 'none' | 'visual' | 'audio' | 'both', durationMs }
  mouse:      { ... }
  performance:{ ... }
}
```

Every knob has a sensible default, so users see zero change unless they opt in to customize. Foundation for user-facing settings in a later release.

## Still not in Kessel (carried forward to 0.34.2+)

- **Mouse reporting** (tmux scroll-wheel forwarding, click-to-position-cursor). Will need `/cli/sessions/write-mouse` and an X10/SGR encoder on the renderer side.
- **True scrollback re-flow.** See above — the "pad rows to new width" fix handles the visible gap, but doesn't re-join soft-wrapped logical lines. Requires tracking the "this row wraps into the next" bit at write time.
- **Theme support in the config.** The `colors` surface exists; wiring it to user-selectable themes lands later.
- **Phase 3.2 hardening** (harness watchdog, archive rotation, per-coordination-level budgets, real scheduler-wake of offline agents) partially landed under G1–G6 but the manual smoke-test still owes Rosson a three-terminal verification pass.

## Under-the-hood highlights

### Launch-perf plumbing

- `src-tauri/src/commands/kessel.rs` — new `kessel_spawn`, `kessel_write`, `kessel_resize`, `kessel_close`, `kessel_warm_http` commands. All share a persistent `http_client()` `OnceLock<reqwest::blocking::Client>` with `pool_idle_timeout=60s`. `DaemonCreds` cached via `creds_cache()` `RwLock` so the second tab's spawn skips the disk read entirely.
- `src-tauri/src/main.rs` — `warm_http_pool_async()` kicks off the reqwest runtime during Tauri boot so the first `kessel_spawn` doesn't pay the tokio cold-start.
- `crates/k2so-daemon/src/pending_live.rs` — `pending_state()` OnceLock counter cache avoids the per-spawn `read_dir` of the pending-live queue directory (was ~30ms per spawn, now O(1) when queue empty).
- `crates/k2so-core/src/terminal/session_stream_pty.rs` — new `SpawnConfig.track_alacritty_term: bool` flag. Production defaults to `false`, skipping the dual `alacritty_terminal::Term::advance(...)` pass that was burning 4.6× more system time than the PTY reader. Tests still opt in for screen-scrape verification.
- `crates/k2so-core/src/wake.rs` — launchd plist now ships `<string>Interactive</string>` instead of `<string>Background</string>`. Was the single biggest contributor to Claude's 3-second lag (macOS was giving the daemon's PTY child — including Claude's SIMD paths — low-priority scheduling).

### Parser / renderer coverage

- **`LineMux`** — added dispatch for DECSET `?1`, `?7`, `?1004`, `?2026`, `?25`, `?1049`, `?47`, `?2004`; DECSCUSR via CSI+space intermediate; ESC 7 / ESC 8; bell. Bug fix: the `\b` byte is now pushed to `pending_text` (was popped, eating it).
- **`SessionStreamView.tsx`** — resting-cursor settle state machine (including same-row col=0 settle), focus tracking for the solid/hollow cursor, WS frame batching per `requestAnimationFrame`, line-level damage tracking (per-row `React.memo` predicate — ~22× fewer cell iterations per keystroke), optimistic pane mount during spawn, 50ms pixel-accumulator wheel handler.
- **`grid.ts`** — synchronized-output buffer, DECTCEM / DECSCUSR state, autowrap, scrollback-row resize.

### Benchmarks + instrumentation

- `crates/k2so-core/examples/kessel_spawn_bench.rs` — head-to-head `zsh -ilc exit` spawn benchmark. Result: **Kessel 177ms vs Alacritty 181ms** (not the bottleneck).
- `scripts/kessel-ui-bench.ts` — UI-layer latency bench (WS round-trip, throughput, stale-pane). p50 WS round-trip = 1ms (also not the bottleneck).
- First-frame + tui-ready timing instrumentation in both renderers, shared predicate (any of alt_screen / bracketed_paste / focus_reporting mode transition). Console log:

  ```
  [Kessel] ready tab-abc e2e=103ms total=85ms rust=31ms
           (creds=0ms ser=0.1ms http=15ms resp=9.4ms de=0.5ms)
  [Kessel] tab-abc first-paint≈108ms (Cmd+T → cursor visible)
  [Kessel] tab-abc tui-ready=463ms (alt_screen ON → TUI is interactive)
  ```

## Upgrade path

Bit-for-bit compatible with 0.34.0. Flag-off (default renderer = Alacritty) users see zero change. Users who flipped the Settings → Terminal → Terminal Renderer toggle to Kessel in 0.34.0 will pick up all of the above automatically.

Rollback knobs preserved:

- Per-project `use_session_stream='off'` setting — legacy path for any workspace that hits a Kessel edge case.
- `--no-default-features` at build time — disables the Session Stream feature entirely.
- `git reset --hard v0.33.0` + rebuild — nuclear.

## Credits

49 commits on `feat/session-stream` since 0.34.0 by **Rosson Long** + Claude (Opus 4.7, 1M context). Work spanned Phase 4.6 (Kessel parity + polish: cursor, alt-screen, paste, bell, config, keyboard fidelity) and the launch-perf optimization pass (L1.1 / L1.4 / L1.5 / L3.1 + reqwest-warm + process-priority + FD-cleanup). Several bugs were caught by Rosson running real workflows in the dev build — the three-second Claude lag, the FD-exhaustion crash at ~14 tabs, the cursor jumping to col=0, the scroll oversensitivity, the narrow scrollback after resize.

Manual smoke verification: terminal renderer, Claude Code launch, backspace, up-arrow history, trackpad scroll, window resize, cursor-follow, alt-screen entry/exit. Three-terminal awareness-bus end-to-end (Phase 3 signal path) still pending a dedicated smoke — next up.
