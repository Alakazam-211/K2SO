# 0.37.11 — Active-viewer protocol, focus-window simplification, voice + selection coexist

> **Focus Windows are alpha in 0.37.11.** The architecture landed (New-Window pattern + active-viewer protocol — see below) and they work for the common case, but cross-window workspace-switch propagation and a few smaller edges aren't fully polished yet. Expect to find rough spots; report what you hit.

## TL;DR

- **Focus Window** is now the New Window pattern with a workspace hint. Removed the overlay, custom focus-mode tab logic, and the "this workspace is open elsewhere" placeholder. Both windows mirror the same workspace through the existing cross-window sync; the v2 WS protocol already supports multiple subscribers per session. **(alpha)**
- **Active-viewer claim protocol**: daemon now tracks `active_subscriber` per v2 session. The renderer sends `{action:"set_active", active:true}` on window focus and `false` on blur; the daemon ignores `Resize` frames from non-active subscribers. Eliminates the "two windows fighting over the TUI grid size" problem and generalizes to any future subscriber (mobile companion can claim too).
- **Text selection + voice dictation coexist**: the shadow `<textarea>` no longer steals focus during in-flight selection drags. Drag-to-highlight works again; Fn-Fn dictation still engages on the underlying textarea after the selection completes.
- **Terminal ID copy** copies the raw v2 session UUID (works with `k2so terminal read <id>` directly).
- Quieter fixes: FileTree no longer calls `setState` during render, `tabsStore.tabs.length` crash on fresh focus-window mount, A9 PRD added describing the daemon-headless v2 migration as complete.

## The active-viewer protocol

K2SO's vision: the daemon is the source of truth; Tauri is one of N possible viewers; mobile companion is another viewer; future surfaces are more viewers. Today the v2 WS protocol already supports multi-subscribe — every connected client gets the same `Snapshot` and `Delta` stream from the daemon — but resize was racy. Two desktop viewers' `ResizeObserver`s would fight, and the PTY grid oscillated on every focus switch.

0.37.11 adds a `SetActive` frame to the WS protocol:

```jsonc
// Outbound, from renderer to daemon
{ "action": "set_active", "active": true }   // I'm now the active viewer
{ "action": "set_active", "active": false }  // I'm releasing the claim
```

On the daemon side, each WS connection gets a unique `subscriber_id` at accept time, and the session has an `active_subscriber: AtomicU64` (0 = no claim). When a subscriber sends `set_active:true`, it stamps its id. When a `Resize` frame arrives, the daemon checks:

