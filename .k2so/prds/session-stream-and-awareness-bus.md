---
title: Agent-first sessions — Session Stream + Awareness Bus
status: proposed
created: 2026-04-19
authors: [rosson, pod-leader]
---

# Agent-first sessions — Session Stream + Awareness Bus

> ⚠️ **This PRD proposes two new primitives that replace significant
> parts of K2SO's current terminal stack.** Much of the design is
> hypothetical and assumes we'll tinker as we go. Stage the rollout
> so each phase is reversible until we've proven the primitive on
> real harnesses. Target for first landing: 0.34.0.

## Problem

K2SO today sits on two accidental architectural choices that are
actively holding us back from two big product wins:

1. **Cross-agent awareness is a collage.** The inbox is markdown files
   under `.k2so/agents/<name>/work/inbox/`; nudges are DB rows in
   `activity_feed`; wakes are PTY-injected stdin strings via `k2so msg
   --wake`. There's no single emit-and-subscribe bus for "agent A wants
   to reach agent B." Agents can't discover each other's presence,
   status, or reservations without polling different surfaces at
   different cadences. This is visibly clunky when you watch two
   agents try to coordinate.

2. **The baked-width problem — multi-device viewing is lossy and
   width-locked.** One harness session means one PTY with one
   `winsize`. When the harness (Claude Code, Codex, Gemini) starts,
   it queries `TIOCGWINSZ` and **bakes** line-wrapping, box-drawing,
   and tool-call card layout into its output at *that* width. Those
   newlines and `│...│` borders are frozen into the ANSI byte stream
   before our emulator sees the first byte. When the mobile
   companion connects at 40 cols, we reflow the already-baked grid
   and colors/tables/boxes break in ways we've been patching for
   months. **No amount of post-hoc reflow inside Alacritty can
   un-bake what the harness already wrote.** The fix must either
   (a) capture semantic structure *before* it's rendered into ANSI
   (Tier 1 / Tier 2), or (b) re-layout structured content natively
   on each client (Tier 0.5 recognizers). The tier model below does
   both.

Add a third, more recent blocker surfaced during the 0.33.0 daemon
migration: **14 of 60 `/cli/*` routes cannot move to the daemon**
because PTY ownership is split between the Tauri process and the
daemon process. Each has its own `k2so_core::terminal::shared()`
instance in its own address space. `/cli/terminal/read`,
`/cli/agents/running`, `/cli/agents/launch`, `/cli/heartbeat` triage,
`/cli/companion/sessions` all need a single authoritative session
pool, and today there isn't one.

These three problems share a single root cause: **"a session" in K2SO
today means "a PTY owned by whichever process happened to launch
it."** Everything else (companion streaming, cross-agent wake,
daemon/Tauri split) is built on top of that wrong unit.

This PRD proposes two new primitives that redefine "session" from the
ground up.

## Invariants we're preserving

- **K2SO is not a harness.** We orchestrate Claude Code / Codex /
  Gemini / Pi; we don't replace them. Nothing here asks the user to
  stop using their chosen harness. The harness always owns the agent
  loop and the LLM context.
- **No AI-harness code in K2SO itself.** The MIT-licensed OSS core
  calls into upstream CLIs via subprocess or npm/extension only. We
  never embed an agent runtime.
- **Desktop users see the native harness TUI.** If they've chosen
  Claude Code, Claude Code's box-drawn UI renders exactly as it does
  in any terminal. No loss of upstream features.
- **Agents keep running when the lid closes.** Persistent-agents
  guarantee from 0.33.0 (#215-#228) is strengthened, not weakened —
  sessions live in the daemon.
- **Filesystem is always a valid ground truth.** Agents that don't
  opt into the awareness bus still send/receive work via
  `.k2so/agents/…` markdown. Nothing we add is required for correct
  baseline operation.
- **Every primitive is forward-compatible with harness opt-in.**
  Baseline (Tier 0) works without harness cooperation. Tier 1 and
  Tier 2 are *bonuses* harnesses can opt into at any pace.

## Core design: two primitives

### Primitive A — K2SO Session Stream

Today: a session = a PTY + winsize, owned by one process.
New: a session = **a typed event stream** owned by the daemon, with
many producers and many consumers.

The daemon owns a **line-oriented** representation of each session.
We use the `vte` crate (Alacritty's foundational ANSI tokenizer,
already a transitive dep) to tokenize bytes, and layer our own
**WezTerm-style state machine** on top that emits `Line` events into
an append-only scrollback plus `Frame` events into the live
broadcast channel. The scrollback has no width — each subscribed
client wraps Lines at its own viewport width. **There is no shared
grid on the daemon side.** This is the core structural difference
from Alacritty's single-grid-per-emulator model, and it's what
unlocks multi-device same-session viewing without reflow fighting.
Reference: WezTerm's mux architecture (clone at
`/tmp/k2so-perf-refs/wezterm`).

```rust
// k2so-core/src/session/stream.rs (hypothetical)
pub enum Frame {
    /// Text run with optional style attributes. Produced by VTE
    /// parser, stream-json content events, and native emitters.
    Text { bytes: Vec<u8>, style: Option<Style> },

    /// Cursor movement / erase operations. Produced by VTE parser
    /// for TUI harnesses; absent on stream-json paths.
    CursorOp(CursorOp),

    /// Harness-recognized semantic event. Produced by per-harness
    /// recognizers (T0.5) or stream-json adapters (T1).
    /// Deliberately small vocabulary — 5 variants + Custom escape
    /// hatch. Small surface keeps the agent-facing mental model
    /// compact and Pi-like.
    ///   - Message     — agent said something (user-facing text)
    ///   - ToolCall    — agent invoked a tool
    ///   - ToolResult  — tool returned
    ///   - Plan        — agent proposed a plan
    ///   - Compaction  — session history compacted
    ///   - Custom(String, Value) — harness-specific escape hatch
    SemanticEvent {
        kind: SemanticKind,
        payload: serde_json::Value,
    },

    /// Cross-agent signal lifted from an APC escape or a CLI emit.
    /// Routed to the Awareness Bus, also visible to consumers for
    /// auditing.
    AgentSignal(AgentSignal),

    /// Opaque PTY byte slice, kept for pixel-perfect replay /
    /// desktop-native rendering / recording.
    RawPtyFrame(Bytes),
}

pub struct Session {
    pub id: SessionId,
    pub harness: HarnessKind,
    pub cwd: PathBuf,
    // Hot broadcast channel — one sender per producer, many
    // receivers (one per consumer). Dropped receivers don't block
    // producers.
    pub frames: tokio::sync::broadcast::Sender<Frame>,
    // Last-N frame ring for late subscribers (mobile reconnect).
    pub replay: Arc<Mutex<RingBuffer<Frame>>>,
}
```

Producers plug in at L3 (see diagram below); consumers subscribe at
L5; the Session Stream at L4 fans frames out. **All sessions live in
the daemon process.** Tauri's terminal pane becomes a WebSocket
consumer, exactly like the mobile companion.

