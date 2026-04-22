# Phase 2 — Session Stream dual-emit + subscribe — COMPLETE

**Branch:** `feat/session-stream` (off `v0.33.0`)
**Duration:** 2026-04-19 → 2026-04-20 (one session, seven commits)
**Status:** All D1–D7 commits landed and green. Ready for Phase 3.

## Commits

| # | Title | Commit |
|---|---|---|
| D1 | SessionRegistry + SessionEntry | `336b9c4f` |
| D2 | `use_session_stream` project setting | `fabd0fd7` |
| D3a | PTY reader drives alacritty Term without EventLoop | `8a884090` |
| D3b | LineMux fork + SessionRegistry publishing | `0421df9f` |
| D4 | Daemon `/cli/sessions/subscribe` WS route | `edc96d99` |
| D5 | Smoke-test consumer example | `75f08f1a` |
| D6 | End-to-end integration test for WS subscribe path | `7336a3bc` |

Plus Phase 1 (C1–C6) commits from 2026-04-19:
`48368247 4fa007af a558c4e4 6c1c9a62 78868ae3 b86ddfa2`.

## Test aggregate after D6

Flag-off workspace build: clean (bit-for-bit v0.33.0).

With `--features "session_stream test-util"`:

| Suite | Count |
|---|---|
| k2so-core lib | 229 |
| k2so-core tests/session_stream_apc | 18 |
| k2so-core tests/session_stream_awareness | 6 |
| k2so-core tests/session_stream_claude_recognizer | 11 |
| k2so-core tests/session_stream_line_mux | 14 |
| k2so-core tests/session_stream_pty | 8 |
| k2so-core tests/session_stream_registry | 16 |
| k2so-core tests/session_stream_setting | 8 |
| k2so-core tests/session_stream_types | 8 |
| k2so-daemon unit | 5 |
| k2so-daemon tests/sessions_ws_integration | 6 |
| **Total** | **329** |

All passing. One pre-existing flake in
`companion::settings_bridge::tests` on the global OnceLock slot
intermittently fails on first run — clears on re-run. Flagged for a
one-line TEST_LOCK fix in a future commit.

## Invariant audit (post-D7)

Phase 2 locked three invariants in design (see `.k2so/prds/session-stream-and-awareness-bus.md`
and the plan file at `~/.claude/plans/happy-hatching-locket.md`):

1. **Subscribers never import alacritty types.**
   ```
   grep -rn "alacritty" crates/k2so-core/src/session/          # no matches
   grep -rn "alacritty" crates/k2so-core/src/awareness/        # no matches
   grep -rn "alacritty" crates/k2so-core/examples/session_stream_*.rs
   # only doc comments referencing the invariant itself
   grep -rn "alacritty" crates/k2so-daemon/src/sessions_ws.rs  # no matches
   ```
   HELD.

2. **LineMux sees raw PTY byte stream, not alacritty's grid.**
   In `crates/k2so-core/src/terminal/session_stream_pty.rs::reader_loop`,
   the same `chunk: &buf[..n]` is handed to `processor.advance(...)`
   AND `line_mux.feed(chunk)` in the same Ok arm. No post-parse
   re-encoding. Phase 5 deletes the `processor.advance` call; LineMux
   continues seeing bit-identical input. HELD.

3. **Feature flag gates consumer side only.**
   `#[cfg(feature = "session_stream")]` gates every new module in
   `crates/k2so-core/src/lib.rs`. No runtime "maybe-LineMux-maybe-not"
   branching. When enabled AND project has `use_session_stream='on'`,
   the dual-emit reader runs unconditionally. HELD.

## What Phase 2 delivers

- **Per-session typed event streams** — device-local, typed, in-process.
  Producer writes via `SessionEntry::publish(frame)`; any subscriber
  gets a real-time stream of `Frame`s via tokio broadcast with a
  1000-Frame replay ring for late joiners.
- **Dual-emit reader thread** — `terminal::spawn_session_stream` spawns
  a PTY child, drives alacritty's `Term` grid for desktop rendering
  parity AND `LineMux` for client-agnostic Line + Frame emission, all
  from a single byte stream.
- **WebSocket subscribe endpoint** — daemon's `/cli/sessions/subscribe`
  fans Frames from any registered session to any connected WS
  subscriber. Replay ring flushed first, then live stream. Multi-
  subscriber fanout works; unknown/malformed session IDs 400 before
  WS upgrade.
- **Smoke-testable dev loop** — `cargo run -p k2so-core --example
  session_stream_subscribe --features session_stream -- <cmd>` prints
  Frames as JSON lines. First testable-in-terminal artifact for 0.34.0.

## Remaining for Phase 3+

Deferred explicitly by scope (per the Phase 2 plan):

- **Archive NDJSON writer** (`.k2so/sessions/<id>/archive.ndjson`) —
  Phase 3. Session Entry currently in-memory only.
- **Awareness Bus routing + egress** — Phase 3. APC types and
  extraction are in place from Phase 1 (C3 + C5); ingress / routing /
  filesystem-backed inbox are the Phase 3 deliverables.
- **Harness watchdog** — Phase 3+. Last-frame-at scan + SIGTERM/SIGKILL
  escalation.
- **Daemon-side session spawn** — Phase 3+ adds a `/cli/sessions/spawn`
  verb so external callers can trigger sessions that register in the
  daemon's registry. Until then, the smoke example is in-process.
- **Cross-process SessionRegistry** — Phase 4. The current registry is
  per-process; a Tauri client running alongside the daemon sees a
  different registry than the daemon serves. Fine for Phase 2 because
  the smoke example + integration tests all run in-process.
- **The remaining 9 of 14 stranded `/cli/*` routes** — Phase 4, once
  the broader `k2so_agents` migration lands. 5 of the 14 become
  unblocked by Phase 2's SessionRegistry: `/cli/sessions/subscribe`
  (landed D4), `/cli/sessions/replay`, `/cli/terminal/read`,
  `/cli/agents/running`, `/cli/companion/sessions`. Subscribe is the
  only one wired; the other four are mechanical follow-ups.
- **Stream-json (T1) adapter for Claude Code** — Phase 6.
- **Codex / Aider / Gemini / Goose recognizers** — Phase 6.
- **Full SGR / color parsing on `Text` frames** — Phase 3 when Metal
  renderer or another high-fidelity consumer needs it.
- **Settings UI toggle for `use_session_stream`** — Phase 3 (deferred
  D8 from the plan). User currently flips via SQL or `k2so mode`
  (allowlist extension lands in D2's change).
- **Metal punch-through desktop rendering** — Phase 8 (target 0.35.0+),
  separate release per PRD.

## Pre-existing issues flagged during Phase 2

- **`companion::settings_bridge::tests` race.** Global OnceLock slot
  contention on parallel runs; intermittently fails first invocation,
  clears on re-run. Fix: add `TEST_LOCK: parking_lot::Mutex<()>` at
  the module top and acquire at each test's start. One-liner.

## Next actions

1. Manual smoke by Rosson: run the D5 example against a few commands
   (`echo`, `cat`, optionally `claude`) and verify Frame output looks
   reasonable.
2. Hold on merging `feat/session-stream` to `main` until Phase 3
   (Awareness Bus routing) lands on the same branch. First 0.34.0
   release requires Phases 1+2+3 together.
3. Plan Phase 3 (awareness-bus.md PRD section + commit list).
