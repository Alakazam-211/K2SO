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
