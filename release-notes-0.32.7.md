## 0.32.7 — One SKILL.md to rule them all

Every CLI LLM now sees the same canonical workspace context. This release unifies the five-plus scattered files K2SO used to maintain into a single `SKILL.md` that fans out to every discovery path via symlinks — `./CLAUDE.md`, `./GEMINI.md`, `./AGENT.md` (Code Puppy), `./.goosehints`, `./.cursor/rules/k2so.mdc`, `./AGENTS.md`, `./.github/copilot-instructions.md`, and `./.aider.conf.yml` all resolve to the same composed body. Users edit exactly three file types; K2SO composes one; twelve CLI LLMs pick it up. When a user adds a workspace, pre-existing CLAUDE.md/GEMINI.md/etc. are archived (never deleted) and their bodies imported into SKILL.md so nothing is lost. When they disconnect, they pick between keeping the frozen snapshot or restoring originals.

### Knowledge architecture — three files in, one file out

The user-editable surface collapses to three file types:

- **`AGENT.md`** — per-agent persona + standing orders. Path: `.k2so/agents/<name>/AGENT.md`.
- **`PROJECT.md`** — workspace-scope codebase knowledge shared by every agent. Path: `.k2so/PROJECT.md`.
- **`WAKEUP.md`** — per-heartbeat trigger message. Path: `.k2so/agents/<agent>/heartbeats/<schedule>/WAKEUP.md`.

K2SO composes all three plus its own managed layers into one canonical `.k2so/skills/k2so/SKILL.md`. Symlinks at every harness discovery path point to it. Per-agent CLAUDE.md files are retired (regeneratable via `k2so agents generate-md` on demand); their former content now flows via `--append-system-prompt` at launch.

### Filenames standardized to UPPERCASE

`AGENT.md` and `WAKEUP.md` match the ecosystem convention (`CLAUDE.md`, `AGENTS.md`, `CONVENTIONS.md`) and signal "this is a schema file" at a glance. A startup migration renames any existing lowercase `agent.md` / `wakeup.md` on disk using a case-insensitive-filesystem-safe two-step pattern (macOS HFS+/APFS) so the renames don't no-op. The DB's `agent_heartbeats.wakeup_path` column is updated in the same pass. A one-release read-time shim still accepts lowercase names so partial migrations don't lose context.

### Drift adoption — agents can write back to SKILL.md

Claude Code's `# memory` feature writes into whatever CLAUDE.md it discovers. Since `./CLAUDE.md` is now a symlink to the canonical, those writes land in `SKILL.md`'s tail. New sub-region markers below the managed block (`K2SO:SOURCE:PROJECT_MD:BEGIN/END` and `K2SO:SOURCE:AGENT_MD name=<agent>:BEGIN/END`) track which portion came from which source file. On each regen, K2SO parses the sub-regions, diffs against the source files, and adopts drift back:

- If `PROJECT.md`'s body in `SKILL.md` has diverged from `.k2so/PROJECT.md`, the diverged body is committed back to `PROJECT.md`.
- Same for the primary agent's `AGENT.md`.
- An mtime guard against `.k2so/.last-skill-regen` resolves three-way conflicts: source-file edits (via the GUI) win over SKILL.md-side drift, and the conflict is logged to `.k2so/logs/adoption-conflicts.log`.

Freeform content a user or agent drops below the `<!-- K2SO:USER_NOTES -->` sentinel is preserved verbatim across regens — never adopted, never stripped.

### Safe first-run migration (archive, never destroy)

When a workspace is added or upgraded:

- **Root `./CLAUDE.md`** is archived to `.k2so/migration/CLAUDE.md-<timestamp>.md` before any mutation.
- **Per-agent `.k2so/agents/<n>/CLAUDE.md`** files are archived to `.k2so/migration/agents/<n>/CLAUDE.md-<timestamp>.md`. Gated by `.k2so/.harvest-0.32.7-done` sentinel so files the user later regenerates via `k2so agents generate-md` aren't re-harvested on next boot.
- **Pre-existing CLAUDE.md / GEMINI.md / root AGENT.md / .goosehints / .cursor/rules/k2so.mdc** bodies are imported into `SKILL.md`'s `<!-- K2SO:USER_NOTES -->` tail so Claude/Gemini/etc. still see the user's accumulated memory through the symlink — nothing disappears from view.
- **`.aider.conf.yml`** is merged, not clobbered. K2SO injects `- SKILL.md` into the existing `read:` list, preserves every other top-level key (`model:`, `auto-lint:`, etc.), and archives the original first.
- Idempotent re-imports via `<!-- K2SO:IMPORT:CLAUDE_MD archive=<path> -->` sentinels keyed on archive path — the same archive never imports twice.

