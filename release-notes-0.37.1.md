# K2SO 0.37.1 — Hotfix: self-heal orphan FK rows on launch

If your 0.37.0 install crashed on launch with

```
FATAL: Failed to initialize database: FOREIGN KEY constraint failed
```

this release fixes it. Update + relaunch is enough — no manual
intervention required.

## What was happening

Earlier versions of K2SO had per-connection foreign-key enforcement
toggled off in some code paths. When a project was deleted under
those code paths, child rows in `activity_feed`, `heartbeat_fires`,
and `agent_sessions` were left stranded instead of being cascaded
away. 0.37.0's migration 0039 (the `agent_sessions →
workspace_sessions` rename + collapse) is the first migration that
adds a strict `REFERENCES projects(id)` constraint with a data-copy
INSERT, so it became the first version where stranded rows actually
trip the FK constraint at startup.

One affected client's database had 615 such rows pointing at two
projects that had been deleted long ago.

## The fix

K2SO now sweeps every FK-bearing project-child table at the very
start of database initialization — *before* any migration runs —
and purges rows whose parent `projects` row no longer exists. The
CASCADE rule on every FK declaration says these rows should already
be gone; this sweep just finishes the deletion that didn't happen.

Tables checked (the union of every shape this DB has had):

- `agent_sessions` (pre-0.37 → renamed in 0039)
- `agent_heartbeats` (pre-0.37 → renamed in 0040)
- `workspace_sessions` (post-0.37)
- `workspace_heartbeats` (post-0.37)
- `heartbeat_fires`
- `activity_feed`
- `workspace_layouts` (renamed from old workspace_sessions in 0038)

Each table is verified to exist and to actually carry a
`project_id` column before the DELETE, so partial-migration states
and fresh installs are both safe.

The sweep is idempotent — runs on every launch, no-ops on a clean
DB.

## Tests

Three new unit tests pin the contract:

- `purge_orphan_project_children_removes_stranded_rows` — synthetic
  orphan repro; asserts orphans are removed and non-orphan rows
  preserved.
- `purge_orphan_project_children_idempotent_on_clean_db` —
  re-running the sweep against a clean DB is a no-op.
- `purge_orphan_project_children_handles_pre_migration_db` — sweep
  against an empty DB (before migrations have run) returns Ok cleanly.

## If you already worked around this manually

The repair sequence we shared with the affected client (backup +
manual `DELETE … WHERE project_id NOT IN (SELECT id FROM projects)`
+ `PRAGMA foreign_key_check`) produces the same final state as
this release's automatic sweep. Updating to 0.37.1 over a
manually-repaired DB is a no-op.
