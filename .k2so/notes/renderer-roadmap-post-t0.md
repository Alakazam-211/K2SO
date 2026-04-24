# Renderer Roadmap — Post-T0 Pivot

**Date captured:** 2026-04-24
**Context:** After chasing workspace-swap + reflow regressions in Kessel-T0
for two days, we clarified that T0's architecture (shared byte stream,
single PTY width, LineMux collecting bytes already baked at the child's
current screensize) is structurally incapable of multi-stream per-
subscriber reflow. That was supposed to be Kessel's superpower. T0 is the
fallback tier; the real multi-stream story lives at T1 (stream-json /
NDJSON) and T2 (K2SO-aware APC).

## The three-renderer plan

1. **Alacritty_v1** — today's Tauri-local renderer. Tauri owns the PTY.
   Simple, fast, cheap. Does not survive Tauri quit. Cannot be targeted by
   heartbeats. Stays as-is during the transition; deleted after v2 is
   production-stable.

2. **Alacritty_v2** — rename of today's Kessel-T0 path. Be honest about
   what it actually is: daemon-owned PTY + daemon-owned `alacritty_terminal::Term`
   + bytes streamed to a Tauri-local Term. Supports heartbeats (daemon owns
   the session). Works for every CLI tool we ship, JSON-stream-capable or
   not, because it operates at the byte layer. Single-subscriber by design
   — no pretense of multi-stream.

3. **Kessel** (repurposed, fresh start) — daemon-owned session, stream-json
   / NDJSON protocol, multi-subscription, per-subscriber native-width
   rendering. JSON-stream-capable harnesses only (see T1 support table
   below). Supports heartbeats. Experimental tier for T1+T2 features and
   the multi-device story.

The heartbeat constraint ("daemon must own the session") applies to both
v2 and Kessel; both qualify. The user picks their preferred renderer per
workspace; heartbeat uses whichever was chosen.

## T1-capable harnesses (research verified 2026-04-23)

Six CLI tools support stream-json / NDJSON today via the ~same `-p
--output-format stream-json` pattern:

| Tool | Flag | Notes |
|---|---|---|
| Claude Code | `-p --output-format stream-json` | Reference impl; richest docs |
| Gemini CLI | `-p --output-format stream-json` | Same pattern as Claude |
| Cursor Agent | `-p --output-format stream-json` | Partial-token support |
| OpenAI Codex | `codex exec --json` | JSONL; rich item model |
| Goose | `--output-format stream-json` | v1.30.0+; watch schema |
| pi-mono | RPC mode (stdin/stdout JSONL) | Richest event vocab |

**Needs T0.5 TUI scraper (no JSON path):** Aider, GitHub Copilot CLI,
Code Puppy.

**Use non-CLI path instead:** Ollama (HTTP API), Open Interpreter
(Python SDK / FastAPI SSE server), OpenCode (partial JSON; upgrade when
stream-json RFE #2449 lands).

When Kessel launches as the JSON-only renderer, it advertises only the
six T1-capable tools. Everything else routes to Alacritty_v2.

## Deferred — T0 polish phases

These were scoped against Kessel-T0's architecture. Most don't translate
cleanly to Kessel-JSON (no byte stream to virtualize, no snapshot cache
to deserialize, etc.). Archived here in case a specific scenario
surfaces the need later.

| Phase | Title | Why deferred |
|---|---|---|
| B | Client-side snapshot cache on pane unmount | T0-specific: caches byte-stream-derived grid snapshots. Kessel-JSON doesn't have grid snapshots. Alacritty_v2 might reuse the idea but as a *daemon-term* snapshot cache, not a byte replay. |
| C | Cross-launch persistence (IndexedDB + daemon session metadata) | Still valid conceptually for both renderers. Revisit when we know whether users want session handoff across app launches. |
| D | Daemon warm-up on startup | Orthogonal to renderer choice. Still potentially useful; revisit when we measure daemon cold-start cost. |
| E | Lazy pane initialization within large workspaces | Perf optimization specific to N-pane workspaces. Re-evaluate against v2's actual cost profile. |
| F | `kessel_term_attach` yields early | Attach-latency optimization tied to the T0 attach model. Doesn't apply to JSON streams directly. |
| G | Web Worker for snapshot deserialization | Perf work on the byte-snapshot path. Not relevant to JSON frame path. |
| H | Virtualized row rendering | DOM virtualization for large scrollback. Potentially still useful regardless of data source. |
| I | Content-space selection overlay | Selection/copy UX. Applicable to either renderer's DOM output. |
| Creature comforts (Cmd+click URLs + file links) | Applicable to either renderer. Pick up when DOM rendering path stabilizes. |

## On hold but documented — Kessel-T0 fallback fix

Two-step plan from 2026-04-23 to finish today's Kessel-T0 workspace-swap
persistence work. Applies to Alacritty_v2 once we rename; may not need
to run on T0 at all if we pivot straight to the rename.

### Step 1 — Daemon-side idempotent spawn (silent safety net)

In `crates/k2so-daemon/src/spawn.rs` (or wherever `/cli/sessions/spawn`
is handled): before creating a new session, look up `session_map` by
`agent_name` (format: `tab-<terminalId>`). If found, return its
`{sessionId, cols, rows}` instead of spawning a duplicate.

- Frontend untouched.
- No user-visible effect at T0 (the flow currently never calls spawn
  with an already-used `agent_name`).
- Test gate: manual `curl` to `/cli/sessions/spawn` twice with the same
  `agent_name` returns the same `sessionId`. Existing core behaviors
  still green.

### Step 2 — Don't kill on unmount (frontend)

In `src/renderer/kessel/KesselTerminal.tsx`: remove the `kessel_close`
invoke from the unmount cleanup. In `src/renderer/stores/tabs.ts`: add
`kessel_close` to the `removeTab` path so deliberate tab closure still
cleans up.

- Requires Step 1 first — otherwise remount spawns orphan sessions.
- Test gate: workspace swap preserves the daemon session; tab Cmd+W
  actually terminates it.

## Active work pointer

The only task staying on the live board: **Claude stream-json (T1) adapter** —
the first concrete T1 move. Gate all subsequent per-harness T1 adapters
on this one proving out.