A standalone `.k2so/MIGRATION-0.32.7.md` lists every archive location and instructions for the user to review, prune duplicates, or move content into PROJECT.md / AGENT.md.

### Workspace teardown — disconnect cleanly

New `teardown_workspace_harness_files` + Tauri command + CLI surface for graceful disconnect:

```
k2so workspace remove <path> --mode keep-current      # freeze current SKILL.md into each harness file
k2so workspace remove <path> --mode restore-original  # revert each file from .k2so/migration/
k2so workspace remove <path>                          # DB-only deregister; files untouched (old behavior)
```

- **`keep-current`**: every symlinked file (`CLAUDE.md`, `GEMINI.md`, root `AGENT.md`, `.goosehints`, root `SKILL.md`, `.cursor/rules/k2so.mdc`) becomes a real file holding the current canonical SKILL.md body. All CLIs keep working with context frozen at disconnect time.
- **`restore-original`**: each harness file is restored byte-for-byte from its `.k2so/migration/*` archive. Files K2SO created fresh (no prior version) are removed cleanly. `.aider.conf.yml` is restored from archive too.
- **`.k2so/` is never touched by teardown.** Archives, canonical SKILL.md, adoption log, sentinels, and the user's PROJECT.md / AGENT.md files all survive. Reconnect is idempotent — sentinels gate re-imports, the harness-discovery logic sees existing symlinks and just refreshes.

### Extended harness coverage

File-discovery now reaches every CLI LLM K2SO supports:

