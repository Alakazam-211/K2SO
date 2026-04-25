import { useActiveAgentsStore } from '@/stores/active-agents'
import { TerminalPane } from '@/terminal-v2/TerminalPane'

/**
 * Renders background agent terminals in a hidden off-screen container.
 * Each terminal mounts just long enough to spawn its daemon-side PTY,
 * then auto-removes after 2s. The PTY continues running on the daemon.
 * When the user navigates to the workspace, the saved layout connects
 * to the already-running session via matching terminal ID.
 *
 * Hardcoded to v2 (`<TerminalPane>`) regardless of the user's
 * Settings → Terminal → Renderer choice. Background spawns are
 * heartbeat-wake driven and MUST be daemon-owned: the legacy
 * in-process `<AlacrittyTerminalView>` would die when Tauri quits,
 * defeating the whole "agent runs while you're not looking"
 * purpose of heartbeat wake. v2's daemon-hosted PTY survives.
 * See `.claude/plans/happy-hatching-locket.md` (A8).
 */
export function BackgroundTerminalSpawner(): React.JSX.Element | null {
  const spawns = useActiveAgentsStore((s) => s.backgroundSpawns)

  if (spawns.length === 0) return null

  return (
    <div
      style={{
        position: 'fixed',
        width: 200,
        height: 100,
        overflow: 'hidden',
        opacity: 0,
        pointerEvents: 'none',
        // Position off-screen so it's invisible but has real dimensions
        // for the terminal to initialize with a valid grid size
        top: -9999,
        left: -9999,
      }}
      aria-hidden
    >
      {spawns.map((spawn) => (
        <TerminalPane
          key={spawn.id}
          terminalId={spawn.terminalId}
          cwd={spawn.cwd}
          command={spawn.command}
          args={spawn.args}
        />
      ))}
    </div>
  )
}
