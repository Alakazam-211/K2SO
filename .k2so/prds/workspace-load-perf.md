# Workspace-load performance: non-terminal tabs

Code review notes from 0.35.6 turned up five bottlenecks that
make workspace switches slow when the user has many CodeMirror
or markdown file tabs open. None of these affect terminals
(daemon-hosted, lazy attach via WS) — only the file-viewer
path. This PRD plans the cleanup in phases that can each ship
independently, with perf instrumentation landing first so we
can measure before/after on every change.

## Why this exists

After A8 made every system terminal mount go through v2 +
A9 made the daemon the source of truth, terminal panes
became cheap on workspace switch — the WS reconnect
takes ~50ms and the existing PTY survives. But file tabs
(`FileViewerPane.tsx`) didn't get the same treatment.
Switching into a workspace with 15+ open `.ts` / `.md` /
`.rs` files mounts every CodeMirror instance, reads every
file from disk, and starts a 2-second polling interval per
tab — even tabs the user can't see. With 20 hidden file
tabs, the app continuously fires 10 `fs_read_file` Tauri
calls per second just to detect external changes.

The user-visible symptoms:
1. **Workspace switch hitch** when the new workspace has
   many file tabs (CodeMirror init is the largest cost,
   the synchronous file reads compound).
2. **Persistent CPU + disk activity** in the background even
   when the user isn't looking at any file tab.
3. **Markdown reparse on every keystroke** in edit mode,
   because there's no memoization between renders.

The audit's full file:line citations live below. None of
these is hard to fix in isolation; the trick is doing them
in the right order so we don't rewrite the same code twice.

## Goals

1. **Workspace switch into a 20-tab workspace lands the first
   visible file's content in under 250 ms** (current
   uninstrumented best-guess: ~1–2 s).
2. **Hidden file tabs cost zero ongoing CPU + disk** until
   the user navigates to them.
3. **Every file-tab interaction has a perf log breadcrumb**
   so future regressions are catchable from the dev console
   without having to bisect.

## Non-goals

- Replacing CodeMirror. The extension stack we use is
  load-bearing (vim mode, language grammars, git gutter,
  minimap). Different editor = bigger PRD.
- Changing the markdown renderer. `ReactMarkdown` +
  `remark-gfm` is fine; the cost is in re-rendering on
  every keystroke, which we'll fix via memoization.
- Touching terminal panes. They're already cheap.
- Changing the on-disk persistence shape. The DB layout for
  workspace sessions stays the same.

## Findings (read this before the phase plan)

Cited verbatim from the audit. File:line references are the
load-bearing ones to study before changing anything.

### Workspace switch path
`setActiveWorkspace` (`src/renderer/stores/projects.ts:336`):

1. Stashes current via `tabsStore.stashWorkspace()`
   (`projects.ts:346`).
2. React unmounts most of the active surface; terminal PTYs
   survive in the daemon.
3. Restores via `tabsStore.restoreWorkspace()`
   (`projects.ts:370`):
   - If tabs are live in `backgroundWorkspaces` (in-memory),
     swapped in eagerly (`tabs.ts:2273-2290`).
   - If not, `loadLayoutForWorkspace()` pulls from DB
     (`tabs.ts:2303`), then `restoreLayout()`
     (`tabs.ts:1772`) remounts every pane item, fresh.

### CodeMirror tab mount
`FileViewerPane.tsx` → `CodeEditor.tsx`. Mount sequence:

1. Eager file read: `loadFile()` →
   `invoke('fs_read_file', {path})` in the mount effect
   (`FileViewerPane.tsx:159-182`). Synchronous Tauri command,
   awaited on render path.
2. Eager editor init: `useEffect` at `CodeEditor.tsx:1187`
   constructs the full `EditorView` with every extension
   on mount.
3. Hidden tabs stay mounted: `PaneGroupView.tsx:161-163`
   uses `display:none`, not unmount. CodeMirror is mounted
   against a 0×0 parent and re-measures via
   `useIsTabVisible()` (`CodeEditor.tsx:1177`) when it
   becomes visible.
4. Polling on every mount: 2-second `FILE_POLL_INTERVAL`
   starts on mount, regardless of visibility
   (`FileViewerPane.tsx:237-249`).

### Markdown tab mount
No separate component. Markdown is `FileViewerPane` in view
mode (`FileViewerPane.tsx:37-44`, `:46-49`). Same eager
file read; `ReactMarkdown` re-parses on every content
change without memoization.