**Frame vocabulary is deliberately harness-neutral.** `SemanticKind`
names (ToolCall, FileEdit, Plan, ToolResult, Compaction, Usage,
MessageStop, …) capture concepts every harness has. They are *not*
lifted from any one provider's SDK — Anthropic's `tool_use_id`,
OpenAI's `function_call`, Aider's diff hunks, Pi's steering queue
all map into the same internal vocabulary via per-harness adapters.
Adapters are where provider-specific trivia lives; Frames are where
it stops. If a harness surfaces a concept the core vocabulary
doesn't cover, it rides on `SemanticKind::Custom(String)` until
enough harnesses share it to earn a first-class variant.

### Session persistence model

Three distinct layers, often conflated, kept explicit here:

| Layer | Shape | Lifetime | Purpose |
|---|---|---|---|
| **Live broadcast** | `tokio::sync::broadcast::Sender<Frame>` | in-memory, session-lifetime | instant delivery to connected consumers at whatever their rate is; slow consumers drop frames, fast ones don't |
| **Replay ring** | bounded `RingBuffer<Frame>` (default N=1000) | in-memory, session-lifetime | late-joining consumer (mobile reconnect, new viewer) gets the last N frames before subscribing to live |
| **Archive log** | append-only NDJSON at `.k2so/sessions/<id>/archive.ndjson` | disk, survives restart | full session history for replay, compaction, audit, session fork parentage |

The archive log is **message-log-shaped** (one NDJSON row per Frame,
ordered), not stream-delta-shaped. This matches how real harnesses
persist sessions (Claude Code's `--resume` JSONL, Aider's git-commit
history) while retaining full frame granularity — including APC
`AgentSignal`s and `SemanticEvent`s that the harness's own log
would drop. The daemon owns the format; each producer contributes
frames through the same channel regardless of harness.

This split resolves the tension surfaced in the claw-code pressure
test ("sessions are logs, not streams"). They're both. Live is a
stream; archive is a log; replay ring is the bridge.

### Harness watchdog

Harnesses hang. Tools hang. Network stalls. The daemon is
responsible for detecting these and recovering without user
intervention. Design:

- Each live session tracks `last_frame_at: Instant`.
- A daemon-side supervisor scans sessions every 30s; sessions with
  no frames for > `idle_timeout` (configurable, default 5 min for
  interactive, 20 min for heartbeat-fired) emit a
  `SemanticKind::SessionStalled` frame.
- Stalled sessions are not auto-killed. They're *flagged* — UI shows
  the stall, CLI exposes `k2so sessions kill <id>`, heartbeat
  scheduler decides whether to respawn.
- Force-kill path: `Session::terminate()` sends SIGTERM to the
  producer's underlying transport (PTY / pipe), waits 5s for clean
  exit, escalates to SIGKILL. Archive log is finalized with a
  `SessionKilled { reason }` frame.
