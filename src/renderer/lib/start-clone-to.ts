// Glue between the workspace context-menu "Clone to ▸ <host>" action and
// the orchestration (`cloneWorkspaceTo`) + the progress modal store.
//
// The context-menu handlers (IconRail / Sidebar) call `startCloneTo` with
// the source workspace + the chosen destination host. This opens the
// progress modal and runs the orchestration, piping its hooks into the
// modal store. Errors are swallowed here (the modal already surfaces them
// via setError) so the menu handler doesn't need its own try/catch.

import {
  cloneWorkspaceTo,
  defaultCloneDeps,
  type CloneDeps,
} from './clone-to'
import { useCloneToDialogStore } from '@/stores/clone-to-dialog'
import type { ConnectHost } from '@/stores/connect-host'

/**
 * Kick off a "Clone to" run: open the progress modal and drive
 * `cloneWorkspaceTo`, wiring its hooks into the modal store. `deps` is
 * injectable for tests; production passes the default bag.
 */
export async function startCloneTo(
  projectPath: string,
  projectName: string,
  host: ConnectHost,
  deps: CloneDeps = defaultCloneDeps(),
): Promise<void> {
  const store = useCloneToDialogStore.getState()
  store.start({ projectPath, projectName, host })
  try {
    await cloneWorkspaceTo(projectPath, host, deps, {
      onStage: (stage) => useCloneToDialogStore.getState().setStage(stage),
      onBundled: (summary) => useCloneToDialogStore.getState().setSummary(summary),
      onDone: (result) => useCloneToDialogStore.getState().setDone(result),
      onError: (message) => useCloneToDialogStore.getState().setError(message),
    })
  } catch {
    // The modal already reflects the failure via onError → setError; the
    // user closes it manually. Nothing more to do here.
  }
}
