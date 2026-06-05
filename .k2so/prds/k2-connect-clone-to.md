---
title: "K2 Connect — Clone to: one-click workspace migration to a remote server"
status: draft
owner: Rosson
created: 2026-06-05
source: GH #17 + reporter's full manual migration procedure (issue comment 2026-06-05)
depends_on: "remote-files Phase 1-3 (folder picker, fs/info, fs/upload-binary, uploadToRemote) — landed for 0.39.26"
related: ".k2so/prds/k2-connect-remote-files.md"
---

# K2 Connect — Clone To

Right-click a workspace → **"Clone to"** → pick a connected K2 Connect
server → the **entire agent** (workspace dir + durable memory + live chat
session) is reconstructed on the host, **resumable with history intact**.
One authenticated link replaces today's manual two-channel dance
(private-git for state + AirDrop for secrets).

## Why

Remote servers are only useful if you can *move onto them*. Today that's
manual and error-prone because an agent's state lives in **three places**
and a stale slug silently orphans the session. GH #17 + the reporter's
hand-won procedure give us the exact rule set; the scary 80% (reproducible
slug, wired resume, file transport) already shipped in remote-files Phase
1-3.

## Architecture: bundle → push → unpack

Clone-to is **"generate a scrubbed bundle → push it over the one
authenticated connection → unpack it remotely at recomputed paths."** The
**bundle engine** is the shared core; two consumers sit on top:

- **High bar (v1 — Clone to):** upload the bundle → a daemon `unpack` route
  extracts it to the chosen dest + the **remote-recomputed** slug dir.
- **Graceful degradation (fallback):** save the bundle locally + an
  auto-generated, workspace-specific **README**; the user unpacks by hand.
  This is the path for a mid-flight transfer failure and for
  offline/air-gapped moves. Build it anyway — it's the same bundle.

## The three locations (what moves)

For project at `PROJECT` with `SLUG = claude_project_hash(PROJECT)`
(`chat_history.rs:61` — `/.`→`/-`, `/`→`-`, ` `→`-`, case preserved):

1. **Workspace dir** — the `PROJECT` tree (`.k2so/`, project-local
   `.claude/`, config/persona/tools/docs), minus excludes + scrubbed
   secrets.
2. **Durable memory** — the **entire** `~/.claude/projects/<SLUG>/memory/`
   (`MEMORY.md` + every `*.md`). Nothing project-local to also grab.
3. **Live chat session** — the newest-mtime `<session-id>.jsonl` under
   `~/.claude/projects/<SLUG>/` (`detect_claude_session`). Memory + sessions
   share the slug dir → **one remote slug recompute places both.**

## Slug recompute (the #1 trap, handled)

`claude_project_hash` is PURE, so the remote slug is just
`claude_project_hash(DEST_PATH)` computed **on the remote** (the daemon has
the fn) — or client-side from `fs/info.home` + the chosen dest path. The
local slug is **never** copied. Worktree variants keep their
`<slug>-<branch>` suffix on recompute.

## Rule set (from the manual procedure)

### Secrets — scrub by default, with these rules
- **Patterns (anywhere in the tree):** `.env`, `.env.local`, `.env.*`,
  `.env.*.local`; `.auth/` (login/session state); any token-bearing file in
  `.k2so/` (config like a focus-group name is benign — gate on content).
- **Credential scan (pre-transfer safety net — treat hits as secrets):**
  `eyJ[A-Za-z0-9_-]{20,}` (JWTs), `gh[ports]_[A-Za-z0-9]+` (GH tokens),
  `service_role`, `PRIVATE KEY`, `password\s*[:=]`, `secret\s*[:=]`,
  `api[_-]?key\s*[:=]`.
- **NEVER migrate** `~/.claude/.credentials.json` — that's Claude Code's
  OWN user-level auth, not workspace state; the remote authenticates itself.