- Upstream harness support for cancellation varies (Claude Code TS
  has abort controllers; Rust claw-code port doesn't). Our watchdog
  doesn't rely on harness cooperation — it works at the transport
  layer.

### Primitive B — K2SO Awareness Bus

Today: cross-agent messaging is three different substrates used
inconsistently. New: **a unified emit/subscribe bus** with typed
messages and pluggable ingress / egress.

```rust
// k2so-core/src/awareness/mod.rs (hypothetical)
pub struct AgentSignal {
    pub id: SignalId,              // uuid
    pub from: AgentAddress,
    pub to: AgentAddress,           // Agent / Workspace / Broadcast
    pub kind: SignalKind,           // Msg | Status | Reservation | Presence | …
    pub body: serde_json::Value,
    pub priority: Priority,         // budget-aware routing
    pub reply_to: Option<SignalId>,
    pub at: DateTime<Utc>,
}

pub enum AgentAddress {
    Agent { workspace: WorkspaceId, name: String },
    Workspace(WorkspaceId),        // "workspace inbox"
    Broadcast,                      // visible to all locally-known agents
}

pub enum SignalKind {
    Msg,             // chat-style message, "text" in body
    Status,          // status-line update
    Reservation,     // file claim / release
    Presence,        // online / idle / away / stuck
    TaskLifecycle,   // started / done / blocked
    Custom(String),  // harness-defined extension
}
```

Ingress: APC frames from Session Stream, `k2so msg` CLI, extension
pack calls, scheduler wakes. Egress: target's inbox (atomic rename),
stream-json steer queue (Tier 1+ harnesses), stdin inject (Tier 0
fallback), activity feed (always, for audit).

Transport is two-tier: **daemon-internal broadcast channel** for the
hot path (instant delivery to live subscribers) + **filesystem watch
at `.k2so/awareness/inbox/<agent>/*.json`** for durability across
daemon restarts and across-workspace delivery.

### Four integration tiers

Harnesses participate in K2SO's session model at four increasing
levels of cooperation. Baseline works for every harness on day one;
tiers above it are strictly additive.

| Tier | Cost to harness | What K2SO gains |
|---|---|---|
| **T0** — zero cooperation | None — harness is launched in its normal TUI mode via PTY. K2SO parses ANSI. | `Text` / `CursorOp` frames only. `AgentSignal` only via `k2so msg` CLI injected as stdin. No `SemanticEvent`. |
| **T0.5** — recognizer fallback | None — same transport as T0. K2SO ships a per-harness *recognizer* that pattern-matches ANSI grid output (Claude's `│…│` box regions, Codex's panels, Aider's diff headers) and lifts them into `SemanticEvent` frames. | Structured events for harnesses that *never* ship a stream-json mode. Mobile companion gets semantic rendering at native width. One file per harness: `recognizers/<name>.rs`. Contained surface; cheap to iterate. |
| **T1** — structured mode | Harness supports a stream-json / NDJSON event mode on stdout (Claude Code today: `claude -p --output-format stream-json`). No K2SO-specific code needed from harness. | Clean `SemanticEvent` frames for every tool call / message / edit, no grid parsing. Content decoupled from layout → native rendering at any width. Steer-queue style delivery for `AgentSignal`. Each harness's adapter lives in `streamjson/<name>.rs`. |
| **T2** — K2SO-aware | Harness emits our reserved APC namespace (`\x1b_k2so:… \x07`), *or* runs a K2SO hook/plugin that emits APC on its behalf, *or* links against an extension pack. | First-class `AgentSignal` emit from inside the agent loop. K2SO-specific tool-call annotations. Zero grid parsing; `SemanticEvent` arrives pre-typed. |

The same session stream + awareness bus serves all four tiers; the
producer at L3 is what changes.

**Tier 2 integration paths vary per harness family.** There is no
single "implement Tier 2" pattern — each harness has its own
extension surface. We publish the APC namespace spec; harnesses
adopt it via whatever hook/plugin primitive they already expose.
Targets and mechanisms (TBD entries filled in during Phase 6 research):

| Harness family | Tier 2 mechanism | Notes |
|---|---|---|
| Claude Code & Rust ports (claw-code) | `PostToolUse` hook subprocess that emits APC to stdout | Plugin manifests declare hooks as shell commands; contract is env-var in, stdout+exit-code out. Hook must be <10ms to avoid stalling the harness's next turn. |
| Codex | TBD (Phase 6 research) | Likely a similar hook pattern; needs direct characterization. |
| Aider | `.aider.conf.yml` + pre/post-edit hook | TBD (Phase 6 research) |
| Pi (pi-Mono) | npm extension pack + module-augmented `CustomAgentMessages` | `@alakazamlabs/k2so-pi-extensions`. Cleanest integration of any harness — APC emission is a 10-line TypeScript hook. |
| Gemini CLI | TBD (Phase 6 research) | |
| Goose | TBD (Phase 6 research) | |

The lesson from the claw-code pressure test: **don't commit to a
specific Tier 2 mechanism in the core design.** The core defines the
APC wire format and the ingress handler. How each harness *reaches*
that wire format is per-adapter, and we only need one per harness
family, not per version.

## Integration constraints

Properties the daemon must hold regardless of how a specific harness
behaves. These are belt-and-suspenders guarantees that let us mix
well-behaved and poorly-behaved harnesses in the same pool.

- **Subprocess isolation.** Every harness runs in its own OS
  process. A harness blocking, hanging, or panicking cannot stall
  the daemon's tokio runtime — they're separated at the kernel
  boundary. This holds even for harnesses whose internal runtime is
  fully blocking (the claw-code Rust port is an example; its
  internal blocking never reaches our event loop).
- **No `.await` while holding a blocking handle.** Daemon code that
  interacts with harness transports (PTY fds, pipe writes) uses
  `tokio::task::spawn_blocking` for any operation that could block
  more than ~1ms. Applies to `waitpid`, synchronous `read/write` on
  raw fds, and any `rusqlite` access.
- **Every producer is cancel-safe.** Dropping the producer task
  must tear down the harness transport cleanly. Producers `impl
  Drop` with SIGTERM escalation (see Harness watchdog above).
- **Frame emission is non-blocking.** Producers use
  `broadcast::Sender::send` which returns immediately; slow
  consumers drop frames rather than slow producers. The replay ring
  is the safety net for consumers that fall behind.
- **Archive writes are decoupled from the hot path.** A dedicated
  archive-writer task consumes frames from a receiver and writes
  NDJSON. Disk slowness never stalls live subscribers.
- **Producer failure is isolated.** One harness's parser panicking
  does not take down the daemon. Producers run in supervised
  `JoinHandle`s; on panic, the session is marked crashed and the
  rest of the pool is unaffected.

## Architecture diagram

```
┌────────────────────────────────────────────────────────────────────────────┐
│  L5 — CONSUMERS (many per session, each picks its encoding)                │
├────────────────────────────────────────────────────────────────────────────┤
│                                                                            │
│   Desktop pane      Mobile companion      Heartbeat log    Search index   │
│   ANSI + cursor     semantic events       plain text       events only    │
│        ▲                    ▲                   ▲                 ▲        │
└────────┼────────────────────┼───────────────────┼─────────────────┼────────┘
         │                    │                   │                 │
         └────────────────────┴───────────────────┴─────────────────┘
                                    │
                                    │  subscribe(encoding)
                                    ▼
┌────────────────────────────────────────────────────────────────────────────┐
│  L4 ★  K2SO SESSION STREAM  (Primitive A, in k2so-core, runs in daemon)    │
│                                                                            │
│   typed frames: Text | CursorOp | SemanticEvent | AgentSignal | RawFrame   │
│   frame router: fan frames out to every subscribed consumer                │
│   replay ring: last N frames for late-joining viewers                      │
└─────────────▲──────────────────▲──────────────────▲───────────────────────┘
              │ frames           │ frames           │ frames
              │                  │                  │
              │                  │                  │     ── AgentSignal ──┐
              │                  │                  │         (lifted into  │
              │                  │                  │          bus below)   │
┌─────────────┴────┐  ┌──────────┴────────┐  ┌──────┴──────────┐          │
│  L3 ★ PRODUCER A │  │  L3 ★ PRODUCER B  │  │  L3 ★ PRODUCER C│          │
│  Tier 0:         │  │  Tier 1:          │  │  Tier 2:        │          │
│  zero cooperation│  │  structured mode  │  │  opt-in native  │          │
│                  │  │                   │  │                 │          │
│  K2SO VTE + APC  │  │  stream-json      │  │  Extension pack │          │
│  parser          │  │  adapter          │  │  APC emitter    │          │
│  (replaces       │  │  (per harness)    │  │                 │          │
│   alacritty_term)│  │                   │  │                 │          │
└─────────────▲────┘  └──────────▲────────┘  └──────▲──────────┘          │
              │ ANSI bytes       │ JSON lines       │ direct emit          │
              │                  │                  │  (in-process)        │
┌─────────────┴────┐  ┌──────────┴────────┐  ┌──────┴──────────┐          │
│  L2  portable-pty│  │  L2  pipe (no PTY)│  │  L2  no transport│          │
│      EXISTS      │  │      new flag     │  │      in-process  │          │
└─────────────▲────┘  └──────────▲────────┘  └──────▲──────────┘          │
              │                  │                  │                      │
              ▼                  ▼                  ▼                      │
┌──────────────────────────────────────────────────────────────────────┐  │
│  L1  HARNESSES  (we don't touch — upstream owns the agent loop)      │  │
│                                                                      │  │
│   claude / codex / gemini       claude -p               Pi runtime   │  │
│   (TUI mode, PTY)               --output-format         + @k2so-ext  │  │
│                                  stream-json            (npm)        │  │
└──────────────────────────────────────────────────────────────────────┘  │
                                                                          │
┌─────────────────────────────────────────────────────────────────────────┴──┐
│  L4 ★  K2SO AWARENESS BUS  (Primitive B, in k2so-core, cross-cuts all      │
│        sessions — every agent emits and subscribes through this)           │
│                                                                            │
│   INGRESS                     ROUTING                    EGRESS            │
│   ───────                     ───────                    ──────            │
│   APC frame in stream    ──►  match {to, kind}    ──►    target inbox      │
│   `k2so msg` CLI                                         (Pi-Messenger-    │
│   extension pack emit                                     style atomic     │
│   scheduler wake                                          rename)          │
│                                                          steer queue (T1/2)│
│                                                          stdin inject (T0) │
│                                                          activity feed     │
│                                                                            │
│   transport: daemon-internal channel (hot path)                            │
│            + filesystem watch (durable, survives restarts)                 │
└────────────────────────────────────────────────────────────────────────────┘
```

Layer key:

| Layer | Role | Status |
|---|---|---|
| L0 | LLM API (Anthropic/OpenAI/Google) | untouched, upstream |
| L1 | Harness (Claude Code, Codex, Pi, …) | untouched, upstream |
| L2 | Transport: PTY *or* pipe *or* in-process | PTY exists; pipe + in-process are new code paths |
| L3 | **Producer** — converts harness output → frames | ★ all three producers are new code |
| L4 | **Session Stream + Awareness Bus** — the two primitives | ★ both entirely new |
| L5 | Consumers — each device gets its best encoding | shells exist (React, companion); subscription logic is new |

## APC side-channel namespace

Tier 2 harnesses (and K2SO's own tools) emit cross-agent metadata
through reserved APC escapes that our VTE parser peels off *before*
any rendering, so they never appear in the user's terminal. Format:

```
ESC _ k2so : <verb> <json-payload> BEL
```

Where `ESC = \x1b`, `BEL = \x07`, and `<verb>` is one of:

| Verb | Payload shape | Effect |
|---|---|---|
| `msg` | `{ "to": "agent-name", "text": "..." }` | Emit AgentSignal, SignalKind::Msg |
| `status` | `{ "text": "..." }` | Emit AgentSignal, SignalKind::Status |
| `presence` | `{ "state": "active\|idle\|away\|stuck", "at": "..." }` | Emit AgentSignal, SignalKind::Presence |
| `reserve` | `{ "paths": ["..."] }` | Emit AgentSignal, SignalKind::Reservation |
| `release` | `{ "paths": ["..."] }` | Emit AgentSignal, SignalKind::Reservation |
| `task` | `{ "phase": "started\|done\|blocked", "ref": "..." }` | Emit AgentSignal, SignalKind::TaskLifecycle |
| `tool` | `{ "name": "...", "input": {}, "id": "..." }` | Emit SemanticEvent, SemanticKind::ToolCall |
| `tool-result` | `{ "id": "...", "ok": true, "output": "..." }` | Emit SemanticEvent, SemanticKind::ToolResult |

APC is invisible in any terminal that doesn't recognize the `k2so:`
prefix — xterm, iTerm2, and Alacritty all silently discard unknown
APC strings. Safe to emit from anywhere a harness writes to stdout.

## Filesystem shape

```
.k2so/
├── sessions/                          # daemon session state (new)
│   └── <session-id>/
│       ├── meta.json                  # harness kind, cwd, agent, started
│       ├── replay.ndjson              # last-N frames for reconnect (capped)
│       └── awareness.log.ndjson       # full signal history for this session
├── awareness/                         # cross-agent bus state (new)
│   ├── inbox/
│   │   └── <agent-name>/              # Pi-Messenger-style per-agent inbox
│   │       └── <timestamp>-<uuid>.json
│   ├── presence/
│   │   └── <agent-name>.json          # { state, at, session_id, pid }
│   └── reservations.json              # single-file, atomic-write
├── agents/<agent-name>/               # unchanged (profile, SKILL.md, heartbeats/)
│   └── work/{inbox,active,done}/      # unchanged — still the durable work queue
└── activity_feed.ndjson               # append-only audit log (also in DB)
```

Durability rule: **anything on disk under `.k2so/` must be
reconstructable into an in-memory bus state on daemon boot.** If the
daemon crashes mid-delivery, a restart picks up every `awareness/inbox/…`
file, re-emits it, and marks it delivered via the same target-agent
processing that would have run in-memory.

## How this closes the last 14 /cli/* routes

The remaining 14 routes from task #230 all boil down to "which
process owns the session pool?" Today the answer is split. After this
PRD, every session is daemon-owned, and each route becomes a trivial
daemon-side query / mutation.

| Route | Today's blocker | New path |
|---|---|---|
| `/cli/terminal/spawn` | Which terminal_manager? | Daemon's session pool — one `create_session(producer, transport)` call |
| `/cli/terminal/spawn-background` | Same | Same, with no default consumer |
| `/cli/terminal/read` | Which process's PTY buffer? | Daemon reads from the session's replay ring |
| `/cli/terminal/write` | Which process's PTY stdin? | Daemon writes to the session's producer (PTY or steer queue) |
| `/cli/agents/running` | Which terminal_manager list? | Daemon enumerates sessions filtered by `agent_name` |
| `/cli/agents/launch` | spawn_wake_pty is Tauri-only | Daemon spawns, Tauri subscribes via WS |
| `/cli/agents/delegate` | Same | Same |
| `/cli/heartbeat` (triage side) | Same | Scheduler already runs in daemon; now it creates sessions directly |
| `/cli/companion/sessions` | Which terminal_manager? | Daemon enumerates + exports its own session list |
| `/cli/companion/projects-summary` | terminal_manager count | Daemon groups sessions by workspace |
| `/cli/msg --wake` | Needs PTY inject | Delivers via Awareness Bus; fallback to PTY inject (T0) is now one branch of one egress |

Net: all 14 land as part of the Session Stream migration, not as
separate work.

## How this fixes the companion reflow (the baked-width answer)

Today: mobile receives a grid snapshot whose newlines and box
borders were baked at desktop width by the harness itself. K2SO
reflows, tables and boxes break, user blames K2SO — but the damage
happened before our parser saw the first byte.

After: **no shared grid at all.** Each device owns its own grid
sourced from the client-agnostic Line + Frame stream. Mobile
renders at 40 cols, desktop renders at 140 cols, both native, zero
reflow. But the Line stream still has width baked in at the
*harness* layer — which is why the **tier model is the real fix,
not the Session Stream alone**:

| Tier | How mobile renders | Source |
|---|---|---|
| **T0** (raw ANSI) | Text legible, box-drawn regions look mangled at phone width. Accepted floor. | VTE parser → `Text` + `CursorOp` frames only |
| **T0.5** (per-harness recognizer) | Tool-call cards, plan blocks, user messages extracted from the ANSI grid as structured data. Mobile re-renders each card natively at phone width. **Desktop still sees the original TUI.** | `recognizers/<harness>.rs` pattern-matches `│...│` regions + `╭─ Tool ...` headers into `SemanticEvent` frames |
| **T1** (stream-json) | Semantic events with no width anywhere. Mobile and desktop both re-render at their own width. | `streamjson/<harness>.rs` parses the NDJSON content stream directly |
| **T2** (APC) | Same as T1, plus `AgentSignal` frames emitted from inside the agent loop. | Harness emits `ESC _ k2so:… BEL` escapes |

**T0.5 is the "have your cake and eat it too" tier.** Same harness
process. Same PTY. Desktop gets its native TUI at desktop width,
mobile gets structured cards at phone width. No winsize flapping,
no duplicate harness instances, no extra LLM spend. The first
recognizer ships for Claude Code in Phase 1; Codex / Aider / Gemini
/ Goose follow per-harness in Phase 6.

One recognizer per harness family. When a harness ships a real
structured-output mode, the recognizer retires and the stream-json
adapter takes over (T1).

## Thin-client story

With every session daemon-owned:

- **Tauri app (desktop)** — one possible viewer. Each terminal pane
  subscribes to `Line + CursorOp + Frame` over WS and maintains its
  own grid at desktop width. Closing the Tauri app doesn't kill
  sessions.
- **Mobile companion** — subscribes to `SemanticEvent + Line`
  frames and renders natively at phone width. Reconnects pick up
  from the replay ring.
- **A future CLI viewer** (`k2so sessions attach <id>`) —
  subscribes to raw frames and pipes them to the calling terminal.
  Essentially `tmux attach` for K2SO sessions.
- **A future web UI** — same model. Thin WS client.
- **A future iOS/Android native app** — same model. The daemon is
  the single source of truth; every viewer is replaceable.

This is the "A+ UX per device" payoff: each client picks the frame
shape that looks best in its rendering context, rather than
fighting over a shared grid.

### Desktop rendering — Metal punch-through (later release, not gated on 0.34.0)

Once the multi-device Line/Frame stream is production-stable on DOM
rendering, desktop migrates to a **native Metal view punched
through the Tauri WebKit layer** — the same technique Zed and VS
Code's integrated terminal both use. React tells Rust the terminal
pane's x/y/w/h; a native Metal view draws glyphs directly beneath
the WebView; selection and copy-paste are wired through native
`NSPasteboard` via hit-testing in the Metal layer (Alacritty's
selection code is the reference).

Key property: **Metal punch-through is a pure rendering choice for
one client (desktop).** It doesn't touch the daemon, the parser,
the Line/Frame stream, or any other client. Mobile, future CLI
viewer, future web UI are all unaffected.

References: Alacritty's renderer (`alacritty` crate, not
`alacritty_terminal`), Zed's GPUI text shaping, VS Code's integrated
terminal GPU backend.

Scope: macOS first (Metal). Windows (Direct3D) and Linux (Vulkan)
follow later — consistent with the persistent-agents stance.

**This is a separate release (target 0.35.0+), not gated on
0.34.0.** 0.34.0 ships Line-based DOM rendering on desktop plus
native mobile rendering — both already solve the baked-width
problem. Metal punch-through is vision-aligned bonus UX, not a
blocker.

## CLI surface changes

### Existing commands (semantics refined, UX unchanged)

- `k2so agents launch <name>` — still launches. Now: daemon spawns
  session, Tauri subscribes if open. Works with or without Tauri.
- `k2so msg <from> <to> <text>` — still sends. Now: primary delivery
  is Awareness Bus. Falls back to stdin inject for Tier 0 targets.
- `k2so checkin` — still returns the aggregated bundle. Now:
  messages come from Awareness Bus inbox instead of markdown files
  (markdown inbox becomes one of several egress channels rather than
  the primary storage).

### New commands — deliberately minimal surface

Small so agents don't get overwhelmed picking the right tool. **Two
new verbs total** (`sessions`, `signal`), each with a handful of
subcommands/flags.

- `k2so sessions list` — enumerate live sessions across all
  workspaces.
- `k2so sessions attach <id> [--awareness]` — attach a viewer to a
  live session's stream. `--awareness` also tails the Awareness Bus
  for signals related to this session.
- `k2so sessions replay <id> [--tail N]` — dump the replay ring.
- `k2so signal <to> <kind> <json>` — low-level Awareness Bus emit
  (debugging; power users emit APC from scripts directly).

**Folded-in (previously standalone):** `k2so awareness tail` became
`k2so sessions attach --awareness`. Keeps the mental model: *a
session is the thing you attach to.* This matters — we've already
hit CLI-overwhelm with the 60+ k2so subcommands; every new verb is
a decision cost on the agents using us.

## Patterns adopted from reference harnesses

Specific design choices lifted (or explicitly refused) after
characterizing pi-Mono, Pi-Messenger, and the claw-code Rust port.
All are harness-agnostic patterns, not Anthropic-specific lifts.

- **`workspace_root` bound on every session.** Claw-code's
  `Session` struct binds to an explicit worktree path so parallel
  daemons sharing a global session store can't write to the wrong
  directory. We adopt this verbatim on `Session { cwd, .. }` —
  prevents phantom completions and mis-routed frame archives when
  multiple K2SO workspaces or worktrees run simultaneously.
- **WezTerm mux architecture — line-oriented, client-agnostic
  scrollback.** WezTerm's `local` crate parses each PTY once into a
  canonical line representation (`Line` + `SequenceNo`) that clients
  subscribe to and wrap locally at their own width. No shared grid.
  **This is the load-bearing pattern that makes multi-device same-
  session viewing possible without reflow fighting.** We adopt it as
  the daemon-side model. Reference cloned at
  `/tmp/k2so-perf-refs/wezterm`.
- **Producer trait modeled on `ApiClient`-style abstraction.** A
  `trait Producer` with one method (`fn start(self, frames:
  FrameSink, ctx: SessionCtx) -> JoinHandle`) lets any transport
  contribute to the same Session Stream. PTY producer, stream-json
  producer, in-process producer, and a mock producer for tests all
  implement the same trait. Matches claw-code's `ApiClient` /
  `ProviderClient` split.
- **Generic `Session<P: Producer, S: Storage>`.** Generic over
  producer and persistence backend so the session type is reusable
  in tests (with a mock producer and in-memory storage), in the
  daemon (PTY producer + NDJSON storage), and in a future embedded
  viewer (subscribe-only producer + no storage). Matches claw-code's
  `ConversationRuntime<C, T>` generic shape.
- **Atomic append-only NDJSON for archive.** One frame per line,
  fsync via rename into place for idempotence across daemon
  restarts, and claw-code's `write_atomic` pattern. Matches our
  existing `fs_atomic` crate.
- **Atomic-rename delivery for Awareness Bus inbox.** Lifted from
  Pi-Messenger — `inbox/<agent>/<timestamp>-<uuid>.json` via rename,
  zero chance of partial files visible to watchers.
- **Presence-in-registration.** Pi-Messenger pattern: presence
  state lives *inside* the agent's registration record (PID, session
  id, last activity, status message) rather than in a separate
  heartbeat service. Stale detection is a PID check
  (`process.kill(pid, 0)`), not a liveness ping. We adopt for
  `.k2so/awareness/presence/<agent>.json`.
- **Per-coordination-level message budgets.** Pi-Messenger's
  none/minimal/moderate/chatty = 0/2/5/10 emissions per agent per
  session. Prevents noisy agents from flooding the bus. In-session
  enforcement at the emit path; no runtime magic.
- **Side-channel via APC escape sequences.** Pattern proven by
  pi-mono's `\x1b_pi:c\x07` cursor marker. Every terminal emulator
  silently drops unknown APCs, so emitting them is safe from any
  program anywhere. Our namespace is `\x1b_k2so:<verb> <json>\x07`.
- **Enum-per-layer error types, no `anyhow`.** Claw-code convention
  — concrete error enums per layer, `Display` impls, no generic
  `Box<dyn Error>`. We're partway there (some `thiserror` in
  existing code); we'll hold the line at current density rather
  than converting existing code, and stay concrete-enum for new
  code.
