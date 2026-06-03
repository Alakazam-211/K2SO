// Unit tests for the host-aware daemon-ws URL helpers + getDaemonWs
// remote branch (K2 Connect step #1).
//
// `getDaemonWs()` for the LOCAL host calls the Tauri `invoke` command;
// we mock @tauri-apps/api/core so the local path resolves deterministically
// and assert it carries host '127.0.0.1' (byte-identical to pre-host-aware).
//
// For a REMOTE active host, getDaemonWs derives creds directly from the
// connect-host store with NO invoke — we assert that and the URL shape.

import { describe, it, expect, beforeEach, vi } from 'vitest'

// localStorage stub so the connect-host store (imported transitively) is
// happy in the node env.
const mem = new Map<string, string>()
vi.stubGlobal('localStorage', {
  getItem: (k: string) => (mem.has(k) ? mem.get(k)! : null),
  setItem: (k: string, v: string) => void mem.set(k, v),
  removeItem: (k: string) => void mem.delete(k),
  clear: () => mem.clear(),
})

// Mock the Tauri invoke used by the LOCAL branch.
const invokeMock = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({
  invoke: (...args: unknown[]) => invokeMock(...args),
}))

import {
  getDaemonWs,
  invalidateDaemonWs,
  daemonHttpBase,
  daemonWsBase,
} from './daemon-ws'
import {
  useConnectHostStore,
  __resetConnectHostStoreForTests,
} from '@/stores/connect-host'

describe('daemon-ws URL helpers', () => {
  it('daemonHttpBase builds http://host:port', () => {
    expect(daemonHttpBase({ host: '127.0.0.1', port: 47800, token: 't' })).toBe(
      'http://127.0.0.1:47800',
    )
    expect(daemonHttpBase({ host: 'box.example', port: 9000, token: 't' })).toBe(
      'http://box.example:9000',
    )
  })

  it('daemonWsBase builds ws://host:port', () => {
    expect(daemonWsBase({ host: '127.0.0.1', port: 47800, token: 't' })).toBe(
      'ws://127.0.0.1:47800',
    )
  })
})

describe('getDaemonWs host-awareness', () => {
  beforeEach(() => {
    mem.clear()
    __resetConnectHostStoreForTests()
    invokeMock.mockReset()
    invalidateDaemonWs()
  })

  it('local: invokes daemon_ws_url and carries host 127.0.0.1', async () => {
    invokeMock.mockResolvedValue({ state: 'available', port: 47800, token: 'local-tok' })
    const creds = await getDaemonWs()
    expect(invokeMock).toHaveBeenCalledWith('daemon_ws_url')
    expect(creds).toEqual({ port: 47800, token: 'local-tok', host: '127.0.0.1' })
    // The resulting URL is byte-identical to the old hardcoded literal.
    expect(`${daemonHttpBase(creds)}/boot-status`).toBe('http://127.0.0.1:47800/boot-status')
  })

  it('remote: derives creds from the active ConnectHost, no invoke', async () => {
    const host = {
      id: 'r1',
      label: 'Remote',
      hostname: '10.0.0.9',
      port: 51234,
      token: 'remote-tok',
      remember: false,
      lastConnectedAt: null,
    }
    useConnectHostStore.getState().selectHost(host)
    // Invalidate any cached local creds so we prove the remote path
    // doesn't fall back to invoke.
    invalidateDaemonWs()
    const creds = await getDaemonWs()
    expect(invokeMock).not.toHaveBeenCalled()
    expect(creds).toEqual({ port: 51234, token: 'remote-tok', host: '10.0.0.9' })
    expect(`${daemonWsBase(creds)}/cli/sessions/grid`).toBe(
      'ws://10.0.0.9:51234/cli/sessions/grid',
    )
  })

  it('switching back to local resumes the invoke path', async () => {
    const host = {
      id: 'r1',
      label: 'Remote',
      hostname: '10.0.0.9',
      port: 51234,
      token: 'remote-tok',
      remember: false,
      lastConnectedAt: null,
    }
    useConnectHostStore.getState().selectHost(host)
    await getDaemonWs()
    expect(invokeMock).not.toHaveBeenCalled()

    useConnectHostStore.getState().selectHost('local')
    invokeMock.mockResolvedValue({ state: 'available', port: 47800, token: 'local-tok' })
    const creds = await getDaemonWs()
    expect(invokeMock).toHaveBeenCalledWith('daemon_ws_url')
    expect(creds.host).toBe('127.0.0.1')
  })
})
