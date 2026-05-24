# Phase 2.1: CLI verb redesign + headless-daemon simplification

**Status**: Drafted 2026-05-23 while Unit 4 + Unit 7d run in parallel. **Blocked on Unit 4** (Phase 2.1's new workspace verbs need `/cli/workspaces/*` routes that Unit 4 adds). Launch immediately after Unit 4 + Unit 7d merge.
**Internal version markers**: 0.39.0f (alongside Phase 2 completion) or 0.39.0g (during Phase 3 typed-router workstream).
**Owner**: Rosson + pod-leader
**Date**: 2026-05-23

---

## tl;dr

`cli/k2so` is in better shape than the task #433 description assumed. It's 3,854 LoC across 96 `cmd_*` functions, of which **most are already thin shells** over `cli_request /cli/*` calls. The real work is **NOT** a top-to-bottom rewrite — it's:

1. **New verb taxonomy** — workspace-keyed verbs (yellow pages, launch, profile, update, signal) that don't exist today
2. **Deprecation** of pre-workspace-agent-unification verbs (`agents create`, `agents delete`, agent-keyed `msg`) with deprecation warnings
3. **`help-deprecated` aggregator** so users can discover the new equivalents
4. **Audit + cleanup of the ~19 functions that aren't `cli_request`-based** to confirm they're HOST or migrate them
5. **Test that `K2SO_DAEMON_URL=<remote>` works end-to-end** — the K2SO Connect proof-point for the CLI

After Phase 2.1:
- `cli/k2so` shrinks from 3,854 → ~2,500 LoC (mostly via deprecation removal + dead-verb cleanup, not from re-architecture).
- Every retained verb is a thin shell: `parse args → cli_request /cli/* → format output`. No filesystem, git, or SQL logic in the CLI script.
- `K2SO_DAEMON_URL=https://my-laptop.ngrok.app k2so workspaces list` works against a remote daemon with no CLI changes — the K2SO Connect promise extends to the CLI.

---

## Current state audit

### Stats

- **3,854 lines** in `cli/k2so`
- **96 `cmd_*` functions** total
- **114 `cli_request` calls** (most cmds use this helper; some cmds make multiple calls)
- **3 `cli_post_json` calls** (JSON-body variant for awareness publish + sessions spawn)
- **2 direct filesystem touches** (likely heartbeat-script generators that hb_parse_spec uses)
- **~19 functions don't use `cli_request`** — see fat-function audit below

### Helper

```bash
cli_request() {
    local endpoint="$1"
    shift
    local params="token=${TOKEN}&project=$(urlencode "$PROJECT")"
    for param in "$@"; do
        params="${params}&${param}"
    done
    curl -sG "${BASE_URL}${endpoint}" \
        --connect-timeout 5 --max-time 30 \
        -d "$params" 2>/dev/null
}
```

`BASE_URL` defaults to `http://127.0.0.1:${PORT}` where PORT is read from `~/.k2so/daemon.port`. **Already plumbed for arbitrary URL via env var** — Phase 2.1 needs to add explicit `K2SO_DAEMON_URL` support that overrides the localhost default for K2SO Connect.

### Fat-function audit (the ~19 not using `cli_request`)

| Function | Verdict | Notes |
|---|---|---|
| `cmd_daemon_status` | HOST | Reads `~/.k2so/daemon.port` + `.pid` + `.log` directly. Local-machine introspection. |
| `cmd_daemon_start` | HOST | `launchctl bootstrap`. Local-machine launchd ops. |
| `cmd_daemon_stop` | HOST | `launchctl bootout`. Local-machine launchd. |
| `cmd_daemon_restart` | HOST | `launchctl kickstart`. Local. |
| `cmd_daemon_log` | HOST | `tail ~/.k2so/daemon.log`. Local. |
| `cmd_daemon_uninstall` | HOST | Local launchd cleanup. |
| `cmd_signal` | MIGRATE | Posts an `AgentSignal`. Should route through `/cli/awareness/publish` or new `/cli/signal/send`. **Audit current body** — may already use cli_post_json. |
| `cmd_sessions_spawn` | MIGRATE | Already uses cli_post_json — partial. Audit. |
| `cmd_sessions_list` | MIGRATE | Should hit `/cli/sessions/list` (verify it exists). |
| `cmd_sessions_compact` | MIGRATE | Should hit `/cli/sessions/compact` or equivalent. |
| `cmd_sessions` | MIGRATE | Top-level dispatcher for `sessions` subcommands. |
| `cmd_roster` | MIGRATE | Companion roster; likely needs `/cli/companion/roster`. |
| `cmd_workspace_preview` | MIGRATE | Was `k2so_agents_preview_workspace_ingest` (Unit 7b/7d migrated to k2so-core). Daemon route needed; verify. |
| `cmd_help`, `cmd_help_msg`, `cmd_help_general`, `cmd_help_advanced` | HOST | Static help text. Local. |
| `cmd_app_update` | HOST | Runs the Tauri auto-updater path. Local. |
| `cmd_update` | HOST | Alias / dispatcher for `app_update`. |

**HOST count**: 10 functions stay local (daemon lifecycle + help + updater)
**MIGRATE count**: ~7 functions need to route through `/cli/*` (signal/sessions/roster/preview cluster)

### Daemon lifecycle is HOST — not "the CLI manages remote daemons"

`k2so daemon start/stop/restart/uninstall` operate on the **local** launchd. In K2SO Connect mode (CLI on Machine A pointing at daemon on Machine B), these verbs are meaningless — you can't manage a remote daemon via launchctl. The verbs should either:
- (a) error gracefully when `K2SO_DAEMON_URL` is set to a non-localhost address, OR
- (b) be hidden in Connect mode entirely

Phase 2.1 should pick one; **recommendation: (a)** with a friendly error message ("`k2so daemon start` only works against the local daemon; use the host machine's CLI for daemon lifecycle"). Avoids hiding verbs based on environment which gets confusing.

---

## New verb taxonomy (workspace-keyed)

The current CLI has agent-keyed verbs (legacy: `k2so msg --agent <name>`, `k2so agents create`, etc.) that pre-date the workspace-agent unification (0.37.0 trio: 0.37.4 / 0.37.5 / 0.37.6). Phase 2.1 finishes that unification by switching the CLI to workspace-keyed verbs throughout.

### New verbs (added)

| Verb | Daemon route (NEW unless noted) | What it does |
|---|---|---|
| `k2so workspaces list` | `/cli/workspaces/list` (Unit 4) | **Yellow pages**: every registered workspace + its agent + status (alive/sleeping/cold) + last activity time. Replaces and extends `k2so agents list`. |
| `k2so workspaces running` | NEW `/cli/workspaces/running` | Just the workspaces with a live agent session. Replaces `k2so agents running`. |
| `k2so workspace launch [--workspace <path>]` | NEW `/cli/workspaces/launch` | Smart cascade: if agent is alive, attach; if sleeping, wake; if cold, spawn. Single verb covers all three paths users currently navigate via separate commands. |
| `k2so workspace profile [--workspace <path>]` | NEW `/cli/workspaces/profile` | Reads `.k2so/agent/AGENT.md` for the workspace's primary agent. |
| `k2so workspace update --field <f> --value <v>` | NEW `/cli/workspaces/update` | Edits the workspace's primary agent profile fields (role, persona, etc.). |
| `k2so signal --workspace <path> <kind> <payload>` | EXISTS `/cli/awareness/publish` (extend) | Workspace-keyed signal addressing parallel to `msg`. Today `cmd_signal` exists but likely uses agent-keyed addressing — update to workspace-keyed. |
| `k2so template list/create/delete` | NEW `/cli/templates/*` | Manages `.k2so/agent-templates/`. Replaces `k2so agent template *`. |
| `k2so help-deprecated` | None (static text) | Lists every retired verb + its new equivalent. |

### Behavior changes

| Today | Phase 2.1 |
|---|---|
| `k2so work create --agent <name> ...` | `k2so work create ...` (workspace-implicit; `--agent` becomes ignored with a deprecation warning) |
| `k2so agents create <name> --role ...` | `k2so help-deprecated agents-create` → "Workspaces own their primary agent; bare-agent CRUD is gone. Use `k2so workspace launch` or `k2so workspace update`." |
| `k2so agents delete <name>` | `k2so help-deprecated agents-delete` → "Workspace agents are deleted by removing the workspace via `k2so workspace remove` or in the UI." |
| `k2so msg --agent <name>` | Already inbox-default (0.37.0); `--agent` works but deprecation hint shown. Encourage `k2so msg --workspace <path>`. |

### Deprecation policy

- **Soft deprecation** (deprecation warning, verb still works): keep behavior; print one-line warning to stderr the first time per shell session. Verbs: `agents list` (alias for `workspaces list`), `agents running` (alias for `workspaces running`), `msg --agent`, `work create --agent`.
- **Hard deprecation** (verb removed, error message points at replacement): `agents create`, `agents delete`, `agent template *`.
- **`k2so help-deprecated`**: prints every hard-deprecated verb + its replacement. Each hard-deprecated verb's error message also references `help-deprecated`.

---

## File-by-file plan

### `cli/k2so` changes

**Add (~150 LoC):**
- `cli/k2so::cli_request` — extend to honor `K2SO_DAEMON_URL` env override. If set, ignore `~/.k2so/daemon.port` and use the provided URL. Token comes from a per-host file at `~/.k2so/connect-hosts/<host>.token` or from `K2SO_DAEMON_TOKEN` env.
- `cmd_workspaces_list`, `cmd_workspaces_running` — thin `cli_request` shells.
- `cmd_workspace_launch`, `cmd_workspace_profile`, `cmd_workspace_update` — thin shells.
- `cmd_signal_workspace` — workspace-keyed; replaces or augments existing `cmd_signal`.
- `cmd_template_list`, `cmd_template_create`, `cmd_template_delete` — thin shells.
- `cmd_help_deprecated` — static text emitting deprecation map.
- Deprecation warning helpers (`warn_deprecated_once "agents-create"`).

**Remove (~1,200 LoC):**
- Hard-deprecated functions: `cmd_agents_create` (~25 LoC), `cmd_agents_delete` (~25 LoC), agent-template functions (~80 LoC each, several).
- Dead code paths and stale comments.
- Duplicate dispatch arms (the current dispatcher has some duplication; consolidate).

**Audit + possibly migrate (~200 LoC):**
- The ~7 MIGRATE-marked functions above (signal/sessions/roster/preview cluster). Each gets its body inspected; if it's already a `cli_request` call with extra formatting, leave it; if it's doing real work locally, route through a daemon endpoint.

**Net**: 3,854 → ~2,800 LoC (slightly higher than the original ~2,500 estimate, because we're adding more new verbs than I initially scoped).

### Daemon side (`crates/k2so-daemon/`) changes

New routes (assumes Unit 4 already added `/cli/workspaces/{list, create, delete}`):

- `/cli/workspaces/running` — filter list by `status='alive'`
- `/cli/workspaces/launch` — smart cascade. Returns the session id on success. POST.
- `/cli/workspaces/profile` — returns the workspace agent's AGENT.md content
- `/cli/workspaces/update` — PATCH-style field update. POST with `{"field": "...", "value": "..."}`.
- `/cli/templates/{list, create, delete}` — read/write `.k2so/agent-templates/` (look for existing core helpers; the agent template code might already live in k2so-core after Unit 7d).

**All POST routes get inline method gates** per the established pattern (Unit 5 / 7a / 7c references).

### k2so-core changes (if needed)

If the new "smart cascade" launch logic doesn't have a home: new `k2so_core::agents::workspace::smart_launch(workspace_path)` helper that:
1. Checks if a live session exists in `v2_session_map`
2. If yes: returns the session id
3. If no: checks `pending_live` for queued signals
4. If queued: spawns + drains
5. If cold: spawns fresh

Most of this logic probably already exists across `spawn.rs` and `providers.rs` — just needs a single public entry point.

### Renderer

**No renderer changes.** The CLI is independent of the renderer.

---

## K2SO Connect test cases

These are the proof-points that show the CLI works against a remote daemon, not just localhost:

1. **List remote workspaces**:
   ```bash
   K2SO_DAEMON_URL=https://my-laptop.ngrok.app k2so workspaces list
   ```
   Returns the remote machine's workspaces, not localhost's. Validates: env-var override, ngrok TLS, token resolution.

2. **Launch a remote workspace's agent**:
   ```bash
   K2SO_DAEMON_URL=https://... k2so workspace launch --workspace /Users/rosson/Projects/k2so
   ```
   Spawns/attaches the agent on the remote machine. Validates: workspace-keyed addressing over network.

3. **Send a signal cross-machine**:
   ```bash
   K2SO_DAEMON_URL=https://... k2so signal --workspace /path/on/remote heartbeat "wake up"
   ```
   Delivers a signal to a workspace on the remote daemon. Validates: end-to-end awareness bus over network.

4. **Daemon lifecycle gracefully refuses**:
   ```bash
   K2SO_DAEMON_URL=https://... k2so daemon restart
   # → "Error: `k2so daemon restart` only works against the local daemon (daemon URL is https://...)
   #    Run this command on the host machine that owns the daemon."
   ```
   Validates the HOST-only guard.

5. **Test against the bundled daemon** (no `K2SO_DAEMON_URL`):
   ```bash
   k2so workspaces list
   ```
   Still works — defaults to `http://127.0.0.1:<port>` from `~/.k2so/daemon.port`. Backwards-compatible.

---

## Tests (new + updated)

- `tests/cli/workspaces_list_yellow_pages.sh` — register 3 workspaces, run `k2so workspaces list`, assert all 3 with correct statuses.
- `tests/cli/msg_no_agent_flag_deprecation.sh` — `k2so msg --agent foo` should emit deprecation warning to stderr but still work.
- `tests/cli/agents_create_hard_deprecation.sh` — `k2so agents create foo` should exit non-zero with the help-deprecated message.
- `tests/cli/k2so_daemon_url_remote.sh` — point `K2SO_DAEMON_URL` at a foreground-launched daemon on a different port; verify `workspaces list` hits the right port.
- `tests/cli/daemon_lifecycle_refuses_remote.sh` — set `K2SO_DAEMON_URL` to non-localhost; verify `k2so daemon restart` errors with the friendly message.

---

## Sequencing + dependencies

**Hard dependency**: Unit 4 must merge first. Phase 2.1's `workspaces list` (the most-touted new verb) needs `/cli/workspaces/list` which Unit 4 adds.

**Soft dependency on Unit 7d**: the `cmd_workspace_preview` function was previously calling `k2so_agents_preview_workspace_ingest` (now living in `k2so-core::agents::workspace` post-7b). Phase 2.1 should verify the daemon route that exposes it still works after Unit 7d's residual migration.

**Recommended sequence:**
1. Unit 4 merges → wait for smoke verification
2. Unit 7d merges → wait for smoke verification
3. Launch Phase 2.1 subagent with this PRD as the brief
4. ~30-60 min for subagent work (smaller than Unit 4; mostly bash + a few new daemon routes)
5. Merge + final-deletion micro-unit closes Phase 2

Phase 2.1 can also land **alongside Phase 3 Workstream A** (typed router) if Phase 3 starts before 2.1 finishes — the new routes get registered in the typed router instead of the old match dispatcher. No semantic difference.

---

## Out of scope (explicit non-goals for Phase 2.1)

- **Rewriting `cli/k2so` in Rust** — bash works fine, `K2SO_DAEMON_URL` already plumbs through `curl`. Don't fix what isn't broken.
- **Adding new "agent" verbs** — the unification goes the other way; agents fold into workspaces.
- **TUI / interactive prompts** — `k2so workspaces list --interactive` is a future feature, not Phase 2.1.
- **Shell completions** — bash/zsh completion script generation is a follow-up.
- **`k2so daemon log --follow`** (tail -f) — would need a streaming `/cli/daemon/log` endpoint; out of scope.
- **K2SO Connect address book CLI** (`k2so connect add/remove/list`) — that's K2SO Connect's UI workstream (Phase 3 Workstream E), not Phase 2.1.

---

## Open questions

1. **Where does `K2SO_DAEMON_URL`'s token come from?** Options:
   - (a) `K2SO_DAEMON_TOKEN` env var — simple, requires user to set both
   - (b) Per-host token file at `~/.k2so/connect-hosts/<sha256-of-url>.token` — Connect-style address book backing store
   - (c) Both, with env var winning
   
   **Recommendation**: (c). Env wins (for one-off testing); file is the persistent store for the K2SO Connect address book (Phase 3 Workstream E).

2. **Should `k2so msg --workspace <path>` exist today, or only after Phase 2.1 lands?** Adding it now (without removing `--agent`) is the "soft deprecation" path. Recommend: yes, add it in Phase 2.1; both work; `--agent` warns.

3. **Smart cascade behavior under contention**: if two `k2so workspace launch` commands race against the same cold workspace, who wins? Currently `spawn_agent_session_blocking` has idempotency (canonical_key); the smart cascade should inherit that.

4. **`k2so template *`** — does the template code already live in `k2so-core` after Unit 7b/7d? Need to verify before scoping the templates routes.

5. **Bash completion script** — out of scope here, but worth a follow-up task. Phase 3 might be the natural time (alongside OpenAPI codegen for typed shells in zsh/fish).

---

## References

- Task #433 (original Phase 4.1 scope from 0.37.1) — superseded by this PRD.
- `.k2so/prds/phase-2-daemon-headless-migration.md` — Phase 2 context.
- `.k2so/prds/phase-3-contract-hardening.md` — Phase 3 (Workstream A typed router may absorb Phase 2.1's new routes).
- `cli/k2so` — current implementation; 3,854 LoC, 96 cmd_ functions, 114 cli_request calls, ~19 non-cli_request functions audited above.
- Memory: `feedback_post_only_route_guards` (Phase 2.1's new daemon routes inherit this rule).

---

# Appendix A — CLI Surface Area Review (2026-05-23)

User-driven review covering: (1) which verbs duplicate each other, (2) which concepts could collapse to flags on a single verb, (3) which are deprecation candidates, (4) which are named poorly, (5) how `k2so help` should be reorganized.

The four user verdicts that drive this appendix (decided 2026-05-23 in a single conversation):
1. ✅ Consolidate `msg` + `work send` + `signal` under `msg` with flags.
2. ✅ Consolidate `checkin` + `status` + `done` under `checkin` with flags.
3. ✅ Move to 3-tier help: daily / advanced / internal.
4. ✅ Capture all findings in this PRD (this appendix).

---

## A1 — Real duplicates (collapse)

These are the same surface implemented twice. **All 5 land in Phase 2.1.**

| Today | Phase 2.1 result | Reason |
|---|---|---|
| `agent <op>` + `agents <op>` (both dispatch to `cmd_agents_*` for create/delete/update/list/profile) | **`agent` only** — `agents` becomes an alias with deprecation warning | Two paths to the same function. Singular for single-item ops, plural for list views — pick singular as the canonical to match `git`, `docker`, etc. |
| `whatsnew` + `whats-new` | **`whats-new` only** | Consistency with `app-update`, `commit-merge`. `whatsnew` becomes a deprecation-warned alias. |
| `agent delete` + `agent remove` (and same for `agents` cluster) | **`agent remove`** | Parallels `workspace remove`, `heartbeat remove`. "remove" implies the entity can be re-added; "delete" sounds permanent. |
| `heartbeat archive` + `heartbeat remove` (share an impl) | **`heartbeat remove [--purge]`** | Verb split implied two different things when it was always one impl. `--purge` makes the difference explicit. |
| `app-update` + `update` | **`update [--cli\|--app]` (default: `--app`)** | Users confuse these every time. One verb, semantic flags. |

**Net**: -5 distinct verb spellings, 0 lost functionality.

---

## A2 — Conceptual overlaps → unify under flags

The biggest UX wins. **Each is a user-approved consolidation.**

### A2.1 — `msg` + `work send` + `signal` → `msg` with flags ⭐

Three current verbs for "deliver content to another workspace":

| Today | Phase 2.1 |
|---|---|
| `msg <ws> "text"` (live delivery, blocks until landed) | `msg <ws> "text"` *(default: live)* |
| `work send <ws> --title "..." --body "..."` (inbox queue) | `msg <ws> --inbox --title "..." --body "..."` |
| `signal <target> <kind> <payload>` (raw AgentSignal; `--inbox` flag for async) | `msg <ws> --signal <kind> <payload>` |

**Implementation:**
- `cmd_msg` gains `--inbox` and `--signal <kind>` flags
- `cmd_work_send` becomes a deprecation-warned wrapper that calls `cmd_msg --inbox` (kept for one release cycle, then removed)
- `cmd_signal` becomes a deprecation-warned wrapper that calls `cmd_msg --signal`
- Top-level `signal` verb → `k2so help-deprecated signal` → "Use `k2so msg --signal <kind>` instead"
- `work` cluster keeps `create / inbox / move`; loses `send`

**Mental model**: one verb for "send something to a workspace"; three flags = three delivery modes.

**Daemon side**: routes don't change (msg still hits `/cli/msg/send` or whatever the daemon-side route is; inbox hits the inbox endpoint; signal hits `/cli/awareness/publish`). The CLI is what consolidates, not the protocol.

### A2.2 — `checkin` + `status` + `done` → `checkin` with flags

Three current verbs for "agent reports in":

| Today | Phase 2.1 |
|---|---|
| `checkin` (heartbeat ping; "I'm alive") | `checkin` *(default: ping)* |
| `status "message"` | `checkin --status "message"` |
| `done` | `checkin --done` |
| `done --blocked "reason"` | `checkin --done --blocked "reason"` |

**Implementation:**
- `cmd_checkin` gains `--status` and `--done` flags
- `cmd_status` and `cmd_done` become deprecation-warned wrappers
- **Keep `done` as a shortcut alias** — agents type `done` constantly; muscle memory matters. Soft-deprecate `status`; preserve `done` with a longer deprecation timeline (or permanently keep as a shortcut)

**Mental model**: `checkin` is the umbrella for agent self-reporting; flags clarify what's being reported.

### A2.3 — `commit` + `commit-merge` → `commit [--merge]`

```
k2so commit -m "fix"              # commit only
k2so commit -m "fix" --merge      # commit then merge into main
```

`cmd_commit_merge` becomes a deprecation-warned wrapper.

### A2.4 — `agents lock` + `agents unlock` → `agent lock [--release]`

```
k2so agent lock <n>               # acquire lock
k2so agent lock <n> --release     # release lock
```

(Note: dropping `s` per A1 unification.) `cmd_agents_unlock` deprecates to `cmd_agent_lock --release`.

### A2.5 — `heartbeat enable` + `heartbeat disable` → `heartbeat enable <n> [--off]`

Trivial collapse; `cmd_heartbeat_disable` deprecates to `cmd_heartbeat_enable_disable false`.

---

## A3 — Deprecation candidates

| Verb | Phase 2.1 verdict | Replacement |
|---|---|---|
| `agents create` (top-level) | **Hard-deprecate** | Workspaces own their primary agent; use `workspace launch` or `workspace update` |
| `agents delete` (top-level) | **Hard-deprecate** | Use `workspace remove` |
| `agents generate-md` | **Hide → `help --internal`** | Called by orchestrators, not humans |
| `agents launch <n>` | **Hide → `help --advanced`** | Power-user; `workspace launch` (Phase 2.1) covers the user case |
| `agents triage`, `agents reap` | **Hide → `help --advanced`** | Diagnostic verbs |
| `agents lock` / `unlock` | **Hide → `help --advanced`** + collapse per A2.4 | Debugging |
| `workspace agent-name`, `set-agent-name`, `resume-chat-args` | **Hide → `help --internal`** (or remove if no human callers) | Looks like orchestrator-internal RPC. Audit before removing. |
| `agentic on/off` | **Fold into `settings`** | Single global toggle; `settings --agentic on` |
| `state list/get/set` | **Fold into `settings`** | Workspace state IS a setting; `settings --state <id>` |
| `mode <off\|agent\|manager>` | **Fold into `settings`** | Same: `settings --mode manager` |
| `hooks status` | **Move to `help --advanced`** + relocate to `daemon hooks` | Diagnostic; hooks are daemon-managed |
| `feed` | **Rename to `activity`** + hide in `help --advanced` | "feed" is generic; `activity` matches the SQL table name |
| `roster` | **Rename to `who`** + hide in `help --advanced` | Cute jargon → CLI convention (Unix `who`) |
| `reserve` / `release` | **Move to `help --advanced`** | File-lock primitive; niche |
| `companion` | **Move to `daemon companion`** | Daemon-managed (Unit 1 + Unit 7c) |
| `skills` | **Rename → `workspace skills`** if it manages workspace skill layers | Verb too vague; clarify by scoping |
| `onboarding later` / `fresh` | **Rename to `defer` / `start-fresh`** | Unclear current names |

---

## A4 — Help reorganization (3 tiers)

Replace current 2-tier help with **3 tiers** plus the existing `help-deprecated`:

### Tier 1: `k2so help` — daily verbs (~15)

```
TALK TO OTHERS
  msg <workspace> "text"                Deliver live (default)
      --inbox [--title "..."] [--body]    Queue to inbox instead
      --signal <kind> <payload>           Emit raw signal (power-user)
  who                                   Who's online (was: roster)
  connections list/add/remove           Cross-workspace links

YOUR WORK
  work create --title "..." --body "..." [--priority H|N|L]
  work inbox                            Workspace inbox
  work move --file <f> --from <a> --to <b>
  checkin                               Agent ping (default)
      --status "message"                  Report status (was: status)
      --done [--blocked "reason"]         Task complete (was: done)
  done                                  Shortcut for `checkin --done`

DELEGATION + REVIEW
  delegate <agent> <file>               Spawn agent in worktree
  reviews                               Pending reviews
  review approve|reject|feedback <agent> [--reason]

WORKSPACE
  workspaces list                       Yellow pages (Phase 2.1 new)
  workspaces running                    Live sessions
  workspace launch [--workspace <path>] Smart cascade spawn-or-attach
  workspace profile [--workspace <path>] Read agent AGENT.md
  workspace update --field <f> --value <v>

INFO
  help                                  This help
  help --advanced                       Power-user verbs (heartbeats, daemon, settings, etc.)
  help --internal                       Orchestrator-RPC verbs (rare; agent runtimes use)
  help-deprecated                       Retired verb → new equivalent map
  whats-new                             Changelog popup
  version                               CLI version
```

### Tier 2: `k2so help --advanced` — power-user verbs

Daily verbs + plus:
- `heartbeat *` (entire cluster — add/list/remove/fire/show/edit/rename/status/wakeup/schedule/etc.)
- `daemon status/start/stop/restart/log/uninstall/companion/hooks`
- `settings` (with --mode, --state, --agentic, --companion flags subsuming the old top-level verbs)
- `update [--cli|--app]` (subsuming `app-update`)
- `onboarding scan/adopt/defer/start-fresh`
- `commit [--merge]` (subsuming `commit-merge`)
- `workspace create/open/remove`
- `agent update/profile` (single-item ops; multi-item moved to `workspaces`)
- `whats-new` (with `--reset`, `--mark-seen` flags)

### Tier 3: `k2so help --internal` — orchestrator-RPC

Verbs that humans rarely run directly; documented as "called by agent runtimes":
- `terminal spawn/write/read`
- `sessions spawn/list/live/compact/set-label`
- `agents triage/reap/lock/launch/generate-md` (`agents` plural here because they operate on multiple)
- `workspace agent-name/set-agent-name/resume-chat-args`
- `reserve/release`
- `hooks status`
- `signal <target>` (raw — `msg --signal` is the daily form)

### `k2so help-deprecated`

Static text listing every retired or hidden-by-default verb + its new equivalent. Per-deprecated-verb error messages reference this.

---

## A5 — Naming issues (pure renames)

| Today | Phase 2.1 | Reason |
|---|---|---|
| `whatsnew` | `whats-new` (deprecate spelling) | Consistency |
| `agentic` (top-level) | (folded into `settings --agentic`) | Vague verb name |
| `state` (top-level) | (folded into `settings --state`) | Generic; collides with mental models |
| `mode` (top-level) | (folded into `settings --mode`) | Generic |
| `feed` | `activity` | Generic → table-aligned |
| `roster` | `who` | CLI convention |
| `heartbeat noop` / `action` | Keep but document as "called by agent runtime" | Internal terminology |
| `heartbeat use-pinned-session on/off <n>` | `heartbeat edit <n> --pinned-session [on/off]` | Wordy → flag on edit |
| `workspace resume-chat-args` | (remove if internal-only) | Implementation detail in verb name |
| `onboarding later` | `onboarding defer` | Clearer intent |
| `onboarding fresh` | `onboarding start-fresh` | Clearer intent |

---

## A6 — Proposed final verb taxonomy (REVISED post-synthesis)

**Daily verbs (13 top-level):**
`msg`, `work`, `checkin`, `done` (alias), `reviews`, `review`, `workspace`, `who`, `activity`, `connections`, `commit`, `whats-new`, `help`, `version`

**Power-user (6 top-level):**
`heartbeat`, `daemon`, `settings` (subsumes `mode`/`state`/`agentic`/`companion`), `update` (subsumes `app-update`), `onboarding`, `delegate` (moved from daily per A16 — legacy multi-agent workflow; prefer cross-workspace `msg --inbox` for new work)

**Internal (4 top-level):**
`terminal`, `sessions`, `agent` (single-item subcommands), `hooks`

**Hard-deprecated (removed; error message → `help-deprecated`):**
`agents create`, `agents delete`, `agentic`, `state`, `mode`, `app-update`, `commit-merge`, `companion`, `whatsnew`, `roster`, `feed`, `signal` (top-level; `msg --signal` is the new form), `work send` (top-level; `msg --inbox` is the new form), `status` (`checkin --status` is the new form)

**Net change**: 37 top-level verbs → ~23 (14 daily + 5 power + 4 internal). Plus the corresponding flag additions on the surviving verbs.

### A6.1 — Workspace singular, not plural (synthesis change)

The original Appendix A had `workspace` (singular, item ops) AND `workspaces` (plural, list ops). The cross-reference review flagged this as an LLM-failure mode: the plural/singular distinction is invisible in help text, and an LLM agent will try `workspace list` (singular), fail, and need to retry.

**Revised**: single `workspace` noun for everything. Sub-verbs distinguish operations:

```
workspace list                              # yellow pages (was workspaces list)
workspace list --running                    # filter to live agents (was workspaces running)
workspace launch [<path>]                   # smart cascade (attach|wake|spawn)
workspace profile [<path>]                  # read agent AGENT.md
workspace update --field <f> --value <v>    # edit agent profile
workspace create <path>                     # create new (existing)
workspace open <path>                       # register existing (existing)
workspace remove <path>                     # deregister (existing)
```

Matches `docker container ls / create / start / rm` and `git branch list / branch create / branch -D` patterns LLMs already know. One noun, sub-verbs carry the action.

---

## A10 — Glossary (NEW from synthesis)

**Problem**: K2SO uses domain-specific terms in verb names + flags that LLMs without context can't decode: `workspace`, `heartbeat`, `signal`, `agentic`, `companion`, `onboarding`, `hooks`, `harness`. Phase 2.1's tier-based help hides these from the daily view but doesn't explain them. The fresh-eyes UX review's #1 issue was "unexplained jargon blocks entry."

**Solution**: a `k2so glossary [<term>]` verb that prints 1-2 sentence definitions. Also exposed as `k2so help --glossary` for discoverability symmetry.

### Implementation

```
k2so glossary                       # list all defined terms
k2so glossary heartbeat             # one-term definition
k2so glossary --json                # machine-readable
```

### Initial glossary entries (alphabetical)

```
activity        Append-only audit log of every workspace event (agent
                spawn, message, heartbeat fire, etc.). View with
                `k2so activity`. Persisted in the `activity_feed`
                table; survives daemon restart.

agent           **The configurable assistant for a workspace.** K2SO
                enforces a 1:1 invariant: each workspace has exactly
                one primary agent. There is no meaningful distinction
                between "the workspace" and "the workspace's agent" —
                they're two names for the same thing in the new model.
                Use `k2so workspace profile` to read agent metadata.

                Legacy: pre-0.37 K2SO supported multiple sub-agents
                per workspace (`.k2so/agents/<sub-agent>/`), managed
                via `k2so delegate` and `k2so agents *` verbs. That
                model is in transition — long-term replacement is
                cross-workspace coordination via `k2so msg --inbox`
                (each agent gets its own workspace; workspaces talk
                to each other). The legacy multi-agent surface still
                works during the transition; see Phase 3 deferred
                cleanup.

agentic         Global toggle for K2SO's agentic systems (heartbeats,
                scheduled launches, autonomous wake). When off, K2SO
                acts as a plain workspace manager with no background
                activity. Configure via `k2so settings --agentic`.

companion       The local server K2SO exposes via ngrok for Mobile
                Companion + K2SO Connect remote access. Daemon-owned
                (post-Phase-2). Configure via `k2so daemon companion`.

delegate        Assign work to an agent. Creates a git worktree on
                a new branch, writes the agent's CLAUDE.md, and
                launches the agent's CLI session. See `k2so help
                delegate`.

harness         The IDE/agent integration layer K2SO writes into a
                workspace (Claude Code settings, Cursor hooks, etc.).
                Managed by `k2so onboarding adopt` on first registration.

heartbeat       A per-workspace scheduled wake (cron-like) that
                triggers the workspace's agent to triage its inbox.
                See `k2so heartbeat help` for the full scheduling
                surface.

hooks           K2SO's CLI-tool integration hooks (Claude Code
                channels, Cursor file hooks). Not the same as git
                hooks. `k2so daemon hooks` shows pipeline state.

inbox           A workspace's queue of pending work items (tasks,
                messages, deferrals). Read with `k2so work inbox`.
                Write to another workspace's inbox with
                `k2so msg <ws> --inbox`.

onboarding      First-launch flow for registering a new workspace
                or adopting an existing project. See
                `k2so onboarding --help`.

reservation     A short-term lock on a file path so two agents don't
                edit the same file concurrently. Acquire with
                `k2so reserve`, release with `k2so release`.

signal          A typed event K2SO sends between workspaces (msg,
                status, presence, reservation, task-lifecycle, custom).
                Sent via `k2so msg --signal <kind>`. The default
                `k2so msg <ws> "text"` is sugar for `--signal msg`.

skill           A workspace-scoped capability or instruction layer.
                K2SO maintains a workspace SKILL.md that the agent
                reads on every wake. Manage via `k2so workspace skills`.

state           A workspace's capability tier (build / managed /
                maintenance / locked) that gates which actions the
                agent can take autonomously. Configure via
                `k2so settings --state <id>`.

workspace       A project folder that K2SO knows about. Has at most
                one primary agent, plus heartbeats, settings, and
                inbox. List all with `k2so workspace list`.
```

(Pull final list from k2so-core constants — `K2SO_GLOSSARY_TERMS` static — so glossary stays in sync with the actual schema.)

### Tests

- `tests/cli/glossary_lists_all_terms.sh` — `k2so glossary` returns at least 12 terms
- `tests/cli/glossary_individual_term.sh` — `k2so glossary heartbeat` returns the heartbeat definition
- `tests/cli/glossary_json_format.sh` — `k2so glossary --json` returns parseable JSON
- `tests/cli/help_glossary_alias.sh` — `k2so help --glossary` returns same output as `k2so glossary`

**LoC**: ~80 (verb + 12-term static table + tests). Trivial to maintain — new terms get added when new verbs introduce them.

---

## A11 — Heartbeat sub-namespace reorganization (NEW from synthesis)

**Problem**: `heartbeat` has 15+ flat subcommands. The fresh-eyes review's #3 issue: "Heartbeat is a bloated sub-tool — rivals a standalone tool." Original Appendix A moved heartbeat to `--advanced` (hidden) but didn't simplify the namespace. The cross-reference reviewer flagged this as a missed opportunity.

**Solution**: regroup 15+ subcommands into 3 conceptually-coherent families. Same daemon routes; just CLI reshape.

### Before (15+ flat subcommands)

```
heartbeat add, remove, list, list-archived, archive, unarchive,
heartbeat fire, wake, wakeup, edit, rename, show, status, log,
heartbeat enable, disable, use-pinned-session, schedule, noop, action
```

### After (3 families, ~11 subcommands across logical groupings)

```
SCHEDULE MANAGEMENT — CRUD on schedule definitions
  heartbeat schedule add --name <n> <spec>      Create a new heartbeat
  heartbeat schedule list                        Active heartbeats
  heartbeat schedule list --archived             Archived heartbeats
  heartbeat schedule remove <n> [--purge]        Soft-archive (default) or hard-delete
  heartbeat schedule unarchive <n>               Restore from archive
  heartbeat schedule edit <n> <spec>             Change schedule spec
  heartbeat schedule rename <old> <new>          Rename
  heartbeat schedule enable <n> [--off]          Resume/pause (collapses enable/disable)

SIGNAL ACTIONS — immediate one-shot triggers
  heartbeat signal fire <n>                      Fire one heartbeat now
  heartbeat signal wakeup <n>                    Open the WAKEUP.md
  heartbeat signal wake                          Auto-wake (no name needed)

INSPECTION — read-only state
  heartbeat show <n> [--json]                    Single heartbeat details
  heartbeat status <n> [-n N]                    Recent fire history
  heartbeat log [-n N]                           Scheduler decisions
```

Per-agent adaptive flags (`heartbeat noop`, `heartbeat action`, `heartbeat use-pinned-session`) move under `heartbeat schedule edit <n> --noop / --action / --pinned-session [on/off]` — they're really schedule configuration, not standalone verbs. This drops 3 more verbs.

### Net change

- 18 flat subcommands → 14 across 3 named families
- LLMs reading `k2so heartbeat help` see 3 short groups instead of one wall of 18 verbs
- Backwards-compat: keep `k2so heartbeat add` as a deprecation-warned alias of `k2so heartbeat schedule add` for one release cycle

### Tests

- `tests/cli/heartbeat_schedule_add_subsumes_add.sh` — old form + new form produce identical DB row
- `tests/cli/heartbeat_signal_fire_subsumes_fire.sh` — same parity test
- `tests/cli/heartbeat_help_three_families.sh` — `k2so heartbeat help` shows SCHEDULE / SIGNAL / INSPECTION sections

---

## A12 — Help text templates for new/modified verbs (NEW from synthesis)

**Problem**: Appendix A introduces new flags (`msg --signal <kind>`, `checkin --done [--blocked]`, `workspace launch`) without spec'ing the help text. Cross-reference reviewer flagged `--signal <kind>` specifically: "Is this LLM-discoverable from `msg --help`?" — no, not without a kind enumeration.

**Solution**: spec `<verb> --help` output for every new or modified surface. The Phase 2.1 subagent implements these verbatim.

### `k2so msg --help`

```
k2so msg <workspace> "text" [options]

Deliver text to another workspace's agent. Three delivery modes via flags:

DELIVERY MODES
  (default)                 Live delivery — spawns the recipient's agent
                            if not running, blocks until the message lands
                            in their session. Use for synchronous comms.
  --inbox                   Queue to the recipient's inbox. Recipient
                            reads when they next checkin. Async; doesn't
                            spawn anything. Use for non-urgent work.
  --signal <kind>           Emit a typed signal. <kind> is one of:
                              msg            Plain message (= default)
                              status         Status update (telemetry)
                              presence       Online/away/offline state
                              reservation    File path lock event
                              task-lifecycle Task started/done/blocked
                              custom         Escape hatch (with --payload)

ARGUMENTS
  <workspace>               Workspace name (preferred — see `k2so workspace list`).
                            Absolute path or project UUID also accepted.
  "text"                    Message body. Wrap in quotes if it has spaces.
                            Not required when --signal <kind> needs structured payload.

OPTIONS
  --from <name>             Sender identity (default: K2SO_AGENT_NAME or "cli")
  --title "..."             Inbox title (--inbox only)
  --body "..."              Inbox body (--inbox only)
  --priority H|N|L          Inbox priority (--inbox only; default N)
  --payload <json>          Custom signal payload (--signal custom only)

EXAMPLES
  k2so msg scout_v3 "ready when you are"
  k2so msg scout_v3 "ping" --from sms-bridge
  k2so msg scout_v3 --inbox --title "deploy ready" --body "..."
  k2so msg scout_v3 --signal status --payload '{"text":"deploying..."}'
```

### `k2so checkin --help`

```
k2so checkin [options]

Agent self-report. Sends a heartbeat ping, status update, or
done/blocked report.

DEFAULT
  (no flags)                Plain heartbeat ping — "I'm alive, no action".

OPTIONS
  --status "message"        Report a status update visible in the workspace
                            activity feed. Doesn't change agent state.
  --done                    Report task completion. Marks the current
                            work item as done.
  --done --blocked "reason" Report blocked state. Stops the work item;
                            requires human or peer-agent unblock.

EXAMPLES
  k2so checkin
  k2so checkin --status "still on the OAuth migration"
  k2so checkin --done
  k2so checkin --done --blocked "waiting on Stripe API access"
```

### `k2so workspace launch --help`

```
k2so workspace launch [<path>]

Smart cascade — get the workspace's agent into a running state:
  * If the agent is alive (live session exists): attach to it
  * If the agent is sleeping (registered, no session): wake it
  * If the agent is cold (no registered session): spawn fresh

Returns the agent's session id on success.

ARGUMENTS
  <path>                    Workspace path (default: $K2SO_PROJECT_PATH or PWD)

OPTIONS
  --json                    Return session metadata as JSON
  --no-attach               Spawn/wake but don't attach the current shell
                            (useful for scripting)

EXAMPLES
  k2so workspace launch
  k2so workspace launch /Users/me/Projects/k2so
  k2so workspace launch --json
```

### `k2so settings --help`

```
k2so settings [options]

Show or modify workspace settings. Without flags: prints current settings.

WORKSPACE MODE — replaces the old `k2so mode` verb
  --mode <off|agent|manager>
                            off: K2SO ignores this workspace
                            agent: workspace has a primary agent
                            manager: workspace is a manager of other workspaces

WORKSPACE STATE — capability tier (replaces old `k2so state`)
  --state <build|managed|maintenance|locked>
                            Drives which actions the agent can take
                            autonomously. See `k2so glossary state` for
                            full capability matrix.

GLOBAL TOGGLES (apply across all workspaces)
  --agentic <on|off>        Enable/disable all background agent systems
                            (heartbeats, autonomous wakes, scheduled work).
                            When off, K2SO is a passive workspace tool.

UPDATE COMPANION
  --companion <on|off>      Enable/disable the daemon's companion server
                            (mobile + remote-desktop access via ngrok)
  --companion-password "<pw>" Set the companion auth password

EXAMPLES
  k2so settings
  k2so settings --mode manager
  k2so settings --state managed
  k2so settings --agentic off
  k2so settings --companion on
```

### Tests

- `tests/cli/help_text_no_undefined_jargon.sh` — for every term in `k2so glossary`, verify it appears in at least one verb's `--help` output with context (catches: jargon introduced in help text without a glossary entry)
- `tests/cli/help_text_signal_kind_enum.sh` — `k2so msg --help` lists all 6 SignalKind values
- `tests/cli/help_text_smart_cascade_explanation.sh` — `k2so workspace launch --help` contains the words "attach", "wake", "spawn"

---

## A13 — Sessions / terminal / workspace-running overlap (NEW from synthesis)

**Problem**: Three verb clusters show "what's currently active":
- `workspace list --running` (daily tier, post-A6.1)
- `sessions list` (internal tier — PTY sessions)
- `daemon status` (power-user tier — daemon-process state)
Plus the old `agents running` which collapses into `workspace list --running`.

Each shows a different scope, but an LLM reading help won't realize they exist in parallel. Original Appendix A tiered them but didn't cross-reference.

**Solution**: add cross-reference breadcrumbs in each tier's help text. No new verb; just discoverability glue.

### In daily help under `workspace list`

```
workspace list [--running]                Yellow pages (all workspaces).
                                          --running filters to live agents.
                                          For raw PTY sessions, see
                                          `k2so help --internal sessions`.
                                          For daemon health, see
                                          `k2so daemon status`.
```

### In `--internal` help under `sessions`

```
sessions list [--json]                    Raw PTY sessions (low-level).
                                          For workspace-level "what's running",
                                          use `k2so workspace list --running`.
```

### In `--advanced` help under `daemon`

```
daemon status [--json]                    Daemon process state (PID, port,
                                          uptime). For workspace-level
                                          "what's running", use
                                          `k2so workspace list --running`.
```

### Optional future consolidation (out of scope for Phase 2.1)

A `k2so running [--type workspaces|sessions|daemon]` unified verb would solve the discoverability problem structurally. The cross-reference reviewer suggested this. Phase 3 could add it; Phase 2.1 just adds breadcrumbs.

---

## A14 — Deferred to Phase 3 (NEW from synthesis)

The fresh-eyes UX review flagged capabilities a user would expect but the CLI doesn't have. Honest about gaps:

| Capability | Today | Phase 2.1 verdict | Phase 3 home |
|---|---|---|---|
| **Undo last action** | None | Out of scope | Phase 3 Workstream G (observability) — needs an event-sourced undo log first |
| **Export everything** | None | Out of scope | Phase 3 Workstream C (OpenAPI) — `k2so export --format json` would dump every `/cli/*` GET endpoint's response |
| **Per-agent pause** (not just heartbeat-disable) | None | Out of scope | Phase 3 — needs a daemon-side "paused" state machine, not just a heartbeat flag |
| **Manual retry of failed task** | None | Out of scope | Phase 3 — needs a failed-work-queue in the daemon |
| **Workspace/agent-level logs** (vs `daemon log` only) | Partial — daemon log only | Out of scope | Phase 3 — daemon needs to tag log lines with workspace context |
| **In-CLI git diff review** | None — must use `git diff` | Out of scope; arguably never K2SO's job | n/a |
| **`k2so search <intent>` semantic CLI search** | None | Out of scope | Phase 3 with OpenAPI codegen — generated client could include keyword/intent search |

**Recommendation**: each of these gets a one-line entry in Phase 3 PRD (`.k2so/prds/phase-3-contract-hardening.md`) under a new "Deferred CLI features" appendix. Phase 2.1 doesn't add them; just acknowledges the gap so future work has a clear pointer.

---

## A15 — Summary of synthesis changes from cross-reference review

The cross-reference review (read by an independent Explore agent, scored against the fresh-eyes UX review's findings) drove these changes from the original Appendix A:

| Synthesis change | Source finding | Impact |
|---|---|---|
| A6.1 — drop `workspaces` plural; `workspace` singular for everything | LLM-lens "plural/singular invisible" footgun | 1 fewer top-level verb; matches docker/git conventions |
| A10 — Glossary verb | Fresh-eyes #1 "unexplained jargon blocks entry" | Each K2SO-specific term gets a 1-2 sentence inline definition |
| A11 — Heartbeat sub-namespace into 3 families | Fresh-eyes #3 "heartbeat is a bloated mini-tool" | 18 flat subcommands → 14 across 3 logical groupings |
| A12 — Help text templates for new flags | Cross-ref Task 5 "Appendix A's own jargon" — `--signal <kind>` undefined | Every new flag has a spec'd `--help` with examples |
| A13 — Cross-reference breadcrumbs for overlapping verbs | Fresh-eyes #10 "sessions/terminal/agents-running overlap" | Discoverability glue; no new verbs |
| A14 — Deferred-to-Phase-3 capabilities | Fresh-eyes #7 "missing capabilities" | Honest about gaps; clear handoff to Phase 3 |

**LLM-friendliness score estimate** (per cross-reference Task 2 methodology):
- Original Appendix A: 15/24 verbs LLM-friendly
- Post-synthesis Appendix A: estimated 20/23 verbs LLM-friendly (1 fewer verb after `workspaces` collapse; jargon now glossary-discoverable; smart-cascade explained; signal-kind enumerated)

**Updated cli/k2so LoC estimate**: 3,854 → ~2,500 (slightly higher than A7's 2,400 due to A10 glossary table + A11 heartbeat family aliases).

---

## A16 — Workspace-Agent invariant: Phase 2.1's organizing principle (NEW from synthesis)

**Why this exists**: the user flagged that the original Appendix A reflected the workspace-agent invariant (1 workspace = 1 primary agent) in *places* but not consistently — and that the inconsistency could skew LLM agent exploration. Investigation confirmed two architectural realities coexist today:

1. **New model**: `.k2so/agent/` (singular) — one primary agent per workspace, managed via `workspace launch / profile / update`. This is the long-term shape.
2. **Legacy model**: `.k2so/agents/<sub-agent>/` (plural) — multiple sub-agents per workspace, managed via `delegate`, `agents create`, `agents work <name>`. K2SO itself still uses this internally (the pod-leader → backend-eng/frontend-eng/etc. pattern).

The new model wants to eat the old model. **The long-term replacement for within-workspace multi-agent is cross-workspace coordination**: each agent gets its own workspace, and workspaces talk via `k2so msg --inbox`. Until that transition completes (post-Mobile-Companion + K2SO Connect), the legacy multi-agent surface stays but is clearly labeled deprecated.

### The principle

**Verbs should default to workspace-keyed semantics. The "agent" concept is sugar for "the workspace's primary agent." Multi-agent (sub-agent) verbs are legacy and tier-3-deprecated.**

### Concrete cleanup driven by this principle

#### A16.1 — Hard-deprecate `agents` (plural) verbs that assume sub-agents

These verbs presuppose the multi-agent model. In the workspace-keyed world, they have no meaning:

| Today | Phase 2.1 verdict | Replacement |
|---|---|---|
| `agents create <name>` | **Hard-deprecate** | `workspace launch [path]` — create the workspace if needed, agent comes along with it |
| `agents delete <name>` | **Hard-deprecate** | `workspace remove [path]` — removes the workspace + its agent |
| `agents triage` | **Hard-deprecate** | `workspace activity --triage` or just `activity --triage` |
| `agents reap` | **Hard-deprecate** | `daemon reap` — it's a daemon-level garbage collect, not an agent op |
| `agents lock <name>` | **Hard-deprecate** | `workspace --lock` flag, or just remove (was diagnostic) |
| `agents unlock <name>` | **Hard-deprecate** | `workspace --unlock` flag, or just remove |
| `agents launch <name>` | **Hard-deprecate** | `workspace launch [path]` |
| `agents list` | **Soft-deprecate** (already aliased) | `workspace list` — same data, workspace-keyed |
| `agents work <name>` | **Hard-deprecate** | `work inbox` (workspace-implicit) or `work inbox --workspace <path>` |
| `agents status <name>` | **Hard-deprecate** | `workspace status` or `checkin --status` |
| `agents profile <name>` | **Hard-deprecate** | `workspace profile [path]` |
| `agents generate-md <name>` | **Hide** (internal) | `workspace regen-skill [path]` or daemon-side automatic |
| `agents running` | **Soft-deprecate** | `workspace list --running` |

Each hard-deprecated verb's error message points at the workspace-keyed replacement + `help-deprecated` for the full map.

#### A16.2 — Keep `agent` (singular) in `--internal` tier as a bridging concept

Some operations are conceptually agent-level even though they target the workspace's primary agent. Keep these in the `--internal` tier with crystal-clear help text:

| Verb | Help text spec |
|---|---|
| `agent profile [<path>]` | "Read the workspace's primary agent's AGENT.md. Equivalent to `workspace profile`." (provide both for muscle-memory) |
| `agent update --field <f> --value <v>` | "Update a field in the workspace's primary agent's AGENT.md. Equivalent to `workspace update`." |
| `agent complete --file <f>` | "Mark a work item as complete. Workspace-implicit." |

These exist to reduce friction for agents that mentally model the world as "I am an agent, I do things." But they're tier-3 internal; daily verbs are workspace-keyed.

#### A16.3 — Audit every `--agent <name>` flag → `--workspace <path>`

Every verb that currently takes `--agent <name>` (often as a workspace selector) should accept `--workspace <path>` as the canonical form. `--agent` becomes a soft-deprecated alias for one release cycle.

Verbs to audit:
- `work create --agent <name>` → `work create [--workspace <path>]` (workspace-implicit defaults to PWD)
- `work move --agent <name>` → `work move [--workspace <path>]`
- `heartbeat * --agent <name>` → `heartbeat * [--workspace <path>]` (covered by A11's workspace-implicit semantics)
- `feed --agent <name>` (now `activity --agent <name>`) → `activity [--workspace <path>]`
- `status "msg" --agent <name>` (now `checkin --status "msg" --agent <name>`) → `checkin --status "msg"` (workspace-implicit)
- `agent update --name <n>` → `workspace update [--workspace <path>] --field <f> --value <v>`
- `agent complete --agent <n>` → `agent complete [--workspace <path>]` (note: keeps `agent` namespace per A16.2)

Implementation pattern: each affected `cmd_*` function checks `--workspace` first, then falls back to `--agent` with a one-time deprecation warning per shell session, then to `K2SO_PROJECT_PATH` / PWD.

#### A16.4 — `delegate` moves to `--advanced` with legacy warning

`k2so delegate <agent> <file>` is the keystone verb for the legacy multi-agent workflow. It's not removed (K2SO itself uses it; many active users likely do too), but:

- **Tier**: moves from daily (per original Appendix A) to `--advanced`
- **Help text**: "Legacy multi-agent workflow: creates a worktree + writes CLAUDE.md + launches a sub-agent's Claude session. Each sub-agent lives under `.k2so/agents/<name>/`. **Prefer the workspace-centric pattern** for new work: register each agent as its own workspace via `k2so workspace create`, then coordinate via `k2so msg --inbox`. See Phase 3 PRD for the planned full retirement of the multi-agent surface."
- **Deprecation timeline**: not removed in Phase 2.1. Reviewed for full retirement after Mobile Companion + K2SO Connect ship and the cross-workspace coordination pattern proves out (see A17).

### LLM-agent benefit

After A16, an LLM agent reading `k2so help` sees **one clear path**: workspace-keyed verbs. The `agent` singular namespace in `--internal` is bridging sugar; the `agents` plural namespace is gone (errors with helpful pointers). No more "which of these three concepts am I supposed to use?"

### A16 tests

- `tests/cli/agents_create_hard_deprecated.sh` — exits non-zero with `workspace launch` pointer
- `tests/cli/agents_delete_hard_deprecated.sh` — exits non-zero with `workspace remove` pointer
- `tests/cli/agents_running_soft_deprecated.sh` — works but emits deprecation warning pointing at `workspace list --running`
- `tests/cli/agent_flag_accepts_workspace_alias.sh` — verify every audited verb accepts both `--agent` (deprecated) and `--workspace` (canonical)
- `tests/cli/delegate_advanced_tier_legacy_warning.sh` — `delegate --help` text mentions "legacy multi-agent workflow"

---

## A17 — Deferred to Phase 3: `.k2so/agents/` retirement audit (NEW from synthesis)

After Mobile Companion + K2SO Connect ship and the cross-workspace coordination pattern proves out, audit:

- **Does anyone still use `.k2so/agents/<sub-agent>/` outside of K2SO itself?** Survey production users (K2SO Connect installations) to see whether the multi-agent surface has any third-party adoption.
- **Can K2SO migrate its own pod model to multi-workspace coordination?** Today K2SO uses `.k2so/agents/pod-leader/` + delegated sub-agents. The dogfood test: can pod-leader be its own workspace, and can each sub-agent (backend-eng, frontend-eng, qa-eng) be its own workspace too? Cross-workspace `msg --inbox` replaces in-workspace delegation.
- **If yes to both**: hard-remove the multi-agent surface. Delete `cmd_delegate`, delete `.k2so/agents/` from new installations, remove `commands::k2so_agents::archive_orphan_*` migration paths. Estimated cleanup: ~500 LoC across `cli/k2so` + `crates/k2so-core/src/agents/*`.
- **If no**: defer further; keep the multi-agent surface as a supported-but-not-recommended pattern.

**Phase 3 PRD home**: add a section "Deferred CLI features and legacy retirement" under Phase 3's "Open questions" section, with bullets for this audit and the A14 capability gaps (undo, export, retry, etc.).

---

## A18 — Final summary: what Phase 2.1 actually ships

After all the synthesis additions, Phase 2.1's subagent brief boils down to:

1. **A6 final taxonomy**: 23 top-level verbs (13 daily + 6 power + 4 internal); workspace-singular for everything; `delegate` in power-user tier
2. **A10 glossary**: `k2so glossary [term]` verb + 12-term initial table
3. **A11 heartbeat reorganization**: 18 flat subcommands → 14 across 3 named families (schedule / signal / inspection)
4. **A12 help text templates**: spec'd `--help` for msg, checkin, workspace launch, settings (+ any others touched)
5. **A13 breadcrumbs**: cross-references in help text for sessions/terminal/workspace-running overlap
6. **A14 deferred to Phase 3**: explicit list of missing capabilities
7. **A16 invariant propagation**: hard-deprecate `agents` plural verbs; bridge `agent` singular in --internal; audit `--agent` → `--workspace`; `delegate` to --advanced with legacy label
8. **A17 deferred audit**: `.k2so/agents/` retirement timing tied to Mobile Companion + K2SO Connect proof-out

**Subagent time estimate**: 90-120 min (up from A9's 60-90 min — Appendix A grew substantially through synthesis).

**Final cli/k2so LoC**: 3,854 → ~2,500 (A16's hard deprecations delete more than A10/A11 add).

**LLM-friendliness target**: ≥20/23 verbs an LLM agent can confidently pick on first read.

---

## A19 — Skill reframe: "many agents per workspace" is dead, long live skills (NEW from synthesis)

**The user's reframe (2026-05-23, mid-Phase-2.1-spec)**:

> "Ultimately the 'many agents in a single workspace' concept is dead. In the future it will become unique skills and a documentation map. This is also why heartbeats were moved to the workspace itself `.k2so/heartbeats/` — so lets keep moving in that direction."

This is the architectural direction A16 was groping toward but didn't fully name. A16 said "deprecate the multi-agent surface." A19 says: **what we called sub-agents were never agents — they're skills.** Each `.k2so/agents/<name>/` folder literally contains a SKILL.md plus heartbeats. The folder's name lies about what's inside.

### The reframed model

- **Workspace** = the project folder K2SO knows about
- **Agent** = the workspace's one primary assistant (1:1, enforced)
- **Skill** = a documented capability profile the agent can apply to specific work
  - Master definitions: `.k2so/agent-templates/<role>/` (rename to `.k2so/skill-templates/<role>/` in A19.2)
  - Instantiated skills: `.k2so/agents/<name>/` (rename to `.k2so/skills/<name>/` in A19.2)
- **Documentation map** = the SKILL.md / AGENT.md / CLAUDE.md hierarchy that gives the agent context
- **Heartbeat** = a workspace-owned scheduled wake (`.k2so/heartbeats/` — already in this shape)
- **Delegate** = "apply skill X to work item Y" (not "spawn agent X"). **Recovers as a daily-tier verb.**

### A19.1 — CLI verb renames (Phase 2.1 scope)

Rename `agents *` → `skills *`. Each old verb gets a deprecation-warned alias for one release cycle. The existing `cmd_skills` (which today only does `regenerate`) expands to cover the full surface.

| Old verb | New verb | Notes |
|---|---|---|
| `agents create <name>` | `skills create <name>` | Instantiate a skill (from a template if provided via `--template <role>`) |
| `agents delete <name>` | `skills remove <name>` | Match `workspace remove` / `heartbeat remove` convention |
| `agents list` | `skills list` | Lists skills (instantiated) in the workspace |
| `agents work <name>` | `skills work <name>` | Read a skill's work queue |
| `agents triage` | `workspace triage` | This was workspace-scoped, not skill-scoped — fix the misattribution |
| `agents reap` | `daemon reap` | Daemon-level GC, not skill-level |
| `agents lock <name>` | `skills lock <name>` | Lock a skill's session (debugging) |
| `agents unlock <name>` | `skills unlock <name>` | (Or collapse per A2.4 to `skills lock <n> --release`) |
| `agents launch <name>` | `skills launch <name>` | Spawn a Claude session pre-loaded with the named skill |
| `agents profile <name>` | `skills profile <name>` | Read the skill's SKILL.md/AGENT.md |
| `agents generate-md <name>` | `skills regenerate <name>` | Already exists as `skills regenerate` (today regenerates all; add per-skill variant) |
| `agents running` | `workspace list --running` (A6.1) | Workspace-keyed via existing rename |
| `agents status <name>` | `skills status <name>` | Read skill state (alive/sleeping/cold) |

**Hard-deprecate** the misnomers that don't map cleanly to skills: `agents reap`, `agents triage` (both move to other namespaces per table above).

**Soft-deprecate** the verbs that do map (with `skills *` as the canonical form): `agents create/delete/list/work/lock/unlock/launch/profile/status`. Old verbs print a one-line deprecation warning to stderr, pointing at the `skills *` equivalent, then run.

### A19.2 — Filesystem rename (DEFERRED to Phase 3)

Renaming `.k2so/agents/<name>/` → `.k2so/skills/<name>/` is heavier than Phase 2.1 can absorb:

- **Naming collision**: `.k2so/skills/` already exists for a different concept (k2so-core's `skill_layers` system from Unit 6 — per-workspace skill layer files). Renaming the multi-agent dir to `.k2so/skills/` would collide.
- **Migration risk**: requires moving live data on every existing K2SO installation. A botched migration loses user state.
- **Backwards-compat surface**: the daemon currently writes to and reads from `.k2so/agents/<name>/`. Renaming requires daemon code changes + a migration path.

**Resolution proposal for Phase 3 (or its own dedicated unit)**:
- Rename existing `.k2so/skills/` (the skill-layer system) → `.k2so/skill-layers/` to match the k2so-core module name and free the cleaner `.k2so/skills/` namespace
- Rename `.k2so/agent-templates/<role>/` → `.k2so/skill-templates/<role>/`
- Rename `.k2so/agents/<name>/` → `.k2so/skills/<name>/`
- Daemon migration: on first boot after upgrade, detect old directories and move them atomically (tmp+rename pattern from `fs_atomic`); leave a `.k2so/.skills-migration-<version>-done` marker

For Phase 2.1: **filesystem stays as-is.** Glossary + help text explain the rename direction. CLI verbs use the new names with old as aliases.

### A19.3 — `delegate` recovers as daily-tier verb (REVISES A6 + A16.4)

A16.4 demoted `delegate` to `--advanced` with a "legacy multi-agent workflow" label. **A19 undoes this**: `delegate` is the canonical verb for "apply skill X to work item Y." It's not legacy; it's the primary mechanism for routing specialized work to the right capability profile.

| Where | Status |
|---|---|
| **Daily tier** | `delegate <skill> <file>` — apply a skill to a work item |
| Help text | "Apply the named skill to the work item. Creates a git worktree, writes a CLAUDE.md pre-loaded with the skill's role + persona, and launches a Claude session. Skills live under `.k2so/skills/<name>/` (currently `.k2so/agents/<name>/` — being renamed; see `k2so glossary skill`)." |
| Long-term shape | Each skill can also be a standalone workspace (advanced users), but the primary pattern is in-workspace skill application via `delegate`. |

### A19.4 — Glossary additions (REVISES A10)

Add:

```
skill           A documented capability profile the workspace's agent
                can apply to specific work. Includes a SKILL.md (role
                + persona + instructions), heartbeats (the skill's
                wake schedule), and a work queue. List skills with
                `k2so skills list`. Apply to a work item with
                `k2so delegate <skill> <file>`.

                Filesystem: skills currently live at `.k2so/agents/<name>/`
                (historical naming from when they were called sub-agents).
                Templates at `.k2so/agent-templates/<role>/`. Rename
                to `.k2so/skills/` is planned for Phase 3.

skill-template  A master definition for a skill that can be
                instantiated multiple times. Lives at
                `.k2so/agent-templates/<role>/` (current naming;
                rename to `.k2so/skill-templates/` is Phase 3 work).
                Create new skills from a template via
                `k2so skills create <name> --template <role>`.
```

Update the existing `agent` glossary entry to drop the "legacy multi-agent" language (since the multi-agent surface IS skills, not legacy):

```
agent           **The configurable assistant for a workspace.** K2SO
                enforces a 1:1 invariant: each workspace has exactly
                one primary agent. There is no meaningful distinction
                between "the workspace" and "the workspace's agent" —
                they're two names for the same thing in the new model.
                Use `k2so workspace profile` to read agent metadata.

                The agent can apply one or more skills to specific
                work via `k2so delegate <skill> <file>`. See `k2so
                glossary skill` for the skill/agent distinction.

                Historical: pre-0.37 K2SO modeled sub-agents under
                `.k2so/agents/<name>/`. What was called "sub-agents"
                is actually the skill system; rename in progress
                (Phase 3 — see A19).
```

### A19.5 — Implementation order (updates A9)

Insert into A9's implementation order **after step 1 (add new daemon routes)** and **before step 2 (add new CLI verbs)**:

> **Step 1.5 — Add `skills *` CLI surface**:
> Expand `cmd_skills` from its current single subcommand (`regenerate`) into the full surface: `create / remove / list / work / lock / unlock / launch / profile / status / regenerate`. Each delegates to the existing daemon routes (no new daemon routes needed — the routes already exist, they're just named `/cli/agents/*` today; either reuse-as-is or alias via new `/cli/skills/*` routes that forward).

Add **new step 6.5** between steps 6 (hard-deprecate) and 7 (rename feed/roster):

> **Step 6.5 — `agents *` verb deprecation**:
> Per A19.1's mapping table, replace each `agents *` verb body with: (a) one-line deprecation warning to stderr, (b) forward to the `skills *` equivalent. The few that don't map cleanly (`agents reap`, `agents triage`) get hard-deprecation messages pointing at the new home (`daemon reap`, `workspace triage`).

### A19.6 — Why this matters for LLM-agent UX

Under A19, an LLM agent reading `k2so help` sees:
- **Daily**: workspace ops + msg + checkin + delegate (apply skill to work)
- **Power**: heartbeat (workspace-scoped) + skills (capability profiles)
- **Internal**: agent singular ops (bridging sugar for workspace's primary agent)

Three concepts, three clear roles. No "is this an agent or a workspace or a sub-agent?" confusion. The skill concept names what was always there but was hiding under a misleading "agent" label.

### A19.7 — Updated taxonomy (REVISES A6)

**Daily verbs (14 top-level):**
`msg`, `work`, `checkin`, `done` (alias), `delegate`, `reviews`, `review`, `workspace`, `who`, `activity`, `connections`, `commit`, `whats-new`, `help`, `version`

(Adds `delegate` back to daily per A19.3; verb count goes from 13 → 14)

**Power-user (6 top-level):**
`heartbeat`, `daemon`, `settings`, `update`, `onboarding`, `skills`

(Adds `skills` per A19.1; verb count goes from 5 → 6)

**Internal (4 top-level):**
`terminal`, `sessions`, `agent`, `hooks`

(Unchanged)

**Hard-deprecated**: same as A6 + `agents` plural verbs soft-deprecate to their `skills *` equivalents (per A19.1).

**Net change**: 37 top-level verbs → 24 (14 daily + 6 power + 4 internal). One more than the previous A6 estimate because `delegate` returns to daily + `skills` enters power-user.

### A19.8 — Tests

- `tests/cli/skills_create_subsumes_agents_create.sh` — `k2so skills create foo` and `k2so agents create foo` produce identical filesystem state, with `agents create` emitting a deprecation warning to stderr
- `tests/cli/delegate_applies_skill_not_spawns_agent.sh` — verify `delegate` help text says "apply" not "spawn"; verify created worktree's CLAUDE.md includes the skill's role + persona
- `tests/cli/agents_triage_hard_deprecated_to_workspace_triage.sh` — exits non-zero with `workspace triage` pointer
- `tests/cli/agents_reap_hard_deprecated_to_daemon_reap.sh` — exits non-zero with `daemon reap` pointer
- `tests/cli/glossary_skill_term.sh` — `k2so glossary skill` returns the A19.4 definition

---

## A20 — Phase 3 deferred: `.k2so/agents/` → `.k2so/skills/` filesystem rename

Add to Phase 3 PRD (`.k2so/prds/phase-3-contract-hardening.md`) as a deferred item:

**Filesystem rename plan** (post-Mobile-Companion + K2SO-Connect ship):

1. Rename `.k2so/skills/` (k2so-core skill_layers system) → `.k2so/skill-layers/` to free the `skills/` namespace
2. Rename `.k2so/agent-templates/<role>/` → `.k2so/skill-templates/<role>/`
3. Rename `.k2so/agents/<name>/` → `.k2so/skills/<name>/`
4. Daemon first-boot migration: detect old dirs, atomic rename (tmp + rename), write `.k2so/.skills-rename-v1-done` marker
5. Update all daemon code to use new paths; backwards-compat shim that reads from either path during the transition window (1 release)
6. Update `.k2so/CLAUDE.md.generated` + `.k2so/CLAUDE.md.migrated` templates to reflect new naming
7. Update agent-templates (or whatever they're called by then) so newly-created workspaces use the new naming from day one

Estimated effort: ~400 LoC of daemon migration + ~50 LoC of CLI path updates + extensive testing. **One dedicated unit, post-Phase-3 contract hardening.**

---

## A21 — Final consolidated taxonomy after A19

| Tier | Verbs | Count |
|---|---|---|
| **Daily** | `msg`, `work`, `checkin`, `done` (alias), `delegate`, `reviews`, `review`, `workspace`, `who`, `activity`, `connections`, `commit`, `whats-new`, `help`, `version` | 14 |
| **Power-user** (`help --advanced`) | `heartbeat`, `daemon`, `settings`, `update`, `onboarding`, `skills` | 6 |
| **Internal** (`help --internal`) | `terminal`, `sessions`, `agent`, `hooks` | 4 |
| **Total** | | **24** |
| **Hard-deprecated** (error + `help-deprecated` pointer) | `agentic`, `state`, `mode`, `app-update`, `commit-merge`, `companion`, `whatsnew`, `roster`, `feed`, `signal`, `work send`, `status`, `agents reap`, `agents triage` | 14 |
| **Soft-deprecated** (warning + forward) | `agents create/delete/list/work/lock/unlock/launch/profile/status/running`, `whatsnew`, agent-keyed `--agent` flags | ~13 |

**Phase 2.1 ship target**: 37 top-level verbs → 24, with clear daily/power/internal tiering and a skill-centric mental model that an LLM agent can navigate on first read.

---

## A7 — Updated cli/k2so LoC estimate

Original Phase 2.1 estimate: 3,854 → ~2,800 LoC.
**Updated with Appendix A changes**: 3,854 → ~2,400 LoC.

Why lower: the consolidations in A2 (msg + work send + signal; checkin + status + done; commit + commit-merge; etc.) delete more lines than they add, because the new flag handlers are smaller than the duplicate dispatch + duplicate option parsing they replace.

Net per category:
- A1 duplicate removal: ~200 LoC deleted
- A2 flag consolidations: ~400 LoC deleted (3 verbs → 1 verb × 3 collapses)
- A3 hard deprecations: ~300 LoC deleted (entire functions)
- A4 help reorganization: ~50 LoC added (new help tier) but ~100 LoC of advanced-help text restructured
- A5 renames: ~0 net (rename ≠ delete)

Total: ~3,854 → ~2,400. CLI script becomes meaningfully shorter while gaining new workspace-keyed verbs.

---

## A8 — Verification tests (new)

Beyond the K2SO Connect tests in the main PRD body, Appendix A adds:

- `tests/cli/msg_inbox_subsumes_work_send.sh` — verify `msg <ws> --inbox --title "..." --body "..."` lands in the recipient's inbox identically to the legacy `work send`.
- `tests/cli/msg_signal_subsumes_signal_verb.sh` — verify `msg <ws> --signal <kind> <payload>` produces the same `AgentSignal` as the legacy `signal` verb.
- `tests/cli/checkin_status_subsumes_status_verb.sh` — verify `checkin --status "msg"` produces the same activity_feed entry as `status "msg"`.
- `tests/cli/checkin_done_subsumes_done_verb.sh` — verify `checkin --done [--blocked]` parity with `done [--blocked]`.
- `tests/cli/agent_remove_subsumes_delete.sh` — verify `agent remove` and `agent delete` produce identical results (one shows deprecation, the other doesn't).
- `tests/cli/help_three_tiers.sh` — verify `k2so help`, `k2so help --advanced`, and `k2so help --internal` each show the right verb set.
- `tests/cli/help_deprecated_lists_all_retired.sh` — verify every hard-deprecated verb appears in `help-deprecated` output with a replacement.
- `tests/cli/hard_deprecated_verbs_fail_with_helpful_error.sh` — verify `agents create`, `signal`, `app-update`, etc. all exit non-zero with a pointer to the new verb.

---

## A9 — Implementation order

Within Phase 2.1's single landing, the subagent should sequence:

1. **Add new daemon routes first** (so CLI verbs have something to call): `/cli/workspaces/{list, running, launch, profile, update}`, `/cli/templates/*`.
2. **Add new CLI verbs** that use the new routes: `cmd_workspaces_list`, `cmd_workspace_launch`, etc.
3. **Add flag handlers to existing verbs**: `cmd_msg` gets `--inbox` + `--signal`; `cmd_checkin` gets `--status` + `--done`.
4. **Add deprecation wrappers**: `cmd_work_send` calls `cmd_msg --inbox` with deprecation warning; `cmd_signal` calls `cmd_msg --signal`; etc.
5. **Add `help-deprecated`** + reorganize `cmd_help` into the 3-tier structure.
6. **Hard-deprecate** `agents create / delete`, `agentic`, `state`, `mode`, `app-update`, `commit-merge`, `companion`, `roster`, `feed`, `signal` (top-level), `work send` (top-level), `status` (top-level): replace each function body with an error message + `help-deprecated` pointer.
7. **Rename**: `feed` → `activity`, `roster` → `who`, `onboarding later/fresh` → `defer/start-fresh`. The old names become hard-deprecated.
8. **Delete dead code paths** uncovered by the audit (old comments, unused helpers, etc.).
9. **Run the test suite** (existing + new from A8).
10. **`K2SO_DAEMON_URL=<remote>` end-to-end smoke** against a foreground daemon on a different port.

Total subagent time estimate: 60-90 min (larger than the original ~30-60 min estimate because Appendix A roughly doubled the scope).
