import { describe, it, expect } from 'vitest'
import { colorToCss, styleToCss, stylesEqual, DEFAULT_FG, DEFAULT_BG } from './style'
import type { Style } from './types'

describe('colorToCss', () => {
  it('maps 0xRRGGBB to rgb(r,g,b)', () => {
    expect(colorToCss(0xff0000)).toBe('rgb(255,0,0)')
    expect(colorToCss(0x00ff00)).toBe('rgb(0,255,0)')
    expect(colorToCss(0x808080)).toBe('rgb(128,128,128)')
    expect(colorToCss(0x000000)).toBe('rgb(0,0,0)')
  })
})

describe('styleToCss', () => {
  it('returns empty object for null', () => {
    expect(styleToCss(null)).toEqual({})
  })

  it('omits color when it equals the default fg/bg', () => {
    const s: Style = {
      fg: DEFAULT_FG,
      bg: DEFAULT_BG,
      bold: false,
      italic: false,
      underline: false,
    }
    expect(styleToCss(s)).toEqual({})
  })

  it('emits color + backgroundColor for non-defaults', () => {
    const s: Style = {
      fg: 0xff0000,
      bg: 0x0000ff,
      bold: false,
      italic: false,
      underline: false,
    }
    expect(styleToCss(s)).toEqual({
      color: 'rgb(255,0,0)',
      backgroundColor: 'rgb(0,0,255)',
    })
  })

  it('handles null fg/bg (use terminal defaults)', () => {
    const s: Style = { fg: null, bg: null, bold: true, italic: false, underline: false }
    expect(styleToCss(s)).toEqual({ fontWeight: 'bold' })
  })

  it('emits fontWeight=bold when bold', () => {
    const s: Style = { fg: null, bg: null, bold: true, italic: false, underline: false }
    expect(styleToCss(s).fontWeight).toBe('bold')
  })

  it('emits fontStyle=italic when italic', () => {
    const s: Style = { fg: null, bg: null, bold: false, italic: true, underline: false }
    expect(styleToCss(s).fontStyle).toBe('italic')
  })

  it('emits textDecoration=underline when underline', () => {
    const s: Style = { fg: null, bg: null, bold: false, italic: false, underline: true }
    expect(styleToCss(s).textDecoration).toBe('underline')
  })

  it('combines multiple attributes', () => {
    const s: Style = {
      fg: 0xffff00,
      bg: null,
      bold: true,
      italic: true,
      underline: true,
    }
    expect(styleToCss(s)).toEqual({
      color: 'rgb(255,255,0)',
      fontWeight: 'bold',
      fontStyle: 'italic',
      textDecoration: 'underline',
    })
  })
})

describe('stylesEqual', () => {
  it('returns true for identical references', () => {
    const s: Style = { fg: 1, bg: 2, bold: false, italic: false, underline: false }
    expect(stylesEqual(s, s)).toBe(true)
  })

  it('returns true for null === null', () => {
    expect(stylesEqual(null, null)).toBe(true)
  })

  it('returns false when one side is null', () => {
    const s: Style = { fg: 1, bg: 2, bold: false, italic: false, underline: false }
    expect(stylesEqual(s, null)).toBe(false)
    expect(stylesEqual(null, s)).toBe(false)
  })

  it('compares field-by-field for structural equality', () => {
    const a: Style = { fg: 1, bg: 2, bold: false, italic: false, underline: false }
    const b: Style = { fg: 1, bg: 2, bold: false, italic: false, underline: false }
    expect(stylesEqual(a, b)).toBe(true)
  })

  it('detects differences in every field', () => {
    const base: Style = { fg: 1, bg: 2, bold: false, italic: false, underline: false }
    expect(stylesEqual(base, { ...base, fg: 99 })).toBe(false)
    expect(stylesEqual(base, { ...base, bg: 99 })).toBe(false)
    expect(stylesEqual(base, { ...base, bold: true })).toBe(false)
    expect(stylesEqual(base, { ...base, italic: true })).toBe(false)
    expect(stylesEqual(base, { ...base, underline: true })).toBe(false)
  })
})
