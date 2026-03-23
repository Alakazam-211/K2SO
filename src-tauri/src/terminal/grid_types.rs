use serde::Serialize;

/// A single cell in the terminal grid.
#[derive(Serialize, Clone, Debug)]
pub struct TerminalCell {
    pub c: String,
    pub fg: u32,
    pub bg: u32,
    pub flags: u8,
}

/// Attribute flag constants.
pub const ATTR_BOLD: u8 = 1;
pub const ATTR_ITALIC: u8 = 2;
pub const ATTR_UNDERLINE: u8 = 4;
pub const ATTR_STRIKETHROUGH: u8 = 8;
pub const ATTR_INVERSE: u8 = 16;
pub const ATTR_DIM: u8 = 32;
pub const ATTR_HIDDEN: u8 = 64;
pub const ATTR_WIDE: u8 = 128;

/// A style span — defines fg/bg/flags for a range of columns in a line.
/// Only emitted for cells that differ from default (fg=0xe0e0e0, bg=0x0a0a0a, flags=0).
#[derive(Serialize, Clone, Debug)]
pub struct StyleSpan {
    /// Start column (inclusive).
    pub s: u16,
    /// End column (inclusive).
    pub e: u16,
    /// Foreground color. Omitted if default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fg: Option<u32>,
    /// Background color. Omitted if default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bg: Option<u32>,
    /// Attribute flags. Omitted if 0.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fl: Option<u8>,
}

/// A compact line representation: text content + sparse style spans.
/// This is ~10-20x smaller than per-cell TerminalCell arrays for typical content.
#[derive(Serialize, Clone, Debug)]
pub struct CompactLine {
    /// Row index (0 = top of visible area).
    pub row: u16,
    /// Plain text content of the line (trimmed trailing spaces).
    pub text: String,
    /// Style spans for non-default cells. Empty array = all default styling.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spans: Vec<StyleSpan>,
}

/// A grid update sent from Rust → frontend via Tauri event.
#[derive(Serialize, Clone, Debug)]
pub struct GridUpdate {
    pub cols: u16,
    pub rows: u16,
    pub cursor_col: u16,
    pub cursor_row: u16,
    pub cursor_visible: bool,
    pub cursor_shape: String,
    /// Compact lines (text + sparse style spans).
    pub lines: Vec<CompactLine>,
    pub full: bool,
    pub mode: u32,
    /// Current scroll offset (0 = bottom, >0 = scrolled up into history).
    pub display_offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selection: Option<[u16; 4]>,
    /// Performance instrumentation (debug builds only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub perf: Option<PerfInfo>,
}

/// Performance metrics for debugging.
#[derive(Serialize, Clone, Debug)]
pub struct PerfInfo {
    /// Time to snapshot the grid (microseconds).
    pub snapshot_us: u64,
    /// Number of lines in this update.
    pub line_count: u16,
    /// Total text bytes across all lines.
    pub text_bytes: u32,
    /// Total style spans across all lines.
    pub span_count: u16,
}

// ── Bitmap rendering IPC types ────────────────────────────────────────────

/// A bitmap frame sent from Rust → frontend via Tauri event.
/// Contains a QOI-compressed RGBA image of the terminal, base64-encoded.
#[derive(Serialize, Clone, Debug)]
pub struct BitmapUpdate {
    /// Base64-encoded QOI image bytes.
    pub image_b64: String,
    /// Pixel dimensions of the image.
    pub width: u32,
    pub height: u32,
    /// Terminal grid dimensions.
    pub cols: u16,
    pub rows: u16,
    /// Logical cell dimensions (pre-DPR) for mouse coordinate mapping.
    pub cell_width: u32,
    pub cell_height: u32,
    /// Cursor position.
    pub cursor_col: u16,
    pub cursor_row: u16,
    pub cursor_visible: bool,
    /// Terminal mode bits (APP_CURSOR, APP_KEYPAD, etc.).
    pub mode: u32,
    /// Current scroll offset (0 = bottom, >0 = scrolled up into history).
    pub display_offset: usize,
    /// Performance instrumentation.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub perf: Option<BitmapPerfInfo>,
}

/// Performance metrics for bitmap rendering.
#[derive(Serialize, Clone, Debug)]
pub struct BitmapPerfInfo {
    /// Time to render damaged rows to RGBA buffer (microseconds).
    pub render_us: u64,
    /// Time to QOI encode (microseconds).
    pub qoi_us: u64,
    /// Compressed QOI size in bytes.
    pub qoi_bytes: u32,
    /// Number of rows re-rendered in this frame.
    pub damaged_rows: u16,
}

/// A lightweight update for small changes (1-5 rows).
/// Sends raw RGBA strips instead of QOI-encoding the whole bitmap.
#[derive(Serialize, Clone, Debug)]
pub struct RowStripUpdate {
    /// Base64-encoded raw RGBA bytes for the damaged rows (concatenated).
    pub strips_b64: String,
    /// Row indices that were updated (in order).
    pub rows: Vec<u16>,
    /// Pixel dimensions per strip.
    pub strip_width: u32,
    pub strip_height: u32,
    /// Terminal grid info.
    pub cols: u16,
    pub total_rows: u16,
    pub cell_width: u32,
    pub cell_height: u32,
    /// Cursor info.
    pub cursor_col: u16,
    pub cursor_row: u16,
    pub cursor_visible: bool,
    pub mode: u32,
}

// ── Selection types ──────────────────────────────────────────────────────

/// Selection action sent from frontend → Rust.
#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SelectionAction {
    Start,
    Update,
    End,
}

/// Selection request from the frontend.
#[derive(serde::Deserialize, Debug)]
#[serde(rename_all = "camelCase")]
pub struct SelectionRequest {
    pub action: SelectionAction,
    pub col: u16,
    pub row: u16,
}
