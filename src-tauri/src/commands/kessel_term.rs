//! Canvas Plan Phase 4 — Tauri-side `alacritty_terminal::Term` per
//! Kessel pane.
//!
//! Shifts the responsibility for maintaining a Kessel pane's grid
//! + scrollback from the TypeScript `TerminalGrid` into a real
//! terminal emulator running inside Tauri. The frontend stops
//! subscribing to the daemon's Frame stream and instead asks this
//! module for grid snapshots whenever it needs to render.
//!
//! Lifecycle of a Kessel pane:
//!   1. `kessel_term_attach(pane_id, session_id, cols, rows)` —
//!      allocate a fresh Term, spawn a tokio task that opens a WS
//!      to `/cli/sessions/bytes?session=<session_id>&from=0`,
//!      reads binary frames, runs them through an APC filter, and
//!      drives them into `Processor::advance(&mut term, bytes)`.
//!   2. Frontend calls `kessel_term_grid_snapshot(pane_id)` on
//!      each rAF tick to pull a renderable snapshot.
//!   3. Window resize → `kessel_term_resize(pane_id, cols, rows)`
//!      → `term.resize(...)` (which reflows scrollback at the new
//!      cols natively).
//!   4. Pane unmount → `kessel_term_detach(pane_id)` — abort the
//!      task, drop the Term, free resources.
//!
//! The byte stream is Session-as-bytes per `canvas-plan.md`. The
//! APC filter handles `\x1b_k2so:<kind>:<json>\x07` escapes the
//! daemon injects — specifically `grow_boundary`, which tells the
//! pane to "seal" grow-phase content into scrollback and resize
//! to the daemon's target dimensions before further bytes hit
//! the vte.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Epoch-milliseconds for log timestamps. Aligns Rust-side
/// `[kessel-perf]` lines with the JS `performance.now()` timeline
/// so a single trace can be correlated between both halves of the
/// pipeline.
fn now_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Structured perf log on stderr. One line per event,
/// space-separated `key=value` pairs so the user can grep or paste
/// the whole trace. Consistent `[kessel-perf]` prefix makes a
/// `tail -f ~/.k2so/*.log | grep kessel-perf` filter trivial.
fn perf(op: &str, fields: &str) {
    eprintln!(
        "[kessel-perf] ts={} side=rust op={} {}",
        now_ms(),
        op,
        fields
    );
}

use alacritty_terminal::event::{
    Event as AlacEvent, EventListener, WindowSize,
};
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line, Point};
use alacritty_terminal::sync::FairMutex;
use alacritty_terminal::term::Config as TermConfig;
use alacritty_terminal::term::cell::Cell;
use alacritty_terminal::Term;
use futures_util::{SinkExt, StreamExt};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, async_runtime::JoinHandle};
use tokio_tungstenite::tungstenite::Message;
use vte::ansi::{Processor, StdSyncHandler};

/// Dimensions wrapper matching the pattern in
/// `k2so-core::terminal::session_stream_pty`. `total_lines` is
/// `rows + scrollback_cap` so alacritty reserves space for
/// scrollback we can scroll into.
struct TermSize {
    cols: usize,
    rows: usize,
}

