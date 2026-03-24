/**
 * Global in-window file drag state.
 *
 * The native Tauri `startDrag` plugin hands control to the OS, which means
 * `tauri://drag-drop` events don't fire for drops back into the same window.
 * This module provides a simple global state so FileTree can initiate a drag
 * and TerminalView (or any other target) can detect the drop via mouseup.
 *
 * Flow:
 *   1. FileTree mousedown → mousemove 5px → `beginDrag(paths)`
 *   2. Global mousemove tracks position, shows ghost cursor
 *   3. mouseup → `endDrag()` checks what's under the cursor
 *   4. If mouse leaves the window → `startDrag` is called for Finder drops
 */

import { invoke } from '@tauri-apps/api/core'
import { startDrag } from '@crabnebula/tauri-plugin-drag'

// ── State ────────────────────────────────────────────────────────────

let dragPaths: string[] = []
let active = false
let ghost: HTMLDivElement | null = null

// Track whether a tauri://drag-drop was already handled (e.g. by the terminal)
// so that FileTree doesn't also process it as a file move.
let dropConsumed = false

export function markDropConsumed(): void {
  dropConsumed = true
  // Reset after a tick so the FileTree listener (which fires in the same event loop) sees it
  setTimeout(() => { dropConsumed = false }, 0)
}

export function wasDropConsumed(): boolean {
  return dropConsumed
}

// ── Shell-escape helper ──────────────────────────────────────────────

function shellEscape(p: string): string {
  if (/[^a-zA-Z0-9_\-./]/.test(p)) {
    return "'" + p.replace(/'/g, "'\\''") + "'"
  }
  return p
}

// ── Ghost element ────────────────────────────────────────────────────

function createGhost(paths: string[]): HTMLDivElement {
  const el = document.createElement('div')
  el.style.cssText = `
    position: fixed;
    pointer-events: none;
    z-index: 999999;
    padding: 4px 10px;
    font-size: 11px;
    font-family: 'MesloLGM Nerd Font', Menlo, monospace;
    background: var(--color-bg-surface);
    border: 1px solid var(--color-accent);
    color: var(--color-text-primary);
    border-radius: 4px;
    opacity: 0.9;
    white-space: nowrap;
    max-width: 280px;
    overflow: hidden;
    text-overflow: ellipsis;
  `
  const name = paths[0].split('/').pop() || paths[0]
  el.textContent = paths.length === 1 ? name : `${name} +${paths.length - 1}`
  document.body.appendChild(el)
  return el
}

function moveGhost(x: number, y: number): void {
  if (ghost) {
    ghost.style.left = `${x + 12}px`
    ghost.style.top = `${y + 12}px`
  }
}

function removeGhost(): void {
  if (ghost) {
    ghost.remove()
    ghost = null
  }
}

// ── Public API ───────────────────────────────────────────────────────

export function isFileDragActive(): boolean {
  return active
}

export function getFileDragPaths(): string[] {
  return dragPaths
}

export interface FileDragCallbacks {
  /** Called during mousemove with the directory path under the cursor (or null). */
  onDragOver?: (dirPath: string | null) => void
  /** Called on mouseup if the drop lands on a directory. Return true to consume the drop. */
  onDrop?: (dirPath: string) => boolean
  /** Called when the drag ends (cleanup). */
  onDragEnd?: () => void
}

/**
 * Start tracking a file drag from the FileTree.
 * Call this from FileTree's mousedown handler after the 5px threshold.
 */
export function beginFileDrag(paths: string[], startX: number, startY: number, callbacks?: FileDragCallbacks): void {
  dragPaths = paths
  active = true
  ghost = createGhost(paths)
  moveGhost(startX, startY)

  /** Find the nearest directory element under the cursor within the file tree. */
  function findDirUnderCursor(x: number, y: number): string | null {
    const el = document.elementFromPoint(x, y)
    if (!el) return null
    // Walk up to find a [data-path] element
    const btn = (el as HTMLElement).closest('[data-path]') as HTMLElement | null
    if (!btn) return null
    // If it's a directory, use its path; if it's a file, use its parent directory
    if (btn.dataset.isDirectory === 'true') return btn.dataset.path || null
    // For files, find the parent directory from the path
    const path = btn.dataset.path
    if (path) {
      const idx = path.lastIndexOf('/')
      return idx > 0 ? path.slice(0, idx) : null
    }
    return null
  }

  const handleMouseMove = (ev: MouseEvent): void => {
    moveGhost(ev.clientX, ev.clientY)
    // Notify file tree about the directory under the cursor for highlighting
    if (callbacks?.onDragOver) {
      const dirPath = findDirUnderCursor(ev.clientX, ev.clientY)
      callbacks.onDragOver(dirPath)
    }
  }

  const handleMouseUp = (ev: MouseEvent): void => {
    cleanup()

    if (!active) return
    active = false

    const el = document.elementFromPoint(ev.clientX, ev.clientY)

    // Hit-test: is the drop over a terminal container?
    if (el) {
      const termContainer = (el as HTMLElement).closest('[data-terminal-id]') as HTMLElement | null
      if (termContainer && termContainer.dataset.terminalId) {
        // Paste shell-escaped paths into the terminal
        const escaped = dragPaths.map(shellEscape).join(' ')
        invoke('terminal_write', {
          id: termContainer.dataset.terminalId,
          data: escaped
        }).catch(() => {})
        dragPaths = []
        return
      }
    }

    // Hit-test: is the drop over a directory in the file tree?
    if (callbacks?.onDrop) {
      const dirPath = findDirUnderCursor(ev.clientX, ev.clientY)
      if (dirPath) {
        const consumed = callbacks.onDrop(dirPath)
        if (consumed) {
          dragPaths = []
          return
        }
      }
    }

    // Drop wasn't on a terminal or directory — cancel
    dragPaths = []
  }

  const handleMouseLeave = (): void => {
    // Mouse left the window — hand off to OS native drag for Finder
    cleanup()
    active = false

    if (dragPaths.length > 0) {
      startDrag({ item: dragPaths, icon: 'png' }).catch((err) => {
        console.error('[file-drag] Native drag failed:', err)
      })
    }
    dragPaths = []
  }

  function cleanup(): void {
    removeGhost()
    callbacks?.onDragEnd?.()
    document.removeEventListener('mousemove', handleMouseMove)
    document.removeEventListener('mouseup', handleMouseUp)
    document.documentElement.removeEventListener('mouseleave', handleMouseLeave)
  }

  document.addEventListener('mousemove', handleMouseMove)
  document.addEventListener('mouseup', handleMouseUp)
  document.documentElement.addEventListener('mouseleave', handleMouseLeave)
}
