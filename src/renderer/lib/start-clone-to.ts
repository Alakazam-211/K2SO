// Glue between the workspace context-menu "Clone to ▸ <host>" action and
// the orchestration (`cloneWorkspaceTo`) + the progress modal store.
//
// The context-menu handlers (IconRail / Sidebar) call `startCloneTo` with
// the source workspace + the chosen destination host. This opens the modal
// at its pre-flight OPTIONS phase (the "Include secrets" toggle). Only when
// the user clicks Clone does the store's `confirm()` invoke our `onConfirm`
// runner — receiving the chosen `carrySecrets` value — which then drives the
// orchestration, piping its hooks into the modal store. Errors are swallowed
// here (the modal already surfaces them via setError) so the menu handler
// doesn't need its own try/catch.

import {
  cloneWorkspaceTo,
  defaultCloneDeps,
  type CloneDeps,
} from './clone-to'
import { useCloneToDialogStore } from '@/stores/clone-to-dialog'
import type { ConnectHost } from '@/stores/connect-host'

/**
 * Open the "Clone to" modal at its pre-flight options phase. The actual run
 * (`cloneWorkspaceTo`) is deferred until the user clicks Clone, at which
 * point the store invokes `onConfirm(carrySecrets)` with the chosen toggle
 * value — wiring its hooks into the modal store. `deps` is injectable for
 * tests; production passes the default bag.
 */
export function startCloneTo(
  projectPath: string,
  projectName: string,
  host: ConnectHost,
  deps: CloneDeps = defaultCloneDeps(),
): void {
  const store = useCloneToDialogStore.getState()
  store.start({
    projectPath,
    projectName,
    host,
    onConfirm: (carrySecrets) => {
      void runClone(projectPath, host, deps, carrySecrets)
    },
  })
}

/** Drive the orchestration once the user has confirmed the options panel. */
async function runClone(
  projectPath: string,
  host: ConnectHost,
  deps: CloneDeps,
  carrySecrets: boolean,
): Promise<void> {
  try {
    await cloneWorkspaceTo(
      projectPath,
      host,
      deps,
      {
        onStage: (stage) => useCloneToDialogStore.getState().setStage(stage),
        onBundled: (summary) => useCloneToDialogStore.getState().setSummary(summary),
        onDone: (result) => useCloneToDialogStore.getState().setDone(result),
        onError: (message) => useCloneToDialogStore.getState().setError(message),
      },
      carrySecrets,
    )
  } catch {
    // The modal already reflects the failure via onError → setError; the
    // user closes it manually. Nothing more to do here.
  }
}