impl Dimensions for TermSize {
    fn total_lines(&self) -> usize {
        self.rows + SCROLLBACK_CAP
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// How many rows of scrollback each Kessel pane retains. 5000
/// matches the daemon-side Term config; Alacritty evicts oldest
/// once this fills.
const SCROLLBACK_CAP: usize = 5000;

/// No-op listener — we don't need to forward Bell/Title events
/// from the Tauri-side Term back out. Kessel pane surfaces those
/// from the Frame stream instead (or will migrate them to byte-
/// side APC markers in a future phase).
#[derive(Clone)]
struct NoopListener;

impl EventListener for NoopListener {
    fn send_event(&self, _event: AlacEvent) {}
}

// ── APC filter ───────────────────────────────────────────────────

/// Scans a byte chunk for `\x1b_k2so:<kind>:<json>\x07` APC
/// escapes. Extracted escapes are returned to the caller for
/// side-effect handling (grow_boundary etc.); everything else
/// flows through to the vte unmodified.
///
/// Stateful across chunks — an APC escape that straddles a read
/// boundary resumes on the next call. The buffered prefix is held
/// inside the filter; callers must use the same filter instance
/// across all reads for one pane.
struct ApcFilter {
    /// APC body accumulated across bytes within a single APC
    /// escape. Cleared on each APC close (BEL). Decision on
    /// whether this APC is `k2so:`-namespaced happens at close
    /// time by inspecting the buffered bytes.
    buffered: Vec<u8>,
    /// True when the previous byte was ESC — next byte might
    /// open an APC with `_`.
    saw_esc: bool,
    /// True when we're inside an APC (between `ESC _` and BEL).
    inside_apc: bool,
}

/// One extracted `k2so:` APC escape (kind + JSON payload).
#[derive(Debug, Clone)]
pub struct ApcEvent {
    pub kind: String,
    pub payload: serde_json::Value,
}

impl ApcFilter {
    fn new() -> Self {
        Self {
            buffered: Vec::new(),
            saw_esc: false,
            inside_apc: false,
        }
    }

    /// Process an input chunk. Returns `(clean_bytes, events)`:
    /// `clean_bytes` is the input with ALL APCs stripped, ready
    /// to feed into vte. `events` is the list of `k2so:`-namespaced
    /// APC escapes that closed inside this chunk.
    ///
    /// Non-k2so APCs are stripped but not surfaced — they're
    /// ignored inline, same way xterm handles unknown APCs.
    ///
    /// Decision about "is this APC k2so-namespaced" happens at
    /// the BEL terminator by inspecting the buffered body (which
    /// is either `k2so:...` or not).
    fn feed(&mut self, input: &[u8]) -> (Vec<u8>, Vec<ApcEvent>) {
        let mut clean = Vec::with_capacity(input.len());
        let mut events = Vec::new();

        for &b in input {
            if self.inside_apc {
                if b == 0x07 {
                    // Close APC. Check if the buffered body is
                    // k2so-namespaced; if so, parse + emit.
                    if self.buffered.starts_with(b"k2so:") {
                        let payload = &self.buffered[b"k2so:".len()..];
                        if let Some(ev) = Self::parse_k2so_body(payload) {
                            events.push(ev);
                        }
                    }
                    // In either case, discard the buffered APC —
                    // it's been "consumed" by the filter and
                    // MUST NOT reach the vte.
                    self.buffered.clear();
                    self.inside_apc = false;
                } else {
                    self.buffered.push(b);
                }
                continue;
            }
            if self.saw_esc {
                self.saw_esc = false;
                if b == b'_' {
                    // APC introducer. Next bytes are the APC
                    // body up to BEL.
                    self.inside_apc = true;
                    continue;
                }
                // Not an APC — flush the buffered ESC + this byte
                // back into the clean stream.
                clean.push(0x1b);
                clean.push(b);
                continue;
            }
            if b == 0x1b {
                // Potential start of an escape. Don't commit to
                // "it's APC" until we see the next byte.
                self.saw_esc = true;
                continue;
            }
            clean.push(b);
        }

        (clean, events)
    }

    /// Parse a k2so APC body. Expected format: `<kind>:<json>`.
    /// The `k2so:` prefix has already been stripped by the caller.
    ///
    /// Returns None on malformed input (no colon, non-UTF-8,
    /// invalid JSON). Malformed escapes are silently dropped —
    /// better than crashing a pane over a bad marker.
    fn parse_k2so_body(buf: &[u8]) -> Option<ApcEvent> {
        let s = std::str::from_utf8(buf).ok()?;
        let colon = s.find(':')?;
        let kind = s[..colon].to_string();
        let payload_str = &s[colon + 1..];
        let payload = serde_json::from_str(payload_str).ok()?;
        Some(ApcEvent { kind, payload })
    }
}

// ── Pane state ───────────────────────────────────────────────────

/// Per-pane emulator state. One of these exists per active Kessel
/// pane in the frontend.
struct PaneState {
    /// The emulator itself. Driven by the reader task.
    term: Arc<FairMutex<Term<NoopListener>>>,
    /// Bytes flowing through an APC filter before hitting the vte.
    apc_filter: ApcFilter,
    /// vte parser; stateful across calls.
    processor: Processor<StdSyncHandler>,
    /// Monotonic counter of grid mutations. Bumped on every
    /// feed_bytes that produces damage, every resize, every APC.
    /// Goes into each emit (snapshot OR delta) so the frontend can
    /// skip duplicate updates.
    dirty_counter: u64,
    /// Scrollback size as of the last emit. Used to detect when
    /// new rows have scrolled INTO scrollback between emits so we
    /// can append them on the delta path (damage API only tracks
    /// the visible grid, not scrollback).
    last_history_size: usize,
    /// False until we've pushed our first emit to the frontend.
    /// Keeps the contract: first emit after attach OR after resume
    /// is always a full snapshot. Everything else is a delta.
    has_emitted_since_attach: bool,
    /// When true, `reader_loop` keeps feeding bytes through the
    /// vte but SKIPS emitting snapshots/deltas to the frontend.
    /// Used to stop starving the UI thread with events from panes
    /// the user isn't currently looking at. The Term keeps
    /// advancing underneath; on unpause we emit one full snapshot
    /// so the frontend catches up in one shot.
    paused: bool,
    /// Reader task handle — aborted on detach.
    reader_task: Option<JoinHandle<()>>,
}

impl PaneState {
    fn new(cols: u16, rows: u16) -> Self {
        let config = TermConfig {
            scrolling_history: SCROLLBACK_CAP,
            ..TermConfig::default()
        };
        let size = TermSize {
            cols: cols.max(1) as usize,
            rows: rows.max(1) as usize,
        };
        let term = Term::new(config, &size, NoopListener);
        Self {
            term: Arc::new(FairMutex::new(term)),
            apc_filter: ApcFilter::new(),
            processor: Processor::new(),
            dirty_counter: 0,
            last_history_size: 0,
            has_emitted_since_attach: false,
            // Default to paused. Frontend must explicitly call
            // `kessel_term_resume` on mount for panes it actually
            // wants to render. This prevents the mount-flood
            // beachball at app launch: if 15 Kessel panes mount
            // simultaneously (retained-view across tabs), only
            // the visible one emits — the other 14 accumulate
            // byte state silently and emit one full snapshot
            // each when the user switches to their tab. Bytes
            // are STILL being read by the reader task; only
            // outbound emission is gated.
            paused: true,
            reader_task: None,
        }
    }

    /// Process a chunk of bytes from the daemon. APC filter
    /// first, then anything non-k2so flows into the vte. APC
    /// events are applied as side effects on the Term.
    fn feed_bytes(&mut self, chunk: &[u8]) {
        let t0 = Instant::now();
        let input_len = chunk.len();
        let (clean, events) = self.apc_filter.feed(chunk);
        let apc_events = events.len();
        for ev in events {
            self.apply_apc_event(ev);
        }
        let clean_len = clean.len();
        if !clean.is_empty() {
            let mut term = self.term.lock();
            self.processor.advance(&mut *term, &clean);
            self.dirty_counter = self.dirty_counter.wrapping_add(1);
        }
        perf(
            "feed_bytes",
            &format!(
                "input_bytes={} clean_bytes={} apc_events={} dur_us={}",
                input_len,
                clean_len,
                apc_events,
                t0.elapsed().as_micros()
            ),
        );
    }

    /// Side-effect an APC event. `grow_boundary` seals the
    /// grow-phase paint into scrollback and resizes to the
    /// daemon's target.
    fn apply_apc_event(&mut self, ev: ApcEvent) {
        if ev.kind != "grow_boundary" {
            // Unknown kinds are forward-compat — older panes
            // running against newer daemons should just ignore
            // them.
            return;
        }
        let target_cols = ev
            .payload
            .get("target_cols")
            .and_then(|v| v.as_u64())
            .unwrap_or(80) as usize;
        let target_rows = ev
            .payload
            .get("target_rows")
            .and_then(|v| v.as_u64())
            .unwrap_or(24) as usize;

        let mut term = self.term.lock();
        // Minimum viable seam: use alacritty's native resize,
        // which shrinks-from-top and pushes overflow to its own
        // scrollback. This means the bottom `target_rows` of the
        // grow canvas stay in the live grid — those get wiped by
        // Claude's post-SIGWINCH ClearScreen. Less precise than
        // the TypeScript `sealGrowPhase` (which pushes ALL content
        // rows to scrollback first), but simpler and gets Phase 4
        // off the ground. Phase 5 can refine with explicit cursor-
        // row-aware scrollback pushing if the loss is visible.
        term.resize(TermSize {
            cols: target_cols,
            rows: target_rows,
        });
        self.dirty_counter = self.dirty_counter.wrapping_add(1);
    }

    fn resize(&mut self, cols: u16, rows: u16) {
        let mut term = self.term.lock();
        term.resize(TermSize {
            cols: cols.max(1) as usize,
            rows: rows.max(1) as usize,
        });
        self.dirty_counter = self.dirty_counter.wrapping_add(1);
    }
}

impl Drop for PaneState {
    fn drop(&mut self) {
        if let Some(handle) = self.reader_task.take() {
            handle.abort();
        }
    }
}

// ── Registry ─────────────────────────────────────────────────────

#[derive(Default)]
struct PaneRegistry {
    panes: Mutex<HashMap<String, Arc<Mutex<PaneState>>>>,
}

impl PaneRegistry {
    fn insert(&self, pane_id: String, state: PaneState) -> Arc<Mutex<PaneState>> {
        let arc = Arc::new(Mutex::new(state));
        self.panes.lock().insert(pane_id, Arc::clone(&arc));
        arc
    }

    fn get(&self, pane_id: &str) -> Option<Arc<Mutex<PaneState>>> {
        self.panes.lock().get(pane_id).cloned()
    }

    fn remove(&self, pane_id: &str) -> Option<Arc<Mutex<PaneState>>> {
        self.panes.lock().remove(pane_id)
    }

    fn count(&self) -> usize {
        self.panes.lock().len()
    }
}

fn registry() -> &'static PaneRegistry {
    static REGISTRY: std::sync::OnceLock<PaneRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(PaneRegistry::default)
}

// ── Snapshot types (mirror kessel/types.ts grid shape) ───────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermGridSnapshot {
    pub pane_id: String,
    pub cols: usize,
    pub rows: usize,
    /// Live grid rows, top-to-bottom. Each row is a list of
    /// style-homogeneous runs — adjacent cells sharing SGR state
    /// coalesce into one run, keeping the wire compact.
    pub grid: Vec<Vec<CellRun>>,
    /// Scrollback rows, oldest-first. Same run-encoding as `grid`.
    pub scrollback: Vec<Vec<CellRun>>,
    pub cursor: CursorSnapshot,
    /// Opaque version; frontend skips snapshot diff when this
    /// hasn't changed since last pull.
    pub version: u64,
    /// Current display offset (how far we've scrolled into
    /// scrollback on the daemon side — usually 0 since the Term
    /// only mutates via byte feed, not user input).
    pub display_offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CursorSnapshot {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
}

/// One run of consecutive cells sharing the same SGR style.
/// Serialized as camelCase so the frontend can use the fields
/// directly in JSX style attributes.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct CellRun {
    /// The text of this run, already de-null-padded (spaces for
    /// blanks) and ready to place inside a `<span>`.
    pub text: String,
    /// Foreground color as 0xRRGGBB, or null to mean "terminal
    /// default" (rendered via the user's theme fg variable).
    pub fg: Option<u32>,
    /// Background color as 0xRRGGBB, or null to mean "terminal
    /// default".
    pub bg: Option<u32>,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    /// SGR 7 — inverse video. Renderer swaps fg/bg when true.
    pub inverse: bool,
    /// SGR 2 — dim. Renderer applies reduced opacity.
    pub dim: bool,
    /// SGR 9 — strikeout (line-through).
    pub strikeout: bool,
}

