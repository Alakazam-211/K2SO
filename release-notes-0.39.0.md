# 0.39.0 — First public release: new CLI taxonomy + consolidated storage shapes

K2SO 0.39.0 is the **first public release** after a multi-month refactor of
the CLI surface, the on-disk storage layout, and the daemon's module
structure. The driving idea: K2SO is a *workspace orchestration layer* —
inbox, heartbeats, skills, reviews, cross-workspace messaging — and modern
harnesses (Claude Code, Cursor, etc.) own sub-agent spawn natively. K2SO no
longer tries to be a worktree spawner; it tries to be the place every
agent looks for *what to do next* and *who else is around*.

The headline change is the **new 24-verb CLI taxonomy** (Phase 2.1 A25),
which splits commands into three discoverability tiers (daily / advanced /
internal). Most of what landed under the hood is in service of that
reframing.

> ⚠️ **One-time upgrade migration:** On first daemon boot after upgrading,
> any pre-0.39.0 workspace with `.k2so/work/{inbox,active,done}/` content
> is auto-migrated to `.k2so/inbox/{,active,done}/`. The migration is
> atomic per-file (same-filesystem rename, no copy+delete races) and
> idempotent — a marker file at `.k2so/.work-to-inbox-migration-v1-done`
> short-circuits subsequent boots. After the move, the now-empty
> `.k2so/work/` folder is sent to the macOS Recycle Bin (recoverable for
> ~30 days if you had unexpected content under there).
>
> Skills get the same treatment: `.k2so/agents/<name>/` and
> `.k2so/agent-templates/` consolidate into `.k2so/skills/<name>/` on
> first boot per workspace. Originals also go to Trash.

---

## What's new

### New CLI taxonomy: 24 verbs across three tiers

Run `k2so help` for the daily tier, `k2so help --advanced` for the
power-user tier, and `k2so help --internal` for the orchestrator-RPC tier.

**Daily tier (14 verbs — what users actually type):**

| Verb | Purpose |
|------|---------|
| `msg <workspace> "text"` | Cross-workspace messaging (live, blocks until landed) |
| `who` | Workspaces with live agents right now |
| `connections` | List/add/remove cross-workspace links |
| `inbox` | Per-workspace email-style work queue |
| `checkin` | Heartbeat ping / status / done |
| `done` | Shortcut for `checkin --done` |
| `workspace` | Workspace registration + agent profile management |
| `reviews` | List pending merge reviews |
| `review` | Approve / reject / feedback a pending review |
| `activity` | Audit log of workspace events (renamed from `feed`) |
| `commit` | AI-assisted commit (with `--merge` to also merge) |
| `glossary` | Define K2SO-specific terms (workspace, skill, heartbeat, ...) |
| `whats-new` | Changelog popup |
| `help` | Tiered help system + `help-deprecated` retired-verb map |

**Advanced tier (6 verbs — power-user / scheduling / install):**

| Verb | Purpose |
|------|---------|
| `heartbeat` | Workspace-scoped scheduled wakes (`heartbeat schedule add`, `heartbeat signal fire`, ...) |
| `daemon` | Daemon lifecycle (`status`, `start`, `stop`, `restart`, `log`, `companion`, `uninstall`) |
| `settings` | Show/modify workspace settings (`--mode`, `--state`, `--agentic`, `--companion`) |
| `skills` | Documentation profiles (`list`, `create`, `remove`, `profile`, `regenerate`) |
| `update` | App + CLI updates (`--app`, `--cli`, `--list`) |
| `onboarding` | First-launch flow (`scan`, `adopt`, `defer`, `start-fresh`) |

**Internal tier (4 verbs — orchestrators only; humans rarely run these):**

| Verb | Purpose |
|------|---------|
| `terminal` | Raw PTY I/O (`spawn`, `write`, `read`) |
| `sessions` | Low-level session lifecycle (`spawn`, `list`, `live`, `compact`) |
| `agent` | Single-item ops on the workspace's primary agent (`update`, `complete`) |
| `hooks` | Claude Code / Cursor hook pipeline state |

