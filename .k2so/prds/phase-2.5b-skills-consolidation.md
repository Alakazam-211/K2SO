# Phase 2.5b: Skills folder consolidation + Settings UI rename

**Status**: Drafted 2026-05-24. Launches after Phase 2.5 (build+smoke / test migration / CI sweep) closes and Phase 2.1 final-final (`__lead__` rename, task #540) lands. Runs BEFORE Phase 2.6 (tunnel-provider decision) because the user explicitly wants the skill model fully closed before tackling monetization infrastructure.
**Internal version markers**: 0.39.0g or 0.39.0h
**Owner**: Rosson + pod-leader
**Date**: 2026-05-24

---

## tl;dr

Three folders in `.k2so/` are different costumes for the same concept (documented capability profiles for the workspace's agent):

- `.k2so/agents/<name>/SKILL.md` — instantiated skill (the workspace's customized profile)
- `.k2so/agent-templates/<role>/SKILL.md` — master template (seed for new skills)
- `.k2so/skills/<name>.md` — Unit 6 skill_layers (capability layers; bare markdown)

Per Phase 2.1's skill reframe (A19): all three are documentation profiles the harness loads. The template-vs-instance distinction was useful in the multi-agent era when you stamped copies; under "skills are documentation," any skill can serve as a starting point for a new one.

Phase 2.5b consolidates all three into `.k2so/skills/<name>/SKILL.md` and renames the Settings UI section from "Agents" to "Skills" in the same coordinated change. After this lands:
- `.k2so/agents/` and `.k2so/agent-templates/` are retired (trashed via `safe_delete::trash`)
- `.k2so/skills/<name>/SKILL.md` is the single home for every capability profile in the workspace
- Settings page shows "Skills" with AIFileEditor targeting each skill's SKILL.md

The user's exact direction (2026-05-24): "both the agents and agent-templates technically land in the skill/ folder. The agent/ folder becomes the canonnical agent that is the workspace."

---

## What's locked

- **`.k2so/agent/` stays singular** — the workspace's ONE primary agent (`.k2so/agent/AGENT.md` for identity/persona). Unchanged by this phase.
- **`.k2so/skills/` becomes the unified home** for all capability profile documentation
- **Three sources merge in priority order** (collision rule): instance (`agents/`) > template (`agent-templates/`) > existing layer (bare-md `skills/`)
- **Shape normalization**: bare-md `skills/<file>.md` files become folder-with-SKILL.md (`skills/<file>/SKILL.md`) so every skill has the same shape
- **Settings UI section renamed**: "Agents" → "Skills"
- **AIFileEditor** targets each skill's SKILL.md (no other UX change to the editor)

---

## Daemon migration (single first-boot hook, idempotent)

Hook into the existing `run_workspace_legacy_migrations_sweep` (same pattern as `migrate_work_to_inbox`).

```rust
// crates/k2so-core/src/skills/consolidation.rs (new module)

pub fn consolidate_skills_v1(workspace: &Path) -> Result<ConsolidationOutcome> {
    let marker = workspace.join(".k2so/.skills-consolidation-v1-done");
    if marker.exists() { return Ok(ConsolidationOutcome::AlreadyDone); }

    let agents_dir = workspace.join(".k2so/agents");
    let templates_dir = workspace.join(".k2so/agent-templates");
    let skills_dir = workspace.join(".k2so/skills");

    fs::create_dir_all(&skills_dir)?;

    // Step 1: normalize existing bare-md skill_layers files
    //   .k2so/skills/git.md → .k2so/skills/git/SKILL.md
    for entry in fs::read_dir(&skills_dir)? {
        let path = entry?.path();
        if path.is_file() && path.extension() == Some("md".as_ref()) {
            let stem = path.file_stem().unwrap().to_string_lossy().to_string();
            let target_dir = skills_dir.join(&stem);
            fs::create_dir_all(&target_dir)?;
            fs::rename(&path, target_dir.join("SKILL.md"))?;
        }
    }

    // Step 2: move instances (highest priority) — they always win
    if agents_dir.exists() {
        for entry in fs::read_dir(&agents_dir)? {
            let src = entry?.path();
            if !src.is_dir() { continue; }
            let name = src.file_name().unwrap();
            let dest = skills_dir.join(name);
            // Instance wins: if `skills/<name>/` already exists from step 1,
            // overwrite with the agent's content (the layer was a stub).
            move_dir_recursive(&src, &dest, MergePolicy::SourceWins)?;
        }
    }

    // Step 3: move templates (lower priority — apply suffix on collision)
    if templates_dir.exists() {
        for entry in fs::read_dir(&templates_dir)? {
            let src = entry?.path();
            if !src.is_dir() { continue; }
            let name = src.file_name().unwrap();
            let mut dest_name = name.to_owned();
            let mut suffix = 1;
            while skills_dir.join(&dest_name).exists() {
                dest_name = OsString::from(format!("{}-template{:02}",
                    name.to_string_lossy(), suffix));
                suffix += 1;
            }
            let dest = skills_dir.join(&dest_name);
            move_dir_recursive(&src, &dest, MergePolicy::Move)?;
        }
    }

    // Step 4: trash the source folders (recoverable via macOS Recycle Bin)
    if agents_dir.exists() {
        k2so_core::safe_delete::trash(&agents_dir)?;
    }
    if templates_dir.exists() {
        k2so_core::safe_delete::trash(&templates_dir)?;
    }

    // Step 5: marker
    fs::write(&marker, "v1")?;
    Ok(ConsolidationOutcome::Migrated { moved: ..., suffixed: ... })
}
```

**Collision examples**:
- `agents/frontend-eng/SKILL.md` + `agent-templates/frontend-eng/SKILL.md` → `skills/frontend-eng/SKILL.md` (instance wins) + `skills/frontend-eng-template01/SKILL.md`
- `skills/git.md` (bare-md layer) + `agents/git-helper/` → `skills/git/SKILL.md` (normalized layer) + `skills/git-helper/SKILL.md` (no collision)
- `agents/foo/` + `agents/foo/` (impossible — same source, unique names already)

---

## Code changes (the real engineering after the migration helper)

### A. `skill_writer.rs` — generated SKILL.md templates

Templates currently reference `.k2so/agents/<name>/` paths. Update to `.k2so/skills/<name>/`. Verify the template's `where you live` instruction lines are consistent with the new home.

### B. `k2so-core::skills` (Unit 6 skill_layers reader)

Today's reader walks `.k2so/skills/*.md` (bare files). After migration, every skill is `.k2so/skills/<name>/SKILL.md`. Update the reader to walk folders + read SKILL.md from each. Drop the bare-file handling (or keep as one-shot back-compat for users who haven't yet first-boot-migrated — recommended).

### C. CLI: `k2so skills create --template <name>` semantics shift

Today (post-Phase-2.1): create from `.k2so/agent-templates/<name>/`. Post-2.5b: copy from any existing `.k2so/skills/<name>/` as a starting point. The `--template` flag still works; it just means "use this skill as the seed." Update help text.

### D. Daemon routes

- `/cli/skills/list` — return folders from `.k2so/skills/` (consistent shape across migration)
- `/cli/skills/profile?name=X` — read `.k2so/skills/X/SKILL.md`
- Any route that references the legacy `agent-templates` namespace gets retired

### E. Tauri commands

- `k2so_skills_list(projectPath)` — wraps daemon route
- `k2so_skills_profile(projectPath, name)` — wraps daemon route  
- `k2so_skills_create(projectPath, name, sourceSkill?)` — wraps creation flow
- Legacy `k2so_agent_templates_*` commands deleted

### F. Settings UI

- React Settings page section "Agents" → "Skills"
- List rows enumerate `.k2so/skills/<name>/` folders
- Each row has AIFileEditor pointed at `<workspace>/.k2so/skills/<name>/SKILL.md`
- "Create" button → "Create Skill" — calls `k2so_skills_create` with optional `--from <existing-skill>` dropdown
- Empty state: "No skills yet. Create one to give your agent a specialized capability profile."
- Tooltip on section header (transitional): none needed if migration runs at first boot — by the time the user clicks Settings, `.k2so/skills/` is the live state

---

## Tests

### Migration parity tests
- `tests/cli/skills_consolidation_first_boot.sh` — seed sandbox workspace with `.k2so/agents/foo/SKILL.md` + `.k2so/agent-templates/bar/SKILL.md` + `.k2so/skills/baz.md`; restart daemon; verify `.k2so/skills/{foo,bar,baz}/SKILL.md` exist; verify `.k2so/agents/` + `.k2so/agent-templates/` are trashed; verify marker file present
- `tests/cli/skills_consolidation_collision_handling.sh` — seed `.k2so/agents/frontend-eng/` AND `.k2so/agent-templates/frontend-eng/`; verify instance wins (`skills/frontend-eng/` is from agents/) and template gets suffix (`skills/frontend-eng-template01/`)
- `tests/cli/skills_consolidation_idempotent.sh` — run migration twice; assert second run is no-op (marker check)

### Rust unit tests
- `move_dir_recursive` with `MergePolicy::SourceWins` — overwrites destination
- `move_dir_recursive` with `MergePolicy::Move` — fails on collision (so collision-suffix logic runs explicitly)
- Bare-md normalization — `git.md` → `git/SKILL.md`

### Frontend tests
- `bun run typecheck` — must stay at baseline (47 errors, no new)
- Manual UI smoke: open Settings, see "Skills" section, click row, AIFileEditor opens against the skill's SKILL.md

---

## Hard rules (CRITICAL)

1. **Build only from worktree's `target/`** — never `cargo install`, never touch production.
2. **Sandbox-only smoke** — `HOME=$SANDBOX` for tests that exercise migration.
3. **No `git commit --no-verify`, no `--amend`, no `Co-Authored-By` lines.**
4. **Do NOT touch version strings.**
5. **Inline POST method-gate** on any new daemon POST routes.
6. **`safe_delete::trash` for retirement** — never `fs::remove_dir_all`. Per `memory/feedback_recycle_bin_tests.md`.
7. **Tests must fail loudly.** Per `memory/feedback_test_discipline.md`.
8. **Daemon-first.** Per `memory/feedback_daemon_first.md`.

---

## Definition of done

1. ✅ `.k2so/agents/` + `.k2so/agent-templates/` retired (trashed)
2. ✅ `.k2so/skills/<name>/SKILL.md` is the single home for capability profiles
3. ✅ Settings UI shows "Skills" section, AIFileEditor targets SKILL.md per row
4. ✅ `skill_writer.rs` generated templates reference the new paths
5. ✅ Unit 6 skill_layers reader walks the new folder-with-SKILL.md shape
6. ✅ CLI `k2so skills create --template <name>` works against the new model
7. ✅ Daemon first-boot migration is idempotent + marker'd
8. ✅ `cargo test --release --workspace` baseline preserved or grown
9. ✅ `bun run typecheck` baseline preserved
10. ✅ All new bash tests pass

---

## Sequencing within the phase

Single coordinated commit recommended (daemon migration + readers + CLI + UI in one PR). Splitting across phases risks user-visible inconsistency (CLI says "skill" but UI says "agent" or vice versa).

Subagent runs the order:
1. Add `consolidate_skills_v1` helper + Rust unit tests
2. Wire into `run_workspace_legacy_migrations_sweep`
3. Update `skill_writer.rs` templates
4. Update skill_layers reader for the new shape
5. Update CLI verbs (skills create --template semantics)
6. Add/update Tauri commands
7. Migrate Settings UI section (rename + repoint AIFileEditor)
8. Add bash parity tests
9. Sandbox smoke verification

Estimated effort: ~600-900 LoC across Rust + TS, ~60-90 min subagent run.

---

## Out of scope (explicit non-goals)

- **Renaming `.k2so/skill-layers/`** — that was the original A19.2 plan to free the `.k2so/skills/` namespace; the user's simpler proposal collapses everything INTO `.k2so/skills/` directly, so this is moot.
- **Changing the workspace's primary agent location** — `.k2so/agent/` stays as-is.
- **`k2so delegate` revival** — still hard-deprecated per Phase 2.1; harness owns spawn.
- **DB schema changes** — skills are filesystem-only; no DB table needed (unless a future skill registry wants one, which is Phase 3+).

---

## Open questions

1. **Should the migration also rename `AGENT.md` files in legacy `.k2so/agents/<name>/` to `SKILL.md`?** Some sub-agents may have used `AGENT.md` historically. Verify by inspection — if found, the migration should normalize them too (`AGENT.md` → `SKILL.md` for non-primary roles).
2. **What does the Settings "Create Skill from Template" UX look like?** Dropdown of existing skills as "seeds"? Or just "blank skill" + user copies content manually? Recommend: dropdown of existing skills (any skill can seed any other), with "Blank" as the default.
3. **Per-skill heartbeats?** Today skills have heartbeats nested under `.k2so/agents/<name>/heartbeats/`. Decision: those heartbeats become "for the workspace's primary agent, scheduled with this skill context loaded"? Or do they retire entirely? Probably retire (heartbeats are workspace-scoped per Phase 2.1; skill-bound heartbeats were a per-sub-agent concept). Verify before deletion.

---

## References

- Phase 2.1 PRD (`.k2so/prds/phase-2.1-cli-redesign.md`) — A19 skill reframe, A19.2 deferred filesystem rename (this phase supersedes the three-step shuffle with a simpler consolidate-into-one-folder approach)
- `.k2so/prds/phase-2.5-validation-and-tunnel-decision.md` — Phase 2.5 predecessor (build+smoke)
- `.k2so/prds/phase-2.6-tunnel-decision.md` — runs AFTER Phase 2.5b per user direction (skill model fully closed before monetization decisions)
- Memory: `project_workspace_agent_addressing` — workspace identity is the routing key, agent name is display
- Memory: `project_workspace_agent_invariants` — one primary agent per workspace
- Memory: `feedback_recycle_bin_tests` — `safe_delete::trash` for retirement
- User direction (2026-05-24): "we could just migrate all of the files/folders from those two tables and put them into the skills/ folder and then if there are any files/folders with name collisions just add a 01, 02, 03 to the end as the collisions repeat. Then the agents/ and agent-templates/ folder would be completely retired."
