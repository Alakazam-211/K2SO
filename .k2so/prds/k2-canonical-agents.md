# PRD: Three opt-in agent-setup skills — Workspace Manager, K2 Agent, K2 Canonical Agent

**Status:** Design / single source of truth (no feature code in this pass — planning-doc rewrite only)
**Author:** pod-leader (investigation + design)
**Date:** 2026-06-02 · **Revision:** v2 (restructured around the three-skill model)
**Supersedes / consolidates:**
- `.k2so/prds/optional-unification-skill.md` (default-OFF gate + safe copy-based routine + skill model + current-machinery map). **Superseded — fold here.**
- `.k2so/prds/k2-canonical-setup-button.md` (AIFileEditor button/wiring layer; seed-as-args; `CanonicalSetupEditor` + `__canonical_setup__` dispatch seam). **Superseded — fold here.**
- v1 of THIS PRD (single-skill "harness unification" framing + the k2so-managed marked-block / hash-gated regeneration role-knowledge idea). **Restructured here; the marked-block/regeneration approach is explicitly DELETED — see §3.**
**Supersedes tasks:** #585 (publish standalone K2SO Skill) and #586 (make canonical AGENT.md/SKILL.md optional) — see §12.
**Out of scope:** the `0.37.0` `.k2so/`-internal layout migration (System B; sentinel `.k2so/.unification-0.37.0-done`, `crates/k2so-core/src/migrations/unification_0_37_0.rs:36`). Leave it as-is.
**Ships as:** ONE feature (all three skills + the UI + the gate flip + the Removal Catalog), built in the internal phase order of §8. Not phased releases.

---

## 1. Overview / vision

This PRD covers the **entire agent-setup space** with **three opt-in skills**, each delivered as a version-controlled file under `.k2so/skills/`, each launched from a button in **Workspace Settings → Agent section**:

1. **Workspace Manager** — role-knowledge skill for the Manager role. Teaches the workspace's single agent *how to be a workspace manager* (standing orders, delegation/review surface, CLI verbs, persona) and the agent weaves that role knowledge **organically** into the user's `AGENT.md`.
2. **K2 Agent** — role-knowledge skill for the K2 Agent (planner) role. Same organic model, scoped to the planner role (PRDs / milestones / technical plans).
3. **K2 Canonical Agent** — the **harness-unification** skill (the feature the v1 PRD described): diagnose→intent, per-LLM selection, graceful content merge, bidirectional set-up/unwind, deterministic safety net, existing-users-left-untouched.

The thread tying all three together:

- **Off by default + remove the scary machinery.** Today K2SO **auto-unifies** every registered workspace by **replacing** its AI-harness files with **symlinks** to a generated canonical `SKILL.md`, fanning out on **daemon boot**, **agent create**, and **agent launch**. We gate that fan-out off by default (§4), and we **remove the consent page** that only existed to steer it (§7).
- **The skill IS the upgrade lever.** Role knowledge lives in the skill file, version-controlled by us. As K2SO→K2 evolves, **we update the skill** — no per-workspace migration machinery, no regeneration daemon, no hash gate.
- **Nothing programmatic ever rewrites user content.** Deterministic code is the **safety net only** (back up originals to `.k2so/backups/<ts>/` + manifest, atomic byte-reversible writes, copies-not-symlinks) plus **delivering/updating the skill files**. All content authorship — role integration into `AGENT.md`, harness merges — is the **agent's organic judgment**.
- **`AGENT.md` + `PROJECT.md` are THE source of truth (Model A).** Per-harness files (`CLAUDE.md`, `GEMINI.md`, …) are *generated mirrors*. The canonical merge pulls existing harness content **into** `AGENT.md`/`PROJECT.md` first (so nothing is lost), then mirrors out.

---

## 2. The three-skill model (the backbone)

| | **Workspace Manager** | **K2 Agent** | **K2 Canonical Agent** |
|---|---|---|---|
| **Kind** | Role-knowledge | Role-knowledge | Harness-unification (operator) |
| **What it does** | Integrates Manager-role guidance into the user's `AGENT.md`, organically | Integrates K2-Agent (planner) role guidance into `AGENT.md`, organically | Diagnoses per-harness state, merges harness files into `AGENT.md`/`PROJECT.md`, mirrors out, sets up/unwinds — safely |
| **Content model** | Organic agent integration (§3) | Organic agent integration (§3) | Diagnose→intent + graceful merge (§5) + deterministic safety net (§6) |
| **UI** | Normal **AIFileEditor** pointed at `AGENT.md` (§9.1) | Normal **AIFileEditor** pointed at `AGENT.md` (§9.1) | **Modal with the agent running** + plan/manifest preview (no single-file editor) (§9.2) |
| **Button shown when** | role = Manager | role = K2 Agent | **ALWAYS** (every agent type, incl. custom + agent-mode OFF) (§9.3) |
| **Skill file** | `.k2so/skills/workspace-manager/SKILL.md` | `.k2so/skills/k2-agent/SKILL.md` | `.k2so/skills/k2-canonical-agent/SKILL.md` |
| **Source generator** | `generate_manager_skill_content` (`content.rs:170`) → reframed as a skill body | `generate_k2so_agent_skill_content` (`content.rs:457`) → reframed | New `generate_canonical_agent_skill_content` |

