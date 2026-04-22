// Kessel — TypeScript types mirroring the Rust Session Stream wire
// format. Keep these in lockstep with `crates/k2so-core/src/session/
// frame.rs` and `crates/k2so-core/src/awareness/mod.rs` — adjacent-
// tagged serde variants serialize to specific JSON shapes and the
// grid/renderer layers rely on those shapes being stable.
//
// Wire envelope (from crates/k2so-daemon/src/sessions_ws.rs):
//   - "session:ack"   payload: { sessionId, replayCount }
//   - "session:frame" payload: Frame
//   - "session:error" payload: { message }
//
// Rust `Vec<u8>` serializes to a JSON `number[]`. Decode with
// `Uint8Array.from(arr)` + TextDecoder for UTF-8.

// ── Envelope ────────────────────────────────────────────────────────

export type KesselEnvelope =
  | { event: 'session:ack'; payload: AckPayload }
  | { event: 'session:frame'; payload: Frame }
  | { event: 'session:error'; payload: ErrorPayload }

export interface AckPayload {
  sessionId: string
  replayCount: number
}

export interface ErrorPayload {
  message: string
}

// ── Frame ───────────────────────────────────────────────────────────

// Adjacent-tagged: `{"frame": "<variant>", "data": <body>}`.
export type Frame =
  | { frame: 'Text'; data: TextFrameData }
  | { frame: 'CursorOp'; data: CursorOp }
  | { frame: 'SemanticEvent'; data: SemanticEventData }
  | { frame: 'AgentSignal'; data: AgentSignal }
  | { frame: 'RawPtyFrame'; data: number[] }
  | { frame: 'ModeChange'; data: ModeChangeData }
  | { frame: 'Bell'; data?: null }

export interface ModeChangeData {
  mode: ModeKind
  on: boolean
}

/** Terminal private-mode identifiers. Kept in lockstep with the Rust
 *  `ModeKind` enum at `crates/k2so-core/src/session/frame.rs`. */
export type ModeKind =
  | 'bracketed_paste'
  | 'alt_screen'
  | 'synchronized_output'
  | 'application_cursor'
  | 'autowrap'
  | 'focus_reporting'

export interface TextFrameData {
  /** UTF-8 bytes. Serde emits `Vec<u8>` as a number array. */
  bytes: number[]
  style: Style | null
}

export interface SemanticEventData {
  kind: SemanticKind
  payload: unknown
}

// ── Style ───────────────────────────────────────────────────────────

// Internally tagged via struct — plain object on the wire.
export interface Style {
  /** Foreground color as 0xRRGGBB or palette index. */
  fg: number | null
  /** Background color as 0xRRGGBB or palette index. */
  bg: number | null
  bold: boolean
  italic: boolean
  underline: boolean
}

// ── CursorOp ────────────────────────────────────────────────────────

// Adjacent-tagged: `{"op": "<variant>", "value": <body>}`.
// Phase 1 variants — extend as line-mux surfaces more ops.
export type CursorOp =
  | { op: 'Goto'; value: { row: number; col: number } }
  | { op: 'Up'; value: number }
  | { op: 'Down'; value: number }
  | { op: 'Forward'; value: number }
  | { op: 'Back'; value: number }
  | { op: 'EraseInLine'; value: EraseMode }
  | { op: 'EraseInDisplay'; value: EraseMode }
  | { op: 'ClearScreen'; value?: null }
  | { op: 'SaveCursor'; value?: null }
  | { op: 'RestoreCursor'; value?: null }
  | { op: 'SetCursorVisible'; value: boolean }
  | { op: 'SetCursorStyle'; value: CursorShape }

/** Cursor shape requested by DECSCUSR. Kept in lockstep with the Rust
 *  `CursorShape` enum at `crates/k2so-core/src/session/frame.rs`. */
export type CursorShape =
  | 'blinking_block'
  | 'steady_block'
  | 'blinking_underscore'
  | 'steady_underscore'
  | 'blinking_bar'
  | 'steady_bar'

export type EraseMode = 'to_end' | 'from_start' | 'all'

// ── SemanticKind ────────────────────────────────────────────────────

// Internally tagged by `type`; `Custom` has `type` + `kind` + `payload`.
export type SemanticKind =
  | { type: 'Message' }
  | { type: 'ToolCall' }
  | { type: 'ToolResult' }
  | { type: 'Plan' }
  | { type: 'Compaction' }
  | { type: 'Custom'; kind: string; payload: unknown }

// ── AgentSignal (awareness bus) ─────────────────────────────────────

export interface AgentSignal {
  id: string
  /** SessionId the sender was inside. Missing for CLI `k2so msg` emits. */
  session?: string
  from: AgentAddress
  to: AgentAddress
  kind: SignalKind
  priority?: Priority
  delivery?: Delivery
  /** For reply chains. */
  inReplyTo?: string
  /** ISO-8601 timestamp. */
  at: string
}

export type AgentAddress =
  | { scope: 'agent'; workspace: string; name: string }
  | { scope: 'workspace'; workspace: string }
  | { scope: 'broadcast' }

export type Priority = 'low' | 'normal' | 'high' | 'urgent'

export type Delivery = 'live' | 'inbox'

export type SignalKind =
  | { kind: 'msg'; data: { text: string } }
  | { kind: 'status'; data: { text: string } }
  | {
      kind: 'reservation'
      data: { paths: string[]; action: 'claim' | 'release' }
    }
  | { kind: 'presence'; data: { state: 'active' | 'idle' | 'away' | 'stuck' } }
  | {
      kind: 'task_lifecycle'
      data: { phase: 'started' | 'done' | 'blocked'; task_ref?: string }
    }
  | { kind: 'custom'; data: { kind: string; payload: unknown } }

// ── Decoder helpers ─────────────────────────────────────────────────

const utf8Decoder = new TextDecoder('utf-8', { fatal: false })

/** Convert a TextFrame's `bytes` field (number[]) to a UTF-8 string.
 *
 * Invalid UTF-8 sequences are replaced with U+FFFD per TextDecoder
 * defaults. Multi-byte sequences split across frames are the caller's
 * problem — the TerminalGrid layer buffers partial chunks across
 * frame boundaries (see terminalGrid.ts).
 */
export function decodeTextBytes(bytes: number[]): string {
  return utf8Decoder.decode(Uint8Array.from(bytes))
}

/** Type guard: does this envelope carry a Frame payload? */
export function isFrameEnvelope(
  env: KesselEnvelope,
): env is Extract<KesselEnvelope, { event: 'session:frame' }> {
  return env.event === 'session:frame'
}
