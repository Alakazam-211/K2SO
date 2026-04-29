import './globals.css'
import React from 'react'
import ReactDOM from 'react-dom/client'
import App from './App'
import { installExternalLinkHandler } from './lib/external-link-handler'

const root = document.getElementById('root')!

installExternalLinkHandler()

ReactDOM.createRoot(root).render(
  <App />
)
