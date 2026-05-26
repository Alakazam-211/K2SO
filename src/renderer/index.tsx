import './globals.css'
import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { ConnectionGate } from './components/ConnectionGate'
import { installExternalLinkHandler } from './lib/external-link-handler'

const root = document.getElementById('root')!

installExternalLinkHandler()

// ConnectionGate (0.39.2): polls daemon's /ping until reachable,
// then mounts App. Closes the auto-update race where React mounted
// against a half-restarting daemon and presented as a blank window.
// Reusable for K2 Connect's remote-daemon scenario too.
ReactDOM.createRoot(root).render(
  <ConnectionGate>
    <App />
  </ConnectionGate>
)
