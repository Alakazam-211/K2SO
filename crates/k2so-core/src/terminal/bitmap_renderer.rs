use super::font_renderer::{GlyphCache, STYLE_BOLD, STYLE_BOLD_ITALIC, STYLE_ITALIC, STYLE_REGULAR};
use super::grid_types::{ATTR_BOLD, ATTR_DIM, ATTR_HIDDEN, ATTR_INVERSE, ATTR_ITALIC, ATTR_STRIKETHROUGH, ATTR_UNDERLINE, ATTR_WIDE};

// ── Default colors (must match terminal-theme.ts) ────────────────────────

const DEFAULT_FG: u32 = 0xe0e0e0;
const DEFAULT_BG: u32 = 0x0a0a0a;
const CURSOR_COLOR: u32 = 0x528bff;
const SELECTION_BG: u32 = 0x264f78;

// ── Bitmap buffer ────────────────────────────────────────────────────────

pub struct BitmapBuffer {
    /// RGBA pixel data, row-major order.
    pub pixels: Vec<u8>,
    /// Pixel width of the buffer.
    pub width: u32,
    /// Pixel height of the buffer.
    pub height: u32,
    /// Terminal columns.
    pub cols: u16,
    /// Terminal rows.
    pub rows: u16,
    /// Pixel width of one cell.
    pub cell_width: u32,
    /// Pixel height of one cell.
    pub cell_height: u32,
}

impl BitmapBuffer {
    /// Allocate a new bitmap buffer, filled with background color.
    pub fn new(cols: u16, rows: u16, cell_width: u32, cell_height: u32) -> Self {
        let width = cols as u32 * cell_width;
        let height = rows as u32 * cell_height;
        let pixel_count = (width * height) as usize;
        let mut pixels = vec![0u8; pixel_count * 4];

        // Fill with default background
        let (r, g, b) = unpack_rgb(DEFAULT_BG);
        for i in 0..pixel_count {
            let idx = i * 4;
            pixels[idx] = r;
            pixels[idx + 1] = g;
            pixels[idx + 2] = b;
            pixels[idx + 3] = 255;
        }

        BitmapBuffer {
            pixels,
            width,
            height,
            cols,
            rows,
            cell_width,
            cell_height,
        }
    }

    /// Check if the buffer needs resizing.
    pub fn needs_resize(&self, cols: u16, rows: u16, cell_width: u32, cell_height: u32) -> bool {
        self.cols != cols || self.rows != rows || self.cell_width != cell_width || self.cell_height != cell_height
    }
}

// ── Cell info for rendering ──────────────────────────────────────────────

/// Minimal cell data extracted from alacritty's grid for rendering.
pub struct CellInfo {
    pub ch: char,
    pub fg: u32,
    pub bg: u32,
    pub flags: u8,
}

/// Cursor info for a row.
pub struct CursorInfo {
    pub col: u16,
    pub shape: CursorShape,
    pub visible: bool,
}

#[derive(Clone, Copy)]
pub enum CursorShape {
    Bar,
    Block,
    Underline,
}

impl CursorShape {
    pub fn from_str(s: &str) -> Self {
        match s {
            "block" => CursorShape::Block,
            "underline" => CursorShape::Underline,
            _ => CursorShape::Bar,
        }
    }
}

// ── Row rendering ────────────────────────────────────────────────────────

