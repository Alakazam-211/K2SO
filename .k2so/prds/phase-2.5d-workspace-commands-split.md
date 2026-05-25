# Phase 2.5d: agents/workspace.rs + agents/commands.rs split

**Status**: Drafted 2026-05-25 from Explore agent audit. Launches after this PRD lands. Final agents/ residual cleanup after Phase 2.5c.
**Internal version markers**: 0.39.0i or 0.39.0j
**Owner**: Rosson + pod-leader
**Date**: 2026-05-25

---

## tl;dr

Phase 2.5c deferred two large files because they're too big to safely move in single commits:
- `agents/workspace.rs` — 143 KB / 3,438 LoC / 28 public items
- `agents/commands.rs` — 45 KB / 1,151 LoC / 27 public items

Phase 2.5d splits both into proper-home modules per the Phase 2.5c conceptual model (`workspace/`, `skills/`, `heartbeats/`).

**Split target**: 9 new/expanded files across 3 existing module homes. Zero dead code in either file (Phase 2.1c already removed `work_*` functions). After the split, `agents/` likely contains ONLY the back-compat shim `mod.rs` — at which point we retire the shim and remove the directory.

Audit `a88b497263ee9dc64` produced a function-by-function map; all 6 design open-questions were resolved by accepting the audit's defaults (consistent with Phase 2.5c direction).

**Complexity**: Medium per audit. ~1 day of subagent work + verification.

---

## File-by-file split plan

### `agents/workspace.rs` (143 KB) → 4 new files

| New file | LoC est | Items |
|---|---|---|
| `workspace/migrations.rs` | ~400 | `archive_orphan_top_tier_agents`, `repair_mismigrated_heartbeats`, `promote_legacy_heartbeat`, `ensure_workspace_wakeups`, `migrate_filenames_to_uppercase`, `migrate_or_scaffold_lead_heartbeat`, `detect_interrupted_regen`, `harvest_per_agent_claude_md_files` (8 fns) |
| `workspace/skill_writer.rs` | ~1,200 | `write_workspace_skill_file`, `write_workspace_skill_file_with_body`, `strip_workspace_skill_tail`, `append_workspace_source_regions`, `ensure_all_skills_up_to_date`, `regenerate_workspace_skill` (6 fns + 2 constants: `SKILL_USER_NOTES_SENTINEL`, `USER_NOTES_PLACEHOLDER`) |
| `workspace/harness.rs` | ~600 | `k2so_agents_preview_workspace_ingest`, `k2so_agents_run_workspace_ingest`, `disable_workspace_claude_md`, `WorkspacePreviewEntry`, `HARNESS_WORKSPACE_FILES` constant |
| `workspace/teardown.rs` | ~250 | `k2so_agents_teardown_workspace`, `teardown_workspace_harness_files`, `TeardownResult`, `TeardownMode` |

Total: ~2,450 LoC across 4 files. Difference vs original 3,438 = ~990 LoC of private helpers that get distributed inline with the function clusters they support.

### `agents/commands.rs` (45 KB) → 5 destinations

| New file | LoC est | Items |
|---|---|---|
| `workspace/agent.rs` (NEW) | ~450 | Agent CRUD: `list`, `create`, `delete`, `delete_inner`, `get_profile`, `update_profile`, `update_field`, `update_agent_md_field`, `cleanup_agent_backups`, `log_agent_warning`, `K2soAgentInfo` struct |
| `heartbeats/control.rs` (NEW) | ~150 | `ensure_agent_wakeup`, `get_heartbeat`, `set_heartbeat`, `heartbeat_noop`, `heartbeat_action` |
| `workspace/agent_editor.rs` (NEW) | ~150 | `k2so_agents_get_editor_context`, `k2so_agents_preview_agent_context`, `k2so_agents_regenerate_agent_context`, `k2so_agents_save_agent_md` |
| `workspace/relations.rs` (NEW) | ~80 | `workspace_session_get`, `workspace_relations_list`, `workspace_relations_list_incoming`, `workspace_relations_create`, `workspace_relations_delete` |
| `skills/crud.rs` (EXISTING) | (existing) | `regenerate_skills` — migrate the live callers; delete from commands.rs |

