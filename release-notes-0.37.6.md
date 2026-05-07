## Highlights

**Every deletion of user-authored content now goes to the OS recycle bin, never permanent unlink.** Direct response to a user report in 0.37.5: a workspace's user code (a "bouncy blobs" game) got deleted after a "move + pin" interaction. Forensics pointed at the worktree-removal fallback path which called `git worktree remove` — a permanent deletion with no recycle bin. This release closes that hole and audits every other site that could touch user content.

## What changed

New helper module `k2so_core::safe_delete` with two functions:

- **`trash(path)`** — sends to OS recycle bin. If the trash op fails (no Trash service, AppleScript timeout, etc.), bubbles the error and **does not fall back to permanent delete**. Used for paths that contain user-authored content where the safe failure mode is "leave it in place and let the user investigate."
- **`trash_or_remove(path)`** — trash-preferred with fallback. Used for K2SO-scaffolded paths where the worst case of trash failure is "K2SO has to re-scaffold a file it created." Limited to scratch we own.

Sites converted to `trash`:

| Site | What it deletes |
|---|---|
| `git.rs::remove_worktree` (fallback path) | Entire git worktree directory ← **the bouncy-blobs root cause** |
| `agents::commands::delete_inner` | Agent dir (`AGENT.md` + work items + heartbeat config) |
| `agents::heartbeat::*remove*` | Heartbeat dir (WAKEUP.md user edits + history) |
| `agents::onboarding::adopt_*` | User's original CLAUDE.md / GEMINI.md / etc. (after archive copy) |
| `agents::delegate` | Work item source file (after copy to active/) |
| `agents::unification` (primary + template paths) | work/, heartbeats/, CLAUDE.md, SKILL.md during 0.37.0 migration |
| `commands::k2so_agents` (harvest) | Original CLAUDE.md (after archive copy) |
| `commands::skill_layers::delete` | User-authored skill layer file |

Sites using `trash_or_remove` (K2SO-scratch with fallback for headless test envs):

- `teardown_workspace_harness_files` symlink + aider.conf.yml removals — these are K2SO-scaffolded fresh files; trash preferred but fallback acceptable so the teardown never gets stuck on Finder timeouts.

Sites left as `fs::remove_*` (each with an inline justification comment):

- `fs_atomic.rs` — atomic-write temp files (the temp IS half-finished work that needs cleanup)
- `pending_live::drain*`, `awareness::inbox::drain` — drained signal/work files (already injected into the target session; consumed state)
- `agents::session::*_unlock` — `.lock` file removal
- launchd plist install/uninstall — K2SO-owned, recreatable
- Test cleanup paths under `std::env::temp_dir()`
- Migration internals where data is preserved at `dst` before source removal (`unification::move_dir`, `move_path`)

## The bouncy-blobs root cause, explicitly

Pre-0.37.6, `remove_worktree` had three deletion paths:

1. Fast path: `fs::rename` to a temp name, then `trash::delete` in a background thread (recoverable)
2. Fallback: `git worktree remove [--force] <path>` (PERMANENT — git's CLI does an irreversible recursive unlink)
3. Last resort: `trash::delete` directly (recoverable)

If step 1 failed (cross-volume rename, lock held, permission), step 2 ran — and `git worktree remove` permanently destroyed the worktree directory. That's the chain that ate the user's game.

Post-0.37.6, the fallback path is removed entirely. After step 1 fails, the next attempt is `git worktree prune` (cleans up the dangling ref) followed by `safe_delete::trash` of the worktree directory. If trash itself fails, we surface the error and **stop** — better to leave the worktree in place and let the user investigate than to permanently destroy their work.

## Tests

756 tests passing — same as 0.37.5. One test (`teardown_restore_original_brings_back_every_archive`) needed the new `trash_or_remove` helper because macOS Finder times out in headless test environments; the test now exercises the fallback path correctly.

## What's still permanent (and why)

The remaining `fs::remove_*` calls are documented in the `safe_delete` module's doc-block:

> - `fs_atomic.rs`: atomic-write temp files (the temp IS a half-finished write that needs cleanup; nothing to recover).
> - `pending_live::drain*`: drained signal JSONs (already injected into a session; the file is consumed state, not user content).
> - `awareness::inbox::drain`: same as above.
> - `agents::session::*_unlock`: `.lock` file removal.
> - Test cleanup paths: temp workspaces under `std::env::temp_dir()`.
> - launchd plist install/uninstall: K2SO-owned, recreatable.

If you ever spot a deletion site that doesn't fit the above categories, it's probably a bug — please report.
