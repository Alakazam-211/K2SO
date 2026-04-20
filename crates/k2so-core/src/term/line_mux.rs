//! `LineMux` — WezTerm-style line-oriented terminal multiplexer.
//!
//! Consumes PTY bytes through the `vte` crate's state machine; emits
//! width-free `Line`s into an append-only scrollback plus `Frame`s
//! into a transient buffer that callers drain per `feed()` call.
//!
//! **No shared grid.** Each consumer subscribing to the Line + Frame
//! stream maintains its own grid at its own viewport width — the
//! WezTerm mux pattern that makes multi-device same-session viewing
//! possible without reflow fighting.
//!
//! Phase 1 scope:
//!   - CSI dispatch for CUP/CUU/CUD/CUF/CUB/EL/ED → `CursorOp` frames
//!   - print → buffered Text frames (batched per run — one frame per
//!     contiguous run of printable chars, flushed on any non-print
//!     event or line commit)
//!   - LF commits the current line into the scrollback; BS pops the
//!     last char from the current line
//!   - Scrollback bounded by `cap` (default 10k lines), trimmed from
//!     the front via `VecDeque::pop_front()` (O(1))
//!
//! Out of scope for C4 (lands in later commits or phases):
//!   - APC `ESC _ k2so:...` extraction → C5 (`term::apc`)
//!   - Per-harness recognizer hook → C6 (`term::recognizers`)
//!   - SGR / color parsing → Phase 2 when a consumer needs fidelity
//!   - `\r` semantics (overwrite vs line-reset) — deferred; current
//!     line buffer just ignores `\r`

use std::collections::VecDeque;

use vte::{Params, Parser, Perform};

use crate::log_debug;
use crate::session::{CursorOp, EraseMode, Frame, Line, SeqnoGen, SequenceNo, Style};
use crate::term::apc::{ApcChunk, ApcEvent, ApcExtractor};

/// Bounded line-oriented terminal multiplexer. Feeds PTY bytes
/// through a `vte::Parser`; emits `Frame`s and `Line`s into a
/// scrollback buffer.
pub struct LineMux {
    parser: Parser,
    state: PerformState,
    apc: ApcExtractor,
}

impl LineMux {
    /// Default scrollback cap. Chosen to match the PRD's N=1000
    /// replay-ring target × 10 so a consumer can always fetch at
    /// least the last ring-window from scrollback even after a few
    /// minutes of output. Phase 2 tunes this per the real replay
    /// ring work.
    pub const DEFAULT_CAP: usize = 10_000;

    /// Build a new `LineMux` with `DEFAULT_CAP`.
    pub fn new() -> Self {
        Self::with_cap(Self::DEFAULT_CAP)
    }

    /// Build with a custom scrollback cap. Must be >= 1.
    pub fn with_cap(cap: usize) -> Self {
        assert!(cap >= 1, "LineMux cap must be >= 1");
        let seqno_gen = SeqnoGen::default();
        let first_seqno = seqno_gen.next();
        Self {
            parser: Parser::new(),
            state: PerformState {
                lines: VecDeque::new(),
                current: Line::new(first_seqno),
                pending_text: String::new(),
                frames_out: Vec::new(),
                seqno_gen,
                current_style: None,
                cap,
            },
            apc: ApcExtractor::new(),
        }
    }

    /// Feed a chunk of PTY bytes. Returns the frames emitted by this
    /// chunk. Scrollback can be read via `lines()`.
    ///
    /// APC sequences in the `k2so:` namespace are extracted *before*
    /// reaching vte — vte 0.15 silently drops APC content, and APC is
    /// a K2SO concept anyway — and emitted as `Frame::AgentSignal`
    /// or `Frame::SemanticEvent` at their correct stream position.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Frame> {
        self.state.frames_out.clear();
        let output = self.apc.feed(bytes);
        for chunk in output.chunks {
            match chunk {
                ApcChunk::Bytes(b) => {
                    self.parser.advance(&mut self.state, &b);
                }
                ApcChunk::Event(ApcEvent::Signal(signal)) => {
                    self.state.flush_pending_text();
                    self.state.frames_out.push(Frame::AgentSignal(signal));
                }
                ApcChunk::Event(ApcEvent::Semantic { kind, payload }) => {
                    self.state.flush_pending_text();
                    self.state.frames_out.push(Frame::SemanticEvent { kind, payload });
                }
            }
        }
        for drop in output.drops {
            log_debug!("[apc] dropped: {:?}", drop);
        }
        // Flush any trailing buffered text so callers see it even
        // when the chunk doesn't end on a line or control boundary.
        self.state.flush_pending_text();
        std::mem::take(&mut self.state.frames_out)
    }

    /// Iterator over committed scrollback lines, oldest first.
    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        self.state.lines.iter()
    }

    /// Committed-line count. Does not include the line currently
    /// being built (which isn't visible in `lines()`).
    pub fn line_count(&self) -> usize {
        self.state.lines.len()
    }

    /// Peek the current seqno without bumping.
    pub fn current_seqno(&self) -> SequenceNo {
        self.state.seqno_gen.current()
    }

    /// The partially-built line that hasn't seen LF yet. Returns
    /// `None` if it's empty.
    pub fn current_line_text(&self) -> Option<&str> {
        if self.state.current.text.is_empty() {
            None
        } else {
            Some(&self.state.current.text)
        }
    }
}

