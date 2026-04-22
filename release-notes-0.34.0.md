# 0.34.0 — Session Stream + Kessel: a second terminal renderer

> **tl;dr** The daemon now owns every `/cli/*` route (Tauri is a pure HTTP/WS client), every PTY session is observable by any subscriber via a typed Frame stream, and there's a new React terminal renderer (**Kessel**) that you can toggle on per-preference. Alacritty stays the default — Kessel is explicitly BETA and opt-in. 1045+ tests green. Five months of architecture work, shipped.

This is the pipeline release. 0.33.0 made the daemon persistent; 0.34.0 makes it the *only* HTTP server and gives every byte that flows through it a typed, subscribable shape. Every new capability we build from here (highlight-and-ask-Claude, timeline scrubber, mobile companion parity, Metal rendering) derives from the Session Stream primitive this release introduces.

## The product headlines

### 1. Daemon-complete architecture

**Tauri no longer binds a TCP listener.** Every `/cli/*` route in K2SO is served exclusively by `k2so-daemon`. The Tauri desktop app is a pure consumer: HTTP client for commands, WebSocket client for streams. This means:

- **Lid-closed operation.** Agents, scheduled heartbeats, companion sessions, every `k2so msg` and `k2so agents triage` — none of them require the K2SO window to be open. The daemon runs them.
- **Thin clients are first-class.** The same WebSocket protocol Kessel uses is available to any mobile companion, web viewer, or remote-attach CLI tool. They all see the same Frame stream.
- **Remote connection story is unblocked.** Daemon on workstation, viewer on phone over tailscale — same protocol as local.
- **Open-core split mechanical.** The daemon is MIT. Premium UI viewers can live in a separate crate without compromising the open primitive.

### 2. Session Stream (Frame pipeline)

Every PTY session the daemon owns is now a subscribable Frame stream. Consumers subscribe to `/cli/sessions/subscribe` over WebSocket and receive typed events:

```
Frame::Text        { bytes, style }  — UTF-8 + SGR colors + attrs
Frame::CursorOp    { Goto/Up/Down/EraseInLine/... }
Frame::SemanticEvent { kind, payload }
Frame::AgentSignal { ... }
Frame::RawPtyFrame { bytes }  — opaque passthrough
```

The archive writer writes these to NDJSON on disk (rotating), so every session is replayable. The awareness bus fans signals out to subscribers with `Live` (real-time inject) vs `Inbox` (async notice) semantics.

### 3. Kessel — a second terminal renderer (BETA)

**Settings → Terminal → Terminal Renderer** now lets users pick between:

- **Alacritty (legacy)** — the classic in-process alacritty_terminal engine + DOM renderer. Production-hardened; the default. Unchanged behavior for every user.
- **Kessel (BETA)** — subscribes to the daemon's Session Stream WebSocket and renders from Frame events. Device-agnostic (no Tauri coupling), SGR-colored, cursor-blinking, paste-enabled.

Flipping the toggle only affects **new** terminals; existing tabs keep the renderer they were created with. Users who hit any Kessel gap can flip back and keep working.

**Known Kessel gaps** (documented in-app as BETA):
- Cursor can "hop" during rapid Claude output (500ms blink masks most of it).
- Alt-screen buffer (vim / htop) not yet wired — full-screen TUIs will look garbled.
- Bracketed-paste mode not honored (raw pastes still work fine for bash/zsh/claude single-line).
- Mouse reporting (tmux scroll-wheel) not forwarded.

All of these are fidelity issues, not correctness bugs. Users who need them today pick Alacritty. 0.34.N is where the polish lands.

### 4. Harness Lab (Cmd+Shift+K)

A dedicated sandbox for testing new TUIs or previewing mobile-companion layouts. Drop in a command, hit Spawn, see a live Kessel pane. Independent of your real tabs — safe to experiment without affecting your project terminals. Intended to become K2SO's "tuning bench" for future harness adapters (Phase 6).

### 5. Awareness Bus primitives

Cross-agent signals are now a first-class primitive. The CLI's `k2so msg` routes through it (no more Tauri-only `/cli/msg` handler); the bus handles `Live` vs `Inbox` delivery, pending-live durability (signals persist if the target is offline), and audit (every signal writes an `activity_feed` row).

### 6. Tiered CLI help

`k2so help` shows the ~20 verbs a Workspace Manager or custom agent uses day-to-day. `k2so help --advanced` surfaces the full surface — heartbeat schedule CRUD, daemon lifecycle, session plumbing, hook diagnostics. Less overwhelming on first encounter.

