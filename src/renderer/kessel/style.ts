// Kessel — SGR → CSS mapping.
//
// Takes `Style` (the fg/bg/attributes struct our Rust pipeline
// emits on Frame::Text) and produces CSS properties the renderer
// layer (I5) can apply directly to DOM spans.
//
// Defaults match the existing AlacrittyTerminalView so side-by-side
// tests (alacritty view vs Kessel view on the same session) render
// the same out-of-the-box palette.
//
// Phase 1 LineMux always emits `style: null` — styling parity with
// alacritty requires LineMux to lift SGR. When that lands, this
// module absorbs the new inputs without churn on the renderer
// side.

import type { CSSProperties } from 'react'
import type { Style } from './types'

/** Default foreground = 0xe0e0e0 (matches AlacrittyTerminalView). */
export const DEFAULT_FG = 0xe0e0e0
/** Default background = 0x0a0a0a. */
export const DEFAULT_BG = 0x0a0a0a

/** Convert a 0xRRGGBB int to a CSS `rgb(r,g,b)` string. Alpha is
 *  dropped — we don't do transparency for SGR colors today. */
export function colorToCss(c: number): string {
  const r = (c >> 16) & 0xff
  const g = (c >> 8) & 0xff
  const b = c & 0xff
  return `rgb(${r},${g},${b})`
}

/** Decode a `Style` into a `React.CSSProperties` object ready for
 *  spreading onto a DOM span. `null` input = default style (empty
 *  object — inherits container fg/bg).
 *
 *  Omits color properties that match the default so Kessel spans
 *  don't have `color: rgb(224,224,224)` on every cell — lets the
 *  container's color rule win and keeps the DOM lean.
 */
export function styleToCss(style: Style | null): CSSProperties {
  if (!style) return {}
  const css: CSSProperties = {}
  if (style.fg !== null && style.fg !== undefined && style.fg !== DEFAULT_FG) {
    css.color = colorToCss(style.fg)
  }
  if (style.bg !== null && style.bg !== undefined && style.bg !== DEFAULT_BG) {
    css.backgroundColor = colorToCss(style.bg)
  }
  if (style.bold) css.fontWeight = 'bold'
  if (style.italic) css.fontStyle = 'italic'
  if (style.underline) css.textDecoration = 'underline'
  return css
}

/** Quick equality check — used by the renderer to coalesce adjacent
 *  cells into a single `<span>` when their styles match. */
export function stylesEqual(a: Style | null, b: Style | null): boolean {
  if (a === b) return true
  if (!a || !b) return false
  return (
    a.fg === b.fg &&
    a.bg === b.bg &&
    a.bold === b.bold &&
    a.italic === b.italic &&
    a.underline === b.underline
  )
}
