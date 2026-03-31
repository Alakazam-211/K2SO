import { useActiveAgentsStore } from '@/stores/active-agents'
import { AlacrittyTerminalView } from './Terminal/AlacrittyTerminalView'

/**
 * Renders background agent terminals in a hidden off-screen container.
 * Each terminal mounts just long enough to spawn its PTY (~500ms),
 * then auto-removes after 2s. The PTY continues running in the backend.
 * When the user navigates to the workspace, the saved layout connects
 * to the already-running PTY via matching terminal ID.
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
        <AlacrittyTerminalView
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