- **What we explicitly refuse from reference harnesses.**
  - **In-process agent runtime** (pi-mono) — violates the "K2SO is
    not a harness" invariant.
  - **TypeScript extension runtime embedded in Rust** (pi-mono
    extension-surface style) — wrong substrate for us; Pi
    extension pack lives in a separate npm package.
  - **Slack adapter as a first-class surface** (pi-mono's `mom`) —
    domain-specific, replaced by Awareness Bus.
  - **Single-turn blocking runtime** (claw-code port) — fine for a
    CLI, wrong for a persistent daemon serving many sessions.

## Migration plan

Staged so each phase is reversible. Target 0.34.0 for Phase 3
completion; Phases 1-2 can land earlier as internal plumbing.

### Phase 0 — Scaffolding (1 day)

- Add `session/` module in k2so-core with empty `Frame` / `Session`
  types.
- Add `awareness/` module with `AgentSignal` type.
- No behavior change.

### Phase 1 — Tier 0 producer + first T0.5 recognizer (5-7 days)

- Use the `vte` crate (Alacritty's foundational ANSI tokenizer —
  already a transitive dep, ~2kloc, well-tested) to tokenize bytes.
- Write our own **WezTerm-style line-oriented state machine** on top
  at `crates/k2so-core/src/term/line_mux.rs`. Emits `Line` events
  into an append-only scrollback + `Frame` events into the live
  broadcast channel. **No shared grid at this layer.**
- Wire it as Producer A: PTY bytes → Lines + Frames.
- Ship the first T0.5 recognizer at
  `crates/k2so-core/src/term/recognizers/claude_code.rs`. Pattern-
  matches `│...│` tool-call boxes, `╭─ Tool ...` headers, plan
  blocks; lifts each into `SemanticEvent` frames.
- Behind a feature flag (`use_session_stream`), daemon creates
  sessions via this producer. Tauri still uses `alacritty_terminal`
  by default — flag off = existing behavior unchanged.

### Phase 2 — Session Stream in daemon (3-5 days)

- Wire Session Stream fan-out in daemon.
- Add `/cli/sessions/subscribe` WS endpoint.
- Tauri's React terminal pane gains a "subscribe to session stream"
  mode (feature-flagged). When on, Tauri doesn't spawn PTYs — it
  subscribes. When off, Tauri keeps its current in-process PTYs.
- Flip internal default to "on" once both desktop and mobile render
  correctly.

### Phase 3 — Awareness Bus (2-3 days) — **SHIPPED 2026-04-20**

- APC parser in VTE → `AgentSignal` frames.
- Bus ingress/routing/egress in k2so-core.
- Replace `k2so msg --wake`'s inline PTY-inject with a bus emit.
  Fallback stays for Tier 0 targets that aren't APC-aware.
- `k2so signal` / `k2so awareness tail` CLI commands ship.
- Commits E1–E8 on `feat/session-stream`. See
  `.k2so/notes/phase-3-awareness-bus-complete.md`.

### Phase 3.1 — Live inject + spawn + pending-live durability — **SHIPPED 2026-04-20**

Unplanned sub-phase surfaced during Phase 3 implementation: the
Phase 3 primitives existed but the daemon had no `InjectProvider`
or `WakeProvider` registered, so `Delivery::Live` signals fell
through to audit-only. 3.1 closed that gap:

- Daemon-owned `session_map` (`HashMap<agent_name, Arc<SessionStreamSession>>`).
- `DaemonInjectProvider` + `DaemonWakeProvider` registered at
  daemon startup.
- `POST /cli/sessions/spawn` endpoint + `k2so sessions spawn` CLI
  verb so external callers can create sessions keyed by agent name.
- Pending-live delivery queue at
  `~/.k2so/daemon.pending-live/<agent>/<ts>-<uuid>.json` +
  drain-on-spawn + boot-time replay scan.
- `activity_feed.metadata` now stores the full `AgentSignal` JSON
  for reconstructable audit.
- Commits F1–F3 on `feat/session-stream`. See
  `.k2so/notes/phase-3.1-live-inject-complete.md`.

### Phase 3.2 — hardening before user-visible release (NEW, PENDING)

Unplanned hardening bucket identified during Phase 3.1 that should
land before any user-visible 0.34.0 release. Scope:

- **Harness watchdog** — idle-session detection (no frames for
  N minutes) + SIGTERM/SIGKILL escalation for wedged harnesses.
  Originally in PRD's Phase 3 scope; deferred here.
- **Archive NDJSON rotation** — size + time-based rotation of
  `<project>/.k2so/sessions/<id>/archive.ndjson`. Phase 3.1 MVP
  freezes at 500MB hard-fail-open; 3.2 adds real rotation + a
  `k2so session compact <id>` command.
- **Real scheduler-wake** — `DaemonWakeProvider` currently
  persists queued signals but doesn't actually *launch* the
  target agent's session. A real scheduler-wake primitive that
  launches the session in response to a pending signal closes
  the last delivery latency gap.
- **Per-coordination-level message budgets** — Pi-Messenger
  style none/minimal/moderate/chatty (0/2/5/10 emits per agent
  per session) to prevent noisy agents from flooding the bus.
- **Settings UI toggle** for `use_session_stream` (per-project).
  Today users flip via SQL; 3.2 exposes it in Tauri Settings.

Rough scope: 5-7 commits. No user-visible architecture change —
all hardening on top of what Phase 3.1 already shipped.

### Phase 4 — Finish the daemon migration (1-2 days)

- The 14 stalled `/cli/*` routes drop in as thin dispatchers over
  the daemon's session pool.
- Remove `cli_remove_workspace`'s teardown-mode branch from Tauri
  (teardown helpers migrate to core alongside).
- Task #230 closes.

### Phase 4.5 — Tauri React pane subscribes to Session Stream (NEW, PENDING)

Inserted between Phase 4's route migration and Phase 5's alacritty
removal. This is the **first user-visible wiring moment** — Rosson
opens K2SO, his desktop terminals render from the Frame stream
instead of legacy alacritty grid emission.

- React component opens a WS to daemon's `/cli/sessions/subscribe`.
- Renders `Line + Frame` stream at desktop width using DOM (Metal
  punch-through comes in Phase 8, separately).
- Handles the Phase 2 feature flag — when `use_session_stream='off'`
  on the project, falls back to the legacy grid emission path.
- Feature-flag reversible: desktop can be flipped back to alacritty
  path at any time until Phase 5 actually removes the dep.

Rough scope: 4-6 commits. Conceptually isolated from the daemon
architecture — React-side work + a small Tauri IPC bridge.

### Phase 5 — Delete alacritty_terminal + Tauri in-process pool (1 day)

- Remove `alacritty_terminal` dependency from `src-tauri/Cargo.toml`
  and `crates/k2so-core/Cargo.toml`.
- Delete Tauri's in-process terminal pool.
- Tauri's React pane is now WS-only, consuming Line + Frame from
  the daemon and rendering via DOM (Metal punch-through comes in
  Phase 8, separately).
- Only fires after Phases 1-4.5 have been production-stable for at
  least one release.

### Phase 6 — Tier 1 adapters (1 day per harness)

- `stream-json/claude_code.rs` — parse Claude Code's JSONL.
- `stream-json/codex.rs` — parse Codex's stream mode.
- Each is optional; users opt in via a per-harness flag in workspace
  settings.

### Phase 7 — Extension pack (separate repo, 2-3 days)

- Ship `@alakazamlabs/k2so-pi-extensions` npm package with APC-emitting
  hooks for Pi.
- Document the APC namespace as a public protocol any harness can
  adopt.

### Phase 8 — Metal punch-through desktop rendering (0.35.0+, NOT gated on 0.34.0)

- Native Metal view punched through Tauri's WebView. React sizes and
  positions the pane; Rust draws the glyphs directly beneath.
- Glyph atlas + shaping via `wgpu` or direct Metal-rs, mapping
  cleanly to Zed's GPUI text path.
- Native selection (`NSTextInputClient`-style hit-testing in the
  Metal layer) + copy to `NSPasteboard`.
- References: Alacritty's renderer (`alacritty` crate, not
  `alacritty_terminal`), Zed's GPUI, VS Code integrated terminal
  GPU backend.
