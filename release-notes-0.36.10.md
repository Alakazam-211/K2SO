# K2SO 0.36.10 — Add Workspace: three-option onboarding (Adopt / Start fresh / Do it later)

When you add a workspace that already has CLI-LLM context files (CLAUDE.md, GEMINI.md, .cursor/rules/k2so.mdc, .goosehints, AGENT.md, etc.), K2SO now asks how you want to handle them instead of silently archiving everything. The Add Workspace dialog gets a three-option radio picker — and the dialog itself leads with a plain-language explanation of *why* K2SO unifies these files in the first place, so first-time users aren't dropped into a wall of file paths.

## What changed

The `Add Workspace` dialog now has three modes when existing harness files are detected:

1. **Adopt one as Project Knowledge** — Pick the file whose body should seed `.k2so/PROJECT.md` (the single source of truth K2SO compiles every CLI-LLM SKILL from). The picked file's body becomes PROJECT.md; the original is archived to `.k2so/migration/` like every other file. This is the right choice if one of your existing files already has the project context you want every AI tool to share.

2. **Start fresh** — The previous default. Every existing harness file is archived; PROJECT.md starts empty and you fill it in deliberately. Right when your existing files are stale, tool-specific (e.g., a Cursor-only rule), or you'd rather start clean.

3. **Do it later** — New. Drops a `.k2so/.skip-harness-management` flag. K2SO writes its own internal SKILL.md (so heartbeats and agent launches still work) but **does not touch any of your CLI-LLM harness files**. CLAUDE.md, GEMINI.md, .cursor/rules, etc. stay exactly as you had them. Reversible — the flag is a single file you can delete to re-run onboarding later.

Plus a couple of quality-of-life touches:

- The dialog now leads with **why K2SO does this** in plain language ("Tell K2SO once, every AI tool listens") instead of burying it behind a "▸ Why does K2SO do this?" expandable.
- The "Plan for this workspace" file list is now a `▸ Show file plan` toggle — secondary detail, not the dominant content. Auto-expanded when you pick Adopt so you can see (and click) the candidate files.
- Radio dots are blue (`var(--color-accent)`) in both AddWorkspaceDialog and RemoveWorkspaceDialog for consistency.

## Under the hood

- New `k2so-core::agents::onboarding` module (six unit tests). Logic lives entirely in core so the CLI and Tauri share the same implementation — the renderer is pure display + button-clicks.
- New Tauri commands: `k2so_onboarding_scan` / `_adopt` / `_skip` / `_start_fresh`.
- New daemon HTTP routes: `/cli/onboarding/scan` / `/adopt` / `/skip` / `/start-fresh`.
- New CLI subcommand: `k2so onboarding {scan, adopt <file>, later, fresh}` for headless operation.
- `skill_writer::write_skill_to_all_harnesses` and the workspace regen orchestrator now check the skip flag and short-circuit harness fanout when it's set — K2SO's internal `.k2so/skills/k2so/SKILL.md` still gets written either way.

## Restore symmetry preserved

The existing `k2so workspace remove --mode restore-original` flow continues to work for adopted workspaces: Adopt's archive lives in the same `.k2so/migration/` folder as every other archived file, so Restore Original brings everything back byte-for-byte from the snapshot taken at adopt-time. PROJECT.md edits made *after* adoption don't affect what Restore Original puts back.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
