# Phase 2.5c: k2so-core file/module rename

**Status**: Drafted 2026-05-24 based on Explore agent audit. Launches AFTER Phase 2.5b (skills folder consolidation) lands and BEFORE Phase 2.5 (build+smoke). Pure refactor — no behavior change, no user-facing impact.
**Internal version markers**: 0.39.0h or 0.39.0i
**Owner**: Rosson + pod-leader
**Date**: 2026-05-24

---

## tl;dr

**The goal is to prune and refactor k2so-core's module layout** to align with the post-Phase-2.1 conceptual model. Files like `agents/skill_writer.rs` write skill templates (not agent things); `agents/scheduler.rs` triages the workspace inbox (not anything agent-specific); `agents/checkin.rs` is workspace-scoped agent self-report. The naming lies about contents.

After Phase 2.1 (workspace==agent invariant, inbox-as-email, `__lead__` removal, skills-as-documentation reframe) and Phase 2.5b (filesystem `.k2so/skills/` consolidation), the conceptual model has settled. Phase 2.5c moves source files to homes that match.

**~20-22 file operations** (renames, relocations, audits, one deprecation move) across 5 module homes (`workspace/`, `skills/`, `heartbeats/`, `migrations/`, `deprecated/`) + dead-code audit of `agents/commands.rs`. Rust's strict module system means the compiler catches every broken `use` statement.

**Likely side-effect (not the stated goal)**: `agents/` ends up empty or near-empty, and can be removed. We're NOT framing this as "kill the agents/ folder" because there's a lot to reorganize and the elimination is incidental. The stated goal is "everything lives in a folder that names what it actually does."

Risk is low (Rust compiler enforces correctness); ~290+ `use` updates across the workspace; ~4-6 hour subagent execution.

Goal: new contributors see module names that match what the code does; PRD-level documentation can describe the system in one consistent vocabulary; the `.k2so/` filesystem layout (post-2.5b: `agent/` + `skills/` + `heartbeats/` + `inbox/`) mirrors the code layout (`workspace/` + `skills/` + `heartbeats/` + `inbox.rs`).

---

## Confirmed renames + relocations

Per the audit's per-file analysis + user direction (2026-05-24) to redistribute the residual agents/ contents to proper homes.

### Tier 1 — Originally confirmed (per audit)

| Current path | New path | Why |
|---|---|---|
| `agents/skill_writer.rs` | `skills/writer.rs` | Writes SKILL.md templates + harness discovery files |
| `agents/skill_content.rs` | `skills/content.rs` | Generates skill content bodies per agent type |
| `agents/scheduler.rs` | `workspace/scheduler.rs` | Workspace inbox triage; decides which agents to wake |
| `agents/checkin.rs` | `workspace/checkin.rs` | Aggregated workspace state for `/cli/checkin` |
| `agents/wake.rs` | `workspace/wake_prompts.rs` | Wake-prompt composers (renamed to avoid collision with top-level `wake.rs`) |
| `agents/triage_summary.rs` | `workspace/triage.rs` | Read-only workspace triage report |
| `agents/onboarding.rs` | `workspace/onboarding.rs` | First-time workspace setup |
| `agents/heartbeat.rs` | `heartbeats/mod.rs` | Multi-heartbeat CRUD + tick (workspace-level entities post-2.5b) |
| `agents/heartbeat_install.rs` | `heartbeats/install.rs` | Plist scaffolding + launchctl orchestration |
| `agents/unification.rs` | `migrations/unification_0_37_0.rs` | 0.37.0 one-shot migration helper |
| `agents/build_launch.rs` | `workspace/agent_launch.rs` | Launch JSON for workspace heartbeat firing |

### Tier 2 — Residual agents/ redistribution (per user direction, 2026-05-24)