**Mode→role mapping (verified, `ProjectsSection.tsx:1406-1409`):** `agentMode` ∈ `off | custom | agent | manager` (legacy `coordinator`/`pod` alias to `manager`). The **Workspace Manager** button shows for `manager` (and its legacy aliases); the **K2 Agent** button shows for `agent`; the **K2 Canonical Agent** button shows for **all** modes including `off` and `custom`.

**Single-agent layout (verified):** post-0.37.0/0.39.x each workspace has **one** agent at `.k2so/agent/AGENT.md` (`crates/k2so-core/src/workspace/agent.rs:213-214`, `:227-235`), with skills under `.k2so/skills/`. The legacy `.k2so/agents/<name>/` tree is being migrated away. §I (see §11) tracks the Agent Skills settings UI refresh that depends on this.

---

## 3. Role-knowledge model — ORGANIC, not programmatic (LOCKED)

> **This section replaces, and explicitly deletes, the v1 idea of a "k2so-managed marked block" with "hash-gated regeneration."** That approach is rejected.

### 3.1 The skill file is the home of role knowledge
The knowledge of *how to be a Workspace Manager / K2 Agent* — standing orders, the CLI verb surface (`k2so checkin/status/done/delegate/review/msg/reserve`), the persona framing — **lives inside the skill file** under `.k2so/skills/`, version-controlled by us. We author it in a `content.rs` generator (reframed from the existing `generate_manager_skill_content` / `generate_k2so_agent_skill_content`) and write it on demand to the workspace.

**The skill is the entire upgrade lever.** As K2SO → K2 evolves and the role guidance changes, **we update the skill body**. There is:
- **No per-workspace migration** of `AGENT.md`.
- **No regeneration machinery / daemon sweep** that rewrites role content.
- **No hash gate** comparing a generated block against a stored hash.

### 3.2 Integration is the agent's organic judgment
When a role skill runs, **the agent integrates the role guidance into `AGENT.md` ORGANICALLY**:
1. **Reads** the user's existing `AGENT.md` (and surrounding context).
2. **Weaves** the role context in **with judgment**, preserving the user's existing content.
3. Produces the merged `AGENT.md` text; the deterministic safety net (§6) persists it (backup + atomic write).

**NEVER a programmatic injection.** Programmatic injection — even a marked, fenced block — is **explicitly rejected**: in a long-active workspace it risks **"dumbing down" the agent** by displacing accumulated context the agent relied on. A marked block also fights the user's own edits and invites the hash-gated-regeneration machinery we are deleting. The agent's judgment is the only thing that knows what to keep.

### 3.3 Deterministic code does exactly two things, and no more
- **(1) Delivers/updates the skill files** — writes `SKILL.md` into `.k2so/skills/<name>/` on demand, upgrade-tracked (the file content tracks the generator; "the file is stale → rewrite the *skill file*" is fine, because the skill file is K2SO-owned and templated — see §5.4 file-kind boundary).
- **(2) Is the safety net** (§6) — backs up originals, writes atomically, byte-reversible.

**Nothing programmatic ever rewrites user content files** (`AGENT.md`, `PROJECT.md`, `CLAUDE.md`, `GEMINI.md`). Those are authored by the agent and persisted by the safety net.

### 3.4 "Re-run the skill" is the refresh mechanism
Refresh is **opt-in, agent-driven, never silent**. When the role guidance improves, the user **re-runs the skill** (via the same button); the agent re-reads `AGENT.md`, re-integrates the now-updated role knowledge organically, and the safety net persists it. This **replaces** the v1 hash-gated auto-regeneration entirely.

---

## 4. Off by default + the gate (LOCKED, carried from v1)

The auto-symlink fan-out is gated behind a per-workspace setting, default **false**. New AND existing workspaces get **no** automatic symlinking on boot / agent-create / agent-launch / regen.