---

## Cross-file dependency

Single direct import: `workspace.rs` line 40 imports `commands::ensure_agent_wakeup`. Used by `ensure_workspace_wakeups` (line 460).

Post-split: `workspace/migrations.rs::ensure_workspace_wakeups` → import from `heartbeats/control.rs::ensure_agent_wakeup`. One-line change.

---

## Design decisions (all 6 audit open questions resolved)

| Q | Decision | Reasoning |
|---|---|---|
| Heartbeat control placement (`heartbeats/control.rs` vs `workspace/agent.rs`) | `heartbeats/control.rs` | Heartbeat logic stays together; even though called during agent creation, conceptually it's heartbeat-scoped |
| Skill regen re-export cleanup | Migrate callers to `skills/crud.rs` directly; delete from commands.rs | Don't leave back-compat re-exports unless callers can't be updated |
| Agent editor as separate module | `workspace/agent_editor.rs` (separate file) | Distinct UI-facing layer (AIFileEditor); different from CRUD |
| Migration helpers coupling to heartbeats | Keep in `workspace/migrations.rs` | Boot-time one-shots, not steady-state heartbeat API |
| Skill writer location (`workspace/` vs `skills/`) | `workspace/skill_writer.rs` | Part of workspace lifecycle; references harness + teardown; not just skill versioning |
| Test suite location | Move tests with the functions they test | Standard Rust practice |

---

## Commit strategy

**Goal**: each commit compiles. Group related moves; preserve git blame via `git mv` per file.

### Tier A: workspace.rs split (~6 commits)

```
2.5d.1: Create workspace/migrations.rs from migration helpers (8 fns + private)
2.5d.2: Create workspace/skill_writer.rs from skill-writer cluster (8 fns + 2 constants + private)
2.5d.3: Create workspace/harness.rs from harness/preview/disable cluster (3 fns + WorkspacePreviewEntry + constant)
2.5d.4: Create workspace/teardown.rs from teardown cluster (2 fns + 2 types + private)
2.5d.5: Delete now-empty agents/workspace.rs + update agents/mod.rs back-compat
2.5d.6: Update lib.rs pub mod declarations + cross-crate imports (daemon, src-tauri)
```

### Tier B: commands.rs split (~5 commits)

```
2.5d.7: Create workspace/agent.rs from CRUD cluster (10 fns + K2soAgentInfo struct)
2.5d.8: Create heartbeats/control.rs from heartbeat cluster (5 fns)
2.5d.9: Create workspace/agent_editor.rs from UI editor cluster (4 fns)
2.5d.10: Create workspace/relations.rs from relations cluster (5 fns)
2.5d.11: Migrate regenerate_skills callers to skills/crud.rs; delete from commands.rs
2.5d.12: Delete now-empty agents/commands.rs + update agents/mod.rs back-compat
```

### Tier C: agents/ retirement (~2 commits)

```
2.5d.13: Retire agents/mod.rs back-compat shim (16 pub use aliases); update remaining downstream imports to canonical paths
2.5d.14: Remove agents/ directory + drop pub mod agents from lib.rs; final cargo test + typecheck verification
```

### Cross-crate updates (interleave as needed)

- `src-tauri/src/commands/k2so_agents.rs` re-exports — update paths to new homes, keep external names stable
- `crates/k2so-daemon/src/` — update all `use k2so_core::agents::workspace::*` and `use k2so_core::agents::commands::*` to canonical post-split paths

### Total

~14 commits across 3 tiers. Each must compile (`cargo check --workspace`) on its own.

---

## Cross-crate impact

Per Phase 2.5c's audit, downstream crates affected:

| File | Action |
|---|---|
| `src-tauri/src/commands/k2so_agents.rs` | Update `pub use` re-export source paths; keep external symbol names stable |
| `crates/k2so-daemon/src/cli.rs` | Update use statements for any `k2so_core::agents::workspace::*` or `k2so_core::agents::commands::*` references |
| `crates/k2so-daemon/src/main.rs` | Same — particularly the migration sweep (which calls `agents::workspace::*` migration helpers) |
| `crates/k2so-daemon/src/agents_routes.rs` | Same |

