import './globals.css'
import React from 'react'
import ReactDOM from 'react-dom/client'
import { ConnectionGate } from './components/ConnectionGate'
import { installExternalLinkHandler } from './lib/external-link-handler'

// NOTE: do NOT statically import `./App` here. ConnectionGate uses
// `import('./App')` dynamically only AFTER the daemon is verified
// healthy. Statically importing it would defeat the gate: App's
// transitively-imported stores (projects, tabs, settings, focus-
// groups, timer, assistant, …) fire eager daemon fetches at
// module-init time, and those fetches would race the gate's daemon
// readiness check — the exact bug 0.39.2 left unsolved.

const root = document.getElementById('root')!

installExternalLinkHandler()

// ConnectionGate (0.39.3): polls daemon's /ping until reachable,
// THEN dynamically imports + mounts App. The dynamic import is the
// key — App.tsx (and all its transitive imports) stays out of the
// JS context until the daemon is confirmed healthy, so no store
// fires a fetch against a down daemon. Closes the black-screen
// race + reusable for K2 Connect's remote-daemon scenario.
ReactDOM.createRoot(root).render(
  <ConnectionGate />
)