- **Setting:** `harness_fanout_enabled: bool` (default false). Filesystem-first source of truth (a `.k2so/`-internal marker), daemon-first per project invariants; the Settings UI may mirror it in the `projects` DB row for fast listing (marker authoritative).
- The canonical `.k2so/skills/<name>/SKILL.md` **still always generates** internally (heartbeats and agent launches depend on it). Only the **user-visible fan-out into harness dirs / root** is gated off.
- **Gate inversion is two edits:**
  - `crates/k2so-core/src/skills/writer.rs:185` — `if is_harness_management_skipped { return }` → **invert** to `if !harness_fanout_enabled(project_path) { return }`. Canonical SKILL write above (`:170-179`) stays above the gate.
  - `crates/k2so-core/src/workspace/skill_regen.rs:285` — `if !is_harness_management_skipped` → **invert** to `if harness_fanout_enabled(project_path)`. Guards `:296` root SKILL symlink, `:298` `migrate_and_symlink_root_claude_md`, `:299` discovery-target fan-out.
- Because **all** fan-out funnels through `write_skill_to_all_harnesses` + the Steps 6/7 block, gating those two covers every trigger (boot/create/launch/regen) — no per-call-site special-casing.
- Keep `.k2so/.skip-harness-management` as a **legacy alias**: its presence still forces `harness_fanout_enabled = false`.

---

## 5. The K2 Canonical Agent skill (harness unification)

This is the v1 PRD's feature, retained in full and slotted as skill #3. Its full flow: **diagnose-then-intent → per-LLM selection → graceful merge → bidirectional set-up/unwind → deterministic safety net → existing-users-left-untouched.**

### 5.1 Canonical = structure, not overwrite. Source of truth = Model A.
- Canonical is a **target STRUCTURE the user's content is organized into**, never a template that overwrites.
- **`AGENT.md` + `PROJECT.md` are THE canonical source of truth (Model A).** Per-harness files (`CLAUDE.md`, `GEMINI.md`, …) are **generated mirrors** of them.
- The merge **pulls existing harness content INTO `AGENT.md`/`PROJECT.md` FIRST** (so nothing is lost), **then mirrors out** to the chosen per-harness files.
- **Baseline canonicalization = `AGENT.md` + `PROJECT.md` at minimum.** Coverage grows over time; the baseline is the floor, not the ceiling.

### 5.2 Diagnose-then-intent (every run)
The skill FIRST runs the deterministic `detect_canonical_state(project_path)` (per-harness probe of root/dir symlinks + copies + markers), **summarizes** it in plain language ("Claude is canonicalized; Codex/Gemini aren't; you have rich content in `GEMINI.md`"), then **asks intent** — set up / add a harness / unwind / just review — and **does nothing until the user chooses**. `detect_canonical_state` probes via `fs::symlink_metadata` + `read_link` + the `k2so_generated:` signature, returning per-harness `Unified(copy|symlink) | Skipped | Unmanaged`.

### 5.3 Per-LLM selection
The agent **asks which LLMs the user wants enabled** and only touches those — never creates files in harness dirs the user didn't choose. Dry-run plan, manifest, and unified-state are all **per-harness**: a workspace can have **Claude canonicalized and Gemini untouched** independently.

**Harnesses K2SO actually supports (from code):**

| Harness | Root/dir target(s) | Source |
|---|---|---|
| Claude Code | `CLAUDE.md` (root), `.claude/skills/<n>/SKILL.md` | `skill_regen.rs:298`, `writer.rs:192` |
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

`HARNESS_WORKSPACE_FILES` (`crates/k2so-core/src/workspace/harness.rs:44`) is the root list: `CLAUDE.md, GEMINI.md, AGENT.md, .goosehints, SKILL.md, .cursor/rules/k2so.mdc`. The probe list (`onboarding.rs:130` `HARNESS_PROBES`) adds `AGENTS.md`, `.opencode/agent/k2so.md`, `.pi/skills/k2so/SKILL.md`, `.github/copilot-instructions.md`. **Scope is per-file (the 6 `HARNESS_WORKSPACE_FILES` + probes), NOT directory-level.**

### 5.4 Two kinds of files — the file-kind boundary (the contract)

| Kind | Files | Who authors | How written |
|---|---|---|---|
| **(a) Generated / templated** — K2SO owns these exactly | the K2SO-owned `SKILL.md` files (the three skills' bodies; per-harness skill stubs); the `AGENTS.md` / `.github/copilot-instructions.md` **marker blocks**; `.cursor/rules/k2so.mdc` | **Deterministic core** (`content.rs` generators) | Core writes directly — atomic, signature-stamped, upgrade-tracked. (This is where "the file is stale → rewrite it" is legitimate, because it's K2SO-owned.) |
| **(b) Content-rich** — substance is the user's | `AGENT.md`, `.k2so/PROJECT.md`, and the substantive bodies of `CLAUDE.md` / `GEMINI.md` (which mirror Model A) | **The AGENT** (organic merge / role integration) | Agent produces text → **core persists it safely** (backup + atomic write + manifest). User-owned. |

The boundary is the contract: **(a) is deterministic and never asks the LLM to author it; (b) is the agent's creative-but-safe authorship, persisted by the safety net.** The irreversible/destructive act (overwriting a real file) is always deterministic + backed-up; the judgment call is the agent's.