- macOS first. Windows (Direct3D) and Linux (Vulkan) follow later.

**This is a separate release.** 0.34.0 ships Phases 1-7 with DOM
rendering on desktop — already solves the baked-width problem.
Metal is vision-aligned bonus UX, not a blocker.

### Reversible checkpoints

Phases 1, 2, 5 are gated by feature flags. If the Tier 0 producer
misrenders a specific harness, flip the flag; alacritty_terminal
comes back. Nothing is deleted until Phase 5, and Phase 5 requires
Phases 1-4 to have been production-stable for at least one release.

## What gets torn out

| Component | When | Why |
|---|---|---|
| `alacritty_terminal` crate dep | Phase 5 | Replaced by our WezTerm-style line mux over `vte` |
| Tauri in-process terminal pool | Phase 5 | All sessions daemon-owned |
| Grid-reflow path for companion | Phase 2 | Companion subscribes to semantic frames |
| `k2so msg --wake` as primary emit | Phase 3 | Awareness Bus handles it; PTY-inject is one egress |
| Split terminal_manager instances | Phase 4 | One pool in the daemon |
| DOM-based desktop terminal rendering | Phase 8 (0.35.0+) | Replaced by native Metal punch-through |

## What stays

- `portable-pty` — Tier 0 transport. No reason to rewrite.
- Every harness — untouched, always.
- SQLite — still the persistence substrate for `activity_feed`,
  `agent_sessions`, `heartbeats`, `projects`.
