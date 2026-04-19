## 0.32.8 — Transparent add/remove workspace

This release finishes the UX loop around the knowledge architecture refactor from 0.32.7. When a user adds a workspace, they now see exactly what K2SO will do to their filesystem — a preview of which CLAUDE.md / GEMINI.md / .aider.conf.yml will be archived and merged into the canonical SKILL.md, which files K2SO will create fresh, and a "Why does K2SO do this?" expander that explains the multi-LLM context-sharing rationale. When they disconnect, they pick between keeping the current merged context (so every CLI keeps working) or restoring pre-K2SO files from the archive. Nothing K2SO touches is silent anymore.

### Added

- **Add Workspace dialog**. Fires before `projects_add_from_path` on the "Add Workspace" click. Shows a compact preview with per-file action badges (Archive + Import / Refresh / Create / Marker block), size hints, and a "Why does K2SO do this?" expander explaining that each CLI LLM reads a different file (Claude → CLAUDE.md, Gemini → GEMINI.md, Aider → .aider.conf.yml, Cursor → .cursor/rules/*.mdc, Code Puppy → AGENT.md, Goose → .goosehints, etc.) and K2SO maintains a single canonical SKILL.md that fans out via symlinks. On confirm, the workspace is added AND the skill write runs immediately so the user sees the effect without restarting the app.
- **Remove Workspace dialog**. Replaces the direct `removeProject` call on the context-menu "Remove Workspace" item. Three radio options:
  - **Deregister only (default)** — DB-only delete, filesystem untouched. Re-adding later picks up where it left off.
  - **Keep current context** — replace each symlinked file (CLAUDE.md, GEMINI.md, root AGENT.md, .goosehints, SKILL.md, .cursor/rules/k2so.mdc) with a real file containing the current canonical SKILL.md body. Every CLI keeps working with your K2SO-evolved context, frozen at disconnect time.
  - **Restore pre-K2SO state** — copy each harness file back from `.k2so/migration/*`; files K2SO created fresh are removed cleanly. `.k2so/` is preserved across both modes — archives, canonical, and source files stay — so reconnect later is idempotent.
  After teardown, shows the per-file result list ("froze", "restored", "removed") with notes so the user sees exactly what happened.
- **Tauri commands backing the UI**:
  - `k2so_agents_preview_workspace_ingest(path)` — read-only inspection, returns a structured entry list describing what K2SO would do.
  - `k2so_agents_run_workspace_ingest(path)` — triggers the harvester + workspace skill write on demand (used after Add-Workspace confirm).
- **`AGENTS.md`, `.github/copilot-instructions.md`** added to `.gitignore` in this repo. These are K2SO-maintained marker-injected files — tracking them was noise.

### Fixed

- **Removed a stale per-agent CLAUDE.md write** in the launch-in-project-root path (`k2so_agents.rs` case 3). It was dropping `.k2so/agents/<agent>/CLAUDE.md` side-effects every launch; the content was already flowing via `--append-system-prompt`, so the file was pure garbage. Explicit regeneration paths (`k2so agents generate-md` + the "Regenerate CLAUDE.md" UI button) still write on demand.
- **Tier3 stale assertion** expecting a "Heartbeats" Settings nav entry. Heartbeats were moved to the Workspace panel's right aside in 0.32.6; the assertion now verifies `HeartbeatsPanel` is exported from its section and consumed from the workspace surface, matching current reality.

### Tests

- **14 new tier3 assertions** (section 3.19) pinning the UI + backend wiring: both dialog components + stores exist, `preview`/`run-ingest` Tauri commands registered, the "Why?" explanation present, all three teardown modes exposed, IconRail + Sidebar route through the dialogs (not direct `removeProject`).
- **Tier 3: 329 passed, 0 failed.** First time the suite has been fully clean since the Phase 2 heartbeats nav rework in 0.32.6.
- Tier 1 / CLI integration / Rust unit tests unchanged — all green (19 / 111 / 42).

### Why this ships now

This is the user-visible half of the knowledge-architecture story 0.32.7 started. Without the dialogs, users had to trust K2SO silently or read CLI / doc surfaces to understand what was happening to their files. Now the contract is explicit at both touch points (add + remove), and the "Why?" expander makes the multi-LLM rationale legible without requiring a docs lookup. Shipping this as part of the 0.32.x full vision rather than a follow-up matches the user-facing promise K2SO makes: one SKILL.md, every tool sees it, nothing lost on the way in or the way out.