- `./CLAUDE.md` → symlink (Claude Code)
- `./GEMINI.md` → symlink (Gemini CLI)
- `./AGENT.md` → symlink (Code Puppy, workspace-root variant)
- `./.goosehints` → symlink (Goose, plain-text convention)
- `./.cursor/rules/k2so.mdc` → generated MDC with `alwaysApply: true` frontmatter (Cursor requires this format; can't be a symlink). Self-identifying via `k2so_generated: true` frontmatter key so re-runs don't self-re-archive.
- `./AGENTS.md` → marker-injected between `<!-- K2SO:BEGIN -->` / `<!-- K2SO:END -->` (Codex / OpenCode / Pi native)
- `./.github/copilot-instructions.md` → marker-injected (GitHub Copilot)
- `./.aider.conf.yml` → `read: [SKILL.md]` scaffolded or merged (Aider)
- `./.claude/skills/k2so/SKILL.md` → symlink to canonical
- `./.opencode/agent/k2so.md` → symlink to canonical
- `./.pi/skills/k2so/SKILL.md` → symlink to canonical
- `./SKILL.md` → symlink to canonical (generic)

OpenCode's `.opencode/agent/*.md` files beyond K2SO's own `k2so.md` are untouched — no collision. Cursor's project-specific `.cursor/rules/*.mdc` files beyond our `k2so.mdc` are untouched.

### UI revisions

- **Agent Skills diagram** redrawn for the three-author / one-derived model. Left column: three user-editable file types. Middle: single canonical `SKILL.md` box with "all 12 CLI LLMs see this" subtitle. Right: two delivery channels (file discovery + argv injection) with enumerated reach lists.
- **Agent Skills Settings page**: new **K2SO Agent** tab alongside Manager / Agent Template / Custom Agent. Eight auto layers documented (Identity, Every Wake, Report + Complete, Planning, Heartbeats, Cross-Workspace Messaging, File Reservations, Settings + Diagnostic).
- **All UI copy** updated from `agent.md` / `wakeup.md` to `AGENT.md` / `WAKEUP.md` across Projects, Heartbeats, Agent Persona Editor, Context Layers Preview, AgentPane.

### Retired

- **Per-agent CLAUDE.md auto-writes**. The file-write was redundant with `--append-system-prompt`. User-facing `k2so agents generate-md <agent>` still emits the file on demand (useful for inspection / preview in the UI). Pre-existing files are archived on first boot.
- **Workspace-level `CLAUDE.md` as a generator artifact**. It's now a symlink to canonical SKILL.md. K2SO-generated files (detected by `# K2SO ` header) are archived and replaced; user-authored CLAUDE.md files are also archived but their bodies imported into USER_NOTES before takeover, so nothing is buried.

### Fixed

- **Tail stacking in SKILL.md** when `strip_workspace_skill_tail` used `find()` instead of `rfind()`. Multiple regens accumulated duplicate USER_NOTES sentinels and placeholder comments until the file grew unboundedly. `rfind` collapses stacked sentinels and the placeholder constant is filtered out during preserve, so imports never double.
- **Archive filename extensions** were hardcoded to `.md`, breaking restore for `.aider.conf.yml`, `.goosehints`, and `.cursor/rules/*.mdc`. `archive_claude_md_file` now preserves the original extension.
- **Cursor MDC infinite-archive loop**: the regenerator's body check saw imports stacking in canonical SKILL.md as "user edits" and re-archived its own output. Self-identifying sentinel (`k2so_generated: true`) now short-circuits the check.

### Tests

**New: 17 Rust integration tests** (`migration_safety_tests` module) simulating real user scenarios across all six collision-prone harness files:

- `archive_claude_md_never_deletes_source` — archive copies, never renames / moves
- `harvest_per_agent_claude_md_archives_then_removes_source` — per-agent cleanup
- `harvest_is_idempotent_even_if_file_regenerated_later` — sentinel gates re-runs
- `strip_tail_preserves_user_freeform_but_discards_placeholders` — no stacking
- `strip_tail_returns_none_when_tail_is_empty_or_placeholder_only` — noise suppression
- `migration_banner_is_idempotent_and_appends_new_archives` — banner accumulates phases correctly
- `safe_symlink_archives_existing_regular_file` — archive before symlink
- `safe_symlink_is_idempotent_when_target_is_already_symlink` — re-runs refresh, don't re-archive
- `workspace_remove_then_readd_leaves_data_intact` — reconnect is lossless
- `import_claude_md_lands_in_user_notes_and_is_idempotent` — sentinel-gated imports
- `add_workspace_ingests_all_harness_files_into_skill_and_archives` — full mock: Aider + OpenCode + Gemini + Goose + Code Puppy + Cursor + Claude all imported in one pass
- `add_workspace_is_idempotent_second_launch_imports_nothing_new` — no re-import on second boot
- `teardown_keep_current_freezes_symlinks_into_real_files` — frozen snapshot mode works for every harness
- `teardown_restore_original_brings_back_every_archive` — byte-for-byte restore for every harness
- `reconnect_after_restore_original_reingests_cleanly` — full lifecycle (add → restore → re-add)
- `teardown_leaves_k2so_dir_fully_intact` — `.k2so/` invariant upheld
- `aider_conf_merge_preserves_user_reads_and_archives_original` — YAML merge safety

**New: 46 tier3 source-grep assertions** covering UPPERCASE filename migration, SOURCE region markers, adoption sweep, harvest gating, banner idempotency, teardown modes, harness writer coverage, Agent Context Diagram refresh.

**Updated: tier1 tests** rewritten for DB-backed session state. Previous tests checked for the retired `.last_session` file; now query `agent_sessions.session_id` via sqlite3 directly so `heartbeat noop` transcript pruning stays verified.

**Stable**: 111/111 CLI integration tests, 42/42 cargo unit tests (25 pre-existing + 17 new).

### Migration notes for existing users

- On first boot after upgrade, K2SO will:
  - Rename lowercase `.k2so/agents/*/agent.md` → `AGENT.md` and `wakeup.md` → `WAKEUP.md` (safe on case-insensitive filesystems).
  - Archive any per-agent `CLAUDE.md` files left behind by the pre-0.32.7 generator.
  - Archive root `./CLAUDE.md` + any pre-existing `./GEMINI.md` / `./AGENT.md` / `./.goosehints` / `./.cursor/rules/k2so.mdc` and replace with symlinks pointing at the new canonical SKILL.md.
  - Merge `SKILL.md` into existing `.aider.conf.yml` `read:` list (or scaffold one if missing).
  - Import each archived body into SKILL.md's `<!-- K2SO:USER_NOTES -->` tail so nothing is invisible.
  - Write `.k2so/MIGRATION-0.32.7.md` with a list of every archive location.
- Everything in `.k2so/migration/` is recoverable — nothing is destroyed.
- `.gitignore` additions: `.k2so/migration/`, `.k2so/logs/`, `.k2so/.last-skill-regen`, `CLAUDE.md`, `GEMINI.md`, `/AGENT.md` (root only, not per-agent), `.goosehints`, `.cursor/rules/k2so.mdc`. Tracking the symlinks in git would create cross-machine pointer issues; each dev's K2SO regenerates them locally.

### Out of scope / deferred to 0.32.8

- **Add-workspace explanation card** in the UI describing what K2SO will do to the filesystem. Backend is ready; UI surface is a focused follow-up.
- **Remove-workspace confirmation dialog** with radio buttons for `keep-current` / `restore-original`. CLI already has `--mode`; UI dialog can layer on top of the existing Tauri command.
- **User-tier preferences** (`~/.k2so/AGENT.md` for cross-workspace persona defaults). Research recommended but the contract can wait.
- **Ollama Modelfile + Open Interpreter profile YAML** per-session generation. The other 10 CLIs are covered by file-discovery; these two need per-launch argv injection.
- **Typed-markdown frontmatter** (`type: always | trigger | task`) on source files. Format kept open for a future release if triggers / tasks grow into first-class concepts.