### 5.5 Graceful content MERGE (never back-up-and-replace for content-rich files)
The agent: reads the user's existing harness files → judges valuable user context vs boilerplate/stale → **weaves the substance into `AGENT.md`/`PROJECT.md`** (Model A) → hands the merged text to the core, which persists safely. Canonical is a **target structure the user's content is poured into**, not a template that bulldozes it. This is the philosophical break from today's "archive the file, replace it with a symlink to a generated body."

### 5.6 Bidirectional set-up / unwind
The **same** skill sets up AND unwinds. Unwind is **manifest-driven exact restore**: read the latest `.k2so/backups/<ts>/manifest.json`; per entry restore the backup over the generated copy (atomic), or trash the generated file if it was "created fresh"; strip the K2SO marker block / aider `read:` line from merged files; clear the per-harness enabled flag; leave `.k2so/` intact.

### 5.7 Existing users — left untouched
- **Leave already-unified workspaces untouched. No auto-revert.**
- One-time migration (sentinel-gated, like `migrations/mod.rs`): workspaces detected as **already unified** (any `HARNESS_WORKSPACE_FILES` root target is a symlink resolving under `.k2so/skills/`) → `harness_fanout_enabled = true` (keep regenerating their symlinks so nothing breaks). Everyone else / `Skipped` / `Unmanaged` / **new** → `false`.
- The new copy-based skill is available to everyone; already-symlinked users can optionally convert to copies, never forced.

---

## 6. Deterministic safety net (the ONLY destructive surface)

Shared by all three skills (role integration AND canonical merge). The core's job is **narrow and irreversible-only**:
- **Back up** every original it will touch to `.k2so/backups/<ISO-ts>/<relpath>` **before any write**.
- Write a `manifest.json`: per file → original path, backup path, pre-existing content hash, action taken, target harness. This makes it **byte-reversible** and lets re-runs detect "already done."
- **Atomic writes** (temp-sibling + rename, `crate::fs_atomic::atomic_write_str`) so a crash/concurrent reader never sees a half-written file.
- **Copies, not symlinks.** Real files with a `k2so_generated:` signature + a "regenerated by K2SO — edit `.k2so/...` instead" header. (Symlinks retained only as legacy-compat for already-symlinked workspaces.)
- For agent-authored files (role integration `AGENT.md`; canonical merge bodies): the **agent** supplies the text; the **core** does the backup + atomic-write + manifest around it. **The agent never performs raw file mutation.**

