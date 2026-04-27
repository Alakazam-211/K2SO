# K2SO 0.36.1 — Drag-and-drop + clipboard fixes for Alacritty (v2)

Quick-fix release closing three "it just works in v1, doesn't in
v2" gaps that surfaced after 0.36.0 shipped.

## What's fixed

### Drag from Finder onto a v2 terminal pane

Tauri's webview intercepts external drags at the window level
(emitting `tauri://drag-drop`) when `dragDropEnabled` is on. The
v2 TerminalPane was relying on React's standard `onDrop`, which
never fires for drags that originate outside the window — so
dropping a file from Finder did nothing.

**Fix:** v2 now subscribes to `tauri://drag-drop` exactly like
the legacy AlacrittyTerminalView did, hit-tests the drop position
against its container so split-pane layouts route correctly, and
sends the formatted path payload through its existing WS
sendInput. Multi-file drops are space-joined; image paths trigger
bracketed-paste wrapping so Claude Code's `[Image #N]` detector
fires.

### Drag from K2SO's file tree onto a v2 terminal pane

The internal `lib/file-drag.ts` helper tracks file-tree drags
manually (because `startDrag` hands control to the OS, so
re-entering the same window doesn't fire `tauri://drag-drop`).
On mouseup it hit-tests for `[data-terminal-id]` and called
`invoke('terminal_write', { id, data })` — but that Tauri command
only knows about the legacy in-process `terminal_manager`. v2
sessions live in the daemon's session map and write through their
own WS, so the call would silently fail.

**Fix:** the v2 TerminalPane container now exposes
`data-terminal-id={session_id}` + `data-terminal-kind="v2"`, and
`file-drag.ts` dispatches a `k2so:terminal-write` CustomEvent for
v2 panes instead of the legacy IPC call. v2 listens for the event
and routes the payload through its existing `sendInput`.

### Right-click Copy / Cut in the file tree → Cmd+V in the terminal

`useFileClipboardStore.copy()` and `cut()` only wrote to the
in-app Zustand store — never the OS clipboard. The terminal's
paste handler reads `e.clipboardData.getData('text')` (and falls
back to `clipboard_read_file_paths` for `NSFilenamesPboardType`),
so an in-app Copy was invisible to it.

**Fix:** `copy()` and `cut()` now also call
`navigator.clipboard.writeText(...)` with the shell-escaped paths
so Cmd+V in any terminal pane (or any other native app) picks
them up. The in-app Paste menu still works the same way via the
Zustand state.

## Forward-compat notes

The lessons learned during this fix are captured in
`.k2so/prds/kessel-t1.md` under **Creature-comfort parity
requirements** so the future Kessel-T1 renderer doesn't trip
the same wires:

- External vs internal drags use different code paths
  (`tauri://drag-drop` window event vs `lib/file-drag.ts`
  mouseup-tracking)
- Pane containers must expose both `data-terminal-id` and
  `data-terminal-kind="<harness>"` so file-drag.ts can route
- Don't add fallthroughs to `terminal_write` to "find the
  right map" — use the per-pane CustomEvent pattern so each
  renderer's write path stays scoped to its own component
