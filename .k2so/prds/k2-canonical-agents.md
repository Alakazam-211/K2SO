# PRD: `k2-canonical-agents` — opt-in, agent-driven, content-preserving harness unification

**Status:** Design / single source of truth (no feature code in this pass)
**Author:** pod-leader (investigation + design)
**Date:** 2026-06-02
**Supersedes / consolidates:**
- `.k2so/prds/optional-unification-skill.md` (default-OFF gate + safe copy-based routine + skill model + current-machinery map). **Superseded — fold here.**
- `.k2so/prds/k2-canonical-setup-button.md` (AIFileEditor button/wiring layer; seed-as-args; `CanonicalSetupEditor` + `__canonical_setup__` dispatch seam). **Superseded — fold here.**
**Supersedes tasks:** #585 (publish standalone K2SO Skill) and #586 (make canonical AGENT.md/SKILL.md optional) — see §12.
**Out of scope:** the `0.37.0` `.k2so/`-internal layout migration (System B; sentinel `.k2so/.unification-0.37.0-done`, `crates/k2so-core/src/migrations/unification_0_37_0.rs:36`). Leave it as-is.

---

## 1. Overview / vision

Today K2SO **auto-unifies** every registered workspace's AI-harness files by **replacing them with symlinks** to a single canonical `.k2so/skills/<name>/SKILL.md`, and **marker-injecting** `AGENTS.md` + `.github/copilot-instructions.md`. This fan-out fires automatically on **daemon boot**, **agent create**, and **agent launch** — across **every** workspace — with the only opt-out being a `.k2so/.skip-harness-management` flag that has to exist *before* the first fan-out runs. Symlinks-over-real-files, no-archive paths, live-file races, and "K2SO took over my repo" perception make this the scary thing.

`k2-canonical-agents` replaces that with an **opt-in, agent-driven, content-preserving** model:

1. **Off by default.** The user-visible fan-out is gated behind a per-workspace setting (default **false**). No workspace gets auto-symlinked on boot/create/launch/regen. The canonical `.k2so/skills/.../SKILL.md` keeps generating internally (heartbeats/launches need it); only the **user-visible** fan-out is gated.
2. **Remove the consent page.** The first-run onboarding/agreement page (`AddWorkspaceDialog`, the "tell K2SO once, every AI tool listens" Adopt/Start-Fresh/Do-it-later picker) only existed to gate the now-disabled invasive behavior. Remove it (§ Removal Catalog).
3. **Opt-in via a shipped skill an agent runs**, launched by a **button** that reuses `AIFileEditor`. Ship BOTH.
4. **Graceful content MERGE, never back-up-and-replace.** The agent reads the user's existing rich `CLAUDE.md`/`GEMINI.md`, judges what is valuable, and **weaves it into the canonical structure** — preserving substance. Canonical = a target *structure* the user's content is organized into, NOT a template that overwrites.
5. **Deterministic core = safety net only:** back up originals to `.k2so/backups/<ts>/` + manifest, atomic writes, **copies not symlinks**, byte-reversible. The agent produces (merged) content; the core applies it safely.
6. **Bidirectional:** the same skill sets up AND unwinds (manifest-driven exact restore).
7. **Per-harness selection** + **diagnose-then-intent** opening every run.

---

## 2. Locked design — the 13 refinements (structured)

### 2.1 Name (#1)
`k2-canonical-agents` is the name of **both** the user-facing button/command **and** the shipped skill. Not "setup" — it does more than set up (it diagnoses, merges, adds harnesses, and unwinds). Working/marketing label on the button varies by state (§2.8): "Set up unified files" vs "Manage / Undo".

### 2.2 Off by default (#2)
The auto-symlink fan-out is gated behind a per-workspace setting, default **false**. New AND existing workspaces get **no** automatic symlinking on boot / agent-create / agent-launch / regen.