### Hard-deprecated verbs (and what to use instead)

Every verb below now exits non-zero with a pointer to its replacement.
There is no transition window — Phase 2.1a kept warn-and-forward for one
release; Phase 2.1b cut that completely. Run `k2so help-deprecated` to
get this list from the CLI itself.

**Top-level retirements:**

| Old verb | New equivalent |
|----------|---------------|
| `delegate <agent> <file>` | Your harness's sub-agent / worktree feature (Claude Code's sub-agent, Cursor's worktree). K2SO no longer manages the spawn lifecycle. Use `skills profile <name>` to surface SKILL.md content. |
| `agentic on\|off` | `settings --agentic <on\|off>` |
| `state list\|get\|set` | `settings --state <id>` |
| `mode off\|agent\|manager` | `settings --mode <value>` |
| `app-update` | `update --app` |
| `commit-merge` | `commit --merge` |
| `companion` | `daemon companion <start\|stop\|status>` |
| `whatsnew` | `whats-new` |
| `roster` | `who` |
| `feed` | `activity` |
| `signal <target> <kind> <payload>` | `msg <workspace> --signal <kind> [--payload <json>]` |
| `status "msg"` (top-level) | `checkin --status "msg"` |

**`agents *` subverbs — all retired (Phase 2.1b hard-cut):**

| Old verb | New equivalent |
|----------|---------------|
| `agents list` | `workspace list` (yellow pages) or `skills list` (skill profiles) |
| `agents work <n>` | `inbox` (workspace-implicit; pass `--workspace <path>` to target another) |
| `agents status <n>` | `workspace status` or `checkin --status` |
| `agents create` | `skills create <name>` (or register a new workspace via your IDE/harness) |
| `agents delete` | `skills remove <name>` or `workspace remove <path>` |
| `agents launch` | Your IDE/harness's session-start feature; K2SO no longer manages session spawn |
| `agents generate-md` | `skills regenerate [<name>]` |
| `agents profile` | `skills profile <name>` or `workspace profile` |
| `agents lock` | `skills lock <name>` (debugging only; rarely needed) |
| `agents unlock` | `skills lock <name> --release` |
| `agents triage` | `workspace triage` |
| `agents running` | `workspace list --running` |
| `agents reap` | `daemon reap` |

**`work *` subverbs — all retired:**

| Old verb | New equivalent |
|----------|---------------|
| `work create --title --body` | `inbox compose --title "..." --body "..."` |
| `work inbox` | `inbox` (default verb shows new arrivals) |
| `work send <ws> --title --body` | `msg <ws> --inbox --title "..." --body "..."` |
| `work move --from <a> --to <b>` | `inbox move <id> <folder>` |
| `work done` | `inbox archive <id>` |

**`agent <subverb>` (singular) — `update` and `complete` survive as
internal-tier verbs; `create`, `delete`, `list` are retired. Use
`skills create / remove / list` or `workspace list` instead.**

**`--agent <name>` flag — hard-removed across every verb.** Pass the
workspace explicitly with `--workspace <path>` when targeting another
workspace.

### Storage shape changes

Two storage primitives consolidated to flatter, more obvious shapes:

- **Work queue:** `.k2so/work/{inbox,active,done}/` → `.k2so/inbox/{,active,done}/`.
  Top-level inbox items live at `.k2so/inbox/*.md` (was `.k2so/work/inbox/*.md`).
  Active and done buckets get the same flat treatment.
- **Skills:** `.k2so/agents/<name>/` and `.k2so/agent-templates/` →
  `.k2so/skills/<name>/`. SKILL.md is the canonical filename
  (AGENT.md kept as a back-compat read path during the transition).

Both migrations run automatically on first daemon boot per upgraded
workspace. Originals are recycled to the macOS Trash (recoverable for
~30 days), and a marker file prevents repeat migration on subsequent
boots.

### Codebase reorganization

The `crates/k2so-core/src/agents/` directory has been retired. Modules
moved to topic-scoped homes that match the new CLI taxonomy:

