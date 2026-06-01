import { describe, expect, it } from 'vitest'

import { computeDesiredActive } from './activeViewer'

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
