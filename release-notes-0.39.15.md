# K2SO 0.39.15 — Hide audit sentinels from the user-facing project list (Issue #11)

Daemon/core only (`crates/k2so-core/src/projects_ops.rs` + `db/mod.rs`),
+110/-1. Renderer untouched. No migrations.

## The bug
Every first-time user saw two phantom "projects" in the workspace
sidebar — **"Orphan audit bucket"** (`_orphan`) and **"Broadcast audit
bucket"** (`_broadcast`) — that they didn't create and can't use. A
brand-new DB contains *only* these two rows until the user adds a real
workspace, so they were the literal first impression.

## What they are (working as intended)
`seed_audit_sentinels()` seeds them during DB init as legitimate routing
destinations for the activity feed: `_broadcast` collects
`AgentAddress::Broadcast` signals; `_orphan` is the FK-miss fallback so an
audit row from a since-deleted workspace still lands somewhere. They
**should** exist — they just shouldn't be shown as workspaces.

## The fix
Filter the two sentinel ids in **`projects_ops::projects_list()`** — the
UI-facing wrapper used by the daemon `projects/list` route and the Tauri
command — mirroring what the companion API already does. A shared const
`db::AUDIT_SENTINEL_IDS` (`["_orphan","_broadcast"]`) is centralized next
to `seed_audit_sentinels`.

`Project::list()` itself is **left unfiltered** — internal callers
legitimately need every row (heartbeat/agent scanning in daemon
`main.rs`, the dedup/import check, `next_tab_order()`, migrations). Only
the UI wrapper filters.

## Tested
- New test `projects_list_hides_audit_sentinels_but_project_list_keeps_them`:
  seeds a DB (sentinels + a real workspace), asserts `projects_list()`
  excludes both sentinels but keeps the real workspace, `Project::list()`
  still returns both, and the two surfaces differ by exactly the two
  sentinels.
- **k2so-core: 666 passed, 0 failed** (+ all integration/doc suites green).
- `cargo clippy -p k2so-core` clean for the change (pre-existing
  `delegate.rs` non-semver-`since` findings are unrelated).

## Upgrade notes
- Any 0.39.x → 0.39.15: clean update, no migrations. The buckets simply
  stop appearing in the sidebar.

## What else shipped in this release
Nothing else. See `release-notes-0.39.0.md` through
`release-notes-0.39.14.md` for prior content.
