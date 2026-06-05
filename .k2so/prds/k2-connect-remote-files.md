---
title: "K2 Connect — Remote files: open remote workspaces + drag-drop upload"
status: draft
owner: Rosson
created: 2026-06-04
supersedes_part_of: "#623 (K2 Connect remote-completeness gaps, PRD §7b)"
source: 3 read-only Explore maps (open-workspace/fs-browse, drag-drop pipeline, daemon file-write)
---

# K2 Connect — Remote Files

Make "connected to a remote" feel like the files genuinely live there:
you can **open a workspace that exists on the host**, and **drag files
from your local machine into K2 and have them land on the host**.

## Motivation

Today, when `activeHost !== 'local'`:
- "New Workspace" opens the **local** OS folder dialog → returns a local
  path that is meaningless on the remote daemon. You can't open a folder
  that lives on the host.
- Dragging a file into the window does nothing useful: Tauri's drag-drop
  hands the renderer a **local path with no bytes**, and the terminal
  injects that *local* path into the PTY — which the remote shell/agent
  can't resolve. Nothing is uploaded.

## The three features

1. **Open remote workspaces** — an in-app folder browser over the remote
   daemon's filesystem; pick a directory on the host, open it as a
   workspace. (Core of #623.)
2. **Drag-drop → upload to the host** — dropping a file transfers the
   bytes to the remote, with the destination decided by *where* you drop.
3. **Terminal drop → `.k2so/downloads/` + path injection** (the "bonus")
   — dropping onto a terminal uploads to the workspace's
   `.k2so/downloads/<name>` then injects *that remote path* into the PTY,
   so the agent/CLI can reference a file that really exists on the host.

## Target-driven destination model (decided)

When connected to a remote, the drop destination is chosen by the drop
target (hit-test at drop position):

| Drop target | Behavior |
|---|---|
| A **folder** in the file-tree | Upload the file **into that folder** (user-chosen path). Local equivalent: a plain copy into that folder. |
| A **terminal** pane | Upload to `<workspace>/.k2so/downloads/<name>`, then inject the returned remote path into the PTY (reusing the existing shell-quote + bracketed-paste-for-images payload builder). |
| **Neither** (window chrome) | Open the **"Save to…"** prompt = the same `RemoteFolderPicker` from Feature 1, to choose a destination dir on the host. |

Note: external-file-drop onto a file-tree folder is **not handled today
even locally** — this adds it for both local (copy) and remote (upload),
with transport the only difference.

## Architecture findings (from the maps)

### What already exists (reuse)
- **FileTree** (`src/renderer/components/FileTree/FileTree.tsx`) already
  browses via host-aware `daemonCliGet('fs/read-dir')` — it *already*
  shows the remote fs when connected. Wrap it for the picker.
- **Terminal path injection** is solved: `src/renderer/lib/file-drag.ts`
  `buildDropPayload()` does shell-quoting (`shellEscape`), image handling
  (`quotePathForImageDrop`), and bracketed-paste wrapping so Claude Code's
  `[Image #N]` detection fires. v1 terminals inject via
  `daemonCliPost('terminal/lifecycle-write', {id, data})`; v2 via the WS
  `{action:'input', text}`. We reuse this verbatim — only the path source
  changes (remote instead of local).
- **Binary transport, one direction**: `/cli/fs/read-binary` returns
  `{base64}`. We mirror this pattern for upload. No POST body-size cap in
  the daemon HTTP server.
- **Workspace path**: a project's dir is `project.path`; `.k2so/` lives at
  `<project.path>/.k2so/`. `fs::create_dir_all` is the established pattern
  for creating `.k2so/` subdirs.

### The non-obvious constraint
Tauri's `tauri://drag-drop` event gives the renderer **local file paths,
not bytes** (it intercepts Finder drops before the DOM sees File objects).
So upload needs an extra hop: a Tauri command reads the dropped local file
by path → base64 → POST to the daemon.

### Current open-workspace flow (the gap)
`ProjectsSection.tsx` "New Workspace" → `invoke('projects_pick_folder')`
(native dialog, src-tauri/src/commands/projects.rs) → `addProject(path)`
→ `daemonCliPost('projects/add-from-path', {path})`. The native dialog
always runs on the **local** machine. Fix: branch on `activeHost`.

## New surfaces to build

### Daemon
- **`GET /cli/fs/info`** → `{ home, separator, os }` (~10 lines:
  `dirs::home_dir()`, `std::path::MAIN_SEPARATOR`, OS string). Seeds the
  remote browser + replaces FileTree's hardcoded `/`.
- **`POST /cli/fs/upload-binary`** → `{ dir, filename, base64 }` (or
  `{ path, base64 }`): decode base64, validate `dir` exists + is a
  directory + is within an allowed root, create it if it's the
  `.k2so/downloads/` convenience path, write the file with a
  collision-avoiding suffix (`name (1).ext`), return the final remote
  path. **Size cap** (e.g. 100 MB) → clear error over the cap. Mirrors the
  `read-binary` base64 pattern. Gate: `token_ok` (see Security).
  - Helper in k2so-core: `ensure_downloads_dir(workspace_path) ->
    <ws>/.k2so/downloads/` via `create_dir_all`.

### Tauri (thin client)
- **`read_local_file(path) -> { base64 }`** (or reuse the tauri fs plugin
  with a drag-drop scope): reads the dropped *local* file's bytes so the
  renderer can upload them. This is the bytes-hop the constraint forces.

### Renderer
- **`RemoteFolderPicker`** modal = `FileTree` in a dialog, seeded at
  `/cli/fs/info`'s `home`, "Select this folder" → returns `{path}`. Used
  by both the open-workspace branch AND the "Save to…" miss case.
- **`ProjectsSection` branch**: `activeHost === 'local'` → native dialog;
  remote → `RemoteFolderPicker`. Chosen path flows through the unchanged
  `projects/add-from-path`.
- **Drop handlers** (window + v1/v2 terminals): add `activeHost` awareness.
  On remote: hit-test target → upload to folder / downloads+inject / save-to
  prompt. A small client helper `uploadToRemote(localPath, destDir)`:
  `read_local_file` → `fs/upload-binary` → returns remote path.

## Security

- The existing `fs/write-file`/`move`/`copy`/`create` routes are gated by
  `token_ok` only (any authed connect-user — Member included) and already
  accept arbitrary paths. So an upload route gated the same way is **not
  new exposure** vs. what's already callable.
- New risk a Member gains: **disk-fill / planting files**. Mitigate with a
  **per-file size cap** and (future) a per-session volume guard.
- **Decision (this PRD): gate `token_ok` (any authed user) + size cap**,
  with the gate isolated in one place so flipping to **Owner+Admin**
  (`require_manage`) or **Owner-only** (`require_owner`) is a one-line
  change. Future: an owner setting "allow members to upload files".
- **Path validation**: reject traversal; canonicalize; for the terminal
  convenience path, force the destination to be exactly
  `<workspace>/.k2so/downloads/` (never an arbitrary path from the PTY UX).
- Local-drop behavior is **unchanged** (no upload; today's local copy /
  local-path injection).

## Phasing

- **Phase 1 — Remote folder picker** (independent; closes #623's core):
  `GET /cli/fs/info` + `RemoteFolderPicker` modal + `ProjectsSection`
  branch. Also the reusable "Save to…" prompt for Phase 3.
- **Phase 2 — Upload substrate**: `read_local_file` Tauri cmd +
  `POST /cli/fs/upload-binary` (validate, size cap, collision suffix,
  `ensure_downloads_dir`) + `uploadToRemote` client helper. The plumbing.
- **Phase 3 — Wire drop targets** (depends on 1+2):
  - 3a: file-tree folder drop → upload into that folder (remote) / copy
    (local).
  - 3b: terminal drop → upload to `.k2so/downloads/` → inject remote path
    via `buildDropPayload`.
  - 3c: miss → "Save to…" (reuses Phase-1 `RemoteFolderPicker`).
- **Phase 4 — Polish**: upload progress toast for large files; reveal /
  "open downloads" affordance; overwrite-vs-suffix prompt.

Phase 1 is shippable on its own. 2+3 are coupled.

## Open questions / future
- Path-separator handling for a **Windows** remote host (FileTree
  hardcodes `/`; `/cli/fs/info.separator` unblocks this but the tree code
  needs to honor it). MVP can assume unix hosts and note the gap.
- Overwrite semantics on collision (default: non-destructive suffix).
- Volume/rate guard for Member uploads (deferred).
- Tests: daemon upload route (size cap, traversal reject, downloads-dir
  creation, collision suffix); renderer hit-test routing
  (folder/terminal/miss → correct destination); host-aware branch in
  ProjectsSection.

## Effort (rough)
- Phase 1: small (one ~10-line route + one modal + a branch).
- Phase 2: small-medium (one Tauri cmd + one daemon route + helper).
- Phase 3: medium (drop hit-test routing across window + v1/v2 terminals,
  reusing existing payload builder + picker).