Proposed core entry points:
- `harness::run_canonical_setup(project_path, opts: { harnesses, dry_run, confirm, force, merged_bodies: Map<path, String> })`
- `harness::unwind_canonical(project_path, manifest)` (generalizes `teardown.rs:76` `RestoreOriginal` to the copy+manifest model)
- `harness::persist_agent_md(project_path, merged_body)` — the role-skill safety-net write (backup + atomic + manifest around the agent's organic `AGENT.md` integration). Same primitive, content-rich kind (b).

**All file mutation goes through core.** The agent cannot "skip the backup." Default invocation is **dry-run**; writes require an explicit `--confirm`. **Preflight running-agent check** (best-effort via the v2 session map): if a live session is reading the targets, refuse by default and name it (offers `--force` with a loud warning).

---

## 7. REMOVAL CATALOG

Everything removed, gated, or replaced. Verified against this worktree. The consent page's **value-pitch copy is preserved** → relocated into the skill briefings / button subtitles (it is a value pitch worth keeping; only the *consent gate* is dead weight once fan-out is off by default).

| Item | file:line | Action | Notes |
|---|---|---|---|
| **Consent/onboarding page** (`AddWorkspaceDialog` — "Tell K2SO once, every AI tool listens" Adopt/Start-Fresh/Do-it-later picker) | `src/renderer/components/AddWorkspaceDialog/AddWorkspaceDialog.tsx:31-439` | **Remove** | Only existed to gate the invasive auto-fan-out. Plain-language WHY (`:147-164`) is **preserved** → relocate into the skill/button briefings. Replaced by diagnose-then-intent in the K2 Canonical Agent skill. |
| Add-workspace dialog store | `src/renderer/stores/add-workspace-dialog.ts:10-26` | **Remove / simplify** | Drop preview/onboarding state once the modal is gone; keep only what plain "add workspace" needs. |
| `handleDoItLater` → drops skip flag pre-regen | `AddWorkspaceDialog.tsx:83-97` (`invoke('k2so_onboarding_skip')`) | **Remove** | Skip-before-regen is meaningless once fan-out is off by default. |
| `handleAdoptConfirm` → `k2so_onboarding_adopt` | `AddWorkspaceDialog.tsx:99-119` | **Replace** | "Adopt one file into PROJECT.md" becomes the K2 Canonical Agent's MERGE step (richer: weaves into Model A, not single-file copy). |
| Tauri `k2so_onboarding_adopt` / `k2so_onboarding_skip` | `src-tauri/src/commands/k2so_agents.rs:685` / `:709`; registered `src-tauri/src/lib.rs:1051-1052` | **Remove** (skip) / **Replace** (adopt) | Skip unneeded; adopt folds into skill+core merge. |
| **Boot sweep auto-fan-out** | `crates/k2so-daemon/src/main.rs:1063` `ensure_all_skills_up_to_date` (loop `:1030`) | **Gate** | No call-site change; gated by the two gate inversions in §4 (canonical SKILL still regenerates). |
| **Agent-create fan-out** | `crates/k2so-core/src/workspace/agent.rs:272` `write_agent_skill_file` | **Gate** | Funnels through `write_skill_to_all_harnesses` → gated at `writer.rs:185`. |
| **Agent-launch fan-out** | `crates/k2so-core/src/workspace/agent_launch.rs:159` `write_workspace_skill_file` | **Gate** | Funnels through `skill_regen.rs:285` gate. |
| **Explicit regen** | `crates/k2so-daemon/src/cli.rs:890` (`regenerate_skills`), `:1330`, `:1349` | **Gate** | Same two gates; no special-casing. |
| **Gate check #1** | `crates/k2so-core/src/skills/writer.rs:185` | **Replace** | Invert to consult `harness_fanout_enabled` (§4). Canonical SKILL write above (`:170-179`) stays. |
| **Gate check #2** | `crates/k2so-core/src/workspace/skill_regen.rs:285` | **Replace** | Invert to consult `harness_fanout_enabled` (§4). |
| **Symlink primitive** in fan-out | `writer.rs:128` `force_symlink`; `:192/197/205` | **Replace (new flow) / retain (legacy)** | New opt-in writes copies; symlinks retained only for already-unified legacy workspaces. |
| **`.skip-harness-management` opt-out** | `crates/k2so-core/src/workspace/onboarding.rs:87/99/105/118` | **Replace (keep as legacy alias)** | The positive `harness_fanout_enabled` marker is the opt-IN; `.skip-harness-management` presence keeps forcing `false`. |
| `scan_harness_files` (respects skip flag, fed the consent picker) | `crates/k2so-core/src/workspace/onboarding.rs` (`HARNESS_PROBES:130`) | **Repurpose** | Picker consumer removed; the probe list feeds the K2 Canonical Agent's diagnose-then-intent enumeration. |
| Settings "Workspace Knowledge / harness" copy implying always-on fan-out | `ProjectsSection.tsx:1252`, `:2126`, `:2169` | **Gate / reword** | "compiles into every harness" is only true when fan-out is enabled; reword to reflect opt-in. |
| Agent context diagram "12 harnesses, fans out to every harness" | `AgentContextDiagram.tsx:205`, `:244-245` | **Reword** | Now opt-in + per-harness; "12" overstates the ~10 code targets. |
| `0.37.0` `.k2so/`-internal layout migration + sentinel | `crates/k2so-core/src/migrations/unification_0_37_0.rs:36`; boot `main.rs:436`/`:691` | **OUT of scope — keep** | Internal `.k2so/` reshuffle; does not touch user harness files. |

**Confirmation:** the consent page is found and removable. It is `AddWorkspaceDialog.tsx:31-439`, whose only structural job is Skip / Start-Fresh / Adopt — all three steer the auto-fan-out. With fan-out off by default, none are needed. **Value pitch preserved:** the plain-language explanation at `:147-164` ("Each AI coding tool reads its project notes from a different file… write your context once; every tool sees the same picture") relocates verbatim into the K2 Canonical Agent skill briefing and the button subtitle.

---

## 8. Enable → offer → run flow + internal build order

### 8.1 Enable → offer → run (all three skills)
1. **Enable** — enabling a skill writes its file into `.k2so/skills/<name>/SKILL.md` (deterministic core write, kind (a)).
2. **Offer** — the user is asked **"run it now?"** → launches the editor UI (§9).
3. **Defer** — if declined, it runs later via the **button in Workspace Settings → Agent section** (§9.3).

### 8.2 Internal build order (ONE feature, not phased releases)

**Build order 0 — Stop the scary thing.** Flip fan-out off by default (invert the two gates `writer.rs:185` + `skill_regen.rs:285` per §4; `.skip-harness-management` stays a legacy false-forcer). Add `detect_canonical_state` + the migration that sets the flag true only for already-unified workspaces (no auto-revert). **Remove the consent page** (`AddWorkspaceDialog` + onboarding store/commands; relocate the WHY copy).

**Build order 1 — Safety net core.** `harness::run_canonical_setup`, `harness::unwind_canonical`, `harness::persist_agent_md` (§6). Tests: idempotent re-run, no-clobber, reversibility round-trip, per-harness independence, active-session refusal.

**Build order 2 — The three skill bodies.** `content.rs` generators: reframe `generate_manager_skill_content` (`:170`) → **Workspace Manager** skill (role knowledge + organic-integration instruction); reframe `generate_k2so_agent_skill_content` (`:457`) → **K2 Agent** skill; new `generate_canonical_agent_skill_content` → **K2 Canonical Agent** (diagnose→intent→ask-harnesses→merge-into-Model-A→mirror-out→dry-run→confirm→apply→report→unwind). Write each to `.k2so/skills/<name>/SKILL.md` on demand, upgrade-tracked, internal (not fanned out). Snapshot-tested; assert no deprecated verbs; assert the role skills instruct **organic** integration (never programmatic injection).

**Build order 3 — The UIs + buttons.** Role-skill editors reuse AIFileEditor pointed at `AGENT.md` (§9.1); K2 Canonical Agent gets its modal + plan/manifest preview (§9.2); buttons in the Agent section keyed by role + the always-on canonical button (§9.3). Add `disableSessionResume` to AIFileEditor. Tests: snapshot the seed prompts (encode the safety/organic contract — must not drift); button visibility per mode.

**Build order 4 — Polish.** Git-status-quiet regen (content-hash gate on copies so boots don't churn diffs); the Agent Skills settings section refresh (§11 / §I); k2.dev / standalone-skill (#585) tie-in.

---

## 9. The two UIs

### 9.1 Workspace Manager & K2 Agent → normal AIFileEditor on `AGENT.md`
Both role skills open the **normal AIFileEditor** pointed at the agent's `AGENT.md` (`.k2so/agent/AGENT.md`): file/preview on the left, agent terminal on the right. This is the **same pattern** as the existing persona editors — `AgentPersonaEditor` / `ClaudeMdEditor`, dispatched at `ProjectsSection.tsx:1071-1077`, and launched by `K2SOAgentPersonaButton` / `CustomAgentPersonaButton` (`ProjectsSection.tsx:1416/1421`). The agent **organically integrates** the role knowledge into the existing `AGENT.md` (§3.2); the left preview shows `AGENT.md` updating live as it does.

- **Seed (CLI args, not typed):** mirror the `ProjectContextEditor` exemplar at `ProjectsSection.tsx:1975-1986` = `[...parseCommand(preset).args, '--append-system-prompt', <role-briefing>, "<final positional seed referencing the role skill by name>"]`.
- **Dispatch:** add `'__workspace_manager__'` and `'__k2_agent__'` arms to the `agentEditorName` switch at `ProjectsSection.tsx:1055-1080` (drop-in, alongside `__project_context__` / `__claude_md__`).
- The role-briefing seed says: *"Run the <role> skill. READ the existing AGENT.md, weave the role guidance in organically with judgment, PRESERVE existing context, never inject a templated block. The deterministic core backs up AGENT.md and writes your merged text atomically."*

### 9.2 K2 Canonical Agent → modal with the agent running + plan/manifest preview
**Correction to the v1 draft, which implied a file-watch preview pane.** The K2 Canonical Agent UI is a **modal with the agent running**, seeded with a prompt that starts the skill at session start (the **same `--append-system-prompt` + initial-message seed mechanism** AIFileEditor uses). It has **NO single-file editor**. Instead it **DOES show a plan/manifest preview rendered visually** — the per-harness action table (`merge` / `create` / `back-up-then-write` / `marker-inject` / `skip-already-managed`) and the backup manifest — **styled consistent with the refreshed Agent Skills settings section** (§11).

- Plan written to `.k2so/.canonical-setup/plan.md`; manifest to `.k2so/backups/<ts>/manifest.json`; both feed the visual preview (not a raw file-watch editor pane).
- New dispatch arm `'__canonical_agent__'` at `ProjectsSection.tsx:1055-1080` renders the modal (sibling of the persona editors, but a distinct component: agent terminal + structured plan/manifest renderer, no `<FilePreview>`).
- **Seed (setup mode):** *"Run the K2 Canonical Agent skill. Detect per-harness canonical state, summarize it, ask which harnesses I want and what I want to do. Pull existing harness content INTO AGENT.md/PROJECT.md first (Model A), then mirror out. Produce a DRY-RUN plan to .k2so/.canonical-setup/plan.md and STOP for confirmation before writing."*
- **Seed (manage/undo mode):** *"Run the K2 Canonical Agent skill in manage mode: show the current per-harness state and the exact undo. If I confirm, run the manifest-driven unwind for the harnesses I choose."*
- `disableSessionResume=true` (one-shot ceremony wants a fresh session). Live running-agent preflight refuses if a session is reading the targets.

### 9.3 Button placement + availability (Agent section)
All three buttons live in **Workspace Settings → Agent section** (the block at `ProjectsSection.tsx:1414-1452`, where `CustomAgentPersonaButton` / `K2SOAgentPersonaButton` / `ProjectAgentsPanel` already render per-mode):

| Button | Shown when | Opens |
|---|---|---|
| **Workspace Manager** | role = Manager (`agentMode ∈ {manager, coordinator, pod}`) | AIFileEditor on `AGENT.md` (§9.1) |
| **K2 Agent** | role = K2 Agent (`agentMode === 'agent'`) | AIFileEditor on `AGENT.md` (§9.1) |
| **K2 Canonical Agent** | **ALWAYS** — every agent type, **including custom and agent-mode OFF** | Modal + plan/manifest preview (§9.2) |

Rationale for always-on canonical: canonicalizing `CLAUDE.md` / `GEMINI.md` is useful to **anyone**, independent of whether the workspace runs a K2 agent. Note: the current Agent section hides `ProjectAgentsPanel` when `agentMode === 'off'` (`:1441`); the K2 Canonical Agent button must be rendered **outside** that gate so it shows even when off.

Each button label gates on `detect_canonical_state` (for canonical) / skill-present state (for role skills): **"Set up …"** vs **"Manage / Undo"** / **"Re-run …"**.

---

## 10. Appendix — code anchors (verified, this worktree)

- **Fan-out core:** `crates/k2so-core/src/skills/writer.rs:153` `write_skill_to_all_harnesses` (canonical write `:170-179`; **gate `:185`**; symlinks `:192/197/205`; markers `:211-214`); symlink primitive `:128` `force_symlink`.
- **Root + discovery fan-out:** `crates/k2so-core/src/workspace/skill_regen.rs:285` (**gate**), `:296` root SKILL symlink, `:298` `migrate_and_symlink_root_claude_md` (def `:566`), `:299` discovery targets.
- **Harness root files:** `crates/k2so-core/src/workspace/harness.rs:44` `HARNESS_WORKSPACE_FILES`, `:81` `safe_symlink_harness_file`, `:129` `write_workspace_harness_discovery_targets`, `:201` `scaffold_aider_conf`.
- **Opt-out flag:** `crates/k2so-core/src/workspace/onboarding.rs:87/99/105/118`; probe list `:130` `HARNESS_PROBES`; `adopt_harness_as_project_md` / `scan_harness_files`.
- **Triggers:** boot `crates/k2so-daemon/src/main.rs:1063` (loop `:1030`); create `crates/k2so-core/src/workspace/agent.rs:272`; launch `crates/k2so-core/src/workspace/agent_launch.rs:159`; regen `crates/k2so-daemon/src/cli.rs:890/1330/1349`.
- **Reverse primitive:** `crates/k2so-core/src/workspace/teardown.rs:76` `teardown_workspace_harness_files` (`RestoreOriginal`).
- **Single-agent layout (Model A):** `.k2so/agent/AGENT.md` — `crates/k2so-core/src/workspace/agent.rs:213-214` (canonical primary), `:227-235` (scaffold-at-canonical), `:312` (no-op if exists); `.k2so/PROJECT.md` read at `crates/k2so-core/src/skills/content.rs:711`.
- **Role detection / mode:** `agentMode ∈ off|custom|agent|manager` (legacy `coordinator`/`pod` → manager) — `ProjectsSection.tsx:1406-1409`, `:1290`, `:1448`.
- **Skill content generators:** `crates/k2so-core/src/skills/content.rs:170` `generate_manager_skill_content` (→ Workspace Manager skill), `:369` `generate_custom_agent_skill_content`, `:457` `generate_k2so_agent_skill_content` (→ K2 Agent skill), `:627` `generate_template_skill_content`.
- **Consent page (remove):** `src/renderer/components/AddWorkspaceDialog/AddWorkspaceDialog.tsx:31-439` (WHY copy `:147-164`; skip `:83-97`; adopt `:99-119`); store `src/renderer/stores/add-workspace-dialog.ts`; Tauri `src-tauri/src/commands/k2so_agents.rs:685/709`, registered `src-tauri/src/lib.rs:1051-1052`.
- **Button wiring (reuse):** AIFileEditor (`src/renderer/components/AIFileEditor/AIFileEditor.tsx`) — spawn, resume, watcher, multi-file; `TerminalPane.tsx:676-691` spawn forward; `ProjectsSection.tsx:1055-1080` dispatch, `:1071-1077` persona/claude-md arms, `:1975-1986` `terminalArgs` exemplar, `:1414-1452` Agent section blocks, `:1416/1421` persona buttons, `:1443` `ProjectAgentsPanel`.
- **Agent Skills settings section (§11 / §I dependency):** `src/renderer/components/Settings/sections/AgentSkillsSection.tsx` (tier tabs `:27-32`; `SkillTier` `manager|k2so_agent|agent_template|custom_agent` `:18`; locked-layer maps `:47-93`; `AGENT_SKILLS_MANIFEST` `:10-16`; `AgentContextDiagram` `:227`).
- **0.37.0 migration (out of scope):** `crates/k2so-core/src/migrations/unification_0_37_0.rs:36` sentinel; `crates/k2so-daemon/src/main.rs:436/691`.

---

## 11. Related work — Agent Skills settings section refresh (§I dependency)

The existing **Agent Skills** settings section — `src/renderer/components/Settings/sections/AgentSkillsSection.tsx` — needs updating to reflect the **post-`agents/`-removal reality**: each workspace has a **single agent under `.k2so/agent/`** + skills under `.k2so/skills/` (no more `.k2so/agents/<name>/`).

Current state worth noting (verified):
- The section is a **four-tab tier picker** (`SkillTier = manager | k2so_agent | agent_template | custom_agent`, `:18`; tabs `:27-32`) with locked auto-layers per tier (`LOCKED_LAYERS` `:47-78`) and user layers stored under `~/.k2so/templates/` (`:240`).
- Its mental model is the **old multi-agent tree** (per-tier "shipped to that specific kind of agent", `:205`; `AGENT_SKILLS_MANIFEST` `:10-16`). Post-removal there is **one** agent per workspace; the four-tier framing overstates the structure.

Required changes (keep it **simple** to build user confidence):
- Reframe around **one agent + a flat skills list** under `.k2so/skills/`, surfacing the three opt-in skills of this PRD (Workspace Manager / K2 Agent / K2 Canonical Agent) as first-class entries.
- Drop / simplify the per-tier "kind of agent" framing now that the workspace IS one agent.
- The **K2 Canonical Agent plan/manifest preview (§9.2) must be visually consistent with this refreshed section** — same card/border/typography idiom (the `border + bg-elevated` blocks, `:232`, `:249`).

This is a **dependency** of build order 3-4, not a blocker for the gate flip (build order 0).

---

## 12. Supersession of #585 / #586
- **#586 (make canonical AGENT.md/SKILL.md optional)** is **fully delivered** by §4 (off-by-default gate) + §8.1 (opt-in skills) + §6 (copies not symlinks). The optionality #586 asked for is now the default posture.
- **#585 (publish standalone K2SO Skill)** folds in as the **distribution vector**: the three skills are authored as `content.rs` generators and written on demand to `.k2so/skills/<name>/SKILL.md` — the same authoring/publishing path #585 needs. The k2.dev install-page tie-in lands in build order 4. Both tasks close against this PRD.

---

## 13. Open questions (genuinely-open implementation details only)

Resolved-and-removed since v1: role-knowledge mechanism (now organic, §3 — was the marked-block/hash-gate open question); preview mechanism for canonical (now structured plan/manifest modal, §9.2 — was the file-watch pane); source of truth for content (now Model A on `AGENT.md`/`PROJECT.md`, §5.1); refresh mechanism (now "re-run the skill", §3.4); ship cadence (now one feature, §8).

Still open:
1. **Per-harness flag granularity:** one workspace-level `harness_fanout_enabled` bool, or genuinely per-harness flags persisted (so "Claude on, Gemini off" survives)? The per-harness manifest argues for **per-harness persisted state** — confirm.
2. **`.codex/` and `.gemini/` directories** are not touched by code today (only `GEMINI.md` at root). Per-file SKILL discovery is the scope (§5.3); confirm we are NOT adding directory-level canonicalization in this feature.
3. **Legacy symlink → copy conversion:** offer already-unified users a one-click "convert symlinks to copies," or leave their working symlinks alone unless they unwind+re-run? (Lean: leave alone; offer in manage mode.)
4. **Confirm UX boundary (canonical):** confirmation is conversational (user types "yes" to the agent). Add a hard "Confirm & write" button in the plan/manifest preview header (needs a PTY-write seam), or keep chat-only for v1? (Lean: chat-only v1.)
5. **Provider coverage:** the seed-as-args path is Claude-gated (every existing AIFileEditor caller is). For non-Claude default agents: require Claude for these flows, type the seed into the PTY, or run a headless CLI path? (Lean: require Claude v1, headless fallback later.)
6. **Role-skill safety-net scope:** the role skills only ever touch `AGENT.md` (one content-rich file). Confirm `persist_agent_md` reuses the full backup+manifest machinery (vs a lighter single-file backup) so role re-runs are byte-reversible like canonical.
7. **Agent Skills section refresh depth (§11):** full reframe to one-agent + flat skills list now, or minimal copy/label fix now and the deeper restructure as a fast-follow? (Lean: minimal-but-coherent now, keep it simple per §I.)