impl CellRun {
    /// Identity of a run is its style alone — `text` differs
    /// between runs by definition. Used for coalescing during
    /// build.
    fn style_eq(&self, other: &Self) -> bool {
        self.fg == other.fg
            && self.bg == other.bg
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
            && self.inverse == other.inverse
            && self.dim == other.dim
            && self.strikeout == other.strikeout
    }
}

/// Incremental update for a single live-grid row. Only the rows
/// alacritty's damage API flagged as changed since the last emit
/// appear here; unchanged rows ride forward from the frontend's
/// last-known state. Cheap on the wire relative to a full snapshot.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct DamagedRow {
    /// Row index in the live grid (0 = topmost visible row).
    pub row: usize,
    /// Full row contents, run-encoded. We don't use the damage
    /// API's left/right bounds here — alacritty's bounds are
    /// coarse (post-write unions), and emitting the whole row
    /// keeps the frontend merge logic trivially correct.
    pub runs: Vec<CellRun>,
}

/// Per-tick delta emit. Applied against the frontend's local grid
/// mirror:
///   - `damaged_rows` → replace those rows in the live grid.
///   - `scrollback_appended` → push onto the end of scrollback
///     (oldest-last within the vec; these are rows that scrolled
///     off the top of the live grid between emits).
///   - `cursor` / `cols` / `rows` / `display_offset` → absorb.
///
/// A delta always carries the current cursor + dims so the
/// frontend never has to interpret "did the cursor move?"
/// separately from damage.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TermGridDelta {
    pub pane_id: String,
    pub cols: usize,
    pub rows: usize,
    pub damaged_rows: Vec<DamagedRow>,
    pub scrollback_appended: Vec<Vec<CellRun>>,
    pub cursor: CursorSnapshot,
    pub version: u64,
    pub display_offset: usize,
}

/// Xterm-compatible default palette. Indices 0..15 are the
/// basic 16 colors; 16..231 is the 6x6x6 color cube; 232..255
/// is the 24-step grayscale. Used to resolve NamedColor /
/// Indexed(u8) cell colors into 0xRRGGBB values for the DOM.
///
/// The values here match xterm's defaults which every modern
/// terminal emulator riffs on. Custom theme support (per the
/// user's K2SO theme) is a follow-up — for now, the daemon-side
/// Term just uses the canonical palette.
const PALETTE_16: [u32; 16] = [
    0x000000, 0xcd0000, 0x00cd00, 0xcdcd00,
    0x0000ee, 0xcd00cd, 0x00cdcd, 0xe5e5e5,
    0x7f7f7f, 0xff0000, 0x00ff00, 0xffff00,
    0x5c5cff, 0xff00ff, 0x00ffff, 0xffffff,
];