**Total `use` updates** estimated at ~50-80 across the workspace. Compiler-guided.

---

## Validation strategy

**After each commit**:
- `cargo check --package k2so-core` (fast)

**After commits that update cross-crate imports**:
- `cargo check --workspace` (catches daemon + Tauri impacts)

**After Tier C lands (full split done)**:
- `cargo test --release --workspace` — baseline 907 / 0 failed. Should stay at 907 or grow.
- `bun run typecheck` — baseline 47 errors. Must NOT grow.
- `bash tests/cli/*.sh` — baseline 20 passing. Should stay flat.
- `cargo test --doc -p k2so-core` — must run clean.
- Sanity grep: `grep -rn 'k2so_core::agents::\(workspace\|commands\)' .` — should return zero hits in production code.

---

## Hard rules (CRITICAL)

1. **Build only from worktree's `target/`** — never `cargo install`, never touch production.
2. **`git mv` for all file moves** — preserves git blame.
3. **No `git commit --no-verify`, no `--amend`, no `Co-Authored-By` lines.** Per memory `feedback_commit_attribution`.
4. **Do NOT touch version strings.** Per memory `feedback_no_version_bump`.
5. **Keep public re-export NAMES stable** at the `src-tauri/src/commands/k2so_agents.rs` boundary — external callers see the same symbols, just sourced from new paths.
6. **Tests must fail loudly.** Per memory `feedback_test_discipline`.
7. **Move tests with the functions they test** — don't strand tests in a deleted file.
8. **Daemon-first** principle unchanged — refactor only moves code, doesn't change daemon ownership boundaries.

---

## Definition of done

1. ✅ `agents/workspace.rs` deleted; contents distributed across 4 new workspace/ files
2. ✅ `agents/commands.rs` deleted; contents distributed across 5 destinations (4 new + 1 existing)
3. ✅ `agents/mod.rs` back-compat shim retired; agents/ directory removed
4. ✅ `lib.rs` declares all new modules; old `pub mod agents` dropped
5. ✅ `src-tauri/src/commands/k2so_agents.rs` re-exports updated with stable external names
6. ✅ All daemon imports updated (cli.rs, main.rs, agents_routes.rs, etc.)
7. ✅ `cargo test --release --workspace` baseline preserved (907 or higher)
8. ✅ `bun run typecheck` baseline preserved (47, no growth)
9. ✅ `cargo test --doc -p k2so-core` runs clean
10. ✅ Sanity grep: zero `k2so_core::agents::workspace::` or `k2so_core::agents::commands::` references in production code

---

## Stop-and-report triggers

The subagent should STOP and report (rather than half-implement) if:

- A function move requires significant signature changes (not just module path) — surface what changed and why
- A test fails after a move and the cause isn't an obvious import-path issue
- Cross-crate import updates cascade beyond the audited consumer list
- A private helper has consumers across two of the new files (would need to extract a shared internal module)
- The cargo build hits a non-mechanical compile error (e.g., trait coherence issue triggered by module reorganization)

---

## Sequencing within Phase 2 / 3

```
Phase 2.5d (this PRD) — IN FLIGHT after audit lands
                ↓
Phase 2.5 main close (smoke validation finalizes; user-driven testing)
                ↓
Phase 2.6 — tunnel-provider decision (re-spike)
                ↓
Phase 3 — contract hardening (7 workstreams)
                ↓
PUBLIC 0.39.0 RELEASE
```

Phase 2.5d does NOT block the user-driven Phase 2.5 smoke testing; the two run in parallel.

---

## References

- `.k2so/prds/phase-2.5c-core-rename.md` — Phase 2.5c PRD that deferred these two files
- `.k2so/prds/0.39.0-public-release-roadmap.md` — master roadmap
- Phase 2.5d audit: subagent `a88b497263ee9dc64` (2026-05-25) — full function-by-function map
- Phase 2.5c audit: subagent `afdab30fdf6e64be9` (2026-05-24) — established 5-tier home structure
- Memory `project_workspace_agent_invariants` — workspace has one primary agent
- Memory `project_workspace_agent_addressing` — workspace identity is the routing key
- Memory `feedback_subagent_cherry_pick_pattern` — cherry-pick subagent commits onto current main
