# K2SO 0.36.16 — Hotfix: 200% CPU when sidebar polls hit big workspaces

If you had a JS / Rust / large-monorepo workspace open in the sidebar, the K2SO app was sustaining ~200% CPU at idle. Three independent polling loops were each hitting hot filesystem paths — and one of them, the sidebar's git-status indicator, was doing it across every workspace component on the page every 5 seconds. This release patches all three and drops idle CPU by ~3×.

## What was burning

Profiling showed three separate hot loops in the Tauri main process:

### 1. `repo.statuses(...)` walking into `node_modules/` every 5 seconds

`useGitInfo` in `useGit.ts` polls `git_info` every 5 seconds. The Rust handler called `repo.statuses()` with `recurse_untracked_dirs(true)`. That flag tells libgit2 to descend into every untracked directory — including `node_modules/`, `target/`, `dist/`, `.next/` — to enumerate individual files inside. On a JS workspace with 100k+ files in `node_modules/`, every poll was a multi-hundred-thousand-stat sweep.

The fix: drop `recurse_untracked_dirs(true)` at both call sites in `crates/k2so-core/src/git.rs`. libgit2 now reports an untracked directory as a single entry (e.g. `node_modules/`), which is what every git GUI / `git status` CLI does by default and what the sidebar's "is this workspace dirty?" indicator actually needs. Per-file detail is still available on demand from the commit panel via `get_changed_files`.

### 2. `useGitInfo` polling at 5s × 5+ subscribers

The same `useGitInfo` hook is mounted by `App.tsx`, `IconRail.tsx`, `SectionItem.tsx`, and `Sidebar.tsx` (four times) — each one creates its own `setInterval(5000)` for the same workspace path. Five+ `git_info` calls per workspace per 5 seconds = roughly one `repo.statuses()` per second across the renderer, even before the recurse fix.

The fix: bump `GIT_POLL_INTERVAL` from 5s → 30s in `src/shared/constants.ts`. A "is the workspace dirty?" indicator that lags by tens of seconds is fine UX (the dot just turns red a little later than it could); real dirty-state visibility happens in the commit panel which fetches on demand. Future work: deduplicate multi-component polling via a shared timer + filesystem watcher.

### 3. `chat_history_detect_active_session` polling indefinitely

`AgentChatPane`'s session-detect effect polls `chat_history_detect_active_session` every 5 seconds — and never stops if the session never gets detected. Claude Code only writes its session `.jsonl` after the user types their first prompt; users who open the chat tab and don't immediately interact stayed in this state forever.

The detection itself was also doing `fs::read_dir(~/.claude/projects/)` on every call to handle worktree-branch-suffixed directories — a stat per entry, with hundreds of entries on machines that have used Claude across many projects. Two-tab effective rate: ~150 stats/sec sustained.

The fix:

- `crates/k2so-core/src/chat_history.rs::detect_claude_session` tries the direct `~/.claude/projects/<hash>/<id>.jsonl` path first (1 stat). The `read_dir` fallback only fires for the worktree-branch case where a `<hash>-<branch>` suffix is needed.
- `src/renderer/components/AgentPane/AgentChatPane.tsx` caps the polling at 12 attempts (1 minute total). After that, a console.info line says "gave up — refresh to retry," and clicking the refresh button (added in 0.36.13) bumps `refreshNonce` which is in the effect deps, so the counter resets and detection re-runs. Common case: detection finds the session on attempt 1-3 and the loop stops naturally.

## Numbers

| Stage | Tauri main CPU | Stat rate (sustained) |
|-------|----------------|------------------------|
| Before | ~205% | ~170/sec |
| After chat-history fix only | ~161% | ~13/sec |
| After all three fixes | ~69% | normal |

A ~3× reduction at idle. The remaining ~69% is normal app work (terminal grid subscribers, in-cap chat-history polls × open chat tabs, WebKit baseline).

## Filed for follow-up

- **Test fixtures lagging 0.36.14/15.** Five tests in `session_stream_egress.rs`, `session_stream_ingress.rs`, `providers_inject_integration.rs` assert against the bare `<agent>` key, but production switched to the prefixed `<workspace>:<agent>` form in 0.36.14. Production logic is correct; test fixtures need updating to match.
- **Deduplicate `useGitInfo`** so five components mounting the hook for the same workspace path share one underlying timer (and one `repo.statuses()`).
- **Watch the worktree** for filesystem events and only re-fire `git_info` when something actually changed — eliminates the polling entirely while keeping the indicator responsive.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