fn palette_256(idx: u8) -> u32 {
    if (idx as usize) < 16 {
        return PALETTE_16[idx as usize];
    }
    if idx < 232 {
        let i = (idx - 16) as u32;
        let r = i / 36;
        let g = (i % 36) / 6;
        let b = i % 6;
        let component = |x: u32| if x == 0 { 0 } else { 55 + x * 40 };
        return (component(r) << 16) | (component(g) << 8) | component(b);
    }
    let v = 8 + (idx as u32 - 232) * 10;
    (v << 16) | (v << 8) | v
}

/// Resolve a vte Color (Named / Indexed / Spec) into an optional
/// 0xRRGGBB value. Returns None for the "default" sentinels
/// (Foreground / Background) so the DOM can fall back to the
/// user's theme variables rather than baking in our palette's
/// fg/bg guess.
fn resolve_color(c: vte::ansi::Color) -> Option<u32> {
    use vte::ansi::{Color, NamedColor};
    match c {
        Color::Named(NamedColor::Foreground)
        | Color::Named(NamedColor::Background)
        | Color::Named(NamedColor::Cursor)
        | Color::Named(NamedColor::BrightForeground)
        | Color::Named(NamedColor::DimForeground) => None,
        Color::Named(n) => {
            let idx = n as u32;
            if idx < 16 {
                Some(PALETTE_16[idx as usize])
            } else {
                None
            }
        }
        Color::Spec(rgb) => {
            Some(((rgb.r as u32) << 16) | ((rgb.g as u32) << 8) | rgb.b as u32)
        }
        Color::Indexed(i) => Some(palette_256(i)),
    }
}

/// Turn a single cell into the styled-run view used by the
/// snapshot. Blank cells render as a space so the row's column
/// alignment survives.
fn cell_to_run(cell: &Cell) -> CellRun {
    use alacritty_terminal::term::cell::Flags;
    let flags = cell.flags;
    let ch = if cell.c == '\0' { ' ' } else { cell.c };
    let mut text = String::new();
    text.push(ch);
    CellRun {
        text,
        fg: resolve_color(cell.fg),
        bg: resolve_color(cell.bg),
        bold: flags.contains(Flags::BOLD),
        italic: flags.contains(Flags::ITALIC),
        underline: flags.intersects(Flags::ALL_UNDERLINES),
        inverse: flags.contains(Flags::INVERSE),
        dim: flags.contains(Flags::DIM),
        strikeout: flags.contains(Flags::STRIKEOUT),
    }
}

/// Build a row's run list by coalescing adjacent style-equal
/// cells into one run. Empty-trailing runs (default-styled blank
/// suffixes) are retained so the cell-width of the row stays
/// addressable on the client — trimming them here would lose
/// alignment for subsequent content that drops back into the row.
fn encode_row_runs(grid: &alacritty_terminal::grid::Grid<Cell>, line: Line, cols: usize) -> Vec<CellRun> {
    let mut out: Vec<CellRun> = Vec::new();
    for c in 0..cols {
        let cell = &grid[Point::new(line, Column(c))];
        let run = cell_to_run(cell);
        match out.last_mut() {
            Some(last) if last.style_eq(&run) => {
                last.text.push_str(&run.text);
            }
            _ => out.push(run),
        }
    }
    out
}

/// Project a Term into a full serializable snapshot. Walks every
/// row of the live viewport AND every row of scrollback, building
/// run-encoded rows with per-run SGR state. Used for:
///   - Initial attach (one-time cost when a pane first mounts).
///   - Resume after emission was paused (hidden tab becomes
///     visible again).
///   - Explicit on-demand pulls via `kessel_term_grid_snapshot`.
///
/// **NOT used on the hot path.** Every per-tick mutation emits a
/// `TermGridDelta` instead — see `build_delta` + the reader loop's
/// snapshot-interval logic.
fn snapshot_term(pane_id: &str, state: &PaneState) -> TermGridSnapshot {
    let history_size = state.term.lock().grid().history_size();
    snapshot_term_with_scrollback(pane_id, state, history_size)
}

/// Variant of `snapshot_term` that lets the caller control the
/// scrollback depth. Used by the on-demand scrollback-slice
/// command to serve deeper history than the fast-path snapshot
/// carries. `scrollback_rows_requested` is capped at the Term's
/// actual `history_size` so callers can't ask for more than
/// exists.
fn snapshot_term_with_scrollback(
    pane_id: &str,
    state: &PaneState,
    scrollback_rows_requested: usize,
) -> TermGridSnapshot {
    let term = state.term.lock();
    let cols = term.columns();
    let rows = term.screen_lines();
    let grid = term.grid();

    let mut live_rows: Vec<Vec<CellRun>> = Vec::with_capacity(rows);
    for r in 0..(rows as i32) {
        live_rows.push(encode_row_runs(grid, Line(r), cols));
    }

    let history_size = grid.history_size();
    let take = scrollback_rows_requested.min(history_size);
    let mut scrollback_rows: Vec<Vec<CellRun>> = Vec::with_capacity(take);
    // Walk the most-recent `take` rows of scrollback (closest to
    // the live grid, since that's what a scroll-up hits first).
    // Indices: scrollback above live starts at Line(-1) and goes
    // to Line(-history_size). "Most recent" = Line(-1) side.
    for r in (1..=take as i32).rev() {
        scrollback_rows.push(encode_row_runs(grid, Line(-r), cols));
    }

    let cursor_point = grid.cursor.point;
    let cursor = CursorSnapshot {
        row: (cursor_point.line.0.max(0)) as usize,
        col: cursor_point.column.0,
        visible: true,
    };

    TermGridSnapshot {
        pane_id: pane_id.to_string(),
        cols,
        rows,
        grid: live_rows,
        scrollback: scrollback_rows,
        cursor,
        version: state.dirty_counter,
        display_offset: grid.display_offset(),
    }
}