/// Render a single row of cells into the bitmap buffer.
pub fn render_row(
    buf: &mut BitmapBuffer,
    row_idx: usize,
    cells: &[CellInfo],
    glyph_cache: &mut GlyphCache,
    cursor: Option<&CursorInfo>,
) {
    let y_start = row_idx as u32 * buf.cell_height;
    let stride = buf.width;

    let mut col = 0u32;
    let mut cell_idx = 0usize;

    while cell_idx < cells.len() && col < buf.cols as u32 {
        let cell = &cells[cell_idx];

        // Determine how many columns this cell spans
        let cell_span = if cell.flags & ATTR_WIDE != 0 { 2u32 } else { 1u32 };

        let x_start = col * buf.cell_width;

        // Resolve colors (handle INVERSE and DIM)
        let (mut fg, mut bg) = (cell.fg, cell.bg);

        if cell.flags & ATTR_INVERSE != 0 {
            std::mem::swap(&mut fg, &mut bg);
        }

        if cell.flags & ATTR_DIM != 0 {
            fg = dim_color(fg);
        }

        // Fill background for this cell (may span 2 cells for wide chars)
        let cell_pixel_width = cell_span * buf.cell_width;
        fill_rect(
            &mut buf.pixels,
            stride,
            x_start,
            y_start,
            cell_pixel_width,
            buf.cell_height,
            bg,
        );

        // Draw character (skip space, hidden, and null)
        if cell.ch != ' ' && cell.ch != '\0' && cell.flags & ATTR_HIDDEN == 0 {
            let style = match (cell.flags & ATTR_BOLD != 0, cell.flags & ATTR_ITALIC != 0) {
                (true, true) => STYLE_BOLD_ITALIC,
                (true, false) => STYLE_BOLD,
                (false, true) => STYLE_ITALIC,
                (false, false) => STYLE_REGULAR,
            };

            let glyph = glyph_cache.rasterize(cell.ch, style);
            if glyph.width > 0 && glyph.height > 0 {
                composite_glyph(
                    &mut buf.pixels,
                    stride,
                    glyph,
                    x_start,
                    y_start,
                    fg,
                );
            }
        }

        // Draw underline
        if cell.flags & ATTR_UNDERLINE != 0 {
            let underline_y = y_start + buf.cell_height - 1;
            fill_rect(
                &mut buf.pixels,
                stride,
                x_start,
                underline_y,
                cell_pixel_width,
                1,
                fg,
            );
        }

        // Draw strikethrough
        if cell.flags & ATTR_STRIKETHROUGH != 0 {
            let strike_y = y_start + buf.cell_height / 2;
            fill_rect(
                &mut buf.pixels,
                stride,
                x_start,
                strike_y,
                cell_pixel_width,
                1,
                fg,
            );
        }

        col += cell_span;
        cell_idx += 1;

        // Skip spacer cell after wide char
        if cell_span == 2 && cell_idx < cells.len() {
            cell_idx += 1;
        }
    }

    // Fill remaining columns with background (in case row has fewer cells than cols)
    if col < buf.cols as u32 {
        let x_start = col * buf.cell_width;
        let remaining_width = (buf.cols as u32 - col) * buf.cell_width;
        fill_rect(
            &mut buf.pixels,
            stride,
            x_start,
            y_start,
            remaining_width,
            buf.cell_height,
            DEFAULT_BG,
        );
    }

    // Draw cursor if on this row
    if let Some(cursor) = cursor {
        if cursor.visible {
            let cx = cursor.col as u32 * buf.cell_width;
            render_cursor(
                &mut buf.pixels,
                stride,
                cx,
                y_start,
                buf.cell_width,
                buf.cell_height,
                cursor.shape,
            );
        }
    }
}

// ── Cursor rendering ─────────────────────────────────────────────────────

fn render_cursor(
    pixels: &mut [u8],
    stride: u32,
    x: u32,
    y: u32,
    cell_width: u32,
    cell_height: u32,
    shape: CursorShape,
) {
    let (cr, cg, cb) = unpack_rgb(CURSOR_COLOR);

    match shape {
        CursorShape::Bar => {
            // 2px wide vertical bar
            let bar_width = 2u32.min(cell_width);
            for row in 0..cell_height {
                for col in 0..bar_width {
                    let px = x + col;
                    let py = y + row;
                    if px < stride && py * stride + px < (pixels.len() / 4) as u32 {
                        let idx = ((py * stride + px) * 4) as usize;
                        if idx + 3 < pixels.len() {
                            pixels[idx] = cr;
                            pixels[idx + 1] = cg;
                            pixels[idx + 2] = cb;
                            pixels[idx + 3] = 255;
                        }
                    }
                }
            }
        }
        CursorShape::Block => {
            // Semi-transparent filled rectangle
            for row in 0..cell_height {
                for col in 0..cell_width {
                    let px = x + col;
                    let py = y + row;
                    if px < stride {
                        let idx = ((py * stride + px) * 4) as usize;
                        if idx + 3 < pixels.len() {
                            // Alpha blend at ~60% opacity
                            let alpha = 153u32; // 0.6 * 255
                            let inv_alpha = 255 - alpha;
                            pixels[idx] = ((cr as u32 * alpha + pixels[idx] as u32 * inv_alpha) / 255) as u8;
                            pixels[idx + 1] = ((cg as u32 * alpha + pixels[idx + 1] as u32 * inv_alpha) / 255) as u8;
                            pixels[idx + 2] = ((cb as u32 * alpha + pixels[idx + 2] as u32 * inv_alpha) / 255) as u8;
                            pixels[idx + 3] = 255;
                        }
                    }
                }
            }
        }
        CursorShape::Underline => {
            // 2px horizontal line at bottom
            let line_height = 2u32.min(cell_height);
            for row in 0..line_height {
                let py = y + cell_height - line_height + row;
                for col in 0..cell_width {
                    let px = x + col;
                    if px < stride {
                        let idx = ((py * stride + px) * 4) as usize;
                        if idx + 3 < pixels.len() {
                            pixels[idx] = cr;
                            pixels[idx + 1] = cg;
                            pixels[idx + 2] = cb;
                            pixels[idx + 3] = 255;
                        }
                    }
                }
            }
        }
    }
}

