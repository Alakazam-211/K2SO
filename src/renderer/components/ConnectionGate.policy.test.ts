// Unit tests for the ConnectionGate acceptance POLICIES (K2 Connect
// step #4). The gate component itself is React/Tauri-bound; these tests
// pin the version/protocol decision logic that the policies encapsulate,
// which is the part the PRD requires to differ for local vs remote.
//
//   - localPairedPolicy: exact version-string match (auto-update guard).
//   - remoteHostPolicy:  protocol range-check, NO version-string match
//     (a remote daemon may run a different marketing version).

import { describe, it, expect } from 'vitest'
import { localPairedPolicy, remoteHostPolicy, shouldSurfaceRemoteDrop } from './ConnectionGate'

const status = (over: Partial<{ version: string; protocol: number; phase: string; detail: string }> = {}) => ({
  version: '0.39.15',
  protocol: 1,
  phase: 'ready',
  detail: '',
  ...over,
})

describe('localPairedPolicy', () => {
  it('accepts the exact-version, ready daemon', () => {
    const p = localPairedPolicy('0.39.15')
    expect(p.decide(status()).kind).toBe('accept')
  })

  it('waits on a version mismatch (outgoing old daemon mid-update)', () => {
    const p = localPairedPolicy('0.39.15')
    expect(p.decide(status({ version: '0.39.14' })).kind).toBe('wait')
  })

  it('shows migrating when the right version is not yet ready', () => {
    const p = localPairedPolicy('0.39.15')
    const d = p.decide(status({ phase: 'migrating', detail: 'x' }))
    expect(d.kind).toBe('migrating')
  })

  it('waits when unreachable (null status)', () => {
    expect(localPairedPolicy('0.39.15').decide(null).kind).toBe('wait')
  })
})

describe('remoteHostPolicy', () => {
  it('accepts a DIFFERENT marketing version when protocol is compatible + ready', () => {
    const p = remoteHostPolicy()
    // Remote daemon on a totally different version string — still accepted.
    expect(p.decide(status({ version: '1.2.3-hosted', protocol: 1 })).kind).toBe('accept')
  })

  it('accepts a higher (forward-compatible) protocol', () => {
    expect(remoteHostPolicy().decide(status({ protocol: 5 })).kind).toBe('accept')
  })

  it('waits when the remote protocol is below the minimum', () => {
    expect(remoteHostPolicy().decide(status({ protocol: 0 })).kind).toBe('wait')
  })

  it('shows migrating when ready phase not reached', () => {
    expect(remoteHostPolicy().decide(status({ phase: 'starting' })).kind).toBe('migrating')
  })

  it('waits when unreachable (null status)', () => {
    expect(remoteHostPolicy().decide(null).kind).toBe('wait')
  })

  it('does NOT require version-string equality (the local guard must not leak in)', () => {
    // Same status object that localPairedPolicy('X') would REJECT on
    // version is accepted by the remote policy.
    const local = localPairedPolicy('expected-version')
    const remote = remoteHostPolicy()
    const s = status({ version: 'something-else' })
    expect(local.decide(s).kind).toBe('wait')
    expect(remote.decide(s).kind).toBe('accept')
  })
})

// K2 Connect step #4 — DEBOUNCE the drop. A single slow/blipped health-poll
// over a higher-latency tunnel must NOT surface the reconnect banner while
// the data WS is still streaming; only >= REMOTE_DROP_THRESHOLD (2)
// CONSECUTIVE failed polls count as a genuine drop. This pins the threshold
// rule the gate's poll loop uses (N-1 fails → no banner; Nth → banner).
describe('shouldSurfaceRemoteDrop (debounced drop)', () => {
  it('a single blip does NOT surface the banner (N-1 = 1 fail)', () => {
    expect(shouldSurfaceRemoteDrop(1)).toBe(false)
  })

  it('the threshold-th consecutive fail surfaces the banner (N = 2)', () => {
    expect(shouldSurfaceRemoteDrop(2)).toBe(true)
  })

  it('stays surfaced past the threshold', () => {
    expect(shouldSurfaceRemoteDrop(3)).toBe(true)
    expect(shouldSurfaceRemoteDrop(10)).toBe(true)
  })

  it('zero fails (just connected / recovered) is never a drop', () => {
    expect(shouldSurfaceRemoteDrop(0)).toBe(false)
  })
})
