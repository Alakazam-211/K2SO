// Kessel config — default preservation + override merge tests.
import { describe, expect, it } from 'bun:test'

import {
  defaultKesselConfig,
  mergeKesselConfig,
  type KesselConfig,
} from './config'

describe('defaultKesselConfig', () => {
  it('preserves the pre-config constants exactly', () => {
    // If a future refactor changes any of these defaults, the
    // pane's out-of-the-box look-and-feel drifts from v0.34.0
    // baseline — which is the thing `defaultKesselConfig` is
    // supposed to prevent. Any intentional change to a default
    // should bump the font baseline in a dedicated commit, not
    // ride along with an unrelated refactor.
    expect(defaultKesselConfig.font.size).toBe(14)
    expect(defaultKesselConfig.font.lineHeightMultiplier).toBe(1.2)
    expect(defaultKesselConfig.font.family).toContain('MesloLGM Nerd Font')

    expect(defaultKesselConfig.colors.foreground).toBe(0xe0e0e0)
    expect(defaultKesselConfig.colors.background).toBe(0x0a0a0a)
    expect(defaultKesselConfig.colors.palette).toHaveLength(16)

    expect(defaultKesselConfig.scrolling.cap).toBe(10_000)
    expect(defaultKesselConfig.scrolling.multiplier).toBe(3)

    expect(defaultKesselConfig.cursor.defaultShape).toBe('steady_block')
    expect(defaultKesselConfig.cursor.settleMs).toBe(60)

    expect(defaultKesselConfig.bell.mode).toBe('visual')
    expect(defaultKesselConfig.mouse.shiftOverrideEnabled).toBe(true)
    expect(defaultKesselConfig.performance.syncUpdateTimeoutMs).toBe(150)
  })
})

describe('mergeKesselConfig', () => {
  it('returns the default when no overrides are passed', () => {
    expect(mergeKesselConfig()).toBe(defaultKesselConfig)
  })

  it('shallow-merges top-level sections without mutating defaults', () => {
    const merged: KesselConfig = mergeKesselConfig({
      font: { size: 18 },
      scrolling: { multiplier: 5 },
    })
    expect(merged.font.size).toBe(18)
    expect(merged.font.family).toBe(defaultKesselConfig.font.family)
    expect(merged.scrolling.multiplier).toBe(5)
    expect(merged.scrolling.cap).toBe(defaultKesselConfig.scrolling.cap)
    // Defaults must remain untouched so other panes aren't affected.
    expect(defaultKesselConfig.font.size).toBe(14)
    expect(defaultKesselConfig.scrolling.multiplier).toBe(3)
  })

  it('leaves unspecified sections pointing at the default section', () => {
    const merged = mergeKesselConfig({ font: { size: 20 } })
    // bell / cursor / mouse / performance were not overridden; they
    // should be the defaults, though they may be either the same
    // object reference or a shallow copy depending on the impl.
    expect(merged.bell).toEqual(defaultKesselConfig.bell)
    expect(merged.cursor).toEqual(defaultKesselConfig.cursor)
  })
})
