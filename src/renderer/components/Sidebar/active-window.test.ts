// P1.C — Active-Bar rule 2 ("recently interacted") now honors a
// user-configurable window (settings.activeWindowHours, default 24, min 1)
// instead of a hard-coded 24h. `isWithinActiveWindow` is the extracted,
// pure predicate; this test pins the cutoff behavior at a small window.
//
// ActiveBar.tsx pulls in several stores at import time; mock the load-time
// boundaries so importing the module is inert in the node vitest env.

import { describe, it, expect, vi } from 'vitest'

vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn(async () => null) }))
vi.mock('@tauri-apps/api/event', () => ({
  emit: vi.fn(async () => undefined),
  listen: vi.fn(async () => () => undefined),
}))
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: vi.fn(async () => []),
  daemonCliPost: vi.fn(async () => ({})),
}))
vi.mock('@/lib/daemon-reconnect', () => ({ onDaemonConnected: vi.fn() }))
vi.mock('@/lib/daemon-settings', () => ({
  settingsGet: vi.fn(async () => ({})),
  settingsUpdate: vi.fn(async () => ({})),
  settingsReset: vi.fn(async () => ({})),
}))

import { isWithinActiveWindow, isAutonomouslyActive } from './ActiveBar'

describe('P1.C — isWithinActiveWindow (Active-Bar rule 2)', () => {
  // All values in unix SECONDS (matching ActiveBar's clock).
  const now = 1_000_000

  it('returns false when there is no lastInteractionAt', () => {
    expect(isWithinActiveWindow(null, now, 24)).toBe(false)
    expect(isWithinActiveWindow(undefined, now, 24)).toBe(false)
  })

  it('respects a SMALL configured window (1 hour)', () => {
    const oneHour = 60 * 60
    // 30 minutes ago — inside a 1h window.
    expect(isWithinActiveWindow(now - oneHour / 2, now, 1)).toBe(true)
    // 90 minutes ago — outside a 1h window.
    expect(isWithinActiveWindow(now - oneHour * 1.5, now, 1)).toBe(false)
  })

  it('respects a LARGER configured window (48 hours)', () => {
    const hour = 60 * 60
    // 36h ago — outside the default 24h, but inside a 48h window.
    expect(isWithinActiveWindow(now - hour * 36, now, 48)).toBe(true)
    // 60h ago — outside even the 48h window.
    expect(isWithinActiveWindow(now - hour * 60, now, 48)).toBe(false)
  })

  it('clamps sub-1 / non-finite windows to a 1h floor', () => {
    const hour = 60 * 60
    // windowHours 0 → clamped to 1h. 30min ago is in, 90min ago is out.
    expect(isWithinActiveWindow(now - hour / 2, now, 0)).toBe(true)
    expect(isWithinActiveWindow(now - hour * 2, now, 0)).toBe(false)
    expect(isWithinActiveWindow(now - hour / 2, now, NaN)).toBe(true)
  })
})

describe('P3 — isAutonomouslyActive (Active-Bar autonomous indicator)', () => {
  const now = 1_000_000
  const hour = 60 * 60
  const recent = now - hour / 2 // 30 min ago — inside the default window

  it('shows for a heartbeat workspace freshly bumped by a work-fire', () => {
    // heartbeatEnabled=1, inside window, user not driving → self-driving.
    expect(isAutonomouslyActive(1, recent, now, 24, false)).toBe(true)
  })

  it('does NOT show when no heartbeat is enabled (user-only workspace)', () => {
    // A user could have bumped lastInteractionAt; with no heartbeat it
    // is NOT autonomous (it shows as a plain Active item, no badge).
    expect(isAutonomouslyActive(0, recent, now, 24, false)).toBe(false)
  })

  it('does NOT show while the user session is actively working', () => {
    // The braille spinner wins — a user-driven turn must not read as
    // self-driving even on a heartbeat-enabled workspace.
    expect(isAutonomouslyActive(1, recent, now, 24, true)).toBe(false)
  })

  it('does NOT show once the workspace has aged out of the window', () => {
    // No recent work-fire → outside the window → ages out normally
    // (mirrors the daemon gate: a no-op wake never bumps the stamp).
    expect(isAutonomouslyActive(1, now - hour * 36, now, 24, false)).toBe(false)
    expect(isAutonomouslyActive(1, null, now, 24, false)).toBe(false)
  })
})