- The Tauri app — still the desktop UX. Now it's a thin client.
- `.k2so/agents/<name>/work/{inbox,active,done}/` — still the
  durable work queue. The Awareness Bus is the *hot* messaging layer;
  this is the *structured work* layer. Different concerns.

## Edge cases

| Case | Resolution |
|---|---|
| Harness emits a user-typed string that happens to look like our APC prefix | APC is `ESC _ k2so:` — users can't type ESC in a TUI without explicit input mode. In the vanishingly rare case a harness outputs `ESC _ k2so:<junk>BEL` as plain text, our parser drops the sequence (unknown verb) and logs a debug warning. Worst case: a dropped display character. |
| Daemon restart mid-session | Sessions reconstruct from `.k2so/sessions/<id>/meta.json` + `replay.ndjson`. Producers that hold OS resources (PTY fds) are not resurrectable — those sessions are marked "crashed" and the harness restarts via normal heartbeat/resume path. |
| Tauri subscribes to a session the daemon hasn't seen | 404. Tauri shows "session not found; daemon may have restarted." No automatic respawn — user decides. |
| Two consumers at very different speeds (desktop 120fps, log tailing 1fps) | Broadcast channel drops oldest frame for slow consumers. Slow consumer re-syncs from replay ring on next read. No effect on fast consumers. |
| APC with malformed JSON payload | Parser logs, drops, continues. Producers must be robust to garbage; we already are in the stream-json adapter too. |
| Mobile at 40 cols reconnecting after 30 min offline | Replay ring holds last N=1000 frames. If the session has moved past that, mobile gets a "session progressed beyond replay horizon, showing live only" banner and subscribes to current. |
| Harness exits cleanly | Session emits `SessionEnded`, consumers see it, bus emits `TaskLifecycle.done`, activity_feed stamped, daemon archives the session replay to `.k2so/sessions/<id>/replay.ndjson`. |
| Agent sends a message to a non-existent target | Awareness Bus egress fails to find an inbox; emits `SignalKind::Custom("undelivered")` back to sender and logs to activity_feed. Sender's recognizer renders "message undelivered: no such agent." |
| T0 harness with no APC support wants to use `k2so msg` | `k2so msg` CLI emits directly to the bus; harness doesn't need to be APC-aware to *send*. Receiving works via stdin-inject on checkin. |
| Workspace-to-workspace delivery (Awareness Bus across projects) | Address is `AgentAddress::Agent { workspace: WorkspaceId, … }`. Daemon routes across workspaces via `workspace_relations` table (respects connection graph — same rule as today). |
| Two harnesses in the same session (multiplexed pane) | Not supported in v1. A session has one producer. Multiplexing is a future primitive. |
| Tier 0 + Tier 2 mixed output (Pi with @k2so-ext running in a PTY) | Producer A (VTE+APC) handles it natively. ANSI goes to Text frames, APC goes to AgentSignal frames. Same session, both channels. |

