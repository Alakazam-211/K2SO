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
use std::time::Duration;

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
    /// Monotonic counter of grid mutations since last snapshot —
    /// frontend uses this to skip snapshots when nothing has
    /// changed between rAF ticks.
    dirty_counter: u64,
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
            reader_task: None,
        }
    }

    /// Process a chunk of bytes from the daemon. APC filter
    /// first, then anything non-k2so flows into the vte. APC
    /// events are applied as side effects on the Term.
    fn feed_bytes(&mut self, chunk: &[u8]) {
        let (clean, events) = self.apc_filter.feed(chunk);
        for ev in events {
            self.apply_apc_event(ev);
        }
        if !clean.is_empty() {
            let mut term = self.term.lock();
            self.processor.advance(&mut *term, &clean);
            self.dirty_counter = self.dirty_counter.wrapping_add(1);
        }
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
    /// Live grid rows, top-to-bottom. Each row is a string of
    /// character cells. Styling collapsed for Phase 4 — v2 can
    /// serialize per-cell style for SGR fidelity.
    pub grid: Vec<String>,
    /// Scrollback rows, oldest-first.
    pub scrollback: Vec<String>,
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

/// Project a Term into a serializable snapshot. Walks both the
/// live viewport and scrollback, building one string per row. For
/// Phase 4 this is text-only — SGR styling is dropped. Phase 5
/// will enrich with per-cell style when the DOM renderer is
/// ready to consume it.
fn snapshot_term(pane_id: &str, state: &PaneState) -> TermGridSnapshot {
    let term = state.term.lock();
    let cols = term.columns();
    let rows = term.screen_lines();
    let grid = term.grid();

    // Live rows: Line(0) to Line(rows-1).
    let mut live_rows: Vec<String> = Vec::with_capacity(rows);
    for r in 0..(rows as i32) {
        let line = Line(r);
        let mut s = String::with_capacity(cols);
        for c in 0..cols {
            let cell: &Cell = &grid[Point::new(line, Column(c))];
            let ch = cell.c;
            if ch == '\0' {
                s.push(' ');
            } else {
                s.push(ch);
            }
        }
        live_rows.push(s);
    }

    // Scrollback: lines above Line(0). Alacritty stores them at
    // negative line indices up to -total_lines_above. display_offset
    // tells us how far we've scrolled; we want the full history
    // regardless, so we walk from (-history) up to (-1).
    let history_size = grid.history_size();
    let mut scrollback_rows: Vec<String> = Vec::with_capacity(history_size);
    for r in (1..=history_size as i32).rev() {
        let line = Line(-r);
        let mut s = String::with_capacity(cols);
        for c in 0..cols {
            let cell: &Cell = &grid[Point::new(line, Column(c))];
            let ch = cell.c;
            if ch == '\0' {
                s.push(' ');
            } else {
                s.push(ch);
            }
        }
        scrollback_rows.push(s);
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

    // Emit snapshots on a light cadence so the frontend sees
    // updates without being hammered every byte. 30 Hz is enough
    // for terminal UI.
    let mut snapshot_interval =
        tokio::time::interval(Duration::from_millis(33));
    snapshot_interval.set_missed_tick_behavior(
        tokio::time::MissedTickBehavior::Skip,
    );

    let mut last_emitted_version: u64 = u64::MAX;

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
                    }
                    Some(Ok(Message::Text(_))) => {
                        // session:ack envelope; ignored for now.
                        // Phase 5 can parse this for the
                        // front/back offset gap indicator.
                    }
                    Some(Ok(Message::Ping(p))) => {
                        let _ = write.send(Message::Pong(p)).await;
                    }
                    Some(Ok(_)) => {}
                }
            }
            _ = snapshot_interval.tick() => {
                // Snap + push if dirty.
                let snapshot = {
                    let p = pane.lock();
                    if p.dirty_counter == last_emitted_version {
                        None
                    } else {
                        last_emitted_version = p.dirty_counter;
                        Some(snapshot_term(&pane_id, &p))
                    }
                };
                if let Some(snap) = snapshot {
                    let _ = app.emit("kessel:grid-snapshot", snap);
                }
            }
        }
    }

    // Final snapshot on close so the frontend sees the true
    // terminal state when the session ends.
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
}

#[tauri::command]
pub async fn kessel_term_attach(
    app: tauri::AppHandle,
    args: AttachArgs,
) -> Result<(), String> {
    // Allocate a pane at the GROW_ROWS size to absorb the grow-
    // phase paint. Alacritty will shrink-from-top to the target
    // when the APC arrives.
    let initial_rows = args.rows.max(500);
    let state = PaneState::new(args.cols, initial_rows);
    let pane_arc = registry().insert(args.pane_id.clone(), state);

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

#[tauri::command]
pub fn kessel_term_detach(pane_id: String) -> Result<(), String> {
    if let Some(pane) = registry().remove(&pane_id) {
        // Dropping the last Arc triggers PaneState's Drop which
        // aborts the reader task. In case there are outstanding
        // Arcs held by the reader task closure, the explicit abort
        // below ensures we don't leak a long-running connection.
        let mut p = pane.lock();
        if let Some(h) = p.reader_task.take() {
            h.abort();
        }
    }
    Ok(())
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
        assert!(snap.grid[0].starts_with("hello"));
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
