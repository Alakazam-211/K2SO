// Plan B (Bulk-2) — vitest coverage for the workspace primary-agent client
// after migrating the 3 daemon-proxy commands (k2so_workspace_agent_display_name,
// k2so_workspace_set_agent_display_name, k2so_agents_resume_chat_args) OFF
// the Tauri invoke proxy ONTO the host-aware `daemonCliGet` HTTP layer.
//
// Routes (all GET, confirmed against crates/k2so-daemon/src/cli.rs):
//   GET workspace/agent-display-name?project=     → {display_name}  (snake)
//   GET workspace/set-agent-display-name?project=&name=  (GET mutation)
//   GET workspace/resume-chat-args?project=       → camelCase ResumeChatArgs

import { describe, it, expect, beforeEach, vi } from 'vitest'

const daemonCliGet = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...args: unknown[]) => daemonCliGet(...args),
}))

import {
  agentDisplayName,
  setAgentDisplayName,
  resumeChatArgs,
} from './workspace-agent'

describe('workspace-agent — Plan B host-aware migration', () => {
  beforeEach(() => {
    daemonCliGet.mockReset()
  })

  it('agentDisplayName GETs the snake_case route and unwraps display_name', async () => {
    daemonCliGet.mockResolvedValueOnce({ display_name: 'manager' })
    const name = await agentDisplayName('/work/proj')
    expect(daemonCliGet).toHaveBeenCalledWith('workspace/agent-display-name', {
      project: '/work/proj',
    })
    expect(name).toBe('manager')
  })

  it('agentDisplayName returns "" when display_name is absent', async () => {
    daemonCliGet.mockResolvedValueOnce({})
    expect(await agentDisplayName('/work/proj')).toBe('')
  })

  it('setAgentDisplayName GETs the mutation route with project + name', async () => {
    daemonCliGet.mockResolvedValueOnce({ success: true })
    await setAgentDisplayName('/work/proj', 'lead')
    expect(daemonCliGet).toHaveBeenCalledWith('workspace/set-agent-display-name', {
      project: '/work/proj',
      name: 'lead',
    })
  })

  it('resumeChatArgs GETs the camelCase route and returns the payload as-is', async () => {
    const payload = {
      command: 'claude',
      args: ['--resume', 'sid-1'],
      cwd: '/work/proj',
      resumeSession: 'sid-1',
    }
    daemonCliGet.mockResolvedValueOnce(payload)
    const result = await resumeChatArgs('/work/proj')
    expect(daemonCliGet).toHaveBeenCalledWith('workspace/resume-chat-args', {
      project: '/work/proj',
    })
    expect(result).toEqual(payload)
  })

  it('propagates a daemon error (display-name read)', async () => {
    daemonCliGet.mockRejectedValueOnce(new Error('daemon down'))
    await expect(agentDisplayName('/work/proj')).rejects.toThrow('daemon down')
  })
})
