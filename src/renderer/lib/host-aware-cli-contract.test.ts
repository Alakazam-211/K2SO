// K2 Connect remote-completeness — contract tests for the renderer-side
// `invoke(...)` → `daemonCliGet(...)` swaps that make local-pinned Tauri
// commands hit the ACTIVE host (local OR a remote K2 Connect daemon).
//
// The production call sites are inline in heavy React components / the
// active-agents polling listener (HeartbeatsSection.commitRename,
// AgentPane/AgentChatPane/active-agents agents/lock), which can't be
// exercised without rendering the full component tree + Tauri event
// system. These tests instead lock the WIRE CONTRACT each swap must
// satisfy: the exact `daemonCliGet(route, params)` route name + param
// KEYS, including the two riskiest REMAPS:
//
//   1. heartbeat/rename — renderer var oldName/newName → route reads
//      `from`/`to` (verified crates/k2so-daemon/src/heartbeat_routes.rs).
//   2. agents/lock      — renderer projectPath/agentName/terminalId →
//      route reads project/agent/terminal_id (verified
//      crates/k2so-daemon/src/cli.rs `/cli/agents/lock`).
//
// A regression that reverts a remap (e.g. sends `oldName` again, which the
// route would silently drop → empty rename) fails these loudly. The param
// builders below mirror EXACTLY what the call sites construct.

import { describe, it, expect, beforeEach, vi } from 'vitest'

const daemonCliGet = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
}))

import { daemonCliGet as cli } from '@/lib/daemon-cli'

// Mirrors HeartbeatsSection.commitRename: renderer holds row.name (old) +
// the trimmed/lowercased draft (new); the route reads `from`/`to`.
function renameHeartbeat(project: string, oldName: string, newName: string) {
  return cli('heartbeat/rename', { project, from: oldName, to: newName })
}

// Mirrors the agents/lock call sites (active-agents ×2, AgentPane ×2,
// AgentChatPane ×3): renderer holds projectPath/agentName/terminalId/owner;
// the route reads project/agent/terminal_id/owner.
function lockAgent(
  projectPath: string,
  agentName: string,
  terminalId: string,
  owner: 'user' | 'system',
) {
  return cli('agents/lock', {
    project: projectPath,
    agent: agentName,
    terminal_id: terminalId,
    owner,
  })
}

describe('host-aware CLI swaps — wire contract', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
    daemonCliGet.mockResolvedValue({ success: true })
  })

  it('heartbeat/rename remaps oldName/newName → from/to (no stray oldName/newName)', async () => {
    await renameHeartbeat('/work/proj', 'nightly', 'overnight')
    expect(daemonCliGet).toHaveBeenCalledWith('heartbeat/rename', {
      project: '/work/proj',
      from: 'nightly',
      to: 'overnight',
    })
    const [, params] = daemonCliGet.mock.calls[0] as [string, Record<string, unknown>]
    // The pre-swap renderer keys must NOT survive — the route ignores them,
    // so leaving them in would silently no-op the rename on a remote host.
    expect(params).not.toHaveProperty('oldName')
    expect(params).not.toHaveProperty('newName')
  })

  it('agents/lock remaps projectPath/agentName/terminalId → project/agent/terminal_id', async () => {
    await lockAgent('/work/proj', 'manager', 'agent-chat:proj:manager', 'user')
    expect(daemonCliGet).toHaveBeenCalledWith('agents/lock', {
      project: '/work/proj',
      agent: 'manager',
      terminal_id: 'agent-chat:proj:manager',
      owner: 'user',
    })
    const [, params] = daemonCliGet.mock.calls[0] as [string, Record<string, unknown>]
    expect(params).not.toHaveProperty('projectPath')
    expect(params).not.toHaveProperty('agentName')
    expect(params).not.toHaveProperty('terminalId')
  })
})