/// Outcome of a single emit decision. The caller emits one of
/// these on the appropriate Tauri event name: `Full` on
/// `kessel:grid-snapshot`, `Delta` on `kessel:grid-delta`, `Skip`
/// does nothing.
enum EmitDecision {
    Full(TermGridSnapshot),
    Delta(TermGridDelta),
    Skip,
}

/// Inspect the Term's damage + scrollback growth since the last
/// emit and produce the cheapest correct update. Skips entirely
/// when paused OR when nothing has changed.
///
/// Called inside `reader_loop` on each snapshot-interval tick AND
/// after each feed_bytes chunk is absorbed. Resets Term damage
/// after reading it so the next call starts with a clean slate.
fn build_emit(pane_id: &str, state: &mut PaneState) -> EmitDecision {
    let t0 = Instant::now();
    if state.paused {
        perf(
            "build_emit",
            &format!(
                "pane={} kind=skip reason=paused dur_us={}",
                pane_id,
                t0.elapsed().as_micros()
            ),
        );
        return EmitDecision::Skip;
    }

    // First emit after attach — always a full snapshot. The
    // frontend has no local mirror yet, so deltas would dangle.
    if !state.has_emitted_since_attach {
        state.has_emitted_since_attach = true;
        let snap = snapshot_term(pane_id, state);
        state.last_history_size = snap.scrollback.len();
        state.term.lock().reset_damage();
        let live_cells: usize = snap.grid.iter().map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>()).sum();
        let sb_cells: usize = snap.scrollback.iter().map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>()).sum();
        perf(
            "build_emit",
            &format!(
                "pane={} kind=full reason=first_emit live_rows={} sb_rows={} live_cells={} sb_cells={} dur_us={}",
                pane_id,
                snap.grid.len(),
                snap.scrollback.len(),
                live_cells,
                sb_cells,
                t0.elapsed().as_micros()
            ),
        );
        return EmitDecision::Full(snap);
    }

    // Take the damage under the Term lock, then drop the lock
    // while we serialize. Damage is copied into an owned Vec so
    // we don't hold the lock across the RLE encode.
    let (damage_kind, history_size, display_offset, cursor_point, cols, rows) = {
        let mut term = state.term.lock();
        let history_size = term.grid().history_size();
        let display_offset = term.grid().display_offset();
        let cursor_point = term.grid().cursor.point;
        let cols = term.columns();
        let rows = term.screen_lines();
        let damage_kind = match term.damage() {
            alacritty_terminal::term::TermDamage::Full => DamageKind::Full,
            alacritty_terminal::term::TermDamage::Partial(iter) => {
                let lines: Vec<usize> = iter.map(|d| d.line).collect();
                DamageKind::Partial(lines)
            }
        };
        term.reset_damage();
        (damage_kind, history_size, display_offset, cursor_point, cols, rows)
    };

    let new_scrollback = history_size.saturating_sub(state.last_history_size);

    // If nothing changed: neither live damage nor scrollback
    // growth, skip emission entirely.
    let partial_empty =
        matches!(&damage_kind, DamageKind::Partial(lines) if lines.is_empty());
    if partial_empty && new_scrollback == 0 {
        perf(
            "build_emit",
            &format!(
                "pane={} kind=skip reason=clean dur_us={}",
                pane_id,
                t0.elapsed().as_micros()
            ),
        );
        return EmitDecision::Skip;
    }

    // Full damage → full snapshot. Cheaper than trying to list
    // every row as damaged.
    if matches!(&damage_kind, DamageKind::Full) {
        let snap = snapshot_term(pane_id, state);
        state.last_history_size = snap.scrollback.len();
        let live_cells: usize = snap.grid.iter().map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>()).sum();
        let sb_cells: usize = snap.scrollback.iter().map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>()).sum();
        perf(
            "build_emit",
            &format!(
                "pane={} kind=full reason=full_damage live_rows={} sb_rows={} live_cells={} sb_cells={} dur_us={}",
                pane_id,
                snap.grid.len(),
                snap.scrollback.len(),
                live_cells,
                sb_cells,
                t0.elapsed().as_micros()
            ),
        );
        return EmitDecision::Full(snap);
    }

    // Build a delta. Re-lock the term briefly to encode damaged
    // rows + any new scrollback additions.
    let damaged_lines: Vec<usize> = match damage_kind {
        DamageKind::Partial(lines) => lines,
        DamageKind::Full => unreachable!("handled above"),
    };

    let term = state.term.lock();
    let grid = term.grid();

    let mut damaged_rows: Vec<DamagedRow> = Vec::with_capacity(damaged_lines.len());
    for row_idx in damaged_lines {
        if row_idx >= rows {
            continue;
        }
        let runs = encode_row_runs(grid, Line(row_idx as i32), cols);
        damaged_rows.push(DamagedRow {
            row: row_idx,
            runs,
        });
    }

    // Scrollback appended: the last `new_scrollback` rows above
    // Line(0) are newly-appended history (oldest-first within our
    // return so the frontend just pushes them onto its scrollback
    // vec). alacritty indexes scrollback at Line(-1) (most-recent)
    // down to Line(-history_size) (oldest).
    let mut scrollback_appended: Vec<Vec<CellRun>> =
        Vec::with_capacity(new_scrollback);
    for offset in (1..=new_scrollback as i32).rev() {
        scrollback_appended.push(encode_row_runs(grid, Line(-offset), cols));
    }
    drop(term);

    state.last_history_size = history_size;

    let delta_cells: usize = damaged_rows.iter().map(|r| r.runs.iter().map(|run| run.text.chars().count()).sum::<usize>()).sum();
    let sb_append_cells: usize = scrollback_appended.iter().map(|r| r.iter().map(|run| run.text.chars().count()).sum::<usize>()).sum();
    perf(
        "build_emit",
        &format!(
            "pane={} kind=delta damaged_rows={} sb_append_rows={} damaged_cells={} sb_append_cells={} dur_us={}",
            pane_id,
            damaged_rows.len(),
            scrollback_appended.len(),
            delta_cells,
            sb_append_cells,
            t0.elapsed().as_micros()
        ),
    );

    let delta = TermGridDelta {
        pane_id: pane_id.to_string(),
        cols,
        rows,
        damaged_rows,
        scrollback_appended,
        cursor: CursorSnapshot {
            row: (cursor_point.line.0.max(0)) as usize,
            col: cursor_point.column.0,
            visible: true,
        },
        version: state.dirty_counter,
        display_offset,
    };
    EmitDecision::Delta(delta)
}