### Stash / restore behavior
- **Stashed**: tab objects + mosaic tree + active tab id +
  paths + scroll/cursor offsets (`tabs.ts:139-142`). NOT:
  file content, undo stack, editor instance state beyond
  scroll/cursor.
- **Hot restore** (in-memory `backgroundWorkspaces`):
  components survive, no re-mount. File content stale until
  the 2-second poll fires.
- **Cold restore** (from DB): `restoreLayout()`
  (`tabs.ts:1772`) remounts everything. N tabs = N
  independent `fs_read_file` invokes, each await sequential
  in the restore loop (no `Promise.all`).

### Existing instrumentation
- Terminals: `perfLog()` in `TerminalPane.tsx` covers spawn,
  creds, WS open, first paint.
- Kessel: `performance.mark/measure` for boot timeline.
- File viewer / CodeMirror / markdown: **none.** No
  `perfLog`, no `console.time`, no `performance.mark`.

## Phase plan

Each phase is independently shippable. Order matters: Phase 0
gates the others because we need numbers to know if a fix
helped.

### Phase 0 — Perf instrumentation (foundation)

Add `perfLog()` calls (matching the format in
`TerminalPane.tsx`) at the breadcrumbs we care about. Same
DEV-only gate so production isn't noisy.

**What to log**:

| Event | Where | Fields |
|---|---|---|
| `workspace_switch_start` | `setActiveWorkspace` entry | `from_project_id`, `to_project_id`, `tab_count`, `restore_kind=hot|cold` |
| `workspace_switch_first_paint` | once the first visible tab paints content | `elapsed_ms` since switch_start, `tab_kind=terminal|file|md` |
| `workspace_switch_complete` | when all tabs in the new workspace have rendered first paint | `elapsed_ms`, `tab_count` |
| `file_tab_mount` | `FileViewerPane` mount effect | `path`, `bytes`, `is_visible` |
| `file_read_start` / `file_read_end` | wraps the `invoke('fs_read_file')` call | `path`, `bytes`, `elapsed_ms` |
| `codemirror_init_start` / `_end` | wraps `EditorView` construction in `CodeEditor.tsx:1187` | `language`, `bytes`, `extension_count`, `elapsed_ms` |
| `file_tab_first_paint` | when content first lands in DOM | `elapsed_ms` since mount |
| `file_poll_tick` | every poll interval fire (sample 1 in 10) | `path`, `is_visible`, `bytes_read`, `changed` |
| `markdown_parse` | `ReactMarkdown` render | `bytes`, `elapsed_ms` (sample if hot) |

**Output shape**: same as TerminalPane —
`[file-perf] t=...ms stage=... key=val ...` so they're easy
to grep. Gate behind `import.meta.env.DEV`.

**Acceptance**: cold restore of a 20-tab workspace produces
a clean log of mount → file_read → codemirror_init →
first_paint per tab, plus the `workspace_switch_*` envelope
events. Numbers in hand for every subsequent phase.

### Phase 1 — Stop polling hidden tabs

The 2-second poll exists so users see external file changes
(e.g., another editor saved the file) without having to
manually refresh. There's zero value running it for tabs
the user can't see.

**Change**:
`FileViewerPane.tsx:237-249` — gate the polling interval
on `useIsTabVisible()`. Start it when `isVisible` becomes
true, clear when it becomes false. On the visibility flip,
fire one immediate read so the user doesn't see stale
content for up to 2s.

**Estimated impact**: ~9 `fs_read_file` calls/sec
eliminated for a workspace with 20 hidden file tabs. Disk
I/O drops to zero on the inactive surface.

