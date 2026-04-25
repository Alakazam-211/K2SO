# Kessel — JSON-Stream Multi-Subscriber Renderer (T1)

**Status:** Planned. Resets the Kessel concept from "multi-stream byte-replay"
(T0, proven non-viable) to "multi-stream JSON adapter."
**Captured:** 2026-04-24
**Prerequisite:** Alacritty_v2 shipped and stable. See `alacritty-v2.md`.

## Why Kessel exists (re-scoped)

Kessel's founding promise was **multi-device rendering with per-device
reflow** — a desktop user, a mobile companion, and a web viewer all
watching the same session, each at its native width, each with a
first-class UX.

We tried to build this on a shared byte stream and learned (painfully)
that it's architecturally impossible:

1. A PTY has **one** size. The child process emits bytes laid out for
   that one width.
2. Layout-positioned output (Claude's box-drawn UI, prompt bars, status
   rows) cannot be "reflowed" at the byte layer — only flowing text
   reflows via alacritty's natural wrap behavior.
3. No per-subscriber reflow is possible when all subscribers share one
   byte pipeline.

The real answer is one layer up: consume the harness's **semantic event
stream** (stream-json / NDJSON) instead of its terminal bytes. Events
carry content without layout; every subscriber renders them natively at
its own width. Six CLI tools support this today.

Kessel becomes the renderer for **those six tools**. Every other tool
routes to Alacritty_v2. This is the honest product split.

## The renderer landscape (reminder)

| Renderer | Transport | Subscribers | Tools supported |
|---|---|---|---|
| Alacritty_v1 | Tauri-local PTY | 1 (Tauri) | All | Retires after v2 |
| **Alacritty_v2** | Daemon PTY + Term, WS grid | 1 per session | All CLI tools via TUI | Shipping |
| **Kessel (this PRD)** | Daemon JSON adapter, WS Frames | N per session, native width each | 6 T1-capable tools | Experimental |

## T1-capable tool catalog

Confirmed via research 2026-04-23 (`.k2so/notes/renderer-roadmap-post-t0.md`):

| Tool | Invocation | Event shape |
|---|---|---|
| Claude Code | `claude -p --output-format stream-json` (+ `--include-partial-messages`) | `system` init, `user`, `assistant`, `result` messages (NDJSON) |
| Gemini CLI | `gemini -p --output-format stream-json` | NDJSON events (Google schema) |
| Cursor Agent | `cursor-agent -p --output-format stream-json` | NDJSON: `system`, `user`, `assistant`, tool_call events, `result` |
| OpenAI Codex | `codex exec --json` | `thread.*`, `turn.*`, `item.*`, `error` events (OpenAI schema) |
| Goose | `goose --output-format stream-json` | NDJSON events (Goose-specific) |
| pi-mono | RPC mode (stdin/stdout JSONL) | `agent_*`, `turn_*`, `message_*` with token-delta granularity |