- **Setting:** `harness_fanout_enabled: bool` (default false). Source of truth is filesystem-first (a `.k2so/`-internal marker, e.g. `.k2so/.canonical/state.json` or per-harness markers — see §2.7), daemon-first per project invariants; the Settings UI may mirror it in the `projects` DB row for fast listing (marker authoritative).
- The canonical `.k2so/skills/<name>/SKILL.md` **still always generates** internally (above the gate) — heartbeats and agent launches depend on it. Only the **user-visible fan-out into harness dirs / root** is gated off.
- **Gate inversion is two edits** (§4): `writer.rs:185` and `skill_regen.rs:285`. Because *all* fan-out funnels through `write_skill_to_all_harnesses` + the Steps 6/7 block, gating those two covers every trigger (boot/create/launch/regen) — no per-call-site special-casing.
- Keep `.k2so/.skip-harness-management` as a **legacy alias**: its presence still forces `harness_fanout_enabled = false` (don't break anyone who set it).

### 2.3 Remove the consent page (#3)
The first-run onboarding/agreement page that users currently consent to is **`AddWorkspaceDialog`** (renderer) — the modal that frames "Tell K2SO once, every AI tool listens" and offers **Adopt / Start fresh / Do it later**. It only exists to gate the invasive auto-fan-out. Once fan-out is off-by-default, this consent gate is dead weight. **Remove it** (full catalog in §7). Relocate the one genuinely useful piece — the plain-language *explanation* of why unified context helps — into the `k2-canonical-agents` skill/button briefing.

### 2.4 Opt-in via shipped skill + button (#4)
Ship **both**:
- **Skill** (durable, testable home for the safe procedure): authored as a Rust constant in `crates/k2so-core/src/skills/content.rs` (sibling of `generate_manager_skill_content`, `crates/k2so-core/src/skills/content.rs:170`), written on demand to **`.k2so/skills/k2-canonical-agents/SKILL.md`** (upgrade-tracked via `ensure_skill_up_to_date`). It is an **internal operator skill — NOT fanned out** to harness dirs. Snapshot-tested like the other generators.
- **Button** (thin trigger): reuses `AIFileEditor`. The seed prompt rides in as `--append-system-prompt` (briefing) + a final positional initial message that **references the skill by name** so the agent loads it via its Skill tool. The button carries no procedure of its own.

### 2.5 Graceful content MERGE (#5)
**Never back-up-and-replace for content-rich files.** The agent:
1. Reads the user's existing harness files (rich `CLAUDE.md`/`GEMINI.md`, `AGENT.md`, etc.).
2. Judges what is genuinely valuable user context vs. boilerplate/stale.
3. **Weaves the valuable substance into the canonical structure** (organizes it into the canonical layout — `.k2so/PROJECT.md` / `AGENT.md` sections), preserving the user's accumulated knowledge.
4. Hands the merged content to the deterministic core, which persists it safely (backup + atomic write + manifest).

Canonical is a **target structure the user's content is poured into**, not a template that bulldozes it. This is the core philosophical break from today's "archive the file, replace it with a symlink to a generated body."

### 2.6 Two kinds of canonical files (#6) — the explicit boundary

| Kind | Files | Who produces the content | How it's written |
|---|---|---|---|
| **(a) Generated / templated** — K2SO knows these exactly | canonical `SKILL.md`; the per-harness skill stubs (`.claude/skills/<n>/SKILL.md`, `.opencode/agent/<n>.md`, `.pi/skills/<n>/SKILL.md`); the `AGENTS.md` / `.github/copilot-instructions.md` **marker blocks**; `.cursor/rules/k2so.mdc` | **Deterministic core** (content generators in `content.rs`) | Core writes them directly — atomic, signature-stamped, regen-tracked |
| **(b) Content-rich** — the substance is the user's | `AGENT.md`, `.k2so/PROJECT.md`, and the substantive bodies of `CLAUDE.md` / `GEMINI.md` | **The AGENT** merges existing user content into the canonical layout | Agent produces merged text → **core persists it safely** (backup + atomic write + manifest) |

The boundary is the contract: **(a) is deterministic and never asks the LLM to author it; (b) is the agent's creative-but-safe merge, persisted by the safety net.** The irreversible/destructive act (overwriting a real file) is always deterministic + backed-up; the judgment call (what's worth keeping) is the agent's.

### 2.7 Deterministic core = safety net only (#7)
The core's job is **narrow and irreversible-only**:
- Back up every original it will touch to `.k2so/backups/<ISO-ts>/<relpath>` **before any write**.
- Write a `manifest.json`: per file → original path, backup path, pre-existing content hash, action taken, target harness. This is what makes it **byte-reversible** and lets re-runs detect "already done."
- **Atomic writes** (temp-sibling + rename, `crate::fs_atomic::atomic_write_str`) so a crash/concurrent reader never sees a half-written file.
- **Copies, not symlinks.** Real files with a `k2so_generated:` signature + a "regenerated by K2SO — edit `.k2so/...` instead" header. (Symlinks retained only as legacy-compat for already-symlinked workspaces; never the default for new opt-ins.)
- For merge files (b): the **agent** supplies the merged body; the **core** does the backup+atomic-write+manifest around it. The agent never performs raw file mutation.

Proposed core entry point: `harness::run_canonical_setup(project_path, opts: { harnesses: Vec<Harness>, dry_run, confirm, force, merged_bodies: Map<path, String> })` and `harness::unwind_canonical(project_path, manifest)`. The reverse generalizes the existing `teardown.rs:76` `RestoreOriginal` to the copy+manifest model.