- `active_subscriber == 0` → accept (preserves single-viewer behavior; first resize wins)
- `active_subscriber == sender_id` → accept (active viewer's resize)
- otherwise → drop the frame with a `resize_ignored` log line

CAS on release so a viewer that took over isn't clobbered by a stale release from someone who lost the claim already. On WS disconnect, the same CAS clears the claim if we still hold it.

The renderer sends the claim on window focus and the release on blur (`useWindowFocusStore` tracks this via `tauri://focus` / `tauri://blur` events). The first emit at mount uses the current focus state so a freshly-mounted pane in a non-focused window stays passive until the user interacts.

Mobile companion plugs in trivially: same WS, same frame, same semantics.

## Focus Window simplification

Pre-0.37.11 the focus window had its own everything:

- Custom URL routing (`#focus=<id>` hash + window label fallback)
- A `useFocusWindowsStore` tracking which projects were "owned" by a focus window
- A `FocusWindowOwnedOverlay` that blurred the main window's content area when a workspace was open in a focus window, with a "Surface" button
- Cross-window tab sync opt-out for focus windows
- Daemon-adoption-only behavior in `loadLayoutForWorkspace`
- A 500ms `tokio::sleep` band-aid in `projects_open_focus_window` for a WebKit connection-pool race that turned out to be downstream of the real bug
- A custom emit-on-build → emit-on-destroy lifecycle for "focus-window:opened" / "focus-window:closed" events

All of that was wrestling with a problem that didn't need to exist. **New Window** does exactly the same thing — opens a Tauri window, runs the React app, lets `useWindowSync` keep state aligned. Both windows attach to the same daemon sessions through the v2 WS multi-subscribe.

In 0.37.11, `projects_open_focus_window` is reduced to the same shape:

```rust
// commands/projects.rs
let _window = WebviewWindowBuilder::new(&app, &label, webview_url)
    .title(&project_name)
    .inner_size(1200.0, 800.0)
    .min_inner_size(600.0, 400.0)
    .hidden_title(true)
    .title_bar_style(tauri::TitleBarStyle::Overlay)
    .build()
    .map_err(|e| e.to_string())?;
```

The `focus-<projectId>` window label is the one piece of state that survives — it tells the renderer which workspace to display when the URL fragment gets stripped in production builds. Everything else is shared store + cross-window sync.

## Text selection + voice coexist

0.37.9 introduced a shadow `<textarea>` per pane to host AppKit's dictation engagement (Fn-Fn). Side effect: clicking + dragging to highlight text inside a terminal pane stopped working — the textarea kept stealing focus on every mouse-down, collapsing the selection.

The fix tracks the mouse-down state on the pane container and gates the focus-takeover logic on `window.getSelection().toString().length > 0`. If a selection is in flight, the textarea stays out of the way. When the user finishes the drag, the textarea regains focus (so dictation works on the next Fn-Fn) without disturbing the selected text.

Drag-to-copy works again. Fn-Fn dictation still engages. They coexist now.

## Terminal ID copy

The right-click "Copy Terminal ID" item used to copy a `<workspace>:<agent>` qualified form (e.g. `K2SO:99ce895b-...`). That form wasn't accepted by `k2so terminal read <id>` — the daemon route expects the raw v2 session UUID.

0.37.11 copies `tabTerminalId` directly. Paste it into `k2so terminal read` or any other CLI route that takes a session UUID and it works as expected.

## Smaller fixes

- **FileTree `setState` during render**: `clearSelection` cross-store call wrapped in `queueMicrotask` to defer to after the current commit phase. Eliminates "Cannot update a component while rendering a different component" warnings, plus the file watcher now guards on `rootPath.startsWith('/')` so the `~` literal doesn't try to register a watcher.
- **`tabsStore.tabs.length` crash**: at `projects.ts:311,352`, `tabsStore.getState().tabs.length` was being called on the already-resolved state object. Fixed to `tabsStore.tabs.length` — was crashing fresh focus-window mounts with `tabsStore.getState is not a function`.
- **A9 PRD**: added `.k2so/prds/a9-daemon-headless-session-unification.md` describing the three-phase daemon-headless migration as complete. Phases 1-3 shipped pre-0.37.11; the new PRD prevents future devs (and agents) from re-investigating already-done work.

## New CLI verbs

- **`k2so sessions live <workspace-path>`** — lists live daemon-held v2 sessions whose `cwd` is the workspace or a child. Distinct from `k2so sessions list` (which inspects on-disk archive segments). Supports `--count` (just the integer) and `--json` (raw response). Falls back to `/cli/agents/running` automatically when the running daemon predates the new `/cli/sessions/list-for-workspace` route.

  ```bash
  $ k2so sessions live "/Users/z3thon/DevProjects/Alakazam Labs/K2SO"
  1 live session(s):
    TERMINAL_ID                             AGENT                           CMD             CWD
    ab661773-a344-4b62-872d-9dbdc12570ea    99ce895b-fac7-4609-8442-b276c4  claude          /Users/z3thon/DevProjects/Alakazam Labs/K2SO
  ```

- **`cli/test-focus-handoff.sh <workspace-path>`** — automated handoff test. Writes a sentinel string to a session, asks the operator to open a focus window, then asserts session count is unchanged and the sentinel is still in the grid. Exit codes: 0 PASS, 1 count changed, 2 sentinel lost, 3 no live sessions.

## Files touched

| Layer | File | Direction |
|---|---|---|
| daemon-core | `crates/k2so-core/src/terminal/daemon_pty.rs` | + `active_subscriber: AtomicU64` field |
| daemon | `crates/k2so-daemon/src/sessions_grid_ws.rs` | + `SetActive` Inbound variant, subscriber-id counter, resize gating, claim release on disconnect |
| daemon | `crates/k2so-daemon/src/cli.rs` | + `/cli/sessions/list-for-workspace` route |
| Tauri | `src-tauri/src/commands/projects.rs` | simplified `projects_open_focus_window` to New-Window shape |
| Tauri | `src-tauri/src/agent_hooks.rs` | + `k2so_sessions_list_for_workspace` command |
| Tauri | `src-tauri/src/lib.rs` | + new command registration |
| renderer | `src/renderer/stores/window-focus.ts` | new store: `isFocused` for active-viewer gating |
| renderer | `src/renderer/App.tsx` | focus/blur listeners, restored URL-hash + window-label dual resolution |
| renderer | `src/renderer/terminal-v2/TerminalPane.tsx` | `set_active` emit on focus changes, resize gated on `isFocused`, mouseDownInPaneRef + selection guard |
| renderer | `src/renderer/stores/tabs.ts` | adoption check in `launchDefaultAgent` |
| renderer | `src/renderer/components/TabBar/TabBar.tsx` | raw `tabTerminalId` copy |
| renderer | `src/renderer/components/FileTree/FileTree.tsx` | `queueMicrotask` for cross-store clearSelection; watcher guard |
| renderer | `src/renderer/stores/projects.ts` | `tabsStore.tabs.length` (not `getState().tabs.length`) |
| CLI | `cli/k2so` | + `cmd_sessions_live` |
| CLI | `cli/test-focus-handoff.sh` | new |
| docs | `.k2so/prds/a9-daemon-headless-session-unification.md` | new |

## Known limitations (alpha)

Focus windows aren't quite "perfect" yet — switching workspaces in main doesn't propagate to the focus window in real time (each window's workspace is determined by its own `focusProjectId` resolution at mount). The active-viewer protocol resolves the size-fight cleanly, but full live workspace-switch sync between windows is a 0.38.x follow-up.

What does work well: open a focus window, both windows see the same daemon sessions, switching OS focus between them hands the resize authority cleanly without grid oscillation, and voice + selection don't fight each other.
