# K2SO 0.39.14 — Pinned tabs: stop + heal wrong-workspace binding (Issue #9 / PR #10)

Renderer-only (`src/renderer/stores/tabs.ts` + `tabs.test.ts`), two commits.
No daemon/protocol changes, no migrations.

## The bug
A workspace's pinned **Chat/Inbox** tabs could end up bound to a
**different** workspace's agent and `projectPath` — wrong agent name,
wrong directory, and (for Chat) a leaked Claude `sessionId`. New
terminal tabs opened correctly (live `cwd`), but the pinned tabs kept
routing to the other workspace. Not self-healing, no user workaround.

## Root cause — an async workspace-switch race (the real one)
`ensurePinnedAgentTabForMode` is **async**: it runs in `setTimeout(0)`
and `await`s a `k2so_agents_list` round-trip before calling
`ensureSystemAgentTabs`, which mutates the **globally-active** tab set.
All four call sites pass the correct `project.path` — but if the user
**switches workspaces while a resolution is in flight**, the stale
callback stamps the *previous* workspace's `agentName`/`projectPath`
into whichever workspace is now active, and it persists under the
new workspace's layout key.

Concretely: switching from a K2SO **worktree** workspace (agent
`cli-eng`, path `…/K2SO`) into another workspace (HK47) mid-resolution
wrote `cli-eng`/`…/K2SO` into HK47's pinned tabs. This also explains why
**deleting** a bad layout row didn't stick — the fresh rebuild re-hit
the same race. And the leaked chat `sessionId` came via
`stampAgentSessionId`, which matches pinned chat tabs by
`(agentName, projectPath)` across all in-memory tabs, so the
wrongly-`(cli-eng, …/K2SO)` tab also adopted K2SO's session.

A loose audit on the local `workspace_layouts` (78 rows) found this on
several workspaces, all children spawned/switched under a shared parent.

## The fix — two commits, both needed
**1. Guard the race (prevention).** Capture `activeWorkspaceKey`
synchronously when `ensurePinnedAgentTabForMode` is invoked
(`setActiveWorkspace` sets it before calling), and **bail in the async
callback if the active workspace changed** before `ensureSystemAgentTabs`
runs. Stops any *new* corruption.

**2. Reconcile on build (heal).** `ensureSystemAgentTabs` now reconciles
an existing pinned tab to the authoritative `agentName`/`projectPath`
(via `reconcileSystemAgentTab`) instead of reusing it verbatim — drops
the chat `sessionId` when `projectPath` changes, and returns the **same
object reference** when nothing changed (no needless re-render). Heals
already-persisted bad rows automatically on the next switch into the
workspace.

Together: opening an affected workspace both **stops** new corruption
(guard) and **heals** the existing bad row (reconcile) — no DB surgery,
no migration. (A migration that only rewrote `workspace_layouts` would
be insufficient on an un-patched build, since the race re-creates the
row; the code guard is what makes the fix durable.)

### Why the heal lives in `ensureSystemAgentTabs`, not `restoreLayout`
For **worktree** workspaces the restore `cwd` is the worktree path, but
the pinned agent tab must point at the **project root** (`project.path`)
— only `ensureSystemAgentTabs` receives that authoritative value.

## Tested
- Renderer **vitest: 67/67** — including: *heal-on-switch* (stale A → B,
  agentName + projectPath fixed, stale sessionId dropped, tab id
  preserved), *no-op-when-unchanged* (object identity retained), and the
  race-guard pair (*workspace-changed-mid-resolution → nothing stamped*,
  *workspace-unchanged → stamped with the correct projectPath*).
- Typecheck **43** — unchanged baseline, zero new.
- **Validated live** on the real `workspace_layouts` (78 rows): a precise
  per-agent-item audit found the offenders; switching into **HK47** and
  **K2SO-companion** in the patched build reconciled each back to its own
  agent/path (confirmed by re-running the audit — both dropped out).
  Remaining offenders self-heal identically on next open.

## Upgrade notes
- Any 0.39.x → 0.39.14: clean update, no migrations. Affected workspaces
  self-heal the next time you switch into them, and the guard prevents
  recurrence.

## Known follow-up (not in this release)
- `stampAgentSessionId` matches pinned chat tabs by
  `(agentName, projectPath)` across all in-memory tabs; worth additionally
  keying on the owning workspace/tab id so two workspaces can never share
  a stamp target (defense-in-depth — the race guard + reconcile already
  fix the root cause).

## What else shipped in this release
Nothing else. See `release-notes-0.39.0.md` through
`release-notes-0.39.13.md` for prior content.