| Current path | New path | Why |
|---|---|---|
| `agents/delegate.rs` | `deprecated/delegate.rs` | CLI verb hard-deprecated Phase 2.1b; add `#[deprecated(since = "0.39.0f", note = "Harness owns spawn lifecycle; see Phase 2.1 PRD A23")]` to public functions; preserve code for any back-compat callers but make deprecation explicit. **Audit Tauri/frontend callers first**; if zero callers, delete entirely instead of moving. |
| `agents/launch_profile.rs` | `workspace/launch_profile.rs` | Per-agent launch data, consumed by workspace heartbeat firing — belongs alongside `workspace/agent_launch.rs` |
| `agents/skill.rs` | `skills/version.rs` | SKILL.md versioning + upgrade protocol belongs with skills |
| `agents/cron_schedule.rs` | `heartbeats/cron.rs` | Cron is heartbeat machinery — belongs with heartbeats |
| `agents/session.rs` | `workspace/session.rs` | Workspace's session state |
| `agents/settings.rs` | `workspace/settings.rs` | Workspace-scoped settings reader |
| `agents/terminal_id.rs` | `workspace/terminal_id.rs` | Workspace's terminal IDs |
| `agents/mod.rs` (identity helpers: `find_primary_agent`, `agent_dir`, `agent_type_for`) | `workspace/agent_identity.rs` | The workspace's primary agent identity resolution |

### Tier 3 — Audit during execution (defer if scope blows)

| Current path | Action | Notes |
|---|---|---|
| `agents/work_item.rs` | AUDIT → move OR delete | Phase 2.1 replaced `WorkItem` with `InboxItem`. If still used: move to `workspace/work_item.rs` or fold into `inbox.rs`. If unused: delete. |
| `agents/display.rs` | AUDIT → move OR fold | If used in many places: `workspace/display.rs`. If used in one caller: fold into that caller's module. |
| `agents/events.rs` | AUDIT → move | If awareness-related: `awareness/events.rs`. Else: `workspace/events.rs`. |
| `agents/commands.rs` | AUDIT → DELETE or split | Phase 2.1c Item 2 removed work-queue functions; remaining functions may be 100% dead. Audit all callers; hard-delete if dead, redistribute if alive. |

### Tier 4 — Deferred to Phase 2.5d

| Current path | Why deferred |
|---|---|
| `agents/workspace.rs` (143 KB) | Too large to move safely in one commit. Needs internal split FIRST. Phase 2.5d will (a) split this into smaller modules, (b) move them to `workspace/` proper. Cannot be bundled into 2.5c. |
| Internal `agents/mod.rs` reorg post-move | After Tier 1 + 2 + 3 land, agents/ may be empty or near-empty. If empty: remove the directory + drop `pub mod agents;` from lib.rs. If near-empty: assess whether what's left is truly agent-scoped and just keep it (don't force-move). |

### Outcome to expect

Post-2.5c, `agents/` likely contains: `workspace.rs` (deferred) + whatever Tier 3 audits left behind. If Tier 3 audits all resolve to "move" or "delete," `agents/` may end up holding ONLY `workspace.rs` until 2.5d. That's fine — incidental, not the goal.

### NO-MOVE clarifications

`agents/awareness/` was a candidate I listed initially but the research agent confirmed it's already at top-level `awareness/` per `lib.rs`. No move needed.

---

## End-state module layout

```
crates/k2so-core/src/
├── awareness/         # Cross-workspace signal bus (ALREADY here; possibly + events.rs)
├── workspace/         # NEW: workspace lifecycle + state + identity + launch
│   ├── mod.rs
│   ├── agent_identity.rs    (from agents/mod.rs: find_primary_agent, agent_dir, agent_type_for)
│   ├── scheduler.rs
│   ├── checkin.rs
│   ├── wake_prompts.rs
│   ├── triage.rs
│   ├── onboarding.rs
│   ├── session.rs
│   ├── settings.rs
│   ├── terminal_id.rs
│   ├── launch_profile.rs    (per-agent launch data)
│   ├── agent_launch.rs      (was build_launch.rs — composes launch from workspace context)
│   └── (display.rs, work_item.rs — audit-pending)
├── skills/            # NEW: skill profile writers + content generators + versioning
│   ├── mod.rs
│   ├── consolidation.rs     (from Phase 2.5b)
│   ├── writer.rs            (was agents/skill_writer.rs)
│   ├── content.rs           (was agents/skill_content.rs)
│   └── version.rs           (was agents/skill.rs)
├── heartbeats/        # NEW: workspace heartbeat schedules + launchd + cron
│   ├── mod.rs               (was agents/heartbeat.rs)
│   ├── install.rs           (was agents/heartbeat_install.rs)
│   └── cron.rs              (was agents/cron_schedule.rs)
├── inbox.rs           # Workspace inbox (Phase 2.1a — already here)
├── migrations/        # NEW: historical migration helpers
│   └── unification_0_37_0.rs
├── deprecated/        # NEW: retired-but-preserved surface
│   └── delegate.rs          (CLI verb hard-deprecated; preserved for any back-compat callers with #[deprecated] annotation)
└── agents/            # Possibly empty post-2.5c except for the deferred workspace.rs
    └── workspace.rs   (143 KB — DEFERRED to Phase 2.5d for internal split before move)
```

