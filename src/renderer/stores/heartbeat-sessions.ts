import { create } from 'zustand'
import { invoke } from '@tauri-apps/api/core'

/**
 * Heartbeat sessions store — drives the sidebar Heartbeats panel.
 *
 * Joins three sources for the active workspace:
 *   1. agent_heartbeats rows (active + archived) — via Tauri commands
 *   2. running-agents telemetry — to derive the live indicator
 *   3. CLI session-id history — to know if a row is "Resumable"
 *
 * The panel polls; transitions are not WebSocket-driven yet — see
 * `.k2so/prds/heartbeats-sidebar-audit.md` Phase 2.4 for the lazy
 * subscription rationale (silent autonomous heartbeats avoid the
 * full grid WS bandwidth cost).
 */

export type HeartbeatSessionState = 'live' | 'resumable' | 'scheduled' | 'archived'

export interface HeartbeatRow {
  id: string
  projectId: string
  name: string
  frequency: string
  specJson: string
  wakeupPath: string
  enabled: boolean
  lastFired: string | null
  lastSessionId: string | null
  archivedAt: string | null
  createdAt: number
}

export interface HeartbeatEntry {
  /** The DB row. */
  row: HeartbeatRow
  /** Derived display state. */
  state: HeartbeatSessionState
  /** Live PTY terminal id when state === 'live'; null otherwise.
   *  Used by the click-to-focus handler in the sidebar. */
  liveTerminalId: string | null
}

interface HeartbeatSessionsState {
  /** Active (non-archived) heartbeats for the currently-loaded project. */
  active: HeartbeatEntry[]
  /** Archived heartbeats — rendered in the sidebar's collapsed
   *  Archived section. */
  archived: HeartbeatEntry[]
  /** projectPath the cached lists belong to; null when never loaded. */
  loadedFor: string | null
  /** True while a refresh is in-flight (suppresses overlapping polls). */
  loading: boolean
  /** Last error from the refresh path. The panel surfaces this directly
   *  instead of silently rendering an empty list. */
  lastError: string | null

  refresh: (projectPath: string) => Promise<void>
  /** Clear cached state (call when no workspace is active). */
  clear: () => void
}

interface RunningAgentInfo {
  terminalId: string
  cwd: string
  command: string | null
}

/**
 * Map a heartbeat row to its display state by joining against running PTY
 * telemetry. The match is approximate — heartbeat-spawned PTYs use a
 * `wake-<agent>-<uuid>` terminal id and don't carry the heartbeat name on
 * the wire today, so we fall back to "any PTY for this agent in this
 * project" as a liveness proxy. Will tighten when the daemon learns to
 * tag PTYs with their owning heartbeat row in P3.x.
 */
function deriveState(
  row: HeartbeatRow,
  agentName: string | null,
  projectPath: string,
  running: RunningAgentInfo[],
): { state: HeartbeatSessionState; liveTerminalId: string | null } {
  if (row.archivedAt) {
    return { state: 'archived', liveTerminalId: null }
  }
  // Liveness proxy: any wake-* PTY whose cwd matches the project root.
  // Tighten in P3.x once heartbeat→PTY linking is explicit.
  const live = agentName
    ? running.find(
        (r) =>
          r.terminalId.startsWith(`wake-${agentName}-`) &&
          r.cwd.startsWith(projectPath),
      )
    : null
  if (live) {
    return { state: 'live', liveTerminalId: live.terminalId }
  }
  if (row.lastSessionId && row.lastSessionId.length > 0) {
    return { state: 'resumable', liveTerminalId: null }
  }
  return { state: 'scheduled', liveTerminalId: null }
}

async function resolvePrimaryAgent(projectPath: string): Promise<string | null> {
  try {
    const list = await invoke<Array<{ name: string; isManager?: boolean; agentType?: string }>>(
      'k2so_agents_list',
      { projectPath },
    )
    if (list.length === 0) return null
    const manager = list.find((a) => a.isManager || a.agentType === 'manager' || a.agentType === 'coordinator')
    if (manager) return manager.name
    return list[0].name
  } catch {
    return null
  }
}

export const useHeartbeatSessionsStore = create<HeartbeatSessionsState>((set, get) => ({
  active: [],
  archived: [],
  loadedFor: null,
  loading: false,
  lastError: null,

  refresh: async (projectPath: string): Promise<void> => {
    if (get().loading) return
    set({ loading: true, lastError: null })

    try {
      const [activeRows, archivedRows, running, agentName] = await Promise.all([
        invoke<HeartbeatRow[]>('k2so_heartbeat_list', { projectPath }),
        invoke<HeartbeatRow[]>('k2so_heartbeat_list_archived', { projectPath }),
        invoke<RunningAgentInfo[]>('terminal_list_running_agents').catch(
          (): RunningAgentInfo[] => [],
        ),
        resolvePrimaryAgent(projectPath),
      ])

      const active: HeartbeatEntry[] = activeRows.map((row) => ({
        row,
        ...deriveState(row, agentName, projectPath, running),
      }))
      const archived: HeartbeatEntry[] = archivedRows.map((row) => ({
        row,
        state: 'archived' as const,
        liveTerminalId: null,
      }))

      set({ active, archived, loadedFor: projectPath, loading: false })
    } catch (err) {
      const msg = String(err)
      console.error('[heartbeat-sessions] refresh failed:', msg)
      set({ loading: false, lastError: msg })
    }
  },

  clear: () => {
    set({ active: [], archived: [], loadedFor: null, lastError: null })
  },
}))