impl Default for LineMux {
    fn default() -> Self {
        Self::new()
    }
}

struct PerformState {
    /// Append-only scrollback. Growth bounded by `cap` via front-trim.
    lines: VecDeque<Line>,
    /// The line currently being built. Committed to `lines` on LF.
    current: Line,
    /// Buffered printable chars. Flushed to a Text frame on any
    /// non-print event or line commit, so one run of printable chars
    /// becomes one Text frame rather than one-frame-per-char.
    pending_text: String,
    /// Frames emitted during this `feed()` call. Cleared at start of
    /// each `feed()`; moved out at end.
    frames_out: Vec<Frame>,
    /// Monotonic seqno generator for new lines.
    seqno_gen: SeqnoGen,
    /// Style currently in effect. Phase 1 placeholder — SGR parsing
    /// lands in Phase 2.
    current_style: Option<Style>,
    /// Max scrollback lines to retain.
    cap: usize,
}

impl PerformState {
    fn flush_pending_text(&mut self) {
        if !self.pending_text.is_empty() {
            let bytes = std::mem::take(&mut self.pending_text).into_bytes();
            self.frames_out.push(Frame::Text {
                bytes,
                style: self.current_style.clone(),
            });
        }
    }

    fn commit_line(&mut self) {
        self.flush_pending_text();
        let next_seqno = self.seqno_gen.next();
        let completed = std::mem::replace(&mut self.current, Line::new(next_seqno));
        self.lines.push_back(completed);
        while self.lines.len() > self.cap {
            self.lines.pop_front();
        }
    }

    fn push_cursor_op(&mut self, op: CursorOp) {
        self.flush_pending_text();
        self.frames_out.push(Frame::CursorOp(op));
    }

    /// First parameter from a CSI `Params`, defaulting to `default`
    /// when the param was omitted or zero (ECMA-48 convention: CSI
    /// `n` with `n=0` or omitted means `1` for movement commands).
    fn csi_first(params: &Params, default: u16) -> u16 {
        let raw = params
            .iter()
            .next()
            .and_then(|p| p.first().copied())
            .unwrap_or(0);
        if raw == 0 {
            default
        } else {
            raw
        }
    }

    fn csi_nth(params: &Params, n: usize, default: u16) -> u16 {
        let raw = params
            .iter()
            .nth(n)
            .and_then(|p| p.first().copied())
            .unwrap_or(0);
        if raw == 0 {
            default
        } else {
            raw
        }
    }
}

impl Perform for PerformState {
    fn print(&mut self, c: char) {
        self.current.text.push(c);
        self.pending_text.push(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' => self.commit_line(),
            b'\x08' => {
                // BS — strip the last char from the current line.
                // Also pop from pending_text so the downstream Text
                // frame reflects the post-BS state.
                self.current.text.pop();
                self.pending_text.pop();
            }
            _ => {
                // CR, Bell, Tab, other C0 — flushed but not otherwise
                // surfaced in Phase 1.
            }
        }
    }

    fn csi_dispatch(
        &mut self,
        params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        match action {
            'A' => self.push_cursor_op(CursorOp::Up(Self::csi_first(params, 1))),
            'B' => self.push_cursor_op(CursorOp::Down(Self::csi_first(params, 1))),
            'C' => self.push_cursor_op(CursorOp::Forward(Self::csi_first(params, 1))),
            'D' => self.push_cursor_op(CursorOp::Back(Self::csi_first(params, 1))),
            'H' | 'f' => {
                let row = Self::csi_first(params, 1);
                let col = Self::csi_nth(params, 1, 1);
                self.push_cursor_op(CursorOp::Goto { row, col });
            }
            'J' => {
                // ED — 0 = to end, 1 = from start, 2 = full screen.
                let mode = params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                match mode {
                    0 => self.push_cursor_op(CursorOp::EraseInDisplay(EraseMode::ToEnd)),
                    1 => self.push_cursor_op(CursorOp::EraseInDisplay(EraseMode::FromStart)),
                    2 | 3 => self.push_cursor_op(CursorOp::ClearScreen),
                    _ => {}
                }
            }
            'K' => {
                // EL — 0 = to end, 1 = from start, 2 = full line.
                let mode = params
                    .iter()
                    .next()
                    .and_then(|p| p.first().copied())
                    .unwrap_or(0);
                match mode {
                    0 => self.push_cursor_op(CursorOp::EraseInLine(EraseMode::ToEnd)),
                    1 => self.push_cursor_op(CursorOp::EraseInLine(EraseMode::FromStart)),
                    2 => self.push_cursor_op(CursorOp::EraseInLine(EraseMode::All)),
                    _ => {}
                }
            }
            _ => {
                // Unhandled CSI — silently dropped in Phase 1.
                // Flush any pending text so frame ordering stays sane
                // across the dropped op.
                self.flush_pending_text();
            }
        }
    }

    // Phase 1 passthroughs. C5 fills `osc_dispatch` for APC; DCS
    // (`hook`/`put`/`unhook`) stays empty until a harness needs it.
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, _byte: u8) {
        self.flush_pending_text();
    }

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {
        self.flush_pending_text();
    }

    fn hook(
        &mut self,
        _params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        _action: char,
    ) {
    }

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}
}