If Tier 3 audits resolve cleanly, `agents/` ends up containing only `workspace.rs` (deferred). Phase 2.5d will split that file and complete the elimination. Until then, `agents/` is allowed to exist as a near-empty directory.

---

## Cross-crate impact

Per the audit:

| File | Impact | Action |
|---|---|---|
| `src-tauri/src/commands/k2so_agents.rs` | HIGH — ~15 `pub use k2so_core::agents::*` re-exports | Update each re-export path to new home; re-export NAMES stay stable so external callers don't break |
| `crates/k2so-daemon/src/agents_routes.rs` | MEDIUM — direct `use` statements | Update use statements in same commit as the move |
| `crates/k2so-daemon/src/main.rs` | MEDIUM — boot-time uses agents::heartbeat, agents::unification, agents::workspace | Update in same commit |
| `crates/k2so-daemon/src/triage.rs` | MEDIUM — uses agents::{scheduler, triage_summary, settings, heartbeat, wake} | Update in same commit |
| `crates/k2so-daemon/src/heartbeat_launchd_routes.rs` | LOW — single function from agents::heartbeat_install | Update in same commit |
| `crates/k2so-core/src/lib.rs` | HIGH — `pub mod` declarations + `pub use` re-exports | Add new module declarations; keep external re-exports stable where possible |

**Total estimated `use` statement updates across workspace**: ~290 direct + ~15 internal agent submodule refs.

---

## Commit strategy

**Goal**: each commit compiles. Group tightly-related moves to keep commits small but coherent. Single-rename-per-commit for git blame preservation where files are independent.

### Tier 1 (originally confirmed, ~10 commits)

```
1. Create skills/ + move skill_writer.rs → skills/writer.rs
2. Move skill_content.rs → skills/content.rs
3. Create heartbeats/ + move heartbeat.rs → heartbeats/mod.rs
4. Move heartbeat_install.rs → heartbeats/install.rs
5. Create workspace/ + move scheduler.rs → workspace/scheduler.rs
6. Move checkin.rs → workspace/checkin.rs
7. Move wake.rs → workspace/wake_prompts.rs
8. Move triage_summary.rs → workspace/triage.rs + onboarding.rs → workspace/onboarding.rs
9. Move build_launch.rs → workspace/agent_launch.rs
10. Create migrations/ + move unification.rs → migrations/unification_0_37_0.rs
```

### Tier 2 (residual redistribution, ~8 commits)

```
11. Create deprecated/ + move delegate.rs → deprecated/delegate.rs WITH #[deprecated] annotations
    (audit Tauri/frontend callers FIRST; if zero callers, DELETE instead of relocate)
12. Move launch_profile.rs → workspace/launch_profile.rs
13. Move skill.rs → skills/version.rs
14. Move cron_schedule.rs → heartbeats/cron.rs
15. Move session.rs → workspace/session.rs
16. Move settings.rs → workspace/settings.rs
17. Move terminal_id.rs → workspace/terminal_id.rs
18. Extract identity helpers from agents/mod.rs → workspace/agent_identity.rs
```

### Tier 3 (audits, 1-4 commits depending on outcomes)

```
19. AUDIT + resolve work_item.rs (move or delete)
20. AUDIT + resolve display.rs (move or fold)
21. AUDIT + resolve events.rs (move to awareness/ or workspace/)
22. AUDIT + delete dead code in commands.rs (or split/redistribute live functions)
```

### Tier 4 (downstream + cleanup)