// ── Glyph compositing ───────────────────────────────────────────────────

fn composite_glyph(
    pixels: &mut [u8],
    stride: u32,
    glyph: &super::font_renderer::RasterizedGlyph,
    cell_x: u32,
    cell_y: u32,
    fg_color: u32,
) {
    let (fg_r, fg_g, fg_b) = unpack_rgb(fg_color);

    for gy in 0..glyph.height {
        for gx in 0..glyph.width {
            let alpha = glyph.bitmap[(gy * glyph.width + gx) as usize];
            if alpha == 0 {
                continue;
            }

            let px = cell_x as i32 + glyph.x_offset + gx as i32;
            let py = cell_y as i32 + glyph.y_offset + gy as i32;

            // Bounds check
            if px < 0 || py < 0 || px >= stride as i32 || (py as u32) >= pixels.len() as u32 / (stride * 4) {
                continue;
            }

            let idx = ((py as u32 * stride + px as u32) * 4) as usize;
            if idx + 3 >= pixels.len() {
                continue;
            }

            if alpha == 255 {
                // Fast path: full opacity
                pixels[idx] = fg_r;
                pixels[idx + 1] = fg_g;
                pixels[idx + 2] = fg_b;
                pixels[idx + 3] = 255;
            } else {
                // Alpha blend
                let a = alpha as u32;
                let inv_a = 255 - a;
                pixels[idx] = ((fg_r as u32 * a + pixels[idx] as u32 * inv_a) / 255) as u8;
                pixels[idx + 1] = ((fg_g as u32 * a + pixels[idx + 1] as u32 * inv_a) / 255) as u8;
                pixels[idx + 2] = ((fg_b as u32 * a + pixels[idx + 2] as u32 * inv_a) / 255) as u8;
                pixels[idx + 3] = 255;
            }
        }
    }
}

// ── Utility functions ────────────────────────────────────────────────────

/// Fill a rectangle with a solid color.
fn fill_rect(
    pixels: &mut [u8],
    stride: u32,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: u32,
) {
    let (r, g, b) = unpack_rgb(color);
    let max_x = x + width;
    let max_y = y + height;

    for row in y..max_y {
        let row_start = (row * stride * 4) as usize;
        for col in x..max_x {
            if col >= stride {
                break;
            }
            let idx = row_start + (col * 4) as usize;
            if idx + 3 < pixels.len() {
                pixels[idx] = r;
                pixels[idx + 1] = g;
                pixels[idx + 2] = b;
                pixels[idx + 3] = 255;
            }
        }
    }
}

/// Unpack a 0xRRGGBB color to (r, g, b).
#[inline]
fn unpack_rgb(color: u32) -> (u8, u8, u8) {
    (
        ((color >> 16) & 0xFF) as u8,
        ((color >> 8) & 0xFF) as u8,
        (color & 0xFF) as u8,
    )
}

/// Dim a color by halving its RGB channels.
#[inline]
fn dim_color(color: u32) -> u32 {
    let r = ((color >> 16) & 0xFF) / 2;
    let g = ((color >> 8) & 0xFF) / 2;
    let b = (color & 0xFF) / 2;
    (r << 16) | (g << 8) | b
}