## Out of scope for v1

- **Windows / Linux PTY support** — macOS first, same as persistent-agents.
- **Multiplexed sessions** (one pane, multiple harnesses).
- **Cross-daemon federation** (networked K2SO instances talking).
- **End-to-end encryption on the bus** — local-only trust model for v1, same as Pi-Messenger.
- **Rewriting Tauri's React terminal pane** — it keeps consuming
  bytes-style frames; we just source those from WS instead of
  in-process alacritty. Full semantic rendering inside Tauri is a
  later UX pass.
- **An AI harness of our own** — we're not becoming Pi/Claude Code.
  The extension pack ships; the agent runtime doesn't.
- **Rust-native Pi runtime** — the extension pack targets upstream Pi
  (npm). Wrapping Pi in Rust is a separate decision.

## Test plan

### Unit

- `VteParser::feed(bytes) -> Vec<Frame>` with a curated corpus of
  Claude Code / Codex / Gemini session recordings.
- APC extraction: synthetic APC sequences in a byte stream, verify
  each verb produces the right `AgentSignal`.
- Bus routing: each egress (inbox / steer / stdin / feed) mocked,
  verify correct destinations per SignalKind × target.
- Atomic-rename inbox: concurrent writes from two tasks, verify no
  partial files visible.

### Tier 2 (daemon live)

- Spawn a Tier 0 session against `claude`, assert desktop consumer
  sees `Text + CursorOp` frames, mobile consumer sees `SemanticEvent`
  frames, both render without cross-contamination.
- Spawn a Tier 1 session against `claude -p --output-format
  stream-json`, assert no VTE involvement, semantic events flow
  end-to-end.
- Emit an APC `msg` from one session via scripted fixture, assert
  delivery to a second session's inbox within 50ms (hot path) AND
  the filesystem file exists (durability).
- Kill daemon mid-delivery, restart, assert undelivered inbox file
  completes delivery on boot.
- Connect two mobile clients at 40 cols and 60 cols to the same
  session, assert each renders at its own width with no artifacts.

### Tier 3 (structural)

- Feature-flag off → alacritty_terminal still works (reversibility).
- `cargo build --workspace` with `alacritty_terminal` dep removed at
  Phase 5 → clean build.
- All 14 previously-stalled `/cli/*` routes return expected JSON
  when daemon-served post-Phase 4.
- `k2so sessions attach <id>` from a second terminal attaches to a
  live session without affecting the first viewer.

### Soak

- 24-hour heartbeat-driven session run with periodic reconnects
  from mobile, assert no memory growth in daemon beyond the bounded
  replay rings.
- Two agents exchanging messages at 10 Hz for 1 hour via Awareness
  Bus, assert zero lost messages and correct FIFO ordering
  per-sender.

## Open questions (hypothesis-level, resolve by tinkering)

1. **Replay ring size** — N=1000 frames per session? Configurable?
   Adaptive to device bandwidth? Start at 1000 and measure.
2. ~~**Semantic event schema per-harness.**~~ **Resolved
   2026-04-19.** Five core `SemanticKind` variants + `Custom` escape
   hatch: `Message`, `ToolCall`, `ToolResult`, `Plan`, `Compaction`,
   plus `Custom(String, Value)`. Small surface keeps the agent-
   facing mental model Pi-like. Harnesses with concepts outside this
   set use `Custom(kind_name, payload)`; we promote a variant if
   three harnesses independently need it.
