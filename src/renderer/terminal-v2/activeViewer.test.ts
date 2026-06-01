import { describe, expect, it } from 'vitest'

import { computeDesiredActive, shouldHoldGridWs } from './activeViewer'

// Exhaustive truth table for the Issue #8 active-viewer predicate.
// The active claim must be true ONLY when all three signals hold;
// any one being false means this pane is not the live viewer and
// must release (send `set_active:false`).
describe('computeDesiredActive', () => {
  const cases: Array<{
    visible: boolean
    paneFocused: boolean
    windowFocused: boolean
    expected: boolean
  }> = [
    { visible: false, paneFocused: false, windowFocused: false, expected: false },
    { visible: false, paneFocused: false, windowFocused: true, expected: false },
    { visible: false, paneFocused: true, windowFocused: false, expected: false },
    { visible: false, paneFocused: true, windowFocused: true, expected: false },
    { visible: true, paneFocused: false, windowFocused: false, expected: false },
    { visible: true, paneFocused: false, windowFocused: true, expected: false },
    { visible: true, paneFocused: true, windowFocused: false, expected: false },
    { visible: true, paneFocused: true, windowFocused: true, expected: true },
  ]

  for (const c of cases) {
    it(`visible=${c.visible} paneFocused=${c.paneFocused} windowFocused=${c.windowFocused} -> ${c.expected}`, () => {
      expect(
        computeDesiredActive({
          visible: c.visible,
          paneFocused: c.paneFocused,
          windowFocused: c.windowFocused,
        }),
      ).toBe(c.expected)
    })
  }

  it('only the all-true combination claims active', () => {
    const trueCount = cases.filter((c) => c.expected).length
    expect(trueCount).toBe(1)
  })
})

// Issue #8 (0.39.13) — grid-WS hold predicate. A pane streams the
// session's live grid ONLY while visible AND its child hasn't exited.
// Hidden background tabs / off-screen heartbeat spawns hold no grid-WS
// (their daemon PTY survives untouched); an exited session has nothing
// left to stream.
describe('shouldHoldGridWs', () => {
  const cases: Array<{
    visible: boolean
    exited: boolean
    expected: boolean
  }> = [
    { visible: false, exited: false, expected: false },
    { visible: false, exited: true, expected: false },
    { visible: true, exited: false, expected: true },
    { visible: true, exited: true, expected: false },
  ]

  for (const c of cases) {
    it(`visible=${c.visible} exited=${c.exited} -> ${c.expected}`, () => {
      expect(shouldHoldGridWs({ visible: c.visible, exited: c.exited })).toBe(
        c.expected,
      )
    })
  }

  it('holds the grid-WS only for a visible, not-yet-exited pane', () => {
    const trueCount = cases.filter((c) => c.expected).length
    expect(trueCount).toBe(1)
  })

  it('a hidden pane never holds a grid-WS regardless of exit state', () => {
    expect(shouldHoldGridWs({ visible: false, exited: false })).toBe(false)
    expect(shouldHoldGridWs({ visible: false, exited: true })).toBe(false)
  })
})