Three tools need T0.5 recognizers (Aider, GitHub Copilot CLI, Code Puppy).
Three have non-CLI paths (Ollama → HTTP API; Open Interpreter → Python
SDK/SSE; OpenCode → partial JSON, upgrade on RFE #2449). T0.5 and these
edge cases are deferred; Kessel v1 ships T1 only.

## What we keep from the current build

Material salvage from Kessel-T0 that still makes sense for T1:

| Piece | Current location | Why it survives |
|---|---|---|
| `session::registry` + `SessionEntry` | `crates/k2so-core/src/session/` | Multi-subscriber broadcast primitives — exactly what per-device rendering needs. Fan-out is already solved here. |
| `session::Frame` types | `crates/k2so-core/src/session/frame.rs` | Width-free semantic events (`Text`, `SemanticEvent`, `AgentSignal`, etc.). The normalized frame is already the right shape; we add/refine variants for T1 needs. |
| `sessions_ws.rs` | `crates/k2so-daemon/src/sessions_ws.rs` | Frame-stream WebSocket endpoint. Already multi-subscriber, already width-free. Kessel's main wire. |
| PTY + child-spawning | `crates/k2so-core/src/terminal/session_stream_pty.rs` (subset) | For harnesses that need a PTY (Claude's stream-json runs fine on plain pipes, but some tools do expect a TTY). Keep the spawn + writer; drop grow-shrink + APC coordination + LineMux feed. |
| `awareness::ingress` | `crates/k2so-core/src/awareness/` | Heartbeat + agent-message injection into sessions. Unchanged. Kessel sessions get this for free. |
| T0.5 recognizer scaffold | `crates/k2so-core/src/term/recognizers/` | Not used by T1 path, but kept alive for the three tools that need it. Retires per-tool when T1 becomes available. |
| `Frame::SemanticEvent` with harness-specific payloads | `session::Frame` | The adapter layer emits these. Renderers consume them. |

## What we discard

Code that was written for T0's multi-stream byte-replay and has no role
in T1:

| Piece | Action |
|---|---|
| Byte ring (`bytes_snapshot_from`, `publish_bytes`) | Remove or demote. T1 doesn't replay bytes; subscribers receive Frames from the moment of subscription + a small Frame-level catchup if needed. |
| `sessions_bytes_ws.rs` | Remove after Alacritty_v2 cutover. |
| Grow-then-shrink | Remove. Not applicable to Frame streams. |
| APC `k2so:grow_boundary` emission | Remove with grow-shrink. |
| APC filter in `src-tauri/.../kessel_term.rs` | Remove (goes with the whole Tauri-side byte-stream renderer). |
| `src-tauri/src/commands/kessel_term.rs` current form | Replace with a Frame-consuming renderer. Local `alacritty_terminal::Term` on Tauri side goes away. |
| `KesselTerminal.tsx` + `SessionStreamViewTerm.tsx` current form | Replace with a Frame-consuming component. |
| Tauri-side `alacritty_terminal` + `vte` deps | Remove from `src-tauri/Cargo.toml`. Tauri doesn't parse ANSI for Kessel at all. |
| Grow-settle driver (`terminal/grow_settle.rs`) | Remove. |
| `LineMux` on Kessel's path | Remove from the Kessel reader loop. LineMux stays in the daemon for *other* consumers (heartbeat activity detection, T0.5 where still used). |

## What we build new

### New: Harness Adapter trait

One trait, one implementation per T1-capable tool. Lives in
`crates/k2so-core/src/harness/`.

```rust
/// A harness adapter consumes a CLI tool's structured output stream
/// and normalizes it into K2SO's semantic Frame schema.
pub trait HarnessAdapter: Send {
    /// The adapter's canonical name — e.g. "claude-code", "codex",
    /// "gemini", "cursor-agent", "goose", "pi-mono".
    fn name(&self) -> &'static str;

    /// How to spawn this harness in its T1 mode. Returns command +
    /// args ready to hand to portable-pty's CommandBuilder.
    fn spawn_invocation(&self, cfg: &HarnessSpawnConfig) -> HarnessInvocation;

    /// Feed a chunk of stdout bytes. Emit any normalized Frames that
    /// the chunk completed. Adapter is stateful across calls (it
    /// buffers partial JSONL lines).
    fn feed(&mut self, bytes: &[u8]) -> Vec<Frame>;

    /// Convert a user input (from any subscribing client) into bytes
    /// to write to the harness's stdin. Default = plain UTF-8 + \n.
    fn encode_user_input(&self, text: &str) -> Vec<u8> { ... }
}
```

Adapter modules:
- `harness/claude_code.rs`
- `harness/codex.rs`
- `harness/gemini.rs`
- `harness/cursor_agent.rs`
- `harness/goose.rs`
- `harness/pi_mono.rs`

Each is self-contained; ~200-400 lines of per-tool JSON parsing.

### New: Normalized Frame vocabulary (extends `session::Frame`)

Existing `Frame` variants: `Text`, `CursorOp`, `ModeChange`,
`SemanticEvent`, `AgentSignal`, `Bell`.

Add semantic richness. Concrete additions (exact names TBD during impl):

- `Frame::Message { role, content, timestamp }` — a completed user or
  assistant message.
- `Frame::MessageDelta { role, text, content_type }` — streaming token
  deltas (for harnesses that support partial messages: Claude with
  `--include-partial-messages`, Cursor with `--stream-partial-output`,
  pi-mono native).
- `Frame::ToolCall { name, args, call_id }` — tool/function invocation.
- `Frame::ToolResult { call_id, result, error }` — tool completion.
- `Frame::Thinking { text }` — chain-of-thought / reasoning blocks
  (Codex has these explicitly; Claude via partial-messages).
- `Frame::PlanUpdate { items, current }` — planning blocks (Codex
  native; Claude plan blocks via recognizer-style logic).
- `Frame::FileEdit { path, before, after }` — file modification event.
- `Frame::SystemInit { harness_name, session_metadata }` — first event,
  tells subscribers what they're connected to.
- `Frame::TurnBoundary { state: started | completed | failed }` —
  explicit turn lifecycle.

Each harness's adapter maps its native event shape to this common
vocabulary. Variants are additive; new ones can be added without
breaking existing renderers (unknown variants = fall back to `Text`).

### New: Harness Session spawn path

New module `crates/k2so-core/src/harness/session.rs`. Parallels
`terminal/daemon_pty.rs` but:

- Spawns the harness via its `HarnessAdapter::spawn_invocation()`.
- Reads stdout chunks, feeds into the adapter, publishes resulting
  Frames to the `SessionEntry`'s broadcast channel.
- Writes to the harness's stdin via a writer mutex (for user input +
  heartbeat injection — same primitive Alacritty_v2 uses).
- May or may not allocate a PTY. Adapter decides. Claude/Gemini/Cursor/
  Goose work on plain pipes; pi-mono explicitly uses stdin/stdout; the
  invariant is "whatever the CLI expects for its T1 mode." No PTY is
  required for non-TUI tools.

The `session::registry` already has a place for this; a Kessel session
is just a `SessionEntry` with a different producer.

### New: Per-harness renderer (client side)

Kessel's frontend value is different from Alacritty_v2's. Alacritty_v2
renders terminal grids. Kessel renders **conversation and tool-use UIs**.

The Tauri component (`KesselSessionView.tsx`, replacing the current
Kessel pane) is a Frame subscriber that renders each Frame type as a
dedicated React component:

- `Frame::Message` → `<MessageBubble role="user|assistant" content=... />`
- `Frame::ToolCall` → `<ToolCallCard name=... args=... />` (collapsible;
  shows args inline, expands for full JSON)
- `Frame::ToolResult` → inlines into the corresponding `ToolCallCard`.
- `Frame::Thinking` → collapsible `<ThinkingBlock>` (closed by default).
- `Frame::PlanUpdate` → `<PlanTracker>` with checkboxes.
- `Frame::FileEdit` → `<FileEditPreview path=... diff=... />`.

This is fundamentally different from a terminal-grid DOM renderer. It's
closer to a chat app than a terminal. That's the point: it's the
rendering mobile companions and web viewers will use too.

Desktop Kessel and mobile Kessel share the Frame vocabulary; rendering
diverges per platform (React DOM vs React Native vs whatever mobile
uses). Frame schema is the contract.

### Optional: Upstream-to-Kessel bridge for T0.5 fallback

When a user opens a Kessel pane for a non-T1 tool (Aider, Copilot,
Code Puppy), we can either:

1. **Refuse.** Kessel v1 supports T1-only. Other tools use Alacritty_v2.
2. **Fall back to T0.5 recognizer.** Run the tool in its TUI mode, feed
   output through `LineMux` + `recognizers/<tool>.rs`, convert
   recognized patterns to Frames.

Ship with (1). Revisit (2) only if users ask for rich Kessel rendering
on a non-T1 tool.

## Architecture

```
User opens a Kessel tab for Claude:

[Tauri UI] ─┐                          Multi-subscriber; each at own width.
[Mobile]  ──┼──► WS /cli/sessions/subscribe  (Frame stream)
[Web]     ──┘          ▲
                       │
               [session::registry / SessionEntry broadcast]
                       ▲
                       │ publishes Frames
                       │
          [harness::session with ClaudeCodeAdapter]
                       ▲
                       │ stdout chunks → adapter.feed() → Frames
                       │
             [child process: claude -p --output-format stream-json]

                       ↓ stdin (user input, heartbeat signals)
             [session.write()] ◄──── encode_user_input(text)
```

Symmetry with Alacritty_v2:
- Both use `session::registry` for session ownership + heartbeat
  eligibility.
- Both have a daemon-side producer (Alacritty_v2's `daemon_pty.rs` /
  Kessel's `harness::session`).
- Both expose a WS endpoint with matching patterns (snapshot/delta
  grid for v2; Frame stream for Kessel).
- Both accept resize / input from the client, route through the
  session writer.

Different where it should be different. Shared where it should be
shared.

## Creature-comfort parity requirements (learned during v2)

When Alacritty_v2's TerminalPane shipped in A5/A7, we found that
several "it just works" interaction affordances from Alacritty_v1
weren't automatic — they had to be ported in deliberately. Kessel's
rich-component renderer is architecturally different (it renders
Message / ToolCall / Thinking cards, not terminal cells), but most
of these affordances still apply and are easy to forget until a
user hits the missing behavior. Capture them here as Day-1
requirements for the Tauri Kessel view in K5:

1. **Link detection and click handling.**
   Text content (assistant messages, tool-call results, file
   edits) regularly contains URLs and file paths. Use the same
   `detectLinks` helper from
   `src/renderer/components/Terminal/terminalLinkDetector.ts`
   against rendered text. URL click →
   `invoke('open_external', {url})`. File path click →
   `useTabsStore.openFileInNewTab(path)` OR
   `openFileInPaneGroup(siblingId, path)` if the user has
   "Open Links in Split Pane" on. Respect the Cmd-click-vs-click
   mode from `useTerminalSettingsStore`.

2. **Paste handling with file-clipboard support.**
   Cmd+V of a file copied in Finder comes through via
   `NSFilenamesPboardType`, NOT the web Clipboard API. Always
   call `invoke('clipboard_read_file_paths')` on paste; if it
   returns paths, format them via the shared helpers in
   `src/renderer/lib/file-drag.ts` (shell-escape + bracketed-
   paste for images) before sending. For Kessel this means
   sending the formatted path as a user-message payload (`{action: "input"}`
   in A3's protocol, or the Kessel equivalent for "compose a
   user message").

3. **Drag and drop from Finder + files tab.**
   `onDragOver` + `onDrop` on the pane container. Multi-file →
   space-joined shell-escaped paths; any image path triggers
   bracketed-paste wrapping so Claude Code's `[Image #N]`
   detector fires.

4. **Focus retention (the non-obvious one).**
   App.tsx's global click handler + 200ms refocus-poll
   (`useEffect` at `src/renderer/App.tsx:~315-348`) finds the
   active terminal via `document.querySelector('[data-terminal-container][data-terminal-visible="true"]')`.
   Kessel's top-level pane container MUST carry both of these
   attributes or the app's "return focus to terminal after
   clicking blank canvas / closing Cmd+K / closing Cmd+L"
   behavior won't work. Also mirror the v1 window blur/focus
   listener that records focus-before-blur and restores it on
   window regain — gives you the Cmd+Tab-back-to-K2SO
   behavior cleanly.

5. **Auto-focus on tab-visible transition.**
   `useIsTabVisible` from `@/contexts/TabVisibilityContext` →
   when the pane becomes visible, `requestAnimationFrame(() =>
   container.focus())`. Solves the "workspace swap return and
   start typing doesn't land in the terminal" problem.

6. **Activity detection / braille spinners (0.35.6).**
   `useActiveAgentsStore.recordOutput(paneId)` on every grid
   update bumps the per-pane heartbeat that drives the
   tab-spinner + sidebar Active section. `recordTitleActivity(
   paneId, isWorking)` flips the per-pane working/idle status —
   for v2 it's wired off the WS `{event:"title"}` channel
   (Claude's braille-spinner glyph in the title prefix → working;
   `✱✲✳✴` family glyphs → idle), with a viewport-text fallback
   in `src/renderer/lib/agent-signals.ts` for tools that don't
   cycle the title.

   Kessel won't have an alacritty Term to scan for status-line
   text, but it CAN tap two cleaner sources directly:
   - The harness adapter's parsed events themselves — every
     adapter emits a `Frame::AgentSignal` (or equivalent) when
     an LLM turn starts/ends. Drive `recordTitleActivity` off
     those transitions; no string scanning required.
   - The native lifecycle hooks (`agent:lifecycle` Tauri event,
     `handleLifecycleEvent` in `active-agents.ts`). Already
     fires for any Claude Code / Codex session regardless of
     renderer; just keep wiring it.

   Crucially, **bind paneId → activeProjectId in
   `paneProjectMap` on the first 'working' transition** (we now
   do this in `recordTitleActivity`). Without it the per-project
   sidebar spinner and the Active Bar's `getProjectStatus` can't
   attribute a working pane to a workspace.

7. **Active Bar 24h tenure (0.35.6).**
   `setActiveWorkspace` now calls `touchInteraction(projectId)`,
   so any workspace you visit stays in the Active Bar for 24h
   and expires on its own. Kessel panes share the same store —
   nothing renderer-specific to do here, but verify the user's
   first Cmd+T into a Kessel-mode workspace lights up the
   sidebar entry the same way v2 does. If Kessel chooses to
   build its own pane store path (it shouldn't), it has to
   touch the same projects-store API.

8. **TUI cursor visibility / hollow-on-defocus (0.35.6).**
   Doesn't directly apply — Kessel renders rich Message /
   ToolCall cards, not terminal cells, so there's no
   alacritty-cursor concept to hide. BUT: when the pane is
   unfocused, render SOME visual indication of focus state on
   the input composer ("▌" caret in muted color when blurred,
   bright when focused — the macOS NSTextView convention).
   Users complain fast when v2 doesn't differentiate focused
   from backgrounded panes; assume the same threshold for
   Kessel.

9. **Bell + title from the daemon (0.35.6).**
   The daemon's `sessions_grid_ws.rs` now forwards alacritty's
   `AlacEvent::Title` / `ResetTitle` / `Bell` over the v2 WS as
   `{event:"title",payload:{title}}` and `{event:"bell"}`.
   Kessel uses a different transport (harness adapter → JSON
   stream), but the same TWO signals are valuable:
   - **Title** — adapters that surface a terminal title or
     conversation name should expose it as a Frame so the
     Tauri tab title can update without polling.
   - **Bell** — the universal "agent waiting for input"
     transition. Most CLI LLMs ring the bell when they finish
     a turn; the OpenAI/Anthropic JSON streams have explicit
     `turn_complete` markers that give us this signal even
     more reliably. Wire it as a definitive idle transition
     (same path as v2's `case 'bell'` handler).

10. **Auto-update spawn retry (0.35.5).**
    v2's `TerminalPane.tsx` retries the daemon-spawn fetch with
    exponential backoff (250 → 2000 ms, 10s ceiling) so the
    auto-updater's "install + relaunch" cycle doesn't leave
    panes in a "spawn fetch failed" state during the ~3-5s
    daemon-restart window. Kessel's spawn path goes through the
    same daemon HTTP API and will hit the same window — copy
    the retry shape verbatim. 4xx surfaces immediately, 5xx +
    network errors retry until the deadline.

These are tangential to Kessel's core architectural work
(adapters, Frame schema, multi-subscriber) but they're what make
a pane feel like a first-class terminal pane. Reserve ~half a
day in K5 to port them over from TerminalPane.tsx, plus another
half-day for the activity / Active Bar / lifecycle plumbing in
items 6–10 above.

## Heartbeat integration

Kessel sessions are `SessionEntry`-hosted; heartbeat already works
against them. The scheduler calls `session.write(bytes)` to inject a
signal; the harness child reads it as stdin; adapter decides how to
render the response (typically as a `Frame::Message { role: "user" }`
followed by the assistant's reply stream).

No Kessel-specific heartbeat code needed. This is why we insisted
session-ownership lives in the daemon.

## Phase plan

| Phase | Work | Effort |
|---|---|---|
| **K1** | Define `HarnessAdapter` trait + `HarnessSpawnConfig` + extend `session::Frame` with the new variants. Pure type design + serde. No adapters yet. | 1 day |
| **K2** | Claude Code adapter (`harness/claude_code.rs`) — the reference implementation. Parse Claude's stream-json, normalize to Frames. Unit tests with recorded NDJSON fixtures. | 2-3 days |
| **K3** | `harness/session.rs` — spawn path, stdin/stdout wiring, Frame publishing to `SessionEntry`. Integration test: spawn Claude, exchange one message, verify Frames. | 1-2 days |
| **K4** | Daemon endpoint: `POST /cli/sessions/spawn_kessel?harness=claude&cwd=...&...` returns sessionId. `/cli/sessions/subscribe` already streams Frames — no new WS needed. | 0.5 day |
| **K5** | Tauri `KesselSessionView.tsx` — Frame subscriber + rich-component renderer. Starts with `Message`, `ToolCall`/`Result`, `Thinking`. | 2-3 days |
| **K6** | Parity harness with Alacritty_v2: run the same Claude session in both renderers side-by-side, compare content fidelity. Identify rendering gaps. | 1 day |
| **K7** | Codex adapter (second implementation — validates the trait handles schema differences). | 1-2 days |
| **K8** | Gemini + Cursor Agent adapters (same schema family as Claude, quick adds). | 1 day each |
| **K9** | Goose + pi-mono adapters (higher variance, do last). | 1-2 days each |
| **K10** | Mobile companion Kessel renderer (if mobile app exists by then). Shares Frame schema; different React root. | Depends on mobile scope |
| **Total** | | **~14-20 days focused work**, stageable across weeks |

K1-K5 is the critical path: one tool (Claude) working end-to-end in
Tauri. After that, each additional adapter is ~1-2 days and ships
independently.

## Non-goals for Kessel v1

- T0.5 recognizers for Aider/Copilot/Code Puppy. Deferred.
- T2 APC protocol (agent-originated signals from inside the harness
  loop). Deferred to T2 phase.
- Native peer-to-peer via Kessel. The Awareness Bus already delivers
  `AgentSignal`s; that works for T1 today. Deeper in-agent awareness
  is a T2 play (see below).
- Retroactive history replay. New subscribers get Frames from the
  moment they subscribe forward. Full session history (for "open the
  app, see the whole conversation") requires a history mechanism —
  either a Frame-level ring on `SessionEntry`, or per-harness history
  replay (Claude's `--resume` gives you the whole conversation as a
  series of Frames; that's probably the right answer).
- Tool-specific UI polish (Claude-flavored plan blocks vs Codex-
  flavored). Baseline rendering first; per-harness gloss later.

## Bonus goal (T2 future)

Your stated long-term wish: "inject a native peer-to-peer communication
layer into every CLI LLM we use so each harness has a native knowledge
of its peers."

This is T2 territory, not Kessel v1. Shape it'd take:

1. Each cooperating harness runs a K2SO plugin/hook (form varies per
   harness — Claude Code plugin API, Codex hook, pi-mono extension, etc.).
2. The plugin emits `\x1b_k2so:<kind>:<json>\x07` APC escapes inline
   with the harness's normal output.
3. Our byte pipeline (or the adapter's stdout path) extracts the
   escapes, converts to `Frame::AgentSignal`.
4. The Awareness Bus routes signals to other agents' sessions. Each
   agent sees peer signals *inside* its own event stream — "peer
   agent-X just finished task Y" becomes a semantic event the agent
   can reason about.

Build after T1 is proven and we have at least one cooperating harness.

## Open questions — decide during K1–K3

1. **Frame history for new subscribers.** Do we keep a small ring of
   recent Frames on `SessionEntry` (say, last 1000) so a late
   subscriber sees immediate context? Or require the subscriber to
   explicitly request replay? Leaning: small ring (100-1000 Frames)
   as catchup; full history via `--resume`-style adapter restart.
2. **User input routing across multi-subscribers.** Desktop and mobile
   both connected to the same Claude session; both can send messages.
   First-write-wins? Soft-lock? Optimistic UI with rollback? Needs a
   concurrency design.
3. **Streaming partial messages.** Should `Frame::MessageDelta` always
   fire (at the token level) or gate behind a per-subscriber "watch
   live typing" flag? Bandwidth vs. responsiveness.
4. **Error + session-end Frame variants.** `Frame::HarnessError`,
   `Frame::SessionEnded`, `Frame::SubscriberKicked`. Shape during K1.

## Sign-off

- Kessel is **JSON-stream only**, supporting six CLI tools out of the
  gate.
- Multi-subscriber native — desktop, mobile, web, each at own width.
- Built on primitives we already have (`session::registry`, Frame WS).
- Discards every byte-level ambition; keeps the fan-out infrastructure.
- Shares heartbeat + Awareness Bus with Alacritty_v2. Same plumbing.
- Experimental tier until at least Claude adapter is in daily use.
- Deferred: T0.5 recognizers, T2 in-agent signaling, retroactive replay
  semantics.

Start order: **Alacritty_v2 first (stabilize the heartbeat-capable
foundation). Kessel K1-K5 next (Claude adapter end-to-end). Everything
else stages onto that.**