- **Carry vs scrub:** because Clone-to rides ONE authenticated, TLS link
  (not git+AirDrop), secrets *can* be carried securely. Default =
  **scrub** (matches #17's "excluded by default"); offer an explicit
  **"carry secrets over the encrypted link"** opt-in. `.credentials.json`
  is excluded regardless.
- **Re-supply report (always shown):** every excluded/carried secret **by
  relative path**, plus the external re-auth checklist: MCP server
  creds/config; OS-permission-gated tooling (e.g. macOS Full Disk Access +
  Automation); and "the remote needs its own Claude Code auth."

### Exclude / bulk (reuse the `ignore` crate + skip-list)
`node_modules`, build artifacts (`dist`/`out`/`.next`/`build`),
screenshots/artifacts dirs, `.auth/`, caches (`.cache`/`.turbo`/`.vercel`),
`*.log`, `.DS_Store`/OS junk, disposable per-ticket scratch files. Old
transcripts excluded by default (see Sessions). Respect `.gitignore`/
`.k2soignore` via the existing `ignore = "0.4"` crate
(`llm/file_index.rs`); extend the `projects_ops.rs` skip-list.

### Sessions
- **Default:** the **live** session only (newest mtime) — the conversation
  you're in; small payload, no extra sensitive transcripts.
- **Opt-in "include all history"** checkbox (warn: large + raw tool output).
- **Worktrees:** bring `<slug>-<branch>/` dirs **only** when they hold a
  recent session the agent is mid-work in; the remote recompute applies the
  same `-<branch>` suffix so they land correctly.

### Nested git repos
Copy the nested repo's **working tree as-is, including its `.git`** — it
arrives as a functioning clone. The "useless empty pointer" is a
git-*channel* artifact (gitlink from `git add`-ing a nested repo); Clone-to
transfers files directly, so it doesn't apply. Apply the same bulk-excludes
**inside** the nested repo (its `node_modules`, etc.). No flatten, no skip.

## Mechanics

1. **Inventory (local):** resolve `PROJECT` + `SLUG`; collect the workspace
   tree (minus excludes), the `memory/` dir, and the live session
   `.jsonl`(s). Run the credential scan; build a **manifest** mapping each
   file → its destination class (`workspace` | `memory` | `session`) + the
   re-supply list.
2. **Bundle:** tar+gz the included files + the manifest (new crate: `tar` +
   `flate2`).
3. **Destination:** user picks the connected server + a **PARENT** folder on
   the host via the existing `RemoteFolderPicker` (it browses real dirs over
   `fs/read-dir`/`fs/info`). We then create the workspace folder INSIDE it,
   named after the **source workspace** — `DEST_PATH = <parent>/<name>` —
   with a collision-safe suffix (`name (1)`) if taken. **No new-folder-name
   prompt** (keep it simple); the preview just shows the resulting full
   `DEST_PATH` so the user can see where it'll land. Compute the remote slug
   = `claude_project_hash(DEST_PATH)`; the host home comes from `fs/info`.
4. **Push + unpack (high bar):** upload the bundle (reuse
   `fs/upload-binary` for the bytes, or a streaming variant for large
   bundles) → a NEW daemon route `POST /cli/clone/unpack { bundle_path,
   dest_path }` extracts per the manifest: workspace files → `DEST_PATH`;
   memory + session → `<remote-home>/.claude/projects/<remote-slug>/...`.
   `write_upload`/`create_dir_all` already auto-create dirs.
5. **Done:** show the re-supply checklist + the resume hint. The agent
   resumes via `claude --resume <session-id>` with `cwd = DEST_PATH` (the
   transcript is at the recomputed slug dir) — exactly the wired path
   (`agent_launch.rs` / `resume_chat.rs`).

## UX

Right-click a workspace in the navbar → **"Clone to ▸ \<server\>"** →
RemoteFolderPicker to pick the **parent** folder on the host → a **preview**
(the resulting full `DEST_PATH`, what transfers, what's scrubbed, total
size, session-history scope) → **Clone** → progress → **done** screen with
the re-supply checklist + "resume on the host" note.

## New surfaces

- **Daemon:** `POST /cli/clone/unpack { bundle_path, dest_path }` — extract
  bundle per manifest, recompute slug locally (`claude_project_hash`),
  place memory/sessions under `~/.claude/projects/<slug>/`. Gated `token_ok`
  (same isolated-gate pattern as `fs/upload-binary`).
- **Core (k2so-core):** the **inventory + scrub + manifest + tar/gz engine**
  (`clone/` module) — pure + unit-testable; reused by both the high-bar push
  and the fallback bundle. Add `tar`/`flate2` to Cargo.
- **Tauri:** large-bundle streaming if `fs/upload-binary`'s base64-in-JSON
  is too heavy for a multi-GB workspace (decide via measured bundle size;
  MVP can cap + warn).
- **Renderer:** the "Clone to" context-menu action, the preview/progress/
  done flow, and the re-supply report.

## Phasing

- **P1 — bundle engine** (core): inventory the 3 locations, scrub (patterns
  + credential scan), excludes, nested-git, manifest, tar/gz. Unit-tested
  against a synthetic workspace. Powers BOTH consumers.
- **P2 — Clone to (high bar / v1):** upload + `clone/unpack` daemon route +
  remote slug recompute + the context-menu/preview/progress/done UX.
- **P3 — graceful degradation:** save-bundle-locally + auto-README fallback
  (mid-flight failure + offline). Mostly a different "sink" on the P1 engine.
- **P4 — polish:** carry-secrets opt-in, all-history opt-in, worktree
  handling, large-bundle streaming, cross-OS (Windows separator) dest paths.

## Open questions
- Large workspaces: base64-in-JSON cap vs a streaming/multipart upload — set
  the threshold from a measured real bundle.
- Carry-secrets default — confirm scrub-by-default (per #17) vs carry-by-
  default-since-link-is-encrypted.
- Post-clone: leave the source intact (clone, not move) — confirm "clone"
  semantics (the ticket says clone). A later "move + deregister local" is a
  separate action.
- Re-auth automation: how far to automate MCP/OS-permission re-supply vs
  just listing it.