### 2.8 Bidirectional (#8)
The **same** skill sets up AND unwinds. Unwind is **manifest-driven exact restore**: read the latest `.k2so/backups/<ts>/manifest.json`; for each entry restore the backup over the generated copy (atomic), or trash the generated file if the backup was "created fresh"; strip the K2SO marker block / `read:` line from merged files; clear the per-harness enabled flag. Button label gates on state: **"Set up unified files"** (Unmanaged/Skipped) vs **"Manage / Undo"** (any harness Unified).

### 2.9 Per-harness selection (#9)
The agent **asks which LLMs the user wants enabled** and only touches those — never creates files in harness dirs the user didn't choose. Everything is **per-harness**: dry-run plan, manifest, and unified-state are all per-harness. A workspace can have **Claude canonicalized and Gemini untouched** independently.

**Harnesses K2SO actually supports (from code):**

| Harness | Root/dir target(s) | Source: writer.rs / harness.rs |
|---|---|---|
| Claude Code | `CLAUDE.md` (root), `.claude/skills/<n>/SKILL.md` | `skill_regen.rs:298` (root CLAUDE), `writer.rs:192` (skill) |
| Gemini | `GEMINI.md` (root) | `harness.rs:132` |
| Agent.md (generic) | `AGENT.md` (root) | `harness.rs:133` |
| Goose | `.goosehints` (root) | `harness.rs:134` |
| Cursor | `.cursor/rules/k2so.mdc` | `harness.rs` (`write_workspace_harness_discovery_targets`) |
| Aider | `.aider.conf.yml` (merged `read:` entry) | `harness.rs:201` `scaffold_aider_conf` |
| OpenCode | `.opencode/agent/<n>.md` (symlink) | `writer.rs:197` |
| Pi | `.pi/skills/<n>/SKILL.md` (symlink) | `writer.rs:203` |
| AGENTS.md (multi) | `AGENTS.md` (marker block) | `writer.rs:211` |
| GitHub Copilot | `.github/copilot-instructions.md` (marker block) | `writer.rs:214` |
| (root SKILL.md) | `SKILL.md` (root symlink) | `skill_regen.rs:296` |

`HARNESS_WORKSPACE_FILES` (`crates/k2so-core/src/workspace/harness.rs:44`) is the root list: `CLAUDE.md, GEMINI.md, AGENT.md, .goosehints, SKILL.md, .cursor/rules/k2so.mdc`. The onboarding probe list (`onboarding.rs:130` `HARNESS_PROBES`) adds `AGENTS.md`, `.opencode/agent/k2so.md`, `.pi/skills/k2so/SKILL.md`, `.github/copilot-instructions.md`.
**Gap to note (open question):** `.codex/` and `.gemini/` *directories* are **not** touched by code today — only `GEMINI.md` at root. The `AgentContextDiagram.tsx:205` UI claims "12 harnesses," but code writes the ~10 targets above. The skill must enumerate from the real code list, not the diagram.