/// Intermediate type used by `build_emit` to copy damage state
/// out from under the Term lock so we can serialize without
/// blocking other callers.
enum DamageKind {
    Full,
    Partial(Vec<usize>),
}

// ── WS reader task ───────────────────────────────────────────────

/// Connect to the daemon's `/cli/sessions/bytes` endpoint and
/// pipe bytes into the pane's Term. Runs until the WS closes or
/// the task is aborted (on detach).
async fn reader_loop(
    pane_id: String,
    session_id: String,
    port: u16,
    token: String,
    pane: Arc<Mutex<PaneState>>,
    app: tauri::AppHandle,
) {
    let url = format!(
        "ws://127.0.0.1:{port}/cli/sessions/bytes?session={session_id}&token={token}&from=0"
    );

    let ws = match tokio_tungstenite::connect_async(&url).await {
        Ok((ws, _resp)) => ws,
        Err(e) => {
            eprintln!(
                "[kessel_term] pane={pane_id} ws connect failed: {e}"
            );
            return;
        }
    };
    let (mut write, mut read) = ws.split();

    eprintln!(
        "[kessel_term] pane={pane_id} session={session_id} ws connected"
    );

    // Emission cadence. Byte absorption happens as fast as the
    // WS delivers; emission is throttled to roughly 30 Hz so the
    // frontend isn't hammered per-byte. Only emits when there's
    // something to say — `build_emit` returns `Skip` on quiet
    // ticks, and Delta when only a few rows changed.
    let mut snapshot_interval =
        tokio::time::interval(Duration::from_millis(33));
    snapshot_interval.set_missed_tick_behavior(
        tokio::time::MissedTickBehavior::Skip,
    );

    loop {
        tokio::select! {
            msg = read.next() => {
                match msg {
                    None | Some(Err(_)) | Some(Ok(Message::Close(_))) => {
                        eprintln!(
                            "[kessel_term] pane={pane_id} ws closed"
                        );
                        break;
                    }
                    Some(Ok(Message::Binary(data))) => {
                        let mut p = pane.lock();
                        p.feed_bytes(&data);
                        // Bytes advance the Term regardless of
                        // pause state — a hidden pane still needs
                        // to track its session. Emission is what
                        // pause controls, and that's handled on
                        // the next snapshot_interval tick via
                        // build_emit.
                    }
                    Some(Ok(Message::Text(_))) => {
                        // session:ack envelope; ignored for now.
                        // A future phase can parse this for the
                        // front/back offset gap indicator.
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = write.send(Message::Pong(p)).await;
                    }
                    Some(Ok(_)) => {}
                }
            }
            _ = snapshot_interval.tick() => {
                let decision = {
                    let mut p = pane.lock();
                    build_emit(&pane_id, &mut p)
                };
                match decision {
                    EmitDecision::Skip => {}
                    EmitDecision::Full(snap) => {
                        let t_emit = Instant::now();
                        let _ = app.emit("kessel:grid-snapshot", &snap);
                        perf(
                            "emit",
                            &format!(
                                "pane={} kind=full live_rows={} sb_rows={} dur_us={}",
                                pane_id,
                                snap.grid.len(),
                                snap.scrollback.len(),
                                t_emit.elapsed().as_micros()
                            ),
                        );
                    }
                    EmitDecision::Delta(delta) => {
                        let t_emit = Instant::now();
                        let _ = app.emit("kessel:grid-delta", &delta);
                        perf(
                            "emit",
                            &format!(
                                "pane={} kind=delta damaged_rows={} sb_append={} dur_us={}",
                                pane_id,
                                delta.damaged_rows.len(),
                                delta.scrollback_appended.len(),
                                t_emit.elapsed().as_micros()
                            ),
                        );
                    }
                }
            }
        }
    }

    // Final snapshot on close so the frontend sees the true
    // terminal state when the session ends. Bypasses the pause
    // gate since the reader is exiting anyway.
    let final_snap = {
        let p = pane.lock();
        snapshot_term(&pane_id, &p)
    };
    let _ = app.emit("kessel:grid-snapshot", final_snap);
}

// ── Tauri commands ───────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachArgs {
    pub pane_id: String,
    pub session_id: String,
    pub port: u16,
    pub token: String,
    pub cols: u16,
    pub rows: u16,
    /// Tab-visibility state AT THE MOMENT OF ATTACH. If true, the
    /// pane is created with `paused=false` so the reader task's
    /// first snapshot_interval tick emits immediately. If false,
    /// pane starts paused and waits for a `kessel_term_resume`
    /// call when visibility flips.
    ///
    /// Solves the attach/resume race on initial mount: without
    /// this flag, `kessel_term_resume` invoked from the
    /// visibility useEffect could hit before `attach` registered
    /// the pane (both fire as concurrent async invokes), making
    /// resume a no-op and leaving the pane paused until the user
    /// manually switched tabs away and back.
    ///
    /// Defaults to true for backwards compatibility if the
    /// caller doesn't specify. Frontend always passes it now.
    #[serde(default = "default_initially_visible")]
    pub initially_visible: bool,
}

fn default_initially_visible() -> bool {
    true
}