```
23. lib.rs pub mod declarations updated for new modules; pub use re-exports added where back-compat needed
24. src-tauri/src/commands/k2so_agents.rs re-exports updated (keep external names stable)
25. k2so-daemon imports updated across affected files (agents_routes.rs, main.rs, triage.rs, heartbeat_launchd_routes.rs)
26. Final validation commit (no code changes — just running tests, doc-test, typecheck for verification)
```

Each commit must compile (`cargo check --workspace`) on its own. After Tier 4 lands, run full `cargo test --release --workspace` + `bun run typecheck` + `bash tests/cli/*.sh` + `cargo test --doc -p k2so-core`.

**Alternative**: bundle into fewer commits if intermediate states are hard to keep green. Subagent has discretion to merge adjacent moves when grouping makes the diff cleaner — but never bundle moves that would split git blame on a file.

---

## Validation strategy

After each commit:
- `cargo check --package k2so-core` (fast)

After commits 5-12 (anything touching downstream):
- `cargo check --workspace` (catches daemon + Tauri impacts)

After all commits land:
- `cargo test --release --workspace` — baseline 878 / 0 failed. Must remain at 878 (or grow if you add tests; net should not decrease unless we deleted dead tests in commit 11).
- `bun run typecheck` — baseline 47. Must NOT grow.
- `bash tests/cli/*.sh` — baseline ~11 passing / 5 sandbox-flaky.
- Manual: `k2so agents list` (should work — internal renames don't affect CLI verb behavior); `k2so heartbeat list`; `k2so checkin`.

---

## Hard rules (CRITICAL)

1. **Build only from worktree's `target/`** — never `cargo install`, never touch production.
2. **`git mv` for all file moves** — preserves git blame. Don't `rm` + `add`.
3. **No `git commit --no-verify`, no `--amend`, no `Co-Authored-By` lines.** Per `memory/feedback_commit_attribution.md`.
4. **Do NOT touch version strings.** Per `memory/feedback_no_version_bump.md`.
5. **Keep public re-export NAMES stable** at the `src-tauri/src/commands/k2so_agents.rs` boundary — external callers see the same symbols, just sourced from new paths.
6. **Tests must fail loudly.** Per `memory/feedback_test_discipline.md`.
7. **Daemon-first** principle unchanged — this refactor only moves code, doesn't change daemon ownership boundaries.
8. **Don't speculatively delete from `agents/commands.rs`** — audit first (commit 11). If a function is referenced anywhere, keep it (moving to its proper home is acceptable; deleting is not).

---

## Open questions / risks

1. **`agents/commands.rs` audit** — Phase 2.1c Item 2 removed work-queue functions but the file still exists at 1151 LoC. Are ALL remaining functions dead? Or only the work-queue ones?
   **Action**: Subagent grep all call sites in commit 11. If file is 100% dead, hard-delete. If mixed, move live functions to their proper home (e.g., agent CRUD → `agents/lifecycle.rs` or wherever fits) + delete dead code.

2. **`agents/mod.rs` `find_primary_agent` / `agent_dir` / `agent_type_for`** — these are workspace's primary agent identity resolution. The audit didn't explicitly cover where they should land. Suggested: `workspace/agent_identity.rs`.
   **Action**: Subagent decides during commit 5 or 12 based on what's cleanest.

3. **Serde-stable paths** — low risk per the audit but worth a quick grep before commit 1: any `#[serde(rename)]` that hardcodes a module path.
   **Action**: Subagent runs `grep -rn '#\[serde(rename' crates/k2so-core/src/agents/` before starting; reports findings.

4. **Macro-generated paths** — any macros that build `use` statements via string interpolation won't be compiler-checked.
   **Action**: Subagent runs `grep -rn 'macro_rules!\|#\[macro\]' crates/k2so-core/src/agents/`; reports findings.

5. **Doctest references** — any `///` doc comments that reference module paths in code examples won't be auto-validated until `cargo test --doc` runs.
   **Action**: After all commits, run `cargo test --doc -p k2so-core` to catch doc-comment drift.

---

## Documentation refresh (low-priority follow-up)

Per the audit's documentation impact list, several files outside k2so-core mention old paths:
- `.k2so/prds/*.md` — historical references to `agents/` module structure
- `CLAUDE.md` / generated SKILL.md templates — may document module layout
- `README.md` / contributing guides
- Inline `///` doc comments in renamed files (subagent should update self-referential paths as part of the move)

**Scope**: subagent updates only the self-referential doc comments inside renamed files. PRD/README drift is a separate cleanup pass — not blocking.

---

## Definition of done

### Tier 1 (originally confirmed)

1. ✅ All 11 Tier 1 file renames executed via `git mv`
2. ✅ `lib.rs` `pub mod` declarations updated for new modules (`workspace/`, `skills/`, `heartbeats/`, `migrations/`)
3. ✅ `src-tauri/src/commands/k2so_agents.rs` re-exports updated with stable external names

### Tier 2 (residual redistribution)

4. ✅ `delegate.rs` audited + relocated to `deprecated/` with `#[deprecated]` annotations OR deleted if zero callers
5. ✅ All 8 Tier 2 files moved to proper homes
6. ✅ `deprecated/` module created if delegate.rs was relocated (not deleted)

### Tier 3 (audits)

7. ✅ `commands.rs` audited; dead code deleted or live functions moved
8. ✅ `work_item.rs` resolved (move or delete)
9. ✅ `display.rs` resolved (move or fold)
10. ✅ `events.rs` resolved (move to awareness/ or workspace/)

### Tier 4 (downstream + cleanup)

11. ✅ All `use` statements across workspace updated (compiler-driven)
12. ✅ `cargo test --release --workspace` baseline preserved (878 / 0 failed or higher; net should not decrease unless dead tests were deleted in audits)
13. ✅ `bun run typecheck` baseline preserved (47 errors, no growth)
14. ✅ `cargo test --doc -p k2so-core` runs clean

### Sanity gates

15. ✅ `grep -rn 'k2so_core::agents::\(skill_writer\|skill_content\|scheduler\|checkin\|wake\|triage_summary\|onboarding\|heartbeat\|heartbeat_install\|unification\|build_launch\|delegate\|launch_profile\|skill\|cron_schedule\|session\|settings\|terminal_id\)' .` returns zero hits (all callers updated)
16. ✅ `agents/` directory contains AT MOST: `workspace.rs` (deferred to 2.5d). If Tier 3 audits all resolve cleanly to "move or delete," agents/ contains only `workspace.rs`. This is the "incidental near-elimination" outcome; it's allowed, not required.

---

## Sequencing within Phase 2 / 3

```
Phase 2.5b (skills folder consolidation) — IN FLIGHT now
                ↓
Phase 2.5c (this PRD: core file rename) — LAUNCHES after 2.5b lands
                ↓
Phase 2.5 (build + install + smoke validation) — LAUNCHES after 2.5c lands, validates final layout
                ↓
Phase 2.6 (tunnel-provider decision) — runs after Phase 2.5 closes
                ↓
Phase 3 (contract hardening)
```

Phase 2.5d (the `agents/workspace.rs` 143 KB split + agents/ residual cleanup) is queued but unscheduled — can land any time after 2.5c without blocking other phases.

---

## Complexity estimate

**Medium** per the research:
- ~290 `use` statement updates (compiler-guided)
- 3 downstream crates affected (k2so-daemon, src-tauri, k2so-core internal)
- ~2-4 hour subagent execution
- Risk profile: LOW for compile correctness, MEDIUM for git blame loss if commits are bundled poorly, LOW for functional correctness (no behavior change)

---

## References

- `.k2so/prds/phase-2.5b-skills-consolidation.md` — Phase 2.5b filesystem consolidation that motivates the source-code parallel
- `.k2so/prds/phase-2.5-validation-and-tunnel-decision.md` — Phase 2.5 validation that runs AFTER 2.5c
- `.k2so/prds/phase-2.1-cli-redesign.md` — Phase 2.1 reframes that made these module names lie
- Memory `project_workspace_agent_addressing` — workspace is the routing primitive
- Memory `project_workspace_agent_invariants` — one primary agent per workspace
- Memory `feedback_subagent_cherry_pick_pattern` — cherry-pick subagent commits rather than merging branches
- Phase 2.5c Explore agent audit (`afdab30fdf6e64be9`, 2026-05-24) — full per-file analysis, cross-crate impact matrix, suggested commit sequencing