**Risk**: minimal. The visibility hook is already wired
(used by CodeMirror's resize logic). Worst case: a user
who left a file tab open with auto-reload-on-disk-change
mental model has to click into the tab to see updates,
which is what every other editor on the planet does.

**Acceptance**: `file_poll_tick` log entries only fire for
the visible tab. Confirmed via the Phase 0 instrumentation.

### Phase 2 — Visibility-gated tab mounting

The largest single cost. Today every pane item in a pane
group renders on mount, then 19 of 20 are hidden via
`display:none`. The CodeMirror instances exist; the file
reads happened; the polling started.

**Change**:
`PaneGroupView.tsx:165-287` — render only the active pane
item per group, plus optionally one prefetched neighbor.
For tabs in a tab bar (mosaic group with multiple tabs),
mount only the active tab. Switching tabs becomes a true
mount/unmount, identical to terminals being WS-attached
on demand.

**Implications**:
- CodeMirror state (cursor, scroll, undo, vim register
  history, search history) is lost on tab switch unless we
  persist it. File-viewer state already has cursor/scroll
  in `FileViewerItemData` (`tabs.ts:139-142`) — extend it
  with undo-stack-equivalent (probably the document's
  `EditorState.toJSON()` or a coarser hash + selection
  ranges).
- Restore-on-mount needs the saved state injected into
  CodeMirror's initial config so the user sees their
  cursor where they left it.

**Risk**: medium. Need to verify CodeMirror state restore
preserves vim mode buffers, search regex history, etc.
The prior-state-on-remount problem is well-trodden; vim
plugins for CodeMirror typically expose a serializable
state object.

**Acceptance**: cold restore of a 20-tab workspace
produces 1× `codemirror_init` (the active tab), not 20×.
First-paint metric drops by an order of magnitude.

### Phase 3 — Markdown render memoization

Cheap fix once we have numbers from Phase 0.

**Change**:
`FileViewerPane.tsx` markdown render path — wrap
`ReactMarkdown` in a `useMemo` keyed on the source string
(or a hash if the content is large). Markdown re-parse
only happens on actual content change, not on parent
re-render.

**Acceptance**: `markdown_parse` log fires once per save
in edit mode, not once per keystroke.

### Phase 4 (optional) — Lazy CodeMirror extension loading

Only if Phase 2 doesn't get us under the 250 ms goal.

**Change**: load language grammars (`@codemirror/lang-rust`,
`@codemirror/lang-typescript`, etc.) lazily via dynamic
`import()`. The editor mounts with a "raw text" config
first; the language extension upgrades the view in a
follow-up `dispatch` once the grammar resolves.

**Risk**: highish. Vim mode + search + diagnostics all
hook into the language extension; ordering matters. Save
for after Phase 2 if needed.

## Files affected

| Phase | File | Touch |
|---|---|---|
| 0 | `src/renderer/components/AIFileEditor/AIFileEditor.tsx` | new perf hooks (uses CodeMirror) |
| 0 | `src/renderer/components/Editors/CodeEditor.tsx` | new perf hooks |
| 0 | `src/renderer/components/FileViewerPane.tsx` | new perf hooks (file read, mount, poll) |
| 0 | `src/renderer/stores/projects.ts` | `setActiveWorkspace` envelope events |
| 0 | `src/renderer/stores/tabs.ts` | `restoreLayout` / `stashWorkspace` envelope events |
| 1 | `src/renderer/components/FileViewerPane.tsx` | gate poll on `useIsTabVisible` |
| 2 | `src/renderer/components/PaneLayout/PaneGroupView.tsx` | render only active pane item |
| 2 | `src/renderer/stores/tabs.ts` | extend `FileViewerItemData` with editor-state blob |
| 2 | `src/renderer/components/Editors/CodeEditor.tsx` | hydrate from saved state on mount, save on unmount |
| 3 | `src/renderer/components/FileViewerPane.tsx` | `useMemo` around `ReactMarkdown` |
| 4 | `src/renderer/components/Editors/CodeEditor.tsx` | dynamic-import language extensions |

## Verification

Each phase ships with a hand-checked smoke test using the
Phase 0 instrumentation:

- **Switch into workspace with 20 file tabs.** Read
  `[file-perf]` log lines from DevTools. Note
  `workspace_switch_complete` elapsed_ms.
- **Leave the workspace open for 30 seconds.** Count
  `file_poll_tick` events per second. Phase 1 target: ≤ 1
  per 2 s (only the visible tab polls).
- **Edit a markdown file in edit mode.** Hold a key down
  for 1 second. Count `markdown_parse` events. Phase 3
  target: 1 per debounce window, not 1 per keystroke.

Pre-fix baseline numbers go in this PRD before Phase 1
starts so we can quote concrete improvements.

## Open questions

- **Where's CodeMirror used outside `CodeEditor.tsx`?** The
  audit cites `AIFileEditor.tsx` as another CodeMirror
  consumer. Verify whether it shares the same lifecycle
  helpers or has its own — Phase 0's instrumentation needs
  to cover both.
- **Phase 2's "prefetch neighbor" — worth it?** Mounting
  the active tab + the next tab over could eliminate a
  perceived hitch on tab-switch within a workspace.
  Probably not until we measure tab-switch latency post-
  Phase 2.
- **DB cold-restore vs in-memory hot-restore — same fix?**
  Phase 2 helps both, but the in-memory path already has
  the components mounted. Confirm the visibility-gating
  doesn't regress hot-restore by force-unmounting tabs the
  user just left.

## Sign-off

Land Phase 0 first, run baseline numbers, share results,
then decide whether 1-3 are enough or if we need 4. Each
phase is a separate PR.
