/**
 * K2SO Events — MCP Channel Server for Claude Code
 *
 * This server bridges K2SO's internal event system to Claude Code's channel protocol.
 * It is spawned as a subprocess by Claude Code when launched with:
 *   claude --dangerously-load-development-channels server:k2so-events
 *
 * Architecture:
 * 1. Claude Code spawns this process, communicates over stdio
 * 2. This server polls K2SO's agent_hooks HTTP server for events
 * 3. Events are emitted as MCP channel notifications
 * 4. Claude processes them with full session context already loaded
 *
 * Environment variables (set by K2SO when launching):
 *   K2SO_PORT         - Port of K2SO's agent_hooks HTTP server
 *   K2SO_HOOK_TOKEN   - Auth token for the HTTP server
 *   K2SO_AGENT_NAME   - Name of the agent this channel serves
 *   K2SO_PROJECT_PATH - Project path for event filtering
 */

import { McpServer } from '@modelcontextprotocol/sdk/server/mcp.js'
import { StdioServerTransport } from '@modelcontextprotocol/sdk/server/stdio.js'

// ── Configuration ────────────────────────────────────────────────────────

const PORT = process.env.K2SO_PORT || ''
const TOKEN = process.env.K2SO_HOOK_TOKEN || ''
const AGENT_NAME = process.env.K2SO_AGENT_NAME || '__lead__'
const PROJECT_PATH = process.env.K2SO_PROJECT_PATH || ''
const POLL_INTERVAL_MS = 1000 // Poll every second

if (!PORT || !TOKEN) {
  console.error('[k2so-events] Missing K2SO_PORT or K2SO_HOOK_TOKEN environment variables')
  process.exit(1)
}

const BASE_URL = `http://127.0.0.1:${PORT}`

// ── MCP Server Setup ─────────────────────────────────────────────────────

const server = new McpServer({
  name: 'k2so-events',
  version: '1.0.0',
  capabilities: {
    experimental: {
      'claude/channel': {},
    },
    tools: {},
  },
  instructions: [
    'Events from K2SO workspace orchestration system.',
    'You will receive events when:',
    '- New work items arrive in your inbox',
    '- Git branches are updated',
    '- Sub-agents complete their tasks',
    '- The user sends you a message from the K2SO UI',
    '- Scheduled heartbeat check-ins occur',
    '',
    'React to events by checking your work queue and taking appropriate action.',
    'Use the `reply` tool to send status updates back to K2SO.',
  ].join('\n'),
})

// ── Reply Tool (agent → K2SO) ────────────────────────────────────────────

server.tool(
  'reply',
  'Send a message back to K2SO (status update, question for user, etc.)',
  {
    message: { type: 'string' as const, description: 'Message to send to K2SO' },
  },
  async ({ message }: { message: string }) => {
    try {
      const params = new URLSearchParams({
        token: TOKEN,
        project: PROJECT_PATH,
        agent: AGENT_NAME,
        message,
      })
      await fetch(`${BASE_URL}/cli/agent/reply?${params}`, { signal: AbortSignal.timeout(5000) })
      return { content: [{ type: 'text' as const, text: 'Message sent to K2SO.' }] }
    } catch (err) {
      return { content: [{ type: 'text' as const, text: `Failed to send: ${err}` }] }
    }
  },
)

// ── Event Polling Loop ───────────────────────────────────────────────────

async function pollEvents(): Promise<void> {
  try {
    const params = new URLSearchParams({
      token: TOKEN,
      project: PROJECT_PATH,
      agent: AGENT_NAME,
    })
    const response = await fetch(`${BASE_URL}/cli/events?${params}`, {
      signal: AbortSignal.timeout(5000),
    })

    if (!response.ok) return

    const events = (await response.json()) as Array<{
      type: string
      message: string
      priority?: string
      timestamp?: string
    }>

    for (const event of events) {
      await server.server.notification({
        method: 'notifications/claude/channel',
        params: {
          content: event.message,
          meta: {
            type: event.type,
            ...(event.priority ? { priority: event.priority } : {}),
            ...(event.timestamp ? { timestamp: event.timestamp } : {}),
          },
        },
      })
    }
  } catch {
    // K2SO might not be running or network hiccup — silently retry
  }
}

// ── Main ─────────────────────────────────────────────────────────────────

async function main(): Promise<void> {
  const transport = new StdioServerTransport()
  await server.connect(transport)

  // Start polling for events
  setInterval(pollEvents, POLL_INTERVAL_MS)
}

main().catch((err) => {
  console.error('[k2so-events] Fatal:', err)
  process.exit(1)
})