3. **APC verb namespace governance** — once published, do we bless
   third-party verbs? Reserve a `k2so-ext:…` or `user:…` subnamespace?
   Defer until the first third party asks.
4. ~~**Backpressure policy.**~~ **Resolved 2026-04-19.** Drop-oldest
   at the broadcast channel. Slow consumers re-sync from the replay
   ring; no fast consumer ever waits on a slow one. Locked in — this
   is the only policy compatible with the multi-device vision (one
   bad WiFi link can't be allowed to slow down every other viewer).
   We'll still measure under real load in Phase 2's soak test, but
   the decision is not up for re-litigation.
5. ~~**How much of the Tauri React terminal pane needs to change.**~~
   **Resolved 2026-04-19.** Two-step transition:
   - **Phases 2-5 (0.34.0):** minimal change — bytes come from WS
     instead of in-process; rendering stays DOM.
   - **Phase 8 (0.35.0+):** the pane becomes a placeholder that
     sizes and positions a native Metal view punched through the
     WebKit layer. Rendering owned natively.
   Each step reversible via feature flag until the next release
   confirms stability.
6. **Do we ship a Tauri-free viewer in the same release?** A pure
   CLI `k2so sessions attach` is a ~200-line side quest. Nice to
   have; defer if schedule tight.
7. **Persistence granularity for `activity_feed.ndjson`** — we have
   both a DB table and now also an ndjson log. Pick one as source of
   truth or keep both (ndjson for fast tail, DB for queries). Defer.
8. **Should Tier 2 extension packs be allowed to mutate frames, or
   only emit them?** Mutating (e.g., post-processing tool calls) is
   powerful but risks coupling. Start emit-only; revisit if needed.
9. **Per-session vs global replay ring** — currently per-session;
   may want a global "last N sessions' tails" for the UI's "recent
   activity" view. Additive, not blocking.
10. **Single-harness-sample risk on the pressure test.** The
    Rust-port characterization of claw-code is one sample from the
    Claude Code family. Before Phase 6 (per-harness adapters),
    commit to parallel short-form characterizations of at least
    Codex and Aider, plus one of Gemini CLI or Goose. Goal: verify
    the Frame vocabulary covers each harness's wire format without
    distortion, and flesh out the Tier 2 table above. If *any*
    harness surfaces a concept that doesn't map to existing
    `SemanticKind` variants without forcing, add a variant before
    committing to the enum.
11. **Upstream contribution vs fork for harnesses missing Tier 1.**
    If a well-used harness (e.g., Aider) never exposes stream-json,
    do we (a) live with T0.5 recognizer forever, (b) contribute
    upstream, (c) fork. Preference order: (b) > (a) > (c). Real
    decision point only when a specific harness forces it.

## Sign-off trail

- 2026-04-19 `rosson` brainstorm session triggered by interrupting
  the 0.33.0 daemon migration at 51/60 `/cli/*` routes. Research
  into pi-Mono + Pi-Messenger surfaces the insight that K2SO's
  problems (reflow, cross-agent comms, migration blockers) all
  share one root cause: "a session is the wrong unit." This PRD
  proposes Session Stream + Awareness Bus as the replacement unit.
- 2026-04-19 `pod-leader` synthesizes the two primitives, the
  three-tier integration model, the APC namespace draft, and the
  phased migration plan with feature-flag reversibility at every
  step. Initial draft captured here for cross-review.
- 2026-04-19 Design pressure-tested against the claw-code Rust port
  (a Rust reimplementation of Anthropic's Claude Code TS harness).
  Outcome: two primitives survive; four amendments folded in:
  (1) explicit three-layer session-persistence split (live /
  replay / archive) resolving the "sessions as logs vs streams"
  tension; (2) new **Tier 0.5** "VTE + per-harness recognizer"
  fallback for harnesses that never ship a structured output mode;
  (3) **Harness watchdog** subsection adding cancel-safety and
  force-kill at the transport layer, not relying on harness
  cooperation; (4) **Integration constraints** section making
  subprocess isolation, non-blocking frame emission, and archive
  decoupling explicit so a poorly-behaved harness can't stall the
  daemon. (5) **Patterns adopted** section captures the specific
  reference-harness designs we're lifting (workspace-root bind,
  Producer-trait split, generic `Session<P, S>`, atomic NDJSON) and
  the ones we're refusing (in-process agent, TS extension runtime,
  Slack first-class surface, blocking single-turn runtime).
- 2026-04-19 `rosson` flags that the research sample was
  Claude-Code-family only. Amendments folded in to prevent
  overfitting: Frame vocabulary explicitly harness-neutral (don't
  lift Anthropic's `tool_use_id`/content-block conventions); Tier 2
  table expanded to show per-harness mechanism variance with TBD
  entries for Codex / Aider / Gemini / Goose; new open question
  #10 commits to parallel short-form characterizations of those
  harnesses before Phase 6 lands the first per-harness adapter.
- 2026-04-20 Phases 1, 2, 3, and 3.1 **shipped** on the
  `feat/session-stream` branch across 26 commits. 411 tests green;
  flag-off workspace build bit-for-bit identical to `v0.33.0`.
  End-to-end peer-to-peer collaboration works via CLI + daemon +
  real PTYs. Phase 3.2 (hardening bucket) and Phase 4.5 (Tauri
  React pane subscribes) added to the migration plan to register
  follow-up work surfaced during implementation. Completion
  summaries at `.k2so/notes/phase-{2,3,3.1}-*-complete.md`.
- 2026-04-19 Second review pass during 0.34.0 surface-area check
  with `rosson`. Three substantive amendments:
  (1) **"The baked-width problem"** named explicitly as the root
      cause of mobile reflow pain — the harness bakes wrap points
      and box borders into ANSI at `TIOCGWINSZ` query time, before
      K2SO sees anything. "How this fixes the companion reflow"
      section rewritten around the tier model as the real answer.
  (2) **Daemon-side model locked to WezTerm mux pattern** (line-
      oriented scrollback, no shared grid, clients wrap at their
      own width). Phase 1 reworked: use `vte` crate for tokenizing +
      write our own line-oriented state machine at
      `crates/k2so-core/src/term/line_mux.rs`. Alacritty's single-
      grid model is structurally incompatible with multi-device
      viewing; we learn from it but don't replicate it.
  (3) **Desktop Metal punch-through rendering** added as Phase 8,
      scheduled for a release after 0.34.0 (target 0.35.0+). Pure
      rendering choice for the desktop client; doesn't touch the
      daemon, parser, or other clients. References: Zed's GPUI, VS
      Code integrated terminal, Alacritty's native renderer.
  Also: `SemanticKind` locked to 5 variants + `Custom` (`Message`,
  `ToolCall`, `ToolResult`, `Plan`, `Compaction`); CLI surface
  trimmed from 5 new verbs to 2 (`sessions`, `signal`); Q2 / Q4 / Q5
  resolved and annotated in the open-questions list.

---

> **Status after drafting:** Proposed. Needs at least one cross-agent
> review pass before any Phase 1 code is written. Specifically want
> `cortana` or another deep-systems reviewer to stress-test the APC
> namespace, the backpressure policy, and the replay-ring
> reconstruction path. Open questions above should be resolved (or
> explicitly deferred) before sign-off.