- `workspace/` — workspace lifecycle, settings, sessions, identity, reviews, harness, agent editor, relations
- `skills/` — skill writer, content generators, CRUD, versioning
- `heartbeats/` — heartbeat install, cron schedule parsing, control
- `migrations/` — one-shot data migrations (e.g. unification 0.37.0)
- `connections.rs` — cross-workspace links (was `agents/connections.rs`)
- `inbox.rs` — inbox primitives + the work→inbox first-boot migration

The `agents::*` back-compat aliases have been removed; every call site
references the canonical post-split path.

### Test foundation

Pre-0.39.0 ship included a comprehensive Tier 1/2/3 test cleanup:

- **907 → 948 tests** (~5% growth, all green)
- Inline tests for `workspace::{relations, settings, agent_editor, harness, skill_writer, agent_launch}`
- Inline tests for daemon `CliResponse` constructors, `token_ok`, dispatch, param helpers
- Snapshot tests for `skills/content.rs` generators
- Vitest coverage for `hasLoadedFromDaemon` persist gate
- Inbox + heartbeat coexistence integration test (Tier 2.4)
- Hard-deprecation shell tests under `tests/cli/` — every retired verb
  has a dedicated `*_hard_deprecated.sh` script that asserts the new
  410-style exit + replacement-pointer message
- Cross-provider chat-history dedup regression coverage (Gemini + Cursor)
- `tests/cli/no_lead_sentinel_remains.sh` greps the tree for stale
  references to the retired `__lead__` agent name

### Bug fixes shipping in 0.39.0

- **App Settings race (F3) closed.** The two-writer race between Tauri
  `settings_update` and the daemon `companion_host` write path is now
  serialized via a daemon-side global `SETTINGS_LOCK`. Tauri proxies
  to `POST /cli/settings/update` instead of writing the file directly;
  companion-affecting changes invalidate live sessions in-process
  after the durable write. Side benefit: settings work in
  headless-daemon deployments (no Tauri required).
- **Daemon port stability + frontend settings-load race** fixed in
  0.39.0h Phase 2.5 (commit `29134ebb`). Adds a `chat_history_list`
  daemon command to round out the surface.
- **`skill_writer` no longer emits deprecated verbs** into regenerated
  SKILL.md files (Phase 2.1f baseline refresh, commit `b2b24e7d`).