### 7. Test suite expansion

- 445 core integration tests (from 275 in 0.33.0)
- 79 daemon integration tests (from 48)
- 70 Kessel TypeScript unit tests (new)
- 531 shell-based behavior tests
- Total: 1045+ passing

A new `tests/README.md` maps the full testing surface for future contributors.

## What's new under the hood

### Kessel module (`src/renderer/kessel/`)

```
types.ts               Frame / Style / CursorOp wire types (TS mirror)
client.ts              KesselClient — WS lifecycle, envelope parsing
grid.ts                TerminalGrid — 2D cell + cursor + scrollback
style.ts               SGR → React.CSSProperties
selection.ts           Pure selection geometry (for future programmatic features)
SessionStreamView.tsx  The pane — renders grid, handles keys/resize/paste
KesselTerminal.tsx     Tab-pane wrapper — spawns on mount, feeds SessionStreamView
HarnessLab.tsx         Cmd+Shift+K sandbox
```

All gated behind the user-preference renderer toggle. Dead-code for anyone using Alacritty.

### SGR parsing in LineMux

The daemon's LineMux now maintains an SGR state machine — it parses `ESC[m` sequences and populates `Style` on `Frame::Text`. Supports:

- 16-color palette (30-37, 40-47) + bright variants (90-97, 100-107)
- 256-color (`ESC[38;5;N`)
- Truecolor (`ESC[38;2;R;G;B`)
- Bold / italic / underline
- Reset + attribute-off codes

Wire format matches alacritty's `Style` struct so a future consumer could feed either source into the same renderer.

### Daemon is the sole HTTP server (Phase 4 migration)

H1 — H7 of Phase 4 moved every `/cli/*` route from `src-tauri/src/agent_hooks.rs` (which is now dead code) into `k2so-daemon`. `heartbeat.port` / `heartbeat.token` are owned by the daemon; Tauri reads them to populate its in-process `hook_config` for alacritty child-env injection.

New daemon routes (all of them serving what the old Tauri HTTP listener served):

```
/cli/terminal/read          H1 — replay frames as displayable lines
/cli/terminal/write         H1 — PTY bytes in
/cli/terminal/spawn{,-background}  H3 — spawn via daemon
/cli/agents/running         H2 — session_map enumeration
/cli/agents/{launch,delegate}  H5 — daemon-owned agent launches
/cli/companion/{sessions,projects-summary}  H4 — cross-workspace queries
/cli/sessions/resize        I7 — resize a live session's PTY
/cli/sessions/subscribe     D4 — the Frame stream WS
```

Full Phase 4 completion note at `.k2so/notes/phase-4-daemon-standalone-complete.md`.
Phase 4.5 completion note at `.k2so/notes/phase-4.5-kessel-complete.md`.

## What's NOT in 0.34.0 (deferred to 0.34.N / later)

- **Alt-screen buffer / bracketed-paste / mouse reporting in Kessel.** 0.34.1 polish targets.
- **Theme support in Kessel.** xterm-defaults only for now.
- **Tier 1 harness adapters** (stream-json per-harness for Claude Code / Codex / etc.). Phase 6; 0.34.1 / 0.35.0 target.
- **Metal punch-through renderer** (Phase 8). 0.34.2 target.
- **Pi extension pack** (Phase 7). Deferred indefinitely.
- **Alacritty removal.** Intentionally kept as a fallback alongside Kessel. Reframed as "renderer options" instead of a phase gate.

## Upgrade path

Backwards compatible with 0.33.0. Every user sees zero change unless they:

- Open Settings → Terminal and flip "Terminal Renderer" to Kessel (BETA).
- Open the Harness Lab (Cmd+Shift+K).

For everyone else, terminals render exactly as they did in 0.33.0 — same alacritty engine, same DOM spans, same behavior.

## Credits

65+ commits on `feat/session-stream` by **Rosson Long** + Claude (Opus 4.7, 1M context) across Phases 1 → 4.5. Commit ladder organized as C1-C6 (Tier 0 producer), D1-D7 (Session Stream), E1-E8 (Awareness Bus), F1-F3 (live inject + durability), G0-G6 (hardening), H1-H7 + 4 fixes (daemon migration), I1-I11 + 7 fixes + 2 polish (Kessel). See `.k2so/notes/phase-*-complete.md` for the detailed per-phase narratives.
