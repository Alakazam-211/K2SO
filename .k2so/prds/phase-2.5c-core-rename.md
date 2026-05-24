# Phase 2.5c: k2so-core file/module rename

**Status**: Drafted 2026-05-24 based on Explore agent audit. Launches AFTER Phase 2.5b (skills folder consolidation) lands and BEFORE Phase 2.5 (build+smoke). Pure refactor — no behavior change, no user-facing impact.
**Internal version markers**: 0.39.0h or 0.39.0i
**Owner**: Rosson + pod-leader
**Date**: 2026-05-24

---

## tl;dr

The k2so-core Rust module layout still reflects the pre-cleanup mental model. Files like `agents/skill_writer.rs` write skill templates (not agent things); `agents/scheduler.rs` triages the workspace inbox (not anything agent-specific); `agents/checkin.rs` is workspace-scoped agent self-report. The naming lies about contents.

After Phase 2.1 (workspace==agent invariant, inbox-as-email, `__lead__` removal, skills-as-documentation reframe) and Phase 2.5b (filesystem `.k2so/skills/` consolidation), the conceptual model has settled. Phase 2.5c aligns module names with that model.

**13 file moves** across 4 new module homes (`workspace/`, `skills/`, `heartbeats/`, `migrations/`) + dead-code audit of `agents/commands.rs`. Rust's strict module system means the compiler catches every broken `use` statement — risk is low; ~290 `use` updates across the workspace; ~2-4 hour execution.

Goal: new contributors see module names that match what the code does; PRD-level documentation can describe the system in one consistent vocabulary; the `.k2so/` filesystem layout (post-2.5b: `agent/` + `skills/` + `heartbeats/` + `inbox/`) mirrors the code layout (`workspace/` + `skills/` + `heartbeats/` + `inbox.rs`).

---

## Confirmed renames (13)

Per the audit's per-file analysis:

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
| `agents/commands.rs` | **AUDIT + likely DELETE** | Phase 2.1c removed work-queue functions; audit for remaining live callers |
| **NEW** `agents/awareness/` | NO MOVE | Already at top-level `awareness/` per `lib.rs` (initial list was wrong) |

---

## Pushbacks (do NOT move)

The audit validated my initial list and flagged two pushbacks:

| File | Why it stays |
|---|---|
| `agents/delegate.rs` | Semantically correct — delegates work TO a named agent in a dedicated worktree. The `target_agent` param means this IS agent-specific even though invoked from workspace context. |
| `agents/launch_profile.rs` | Resolves per-agent launch shape (cwd, entrypoint, env). Used by workspace-level scheduler but the data structure is agent-scoped. |

Plus thin utilities that don't need moving (correctly scoped at agents/, no workspace-vs-agent confusion):
`work_item.rs`, `skill.rs`, `cron_schedule.rs`, `display.rs`, `events.rs`, `session.rs`, `settings.rs`, `terminal_id.rs`

---

## Out of scope (deferred to Phase 2.5d or later)

- **`agents/workspace.rs` (143 KB)** — misplaced (it's workspace-scoped, not agent-scoped) but too large to safely move in one commit. Git blame would split. **Defer**: audit internal structure first; consider splitting into smaller units; revisit in a dedicated Phase 2.5d after Phase 2.5 (build+smoke) confirms the layout is stable.
- **Internal `agents/mod.rs` reorg** — after Phase 2.5c lands, the agents/ module shrinks substantially. A second pass might reorganize what remains (e.g., grouping by lifecycle vs identity vs adapters). Defer to a follow-up cleanup pass.

---

## End-state module layout

```
crates/k2so-core/src/
├── awareness/         # Cross-workspace signal bus (ALREADY here)
├── workspace/         # NEW: workspace lifecycle, scheduler, checkin, wake, triage, onboarding, launch
│   ├── mod.rs
│   ├── scheduler.rs
│   ├── checkin.rs
│   ├── wake_prompts.rs
│   ├── triage.rs
│   ├── onboarding.rs
│   └── agent_launch.rs
├── skills/            # NEW: skill profile writers + content generators (post-2.5b consolidation lives here)
│   ├── mod.rs         (existing — post-2.5b consolidation.rs)
│   ├── consolidation.rs  (from Phase 2.5b)
│   ├── writer.rs
│   └── content.rs
├── heartbeats/        # NEW: workspace heartbeat schedules + launchd plist install
│   ├── mod.rs         (was agents/heartbeat.rs)
│   └── install.rs     (was agents/heartbeat_install.rs)
├── inbox.rs           # Workspace inbox (Phase 2.1a — already here)
├── migrations/        # NEW: historical migration helpers
│   └── unification_0_37_0.rs
└── agents/            # Significantly smaller post-2.5c
    ├── mod.rs         (agent identity helpers if any remain — likely move to workspace/agent_identity.rs)
    ├── delegate.rs    (stays — semantically correct)
    ├── launch_profile.rs (stays — agent-scoped data)
    ├── work_item.rs
    ├── skill.rs
    ├── cron_schedule.rs
    ├── display.rs
    ├── events.rs
    ├── session.rs
    ├── settings.rs
    └── terminal_id.rs
```

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

## Commit strategy (10 commits, single-rename-per-commit for git blame)

```
commit 1: Create skills/ module + move agents/skill_writer.rs → skills/writer.rs
commit 2: Move agents/skill_content.rs → skills/content.rs
commit 3: Create heartbeats/ module + move agents/heartbeat.rs → heartbeats/mod.rs
commit 4: Move agents/heartbeat_install.rs → heartbeats/install.rs
commit 5: Create workspace/ module + move agents/scheduler.rs → workspace/scheduler.rs
commit 6: Move agents/checkin.rs → workspace/checkin.rs
commit 7: Move agents/wake.rs → workspace/wake_prompts.rs
commit 8: Move agents/triage_summary.rs → workspace/triage.rs + agents/onboarding.rs → workspace/onboarding.rs (tightly related)
commit 9: Move agents/build_launch.rs → workspace/agent_launch.rs
commit 10: Create migrations/ module + move agents/unification.rs → migrations/unification_0_37_0.rs
commit 11: AUDIT + DELETE dead code in agents/commands.rs (or move live callers if any)
commit 12: lib.rs pub mod declarations + src-tauri re-exports + k2so-daemon imports cleanup (catch any stragglers)
```

Each commit must compile (`cargo check --workspace`) on its own. After commit 12, run full `cargo test --workspace` + `bun run typecheck` + `bash tests/cli/*.sh`.

**Alternative**: bundle into fewer commits if the subagent finds it's hard to keep intermediate states green. Single coordinated commit is acceptable if compilation stays green throughout.

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

1. ✅ All 13 confirmed renames executed via `git mv`
2. ✅ `commands.rs` audited; dead code deleted or live functions moved
3. ✅ `lib.rs` `pub mod` declarations updated for new modules
4. ✅ `src-tauri/src/commands/k2so_agents.rs` re-exports updated with stable external names
5. ✅ All `use` statements across workspace updated (compiler-driven)
6. ✅ `cargo test --release --workspace` baseline preserved (878 / 0 failed or higher)
7. ✅ `bun run typecheck` baseline preserved (47 errors, no growth)
8. ✅ `cargo test --doc -p k2so-core` runs clean
9. ✅ Sanity grep: `grep -rn 'k2so_core::agents::\(skill_writer\|skill_content\|scheduler\|checkin\|wake\|triage_summary\|onboarding\|heartbeat\|heartbeat_install\|unification\|build_launch\)' .` returns zero hits (all callers updated)

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