### 2.10 Diagnose-then-intent opening (#10)
Every run, the skill FIRST runs the deterministic `detect_canonical_state(project_path)` (per-harness probe of root/dir symlinks + copies + markers), **summarizes** it in plain language ("Claude is canonicalized; Codex/Gemini aren't; you have rich content in `GEMINI.md`"), then **asks intent** — set up / add a harness / unwind / just review — and **does nothing until the user chooses**. `detect_canonical_state` is the deterministic diagnosis feed (generalizes the draft's `detect_unified_state`; probes via `fs::symlink_metadata` + `read_link` + the `k2so_generated:` signature, returning per-harness `Unified(copy|symlink) | Skipped | Unmanaged`).

### 2.11 Existing users (#11)
- **Leave already-unified workspaces untouched. No auto-revert.**
- One-time migration sets the per-harness/per-workspace enabled flag: workspaces detected as **already unified** (any `HARNESS_WORKSPACE_FILES` root target is a symlink resolving under `.k2so/skills/`) → `harness_fanout_enabled = true` (keep regenerating their symlinks so nothing breaks under them). Everyone else / `Skipped` / `Unmanaged` / **new** → `false`.
- The new copy-based skill is available to everyone; already-symlinked users can optionally convert to copies, never forced.

### 2.12 Live preview (#12)
The dry-run plan + backup manifest are written to **deterministic paths** and shown in the `AIFileEditor` preview pane before confirming:
- Plan → **`.k2so/.canonical-setup/plan.md`** (stable path for the live watch; also copied into the backup dir for the record).
- Manifest → **`.k2so/backups/<ts>/manifest.json`**.
`CanonicalSetupEditor` uses `AIFileEditor`'s multi-file watch (`files` prop) with `watchDir = <project>/.k2so` and tabs **Plan** (pre-confirm: per-harness action table — `merge` / `create` / `back-up-then-write` / `marker-inject` / `skip-already-managed`) and **Manifest** (post-confirm). The watcher fires `onFileChange` per changed path so the user watches the ceremony unfold live.

### 2.13 Supersession of #585/#586 (#13)
See §12.

---

## 3. Safe-routine + agent-merge architecture

```
USER clicks button  ──▶  CanonicalSetupEditor (renderer, thin)
                            │  spawns: claude --append-system-prompt <briefing> "<seed>"
                            ▼
                         AGENT (Claude) loads k2-canonical-agents SKILL
                            │  1. detect_canonical_state → diagnose → ASK intent (does nothing yet)
                            │  2. ask WHICH harnesses
                            │  3. read rich CLAUDE.md/GEMINI.md → JUDGE → MERGE into canonical layout   ← (b) creative
                            │  4. call core run_canonical_setup(dry_run) → writes plan.md (no mutation)
                            │  5. SHOW plan in preview, WAIT for user confirm
                            ▼
                         CORE harness::run_canonical_setup(confirm)   ← deterministic safety net
                            │  backup originals → .k2so/backups/<ts>/ + manifest.json
                            │  (a) generated files: core authors directly (SKILL stubs, markers, cursor mdc)
                            │  (b) content-rich files: core writes the AGENT's merged body, atomically
                            │  copies-not-symlinks, signature-stamped, byte-reversible
                            │  set per-harness enabled flag = true
                            ▼
                         REPORT: files written/backed-up/merged + exact undo command
```

- **All file mutation goes through core.** The agent cannot "skip the backup" — the backup is inside the core call, not an LLM-performed step.
- **Default invocation is dry-run.** Writes require an explicit `--confirm` the agent passes only after the user confirms.
- **Preflight running-agent check** (best-effort, via the v2 session map / live PTYs): if a live `claude`/`aider` session is reading the targets, refuse by default and name the session (offers `--force` with a loud warning). Detection is best-effort — external invocations outside K2SO won't be in the map.

---

## 4. Button / AIFileEditor wiring + exact seed prompt

### 4.1 Mechanics reused (verified file:line, this worktree)
`AIFileEditor` (`src/renderer/components/AIFileEditor/AIFileEditor.tsx`) is a generic "agent-in-a-terminal beside a live preview" component; **all** domain logic lives in the caller. We reuse it unchanged.
- **Seed is passed as CLI args, not typed into the PTY.** `AIFileEditor` mounts one `<TerminalPane command args>`; `TerminalPane` forwards `command`+`args` verbatim to the daemon `POST /cli/sessions/v2/spawn` (`src/renderer/terminal-v2/TerminalPane.tsx:676-691`). The last positional arg is the initial user message; `--append-system-prompt` injects the longer briefing.
- **Exemplar:** `ProjectContextEditor` builds `terminalArgs` at `ProjectsSection.tsx:1975-1986` = `[...parseCommand(preset).args, '--append-system-prompt', systemPrompt, "<final positional seed>"]`, Claude-gated. `CanonicalSetupEditor` mirrors it exactly.
- **Dispatch seam:** the full-screen takeover switch at `ProjectsSection.tsx:1055-1080` selects on `agentEditorName` (`'__project_context__'`, `'__claude_md__'`, else persona). **Adding `'__canonical_setup__'` is a drop-in.** Buttons call `onOpenEditor('__canonical_setup__')` (clone the Project Context block whose button is at `ProjectsSection.tsx:2844`).
- **Live preview:** multi-file watch (`files` prop), `watchDir`, `onFileChange` fire per changed path — map Plan + Manifest tabs onto it.
- **Session-resume:** `AIFileEditor` auto-`--resume`s the last session on mount. For a one-shot ceremony we want a **fresh** session → add an opt-in `disableSessionResume?: boolean` prop (default false, no change for existing callers).

### 4.2 Exact seed prompt

**`--append-system-prompt` briefing:**
```
You are running K2SO's k2-canonical-agents flow for the workspace at <CWD>.
Your job: bring this workspace's AI-harness files under one canonical structure,
SAFELY and while PRESERVING the user's existing content.

ALWAYS START BY DIAGNOSING. Run the k2-canonical-agents skill. First detect the
per-harness canonical state and summarize it for the user ("Claude is
canonicalized; Codex/Gemini aren't; you have rich content in GEMINI.md"). Then ASK
what they want to do — set up, add a harness, unwind, or just review — and DO
NOTHING until they choose.

NON-NEGOTIABLE RULES:
- Use the k2-canonical-agents skill. Do NOT improvise file operations.
- ASK which harnesses to enable (Claude, Gemini, Codex, Cursor, Aider, OpenCode,
  Pi, Copilot…). Touch ONLY the ones the user picks. Never create files in a
  harness the user didn't choose.
- This writes real-file COPIES, never symlinks.
- MERGE, don't bulldoze. For content-rich files (CLAUDE.md, GEMINI.md, AGENT.md),
  READ the user's existing content, judge what's valuable, and WEAVE it into the
  canonical structure (organized into .k2so/PROJECT.md / AGENT.md). Canonical is a
  target STRUCTURE for their content, not a template that overwrites it.
- The deterministic core does the irreversible part: it backs up every original to
  .k2so/backups/<timestamp>/ with a manifest BEFORE any write, writes atomically,
  and is byte-reversible. You produce the merged content; the core persists it.
- ALWAYS dry-run first. Write the plan to .k2so/.canonical-setup/plan.md, summarize
  it, and WAIT for explicit confirmation before any write.
- If a live agent/session is using this workspace, refuse and tell the user.
- Fully reversible: the SAME skill unwinds via the manifest.
Narrate each step. The user is watching a live preview of the plan on the right.
```

**Final positional seed (setup mode):**
```
Run the k2-canonical-agents skill against this workspace. Start by detecting the
per-harness canonical state and summarizing it, then ask me which harnesses I want
and what I want to do. Produce a DRY-RUN plan to .k2so/.canonical-setup/plan.md and
STOP for my confirmation before writing anything.
```

**Manage / Undo mode** swaps the seed to:
```
Run the k2-canonical-agents skill in manage mode: show me the current per-harness
canonical state and the exact way to undo it. If I confirm, run the manifest-driven
unwind for the harnesses I choose.
```

### 4.3 Props `CanonicalSetupEditor` passes
`command`/`args` (claude + briefing + seed, Claude-gated like `:1975-1986`); `cwd = project.path` (workspace root, NOT `.k2so/`); `files=[plan.md, latest manifest.json]` + `watchDir=<project>/.k2so`; `preview` = plan/manifest renderer (Markdown + per-harness action table); `warningText` = "K2SO backs up your files and writes real-file copies (not symlinks), preserving your existing content. Nothing is overwritten without a backup. Reversible from this same flow."; `title = "k2-canonical-agents: <project>"`; `disableSessionResume=true`.
No change to `AIFileEditor.tsx` or `TerminalPane.tsx` beyond the additive `disableSessionResume` prop.

---

## 5. Bidirectional + per-harness + diagnose-then-intent flows (consolidated)

**Setup flow:** diagnose (§2.10) → ask harnesses (§2.9) → read+merge content-rich files (§2.5/§2.6b) → core dry-run writes `plan.md` (§2.12) → user confirms → core backup+manifest+atomic copy writes (§2.7), generated files authored by core (§2.6a) → set per-harness flag true → report + undo command.

**Add-a-harness flow:** same, scoped to the newly chosen harness only; existing canonicalized harnesses untouched; per-harness manifest appended.

**Unwind flow (§2.8):** read latest manifest → per entry: restore backup over generated copy (atomic) OR trash created-fresh file (`safe_delete`) → strip K2SO marker block / aider `read:` line → clear per-harness flag → leave `.k2so/` intact.

**Just-review flow:** print the diagnosis and stop. No writes.

---

## 6. Existing-user handling (detail)
Per §2.11. Detection: `harness::detect_canonical_state(project_path) -> per-harness {Unified(copy|symlink) | Skipped | Unmanaged}` via `fs::symlink_metadata` + `read_link` (target under `.k2so/skills/`) and the `k2so_generated:` signature for copies. Migration (sentinel-gated, like `migrations/mod.rs`): `Unified` → flag true; else false. **No symlinks are ever auto-reverted.** New workspaces start false.

---

## 7. REMOVAL CATALOG

Everything removed, gated, or replaced. Verified against this worktree.

| Item | file:line | Action | Notes |
|---|---|---|---|
| **Consent/onboarding page** (`AddWorkspaceDialog` — "Tell K2SO once, every AI tool listens" Adopt/Start-Fresh/Do-it-later picker) | `src/renderer/components/AddWorkspaceDialog/AddWorkspaceDialog.tsx:31-439` | **Remove** | Only existed to gate the invasive auto-fan-out. Plain-language WHY (`:147-164`) is worth **preserving** → relocate into the skill/button briefing. Replaced by diagnose-then-intent in the skill. |
| Add-workspace dialog store | `src/renderer/stores/add-workspace-dialog.ts:10-26` | **Remove / simplify** | Drop the preview/onboarding state once the modal is gone; keep only what plain "add workspace" needs. |
| `handleDoItLater` → drops skip flag pre-regen | `AddWorkspaceDialog.tsx:83-97` (`invoke('k2so_onboarding_skip')`) | **Remove** | Skip-before-regen is meaningless once fan-out is off by default. |
| `handleAdoptConfirm` → `k2so_onboarding_adopt` | `AddWorkspaceDialog.tsx:99-119` | **Replace** | "Adopt one file into PROJECT.md" becomes the agent's MERGE step in the skill (richer: weaves, not single-file copy). |
| Tauri `k2so_onboarding_adopt` / `k2so_onboarding_skip` commands | `src-tauri/src/commands/k2so_agents.rs:685` / `:709`; registered `src-tauri/src/lib.rs:1051-1052` | **Remove** (skip) / **Replace** (adopt) | Skip command unneeded; adopt logic folds into the skill+core merge. |
| **Boot sweep auto-fan-out** | `crates/k2so-daemon/src/main.rs:1063` `ensure_all_skills_up_to_date` (loop from `:1030`) | **Gate** | No call-site change; gated by the two gate inversions below (canonical SKILL still regenerates). |
| **Agent-create fan-out** | `crates/k2so-core/src/workspace/agent.rs:272` `write_agent_skill_file` | **Gate** | Funnels through `write_skill_to_all_harnesses` → gated at `writer.rs:185`. |
| **Agent-launch fan-out** | `crates/k2so-core/src/workspace/agent_launch.rs:159` `write_workspace_skill_file` | **Gate** | Funnels through `skill_regen.rs:285` gate. |
| **Explicit regen** | `crates/k2so-daemon/src/cli.rs:890` (`regenerate_skills`), `:1330`, `:1349` (`write_workspace_skill_file`) | **Gate** | Same two gates; no special-casing. |
| **Gate check #1** (the fan-out short-circuit) | `crates/k2so-core/src/skills/writer.rs:185` `if is_harness_management_skipped { return }` | **Replace** | Invert to `if !harness_fanout_enabled(project_path) { return }` (legacy skip still forces false). Canonical SKILL write above (`:170-179`) stays. |
| **Gate check #2** (root SKILL/CLAUDE + discovery fan-out) | `crates/k2so-core/src/workspace/skill_regen.rs:285` `if !is_harness_management_skipped` | **Replace** | Invert to `if harness_fanout_enabled(project_path)`. Guards `:296` root SKILL symlink, `:298` `migrate_and_symlink_root_claude_md`, `:299` discovery-target fan-out. |
| **Symlink primitive** in fan-out | `writer.rs:128` `force_symlink`; `:192/197/205` (claude/opencode/pi) | **Replace (new flow) / retain (legacy)** | New opt-in writes copies; symlinks retained only for already-unified legacy workspaces. |
| **`.skip-harness-management` opt-out** | `crates/k2so-core/src/workspace/onboarding.rs:87` (`SKIP_HARNESS_FLAG_FILENAME`), `:99` (`is_harness_management_skipped`), `:105` (`skip`), `:118` (`unskip`) | **Replace (keep as legacy alias)** | Reconciled: the new positive `harness_fanout_enabled` marker is the opt-IN. `.skip-harness-management` presence keeps forcing `false` so existing opt-outs survive; no longer the primary mechanism. |
| `scan_harness_files` (respects skip flag, feeds the consent picker) | `crates/k2so-core/src/workspace/onboarding.rs` (`HARNESS_PROBES:130`, `scan_harness_files`) | **Repurpose** | Picker consumer is removed; the probe list feeds the skill's diagnose-then-intent enumeration instead. |
| Settings "Workspace Knowledge / harness" copy implying always-on fan-out | `src/renderer/components/Settings/sections/ProjectsSection.tsx:1252`, `:2126`, `:2169` (`ProjectContextEditor` "compiles into every CLI LLM harness on save") | **Gate / reword** | The PROJECT.md editor's "compiles into every harness" is only true when fan-out is enabled; reword to reflect opt-in. |
| Agent context diagram "12 harnesses, fans out to every harness" | `src/renderer/components/Settings/sections/AgentContextDiagram.tsx:205`, `:244-245` | **Reword** | Now opt-in + per-harness; "12" overstates the ~10 code targets and ignores per-harness selection. |
| `0.37.0` `.k2so/`-internal layout migration + sentinel | `crates/k2so-core/src/migrations/unification_0_37_0.rs:36` (`.unification-0.37.0-done`); boot `main.rs:436`/`:691` `run_workspace_unification_sweep` | **OUT of scope — keep** | Internal `.k2so/` reshuffle; does not touch user harness files. Leave as-is. |

**Confirmation:** the consent page is **found and removable**. It is `AddWorkspaceDialog.tsx:31-439`, shown as the first-run add-workspace modal (driven by `stores/add-workspace-dialog.ts`), and its only structural job is to choose between Skip (`k2so_onboarding_skip`, writes `.k2so/.skip-harness-management`), Start-Fresh (runs the fan-out), and Adopt (`k2so_onboarding_adopt`). All three branches exist solely to steer the auto-fan-out. With fan-out off by default, none are needed. **Worth preserving:** the plain-language explanation at `:147-164` ("Each AI coding tool reads its project notes from a different file… write your context once; every tool sees the same picture") — relocate it verbatim into the `k2-canonical-agents` skill briefing and the button block subtitle, so the *value pitch* survives without the consent gate.

---

## 8. Phased implementation plan

**Phase 0 — Stop the scary thing (immediate win).**
Flip fan-out off by default (invert the two gates `writer.rs:185` + `skill_regen.rs:285` to consult `harness_fanout_enabled`; `.skip-harness-management` stays a legacy false-forcer). Add `detect_canonical_state` + the migration that sets the flag true only for already-unified workspaces, false for everyone/new (no auto-revert). **Remove the consent page** (`AddWorkspaceDialog` + its onboarding store/commands; relocate the WHY copy). Tests: fresh workspace → boot/create/launch/regen leave the repo root clean; already-unified → symlinks still refresh.

**Phase 1 — Core safe-routine.**
`harness::run_canonical_setup(opts{harnesses, dry_run, confirm, force, merged_bodies})`: preflight running-agent check → per-harness dry-run plan to `.k2so/.canonical-setup/plan.md` → `.k2so/backups/<ts>/` + `manifest.json` → copy-based generated files (a) + atomic write of agent-merged bodies (b) → per-harness enabled flag. `harness::unwind_canonical` (manifest-driven restore; generalize `teardown.rs:76`). Tests: idempotent re-run, no-clobber, reversibility round-trip, per-harness independence, active-session refusal.

**Phase 2 — The shipped skill.**
`generate_canonical_agents_skill_content()` in `crates/k2so-core/src/skills/content.rs`; write to `.k2so/skills/k2-canonical-agents/SKILL.md` on demand (upgrade-tracked, internal — not fanned out). Encodes diagnose-then-intent → ask-harnesses → merge → dry-run → confirm → apply → report → unwind. Snapshot-test the content (no deprecated verbs); test that it references the merge/backup/copy/per-harness steps.

**Phase 3 — Button + live preview.**
`CanonicalSetupEditor` (sibling of `ProjectContextEditor`); add `'__canonical_setup__'` to the dispatch (`ProjectsSection.tsx:1055-1080`); add a "Canonical Agents" block near Project Context (`:2844`) whose label/mode come from `detect_canonical_state` exposed via a read-only Tauri command. Multi-file Plan/Manifest preview; `disableSessionResume` prop on `AIFileEditor`. Tests: snapshot the seed prompt (encodes the safety contract — must not drift); state→label UI test.

**Phase 4 — Polish.**
Git-status-quiet regen (content-hash gate on copies so boots don't churn diffs); k2.dev / standalone-skill (#585) tie-in; optional onboarding discovery banner (fast-follow).

---

## 9. Open questions (need owner's call)

1. **State source of truth:** filesystem marker (`.k2so/.canonical/state.json` per-harness) authoritative with DB mirror for fast Settings listing — or DB column primary? (Lean: marker authoritative, daemon-first; DB mirrors.)
2. **Per-harness flag granularity:** one workspace-level `harness_fanout_enabled` bool, or genuinely per-harness flags (so "Claude on, Gemini off" is persisted, not just a one-shot choice)? The diagnose-then-intent + per-harness manifest argue for **per-harness** persisted state — confirm.
3. **`.codex/` and `.gemini/` directories** are not touched by code today (only `GEMINI.md` at root). Does the product want directory-level canonicalization, or is per-file SKILL discovery enough? (`AgentContextDiagram` overstates coverage.)
4. **Legacy symlink → copy conversion:** offer already-unified users a one-click "convert symlinks to copies," or leave their working symlink setup entirely alone unless they unwind+re-run?
5. **Confirm UX boundary:** confirmation is conversational (user types "yes" to the agent). Add a hard "Confirm & write" button in the preview header (needs a PTY-write seam) or keep it chat-only for v1? (Lean: chat-only v1.)
6. **Provider coverage:** the seed-as-args path is Claude-gated (every existing `AIFileEditor` caller is). For non-Claude default agents: require Claude for this flow (a), type the seed into the PTY (b), or run a headless CLI path (c)? (Lean: (a) v1, (c) as headless fallback.)
7. **Merge authority for the destructive overwrite:** when the agent's merge would drop user content it judged low-value, do we require the *backup* to be the safety net (always full byte-reversible) and never block, or add a "merge diff review" gate? (Lean: backup is the net; merge is reversible, so don't block — but surface the diff in the plan.)

---

## 10. Appendix — code anchors (verified, this worktree)

- **Fan-out core:** `crates/k2so-core/src/skills/writer.rs:153` `write_skill_to_all_harnesses` (canonical write `:170-179`; **gate `:185`**; symlinks `:192/197/205`; markers `:211-214`); symlink primitive `:128` `force_symlink` → `fs_atomic` `atomic_symlink`.
- **Root + discovery fan-out:** `crates/k2so-core/src/workspace/skill_regen.rs:285` (**gate**), `:296` root SKILL symlink, `:298` `migrate_and_symlink_root_claude_md` (def `:566`), `:299` discovery targets.
- **Harness root files:** `crates/k2so-core/src/workspace/harness.rs:44` `HARNESS_WORKSPACE_FILES`, `:81` `safe_symlink_harness_file`, `:129` `write_workspace_harness_discovery_targets`, `:201` `scaffold_aider_conf`.
- **Opt-out flag:** `crates/k2so-core/src/workspace/onboarding.rs:87/99/105/118`; probe list `:130` `HARNESS_PROBES`; `adopt_harness_as_project_md` / `scan_harness_files`.
- **Triggers:** boot `crates/k2so-daemon/src/main.rs:1063` (`ensure_all_skills_up_to_date`, loop `:1030`); create `crates/k2so-core/src/workspace/agent.rs:272`; launch `crates/k2so-core/src/workspace/agent_launch.rs:159`; regen `crates/k2so-daemon/src/cli.rs:890/1330/1349`.
- **Reverse primitive:** `crates/k2so-core/src/workspace/teardown.rs:76` `teardown_workspace_harness_files` (`RestoreOriginal`).
- **Consent page (remove):** `src/renderer/components/AddWorkspaceDialog/AddWorkspaceDialog.tsx:31-439` (WHY copy `:147-164`; skip `:83-97`; adopt `:99-119`); store `src/renderer/stores/add-workspace-dialog.ts`; Tauri `src-tauri/src/commands/k2so_agents.rs:685/709`, registered `src-tauri/src/lib.rs:1051-1052`.
- **Button wiring (reuse):** `AIFileEditor.tsx` spawn `:437-442`, resume `:180-209`, watcher `:218-221/271-284/229-240`, multi-file `:399-420`; `TerminalPane.tsx:676-691` spawn forward; `ProjectsSection.tsx:1055-1080` dispatch, `:1975-1986` `terminalArgs` exemplar, `:2844` block button.
- **Skill content generators:** `crates/k2so-core/src/skills/content.rs:170/369/457/627`.
- **0.37.0 migration (out of scope):** `crates/k2so-core/src/migrations/unification_0_37_0.rs:36` sentinel; `crates/k2so-daemon/src/main.rs:436/691`.

---

## 11. Relationship to the two superseded drafts
- `optional-unification-skill.md` → this PRD's §2.2 (gate), §2.7 (safe routine), §3, §6 (existing users), §8 (phases). Its `harness_fanout_enabled`, `detect_unified_state`, `run_safe_unification`, manifest, and copy-not-symlink decisions are carried forward (renamed to `k2-canonical-agents` / `detect_canonical_state` / `run_canonical_setup`), and **extended** with the agent-merge / two-kinds-of-files boundary / per-harness / diagnose-then-intent refinements it didn't have.
- `k2-canonical-setup-button.md` → this PRD's §4 (button/AIFileEditor wiring, exact seed, `__canonical_setup__` dispatch, `disableSessionResume`) and §2.12 (live preview). Its `CanonicalSetupEditor` + plan.md/manifest preview contract are carried forward.

## 12. Supersession of #585 / #586
- **#586 (make canonical AGENT.md/SKILL.md optional)** is **fully delivered** by §2.2 (off-by-default gate) + §2.4 (opt-in skill) + §2.7 (copies not symlinks). The optionality #586 asked for is now the default posture.
- **#585 (publish standalone K2SO Skill)** folds in as the **distribution vector**: the `k2-canonical-agents` skill is authored as a `content.rs` generator and written on demand to `.k2so/skills/k2-canonical-agents/SKILL.md` — the same authoring/publishing path #585 needs. The k2.dev install-page tie-in lands in Phase 4. Both tasks should be closed against this PRD.