- **Chat-history parser dedup:** the Gemini parser now dedupes by
  `sessionId` header (closes the React-key-collision bug surfaced in
  audit #550, commit `783c9327`). The Cursor parser already had an
  equivalent `best_by_id` collapse path; new test
  `parse_cursor_sessions_dedupes_chat_id_across_hash_dirs` locks that
  in. Pi + Codex parsers do **not** dedupe today — noted in the Tier 1
  audit, deferred to a follow-up release.
- **Scratch-path bypass for safe_delete** added then reverted after
  surfacing a test flake (commits `cde4b371` + `1fb7c693`); the
  original `safe_delete::trash` path is back in place. macOS-Trash
  tests are still gated as known-flaky during local `cargo test`
  runs (Finder Touch ID prompts).
- **Schema cleanups:** migrations 0046/0047/0048/0049 prune legacy
  layout tables, drop the `agent_sessions_archive` table, remove
  index artifacts, and clear the `__lead__` sentinel out of the
  activity feed.
- **Auto-pin of agent-mode workspaces retired + one-time upgrade
  migration** (commits `4b517687` + `5fa1a562` + `6efd4b40` +
  `b77f818c` + auto_pin_existing_agents_0_39_0). The "AGENTS & PINNED"
  auto-promote behavior is gone from the collapsed icon rail, the
  expanded sidebar, AND the Workspaces Settings page. To avoid users
  thinking their agents "disappeared" on upgrade, a one-shot daemon
  migration (gated by `code_migrations`, fires once per local DB)
  flips `pinned = 1` for every workspace currently in agent mode
  (agent / custom / manager / coordinator / pod) that isn't already
  pinned. Existing agents stay visible at the top of the Pinned list
  on first 0.39.0 launch; users can unpin via the existing UI
  affordance. Future workspaces switched into agent mode do NOT
  auto-pin — they flow through the normal Pinned / focus group /
  ungrouped sections like any other workspace. Single-list-of-
  workspaces model end-to-end.

- **Pinned Chat + Inbox tabs visible regardless of agent mode.** Per
  the workspace==agent model, every workspace IS an agent that can
  receive cross-workspace messages via `k2so msg <workspace>` — even
  workspaces with `agentMode: off`. Pre-0.39.0 the Chat + Inbox pinned
  tabs were hidden when agent mode was off, which hid the receive
  surface even though the underlying capability was always present.
  Now the tabs stay visible in every workspace; clicking the Chat tab
  spawns a CLI session (Claude Code, Codex, etc.) against the
  workspace; the Inbox tab shows incoming messages from connected
  workspaces.
- **Sidebar / Skills section polish** (commits `e395ee23`, `82c95aac`):
  Skills promoted out of Agent Settings into its own top-level section
  above Worktrees; Agent Settings details hidden when workspace mode is
  `off`.

---

## Upgrade notes

For most users on 0.38.x the upgrade is transparent:

1. **App + CLI update.** Run `k2so update --app` (or auto-prompt at
   launch) and `k2so update --cli`. Old top-level `k2so app-update`
   now hard-fails with a pointer.
2. **First daemon boot does the storage migrations.** Both
   `.k2so/work/ → .k2so/inbox/` and `.k2so/agents/ → .k2so/skills/`
   run automatically per workspace, atomic per-file, idempotent across
   reboots. Originals go to Trash.
3. **Update any scripts or shell aliases that called retired verbs.**
   Run `k2so help-deprecated` for the full old→new mapping. The CLI
   itself prints the same pointer when you hit a retired verb, so it's
   self-healing if you just run things and react to the errors.
4. **Stop typing `--agent <name>`.** That flag is gone across every
   verb. Use `--workspace <path>` to target another workspace; the
   default is always `$PWD` (or `$K2SO_PROJECT_PATH`).
5. **Skill profiles live in `.k2so/skills/<name>/SKILL.md`.** Old
   `.k2so/agents/<name>/agent.md` files were migrated and recycled.
   `AGENT.md` is still read for back-compat if you had a custom name.

If you script against K2SO and rely on a verb that retired, the
recommended pattern is:

```bash
# Old:
k2so work create --title "audit auth" --body "..."

# New:
k2so inbox compose --title "audit auth" --body "..."
```

```bash
# Old:
k2so work send my_other_ws --title "task" --body "do this"

# New:
k2so msg my_other_ws --inbox --title "task" --body "do this"
```

```bash
# Old:
k2so signal scout_v3 status "deploying"

# New:
k2so msg scout_v3 --signal status --payload '{"text":"deploying"}'
```

```bash
# Old:
k2so delegate backend-eng .k2so/agents/backend-eng/work/inbox/task.md

# New:
# Use your harness's sub-agent / worktree feature (Claude Code, Cursor).
# Reference the skill content via:
k2so skills profile backend-eng
```

Run `k2so help-deprecated` for the full mapping; the CLI itself owns
the canonical version of this list and stays in sync with the code.

## What this unlocks next

0.39.0 is the foundation for the K2SO Hosted / Companion roadmap:

- **Headless daemon.** Every CLI verb now routes through the daemon's
  HTTP surface (no rusqlite or Tauri imports in the CLI path).
  Deployments that want to run K2SO without the Tauri shell can.
- **Secure tunnel monetization.** The `daemon companion` subcommand
  is now the canonical mount-point for the ngrok-backed mobile
  companion. The `companion` top-level verb is retired in favor of
  `daemon companion <start|stop|status>`.
- **Skill marketplace surface.** With skills consolidated under
  `.k2so/skills/`, the "publish / import a skill" flow has a single
  canonical home to target.