#[tauri::command]
pub async fn kessel_term_attach(
    app: tauri::AppHandle,
    args: AttachArgs,
) -> Result<(), String> {
    let t0 = Instant::now();
    // Allocate a pane at the GROW_ROWS size to absorb the grow-
    // phase paint. Alacritty will shrink-from-top to the target
    // when the APC arrives.
    let initial_rows = args.rows.max(500);
    let state = PaneState::new(args.cols, initial_rows);
    let pane_arc = registry().insert(args.pane_id.clone(), state);

    // Apply initial visibility atomically with registration. If the
    // tab is visible at mount time, flip paused=false here so the
    // reader task's first tick emits without needing a separate
    // resume invoke. Prevents the attach-vs-resume race on mount.
    {
        let mut p = pane_arc.lock();
        p.paused = !args.initially_visible;
        // has_emitted_since_attach stays false (its default) so the
        // first emit — whenever it fires — is a full snapshot.
    }

    let pane_count = registry().count();
    perf(
        "attach",
        &format!(
            "pane={} session={} cols={} rows={} initially_visible={} pane_count={} dur_us={}",
            args.pane_id,
            args.session_id,
            args.cols,
            args.rows,
            args.initially_visible,
            pane_count,
            t0.elapsed().as_micros()
        ),
    );

    // Spawn the reader task; store its handle back into the pane
    // so detach can abort it.
    let pane_for_task = Arc::clone(&pane_arc);
    let pane_id_for_task = args.pane_id.clone();
    let handle = tauri::async_runtime::spawn(async move {
        reader_loop(
            pane_id_for_task,
            args.session_id,
            args.port,
            args.token,
            pane_for_task,
            app,
        )
        .await
    });
    // Stash the handle inside the pane state so detach can abort.
    {
        let mut p = pane_arc.lock();
        p.reader_task = Some(handle);
    }

    let _ = WindowSize {
        num_cols: args.cols,
        num_lines: args.rows,
        cell_width: 8,
        cell_height: 16,
    };
    Ok(())
}

#[tauri::command]
pub fn kessel_term_grid_snapshot(
    pane_id: String,
) -> Result<TermGridSnapshot, String> {
    let pane = registry()
        .get(&pane_id)
        .ok_or_else(|| format!("unknown pane {pane_id}"))?;
    let p = pane.lock();
    Ok(snapshot_term(&pane_id, &p))
}

#[tauri::command]
pub fn kessel_term_resize(
    pane_id: String,
    cols: u16,
    rows: u16,
) -> Result<(), String> {
    let pane = registry()
        .get(&pane_id)
        .ok_or_else(|| format!("unknown pane {pane_id}"))?;
    let mut p = pane.lock();
    p.resize(cols, rows);
    Ok(())
}

/// On-demand deeper scrollback pull. Returns a full snapshot
/// including up to `rows` scrollback rows (most-recent). Intended
/// for the moment a user scrolls past the fast-path cap and needs
/// more history than the steady-state snapshot carries. Cost is
/// roughly O(rows × cols), so callers should request only what
/// they need — not the whole 5000-row buffer.
#[tauri::command]
pub fn kessel_term_scrollback_slice(
    pane_id: String,
    rows: usize,
) -> Result<TermGridSnapshot, String> {
    let pane = registry()
        .get(&pane_id)
        .ok_or_else(|| format!("unknown pane {pane_id}"))?;
    let p = pane.lock();
    Ok(snapshot_term_with_scrollback(&pane_id, &p, rows))
}

/// Pause outbound snapshot/delta emission for a pane. The pane's
/// Term keeps absorbing bytes from the daemon — session state
/// stays current — we just stop spending IPC + serialization on
/// an audience of zero.
///
/// **Note on defaults:** panes are created in the paused state,
/// so explicit `kessel_term_pause` is only needed to *return* a
/// pane to paused after it had been resumed (e.g. user switched
/// away from a tab that was visible a moment ago). First-mount
/// panes are already paused without this call.
///
/// Idempotent. Unknown `pane_id` returns Ok — likely the pane
/// already detached (e.g. tab closed while the visibility
/// effect was mid-flight).
#[tauri::command]
pub fn kessel_term_pause(pane_id: String) -> Result<(), String> {
    let t0 = Instant::now();
    let hit = if let Some(pane) = registry().get(&pane_id) {
        let mut p = pane.lock();
        p.paused = true;
        true
    } else {
        false
    };
    perf(
        "pause",
        &format!(
            "pane={} hit={} dur_us={}",
            pane_id,
            hit,
            t0.elapsed().as_micros()
        ),
    );
    Ok(())
}

/// Unpause emission. Forces the NEXT emit to be a full snapshot
/// by clearing `has_emitted_since_attach` — since the frontend's
/// mirror went stale during the pause window (or was never
/// populated if this is the first resume since attach), a delta
/// would dangle. Full snapshot catches it up in one shot.
///
/// **Must be called at least once per pane for emission to
/// start.** Panes default to paused at construction. The
/// frontend calls this on mount for visible panes, and on
/// visibility transitions `hidden → visible` for panes that
/// were previously in a background tab.
///
/// Idempotent; unknown `pane_id` returns Ok.
#[tauri::command]
pub fn kessel_term_resume(pane_id: String) -> Result<(), String> {
    let t0 = Instant::now();
    let hit = if let Some(pane) = registry().get(&pane_id) {
        let mut p = pane.lock();
        p.paused = false;
        p.has_emitted_since_attach = false;
        true
    } else {
        false
    };
    perf(
        "resume",
        &format!(
            "pane={} hit={} dur_us={}",
            pane_id,
            hit,
            t0.elapsed().as_micros()
        ),
    );
    Ok(())
}

#[tauri::command]
pub fn kessel_term_detach(pane_id: String) -> Result<(), String> {
    let t0 = Instant::now();
    let hit = if let Some(pane) = registry().remove(&pane_id) {
        let mut p = pane.lock();
        if let Some(h) = p.reader_task.take() {
            h.abort();
        }
        true
    } else {
        false
    };
    let pane_count = registry().count();
    perf(
        "detach",
        &format!(
            "pane={} hit={} pane_count={} dur_us={}",
            pane_id,
            hit,
            pane_count,
            t0.elapsed().as_micros()
        ),
    );
    Ok(())
}

/// Frontend-driven timeline marker. Lets the JS side drop labeled
/// breadcrumbs into the perf log (workspace-switch-begin,
/// workspace-switch-end, etc.) so a single trace shows what
/// Rust-side work happened during a specific JS-observed window.
#[tauri::command]
pub fn kessel_term_perf_mark(label: String) -> Result<(), String> {
    perf("mark", &format!("label={:?} pane_count={}", label, registry().count()));
    Ok(())
}

