//! Frame — the atom of the Session Stream.
//!
//! Every byte a harness produces eventually shows up as a Frame
//! somewhere. Consumers (desktop, mobile, CLI viewer) subscribe to
//! the fan-out and pick the variants their rendering context cares
//! about. Frame is intentionally harness-neutral — anything an
//! individual harness surfaces that doesn't map cleanly rides on
//! `SemanticKind::Custom` rather than adding a new Frame variant.
//!
//! See `.k2so/prds/session-stream-and-awareness-bus.md` §"Primitive A"
//! for the full design and §"How this fixes the companion reflow"
//! for how per-client consumption escapes the baked-width trap.
//!
//! `AgentSignal(AgentSignal)` variant is added in C3 once the
//! awareness module's types land; `Frame` is `#[non_exhaustive]`
//! so the add is non-breaking.

use serde::{Deserialize, Serialize};

/// The atom of the Session Stream. Producers emit Frames; consumers
/// subscribe and filter.
///
/// Adjacent tagging — `{"frame":"Text","data":{...}}` — because the
/// enum mixes struct variants (`Text`, `SemanticEvent`) with newtype
/// variants wrapping sequences (`RawPtyFrame(Vec<u8>)`), and internal
/// tagging can't represent newtype-wrapping-non-struct content.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[non_exhaustive]
#[serde(tag = "frame", content = "data")]
pub enum Frame {
    /// Text run with optional style attributes. Produced by the VTE
    /// parser (T0), stream-json content events (T1), or native
    /// emitters (T2). Bytes are UTF-8; `style` stays `None` during
    /// Phase 1 and gets populated once Phase 2 wires full SGR
    /// parsing into the line-mux.
    Text {
        bytes: Vec<u8>,
        style: Option<Style>,
    },
    /// Cursor movement / erase. Produced by the VTE parser for TUI
    /// harnesses; absent on the stream-json path because pure-
    /// semantic modes don't own a grid to move around in.
    CursorOp(CursorOp),
    /// Harness-recognized semantic event. See `SemanticKind` for the
    /// locked 5-variant vocabulary plus `Custom` escape hatch.
    SemanticEvent {
        kind: SemanticKind,
        payload: serde_json::Value,
    },
    /// Opaque PTY byte slice. Kept for pixel-perfect replay,
    /// desktop-native rendering, and session recording. Consumers
    /// that want native terminal rendering can feed this straight
    /// back into their own emulator.
    RawPtyFrame(Vec<u8>),
}

/// The locked vocabulary for semantic events. Five variants + Custom
/// escape hatch. Small surface on purpose — keeps the agent-facing
/// mental model tight and forces authors to reuse existing vocabulary
/// rather than proliferate variants. `#[non_exhaustive]` so downstream
/// crates can't exhaustively match and break when a future release
/// earns a sixth variant.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(tag = "type")]
pub enum SemanticKind {
    /// Agent said something — user-facing message text.
    Message,
    /// Agent invoked a tool. Payload carries tool name + input args.
    ToolCall,
    /// Tool returned. Payload carries result (ok/err + output).
    ToolResult,
    /// Agent proposed a plan.
    Plan,
    /// Session history compacted.
    Compaction,
    /// Escape hatch for harness-specific events. `kind` is the
    /// semantic kind name (e.g. "usage", "message_stop"); `payload`
    /// is the harness-provided JSON.
    Custom {
        kind: String,
        payload: serde_json::Value,
    },
}

/// Cursor and screen operations lifted from ECMA-48 CSI sequences.
/// Only the ops the line-mux surfaces in Phase 1 — additional ops
/// land alongside the harnesses that need them.
///
/// Adjacent tagging (`{"op": "Up", "value": 3}`) rather than internal
/// (`{"op": "Up", ...}`) because internal tagging can't represent
/// newtype variants wrapping primitive integers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(tag = "op", content = "value")]
pub enum CursorOp {
    /// Move cursor to absolute position. Rows/cols 1-indexed per
    /// ECMA-48 CUP; the mux doesn't normalize to 0-indexed because
    /// downstream consumers expect terminal-native coordinates.
    Goto { row: u16, col: u16 },
    Up(u16),
    Down(u16),
    Forward(u16),
    Back(u16),
    /// Erase in line (EL).
    EraseInLine(EraseMode),
    /// Erase in display (ED).
    EraseInDisplay(EraseMode),
    /// Full-screen clear (ED 2 in absolute form).
    ClearScreen,
}

/// EL / ED mode selector.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[non_exhaustive]
#[serde(rename_all = "snake_case")]
pub enum EraseMode {
    /// Cursor to end of line / display.
    ToEnd,
    /// Start of line / display to cursor.
    FromStart,
    /// Entire line / display.
    All,
}

/// Style attributes on a Text run. Phase 1 placeholder — exposes the
/// fg/bg colour ints plus one flag per common attribute. Full SGR
/// parsing (truecolor, blink, reverse, strikethrough) lands in Phase
/// 2 when a consumer actually needs the fidelity.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Style {
    pub fg: Option<u32>,
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
}