/// Snapshot of the whole pane registry at the call instant.
/// Useful as a one-shot "what's mounted right now" dump for
/// diagnosis. Returns a structured list the frontend can log.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PaneInventoryEntry {
    pub pane_id: String,
    pub paused: bool,
    pub has_emitted_since_attach: bool,
    pub last_history_size: usize,
    pub dirty_counter: u64,
    pub cols: usize,
    pub rows: usize,
}

#[tauri::command]
pub fn kessel_term_perf_inventory() -> Result<Vec<PaneInventoryEntry>, String> {
    let t0 = Instant::now();
    let entries: Vec<PaneInventoryEntry> = {
        let panes = registry().panes.lock();
        panes
            .iter()
            .map(|(pane_id, arc)| {
                let p = arc.lock();
                let term = p.term.lock();
                PaneInventoryEntry {
                    pane_id: pane_id.clone(),
                    paused: p.paused,
                    has_emitted_since_attach: p.has_emitted_since_attach,
                    last_history_size: p.last_history_size,
                    dirty_counter: p.dirty_counter,
                    cols: term.columns(),
                    rows: term.screen_lines(),
                }
            })
            .collect()
    };
    perf(
        "inventory",
        &format!(
            "pane_count={} dur_us={}",
            entries.len(),
            t0.elapsed().as_micros()
        ),
    );
    Ok(entries)
}

// ── Tests ────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apc_filter_passes_regular_bytes_through() {
        let mut f = ApcFilter::new();
        let (clean, evs) = f.feed(b"hello world\n");
        assert_eq!(clean, b"hello world\n");
        assert!(evs.is_empty());
    }

    #[test]
    fn apc_filter_extracts_k2so_grow_boundary() {
        let mut f = ApcFilter::new();
        let input = b"before\x1b_k2so:grow_boundary:{\"target_cols\":80,\"target_rows\":24}\x07after";
        let (clean, evs) = f.feed(input);
        assert_eq!(clean, b"beforeafter");
        assert_eq!(evs.len(), 1);
        assert_eq!(evs[0].kind, "grow_boundary");
        assert_eq!(evs[0].payload["target_cols"], 80);
        assert_eq!(evs[0].payload["target_rows"], 24);
    }

    #[test]
    fn apc_filter_strips_non_k2so_apcs() {
        let mut f = ApcFilter::new();
        // A non-k2so APC (e.g. xterm OSC wrapped as APC) should
        // be stripped but not surfaced.
        let input = b"before\x1b_other:garbage\x07after";
        let (clean, evs) = f.feed(input);
        assert_eq!(clean, b"beforeafter");
        assert!(evs.is_empty());
    }

    #[test]
    fn apc_filter_handles_escape_without_apc() {
        let mut f = ApcFilter::new();
        // ESC followed by a non-`_` byte is not an APC — restore it.
        let input = b"\x1b[1;32mhello\x1b[0m";
        let (clean, evs) = f.feed(input);
        assert_eq!(clean, input);
        assert!(evs.is_empty());
    }

    #[test]
    fn apc_filter_handles_straddled_escape() {
        // APC split across two feed() calls — the filter must
        // buffer the partial and pick up on the next call.
        let mut f = ApcFilter::new();
        let (clean1, evs1) =
            f.feed(b"\x1b_k2so:grow_boundary:{\"target");
        assert!(clean1.is_empty());
        assert!(evs1.is_empty());
        let (clean2, evs2) = f.feed(b"_cols\":80,\"target_rows\":24}\x07");
        assert!(clean2.is_empty());
        assert_eq!(evs2.len(), 1);
        assert_eq!(evs2[0].kind, "grow_boundary");
        assert_eq!(evs2[0].payload["target_cols"], 80);
    }

    #[test]
    fn pane_state_absorbs_bytes_into_term() {
        let mut p = PaneState::new(80, 24);
        p.feed_bytes(b"hello\n");
        let snap = snapshot_term("test", &p);
        // Row 0 must start with "hello" — walk its runs and
        // concat `text` to reconstruct the row string, then
        // assert. This also sanity-checks the run-encoding
        // pipeline end-to-end.
        let row0_text: String =
            snap.grid[0].iter().map(|r| r.text.as_str()).collect();
        assert!(row0_text.starts_with("hello"), "row 0 was {row0_text:?}");
    }

    #[test]
    fn run_encoding_coalesces_same_style_cells() {
        let mut p = PaneState::new(80, 24);
        p.feed_bytes(b"plain text");
        let snap = snapshot_term("test", &p);
        // The whole printed word is one style (default), so the
        // first run should contain all 10 chars plus any trailing
        // blanks that share the default style.
        let row0 = &snap.grid[0];
        assert_eq!(row0.len(), 1, "plain text should be one run");
        assert!(row0[0].text.starts_with("plain text"));
        // Default style: fg/bg None, no flags.
        assert_eq!(row0[0].fg, None);
        assert_eq!(row0[0].bg, None);
        assert!(!row0[0].bold);
    }

    #[test]
    fn run_encoding_splits_on_sgr_change() {
        let mut p = PaneState::new(80, 24);
        // "red\x1b[31m red \x1b[0mnormal" — leading 'red' is
        // default-styled, then SGR 31 paints ' red ' in red,
        // then SGR 0 resets.
        p.feed_bytes(b"red\x1b[31m red \x1b[0mnormal");
        let snap = snapshot_term("test", &p);
        let row0 = &snap.grid[0];
        // Expect 3 runs: "red", " red ", "normal..." + possible
        // trailing blank-default run.
        assert!(
            row0.len() >= 3,
            "should have at least 3 style runs; got {:?}",
            row0
        );
        // Find the run containing " red " — its fg should be red.
        let red_run = row0.iter().find(|r| r.text.contains("red ")).unwrap();
        assert!(
            red_run.fg.is_some(),
            "red run should have explicit fg color"
        );
    }

    #[test]
    fn pane_state_grow_boundary_resizes_term() {
        let mut p = PaneState::new(80, 500);
        // Before: rows = 500
        assert_eq!(p.term.lock().screen_lines(), 500);
        let apc = b"\x1b_k2so:grow_boundary:{\"target_cols\":80,\"target_rows\":24,\"grow_rows\":500,\"reason\":\"idle\"}\x07";
        p.feed_bytes(apc);
        // After: rows = 24
        assert_eq!(p.term.lock().screen_lines(), 24);
    }
}
