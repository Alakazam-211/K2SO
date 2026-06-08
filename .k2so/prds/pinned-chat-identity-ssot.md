# Pinned-Chat Identity — Single Source of Truth (SSOT)

**Status:** DRAFT (proposed for 0.40.x) — **Explore-validated 2026-06-08**
(see §10 findings; the headline correction: restart-recovery currently reads
`workspace_tab_sessions`, not the SSOT — that's the coupling to undo).
**Author:** pod-leader (with Rosson)
**Related:** GH#24 (remote pinned-chat re-mint loop), #683 (daemon-owned
pinned chat), #679 (SQLite-canonical session selection), #681 (brand-new
workspace mint), #657 (dismiss → grace-reap), #672 (daemon-canonical Active).
Supersedes the 0.39.40 *converge band-aid* (commit `9541cd6`) as the root fix.

---

## 1. Problem

A workspace has **exactly one** pinned chat. Yet the *identity* of that chat
— **which Claude conversation it is** — is currently stored in **three**
places, written by different mechanics, none of which is authoritative:

| # | Location | Durable? | Written by | Holds |
|---|----------|----------|------------|-------|
| 1 | `workspace_sessions.session_id` (1 row/workspace; `project_id UNIQUE`) | ✅ SQLite | resolver mint, `set-chat-session`, `workspace_msg` | the intended/saved id |
| 2 | `workspace_tab_sessions.session_id` (row where `agent_name = project_id`) | ✅ SQLite | `v2_session_map::register` (argv-derived) | argv id, or NULL |
| 3 | argv inside the live v2 map entry | ❌ RAM | spawn | `--session-id`/`--resume` value |

All three capture **intent** (the id we put in `--session-id`/`--resume`),
never the id **Claude actually adopted on disk**. When the canonical PTY is
spawned *bare* (no `--session-id`) or *reused* (not re-spawned), intent ≠
reality and nothing reconciles them. GH#24 is the visible failure: the
resolver kept minting a fresh id (because the saved id had no `.jsonl`),
overwriting #1 with an unconfirmed pre-allocation that a reused PTY never ran
→ next resolve re-minted → unbounded loop on remote/companion clients (which
re-request resume-args on every reconnect).

The 0.39.40 fix made the resolver *converge* (resume the newest on-disk
session before minting). That stops the loop and is durable, but it heals the
record **on read** rather than recording truth **at the source**. The
identity is still scattered and still argv-derived.

### Root insight

There are **two distinct facts**, and they belong in **two distinct homes**:

- **Identity** — *which Claude conversation is this workspace's pinned chat?*
  Durable, must survive daemon restart, must be correct headless. → **one
  SQLite column.**
- **Liveness** — *is it running right now, and what is the attach handle?*
  Ephemeral, dies with the daemon. → **the v2 session map (RAM).**

When #683 made the pinned chat daemon-owned, it should have leaned on the
existing canonical column for identity. Instead `ensure-pinned-chat` re-derived
identity from argv on every call and let `workspace_tab_sessions` double-book a
second copy. This PRD corrects that.

---

## 2. Goal / Non-Goals

**Goal.** Collapse pinned-chat identity to a **single source of truth**:
`workspace_sessions.session_id`, holding the session **Claude actually
adopted on disk**. Everything else (the map, `active_terminal_id`,
`workspace_tab_sessions`) becomes a *liveness cache* or is scoped to ad-hoc
tabs only. After this, the only state machine left for a pinned chat is the
4-state lifecycle (Absent / Detached / Live / Grace).

**Non-Goals.**
- Ad-hoc Cmd+T tabs keep using `workspace_tab_sessions` (they have **no**
  `workspace_sessions` row — that table stays, unchanged, for them).
- No change to the Active/reaper machinery (#672/#657) beyond reading the SSOT.
- No renderer session-orchestration revival — renderer still attaches only.
- No host-switch / K2 Toge changes.

---

## 3. Canonical model (the rule)

> **Pinned-chat identity lives in `workspace_sessions.session_id` and nowhere
> else.** Its value is the session Claude adopted on disk. The v2 map holds
> *liveness* only. `workspace_tab_sessions` is for ad-hoc tabs; the pinned
> chat never reads or writes its identity there.

| Fact | Home | Lifetime |
|------|------|----------|
| Pinned identity (which conversation) | `workspace_sessions.session_id` | durable |
| Is-it-live + attach handle | v2 session map, keyed `project_id` | ephemeral |
| Durable pointer to last live PTY | `workspace_sessions.active_terminal_id` | durable cache |

---

## 4. Design

### 4.1 Write truth at the source: post-spawn read-back

After `ensure_pinned_chat` spawns (or confirms a reused PTY), stamp
`workspace_sessions.session_id` with the id **Claude actually adopted**,
discovered from disk — the same mechanism the heartbeat/wake path already uses
(`chat_history::detect_claude_session` ~5s after spawn; see
`wake_headless.rs:220/274`). Reuse `newest_claude_session_on_disk`
(added in 0.39.40) as the disk-truth probe.

- **Fresh spawn with `--session-id <X>`:** X is the intended id. Claude will
  write `X.jsonl`. The read-back confirms X (or, if Claude diverged, records
  what it actually wrote). Stamp #1 = confirmed id.
- **Reused PTY:** don't re-resolve identity at all — read the current truth
  (newest on-disk for the project = the running session) and return it. #1 is
  already correct (the resolver/earlier read-back set it); just confirm.
- **Bare-spawned legacy PTY:** read-back finds Claude's auto-created session
  and stamps #1 — self-correcting regardless of how the PTY was born.

The read-back must be **deferred** (Claude writes its `.jsonl` a beat after
spawn). Two layers, both kept:
- **(a) Eager deferred read-back** — mirror the wake path's existing deferred
  block (`wake_headless.rs:259-291`: a `std::thread::spawn` that sleeps ~5s,
  calls `chat_history::detect_active_session("claude", path)` at `:263`, then
  `workspace::session::k2so_agents_save_session_id` → `update_session_id`).
  **Factor that block into a shared `defer_stamp_adopted_session(project_path,
  project_id, provider)` and call it from both `wake_headless` AND
  `ensure_pinned_chat` after a fresh spawn.** This makes the column truthful
  without requiring a second view.
- **(b) Lazy self-heal** — the 0.39.40 resolver fallback
  (`newest_claude_session_on_disk` before mint). Kept as the safety net for
  any path that misses (a). **Do not remove it** — removing it re-opens GH#24.

**Reused-PTY path (recommended minimal change):** keep calling
`resolve_resume_chat_args` on every `ensure` (its converge fallback is already
battle-tested), and rely on the eager read-back (a) to have stamped the
adopted id. The explicit "reused → skip resolve, read SSOT directly"
short-circuit is *cleaner* but duplicates the spawn-helper idempotency
pre-check, so it's optional polish, not required.

### 4.2 Resolver becomes a near-pure read

`resolve_resume_chat_args` should:
1. Read `workspace_sessions.session_id` (the SSOT).
2. If its `.jsonl` exists → `--resume <id>` (happy path).
3. Else fall back to `newest_claude_session_on_disk` and **persist** it
   (the 0.39.40 converge behavior — kept as the self-heal).
4. Only if there is **no** on-disk session at all (state **Absent**) → mint a
   fresh `--session-id` (preserves #681).

It must **never** overwrite a confirmed id with an *unconfirmed* mint. (This is
the precise defect behind GH#24.)

### 4.3 Repoint restart-recovery to the SSOT (the load-bearing change)

**Validated current state:** when the daemon restarts, the v2 map is empty, so
`v2_spawn` restart-recovery (`v2_spawn.rs:186-247`) re-spawns the pinned chat
by reading the prior **`workspace_tab_sessions`** row — `saved_cmd`,
`saved_args`, and that table's argv-derived `session_id`. **This is why the
daemon-owned pinned chat (#683) bypassed the canonical column for identity:
recovery was wired to the tab table, not to `workspace_sessions`.**

`v2_session_map::register` (`:99`–`:131`) also *writes* that
`workspace_tab_sessions.session_id` from argv on **every** registration,
including the pinned chat (`agent_name == project_id`) — a redundant second
identity store.

**This is not an A/B decision; it's a mandatory two-part change:**

1. **Restart-recovery for the pinned chat (`agent_name == project_id`) reads
   `workspace_sessions.session_id`** (the SSOT) to splice `--resume`, instead
   of `workspace_tab_sessions.session_id`. (`v2_spawn.rs:186-247`.) The
   command/cwd can still come from the tab row or default to `claude` +
   project path.
2. **Stop writing the pinned-chat row to `workspace_tab_sessions`** (option B):
   in `v2_session_map::register`, skip the stamp when `agent_name ==
   project_id` (the pinned/heartbeat canonical key). **Validated safe** — the
   session-picker (#679) reads chat history **from disk** (`list_all_sessions`
   → `parse_claude_sessions`), *not* from `workspace_tab_sessions`, so the
   dropdown is unaffected. Existing pinned rows are harmless; optional one-line
   cleanup.

`workspace_tab_sessions` stays exactly as-is for **ad-hoc Cmd+T tabs**
(`agent_name == tab-<paneGroupId>`), which have no `workspace_sessions` row and
genuinely need it for restart-recovery.

### 4.4 Liveness stays in the map; `active_terminal_id` is a cache

No change to the map's role. `active_terminal_id` remains the durable pointer
to the last live PTY (already stamped by `ensure_pinned_chat` step 5). It is
**not** identity and must never be confused with `session_id`.

---

## 5. The 4-state lifecycle (what's left after the collapse)

| State | PTY (map) | `session_id` (SSOT) | on-disk |
|-------|-----------|---------------------|---------|
| **Absent** (brand-new) | — | NULL | none |
| **Detached** (has history) | — | set | yes |
| **Live** | present | set + confirmed | yes |
| **Grace** (dismissed, awaiting reap) | dying | set | yes |

Transitions: Absent→Live (mint+spawn+read-back), Detached→Live (resume+spawn),
Live→Grace→Detached (#657 reap), Live→Live (reattach to reused PTY — **no
re-mint, no re-resolve of identity**). Restart: Live→Detached (map cleared),
recover via SSOT.

---

## 6. Wire contract

`POST /cli/workspace/ensure-pinned-chat` response unchanged in shape. **Once
the eager read-back (§4.1a) lands**, `claudeSessionId` reflects the **SSOT**
(the confirmed/adopted id), stable across reused calls. Until then it reflects
the resolver's converged value (already stable per the 0.39.40 band-aid).
`resumedExisting` reflects whether a real prior conversation was resumed (true
for Detached/Live re-resolve, false only for Absent). No new routes.

---

## 7. Migration / cleanup

- **No schema change required** — `workspace_sessions.session_id` already
  exists and is the right column.
- One-time/boot reconciliation (idempotent): for each workspace with a live or
  recent pinned session whose `session_id` is missing/stale, stamp it from
  `newest_claude_session_on_disk`. (The 0.39.40 resolver already self-heals on
  read; an eager boot sweep makes headless correct without a first view.)
- If decision **(B)**: stop writing the pinned-chat `workspace_tab_sessions`
  row; existing rows are harmless (left in place) or cleaned by a one-liner.

---

## 8. Test plan

- **Unit:** resolver never overwrites a confirmed id with an unconfirmed mint;
  mints only in Absent; read-back stamps the adopted id.
- **Integration (daemon):** extend `pinned_chat_ensure_integration.rs` —
  - reused-PTY `ensure` returns a **stable** `claudeSessionId` across N calls
    (the GH#24 regression lock);
  - bare-spawned PTY → read-back stamps the real session into the SSOT;
  - restart-recovery resumes from `workspace_sessions.session_id`.
- **Headless:** boot reconciliation stamps a stale workspace with no client
  attached (curl `/cli/...`).
- Keep the existing 4 ensure tests green; keep #681 brand-new-mint behavior.

---

## 9. Risks

- **Restart-recovery repoint is load-bearing (§4.3.1).** Recovery currently
  reads `workspace_tab_sessions` (`v2_spawn.rs:186-247`); the racy daemon-
  restart path must be re-pointed to the SSOT and tested across a real restart
  cycle. This is the highest-risk change.
- **Read-back timing.** Claude writes its `.jsonl` slightly after spawn; a
  too-eager read-back misses it. Mitigated by the ~5s deferred poll (§4.1a,
  same window the wake path uses) + the lazy resolver self-heal (§4.1b) as the
  net.
- **Keep the converge fallback.** Removing the resolver's
  `newest_claude_session_on_disk` fallback (`resume_chat.rs`, the 0.39.40 fix)
  re-opens GH#24. It stays as the safety net.
- **Worktree cwd** — `newest_claude_session_on_disk` mirrors
  `claude_session_file_exists`'s dir set (both via `resolve_root_project_path`,
  stripping `/.worktrees/<branch>`); resuming across a `<hash>-<branch>`
  sibling remains resumable (pre-existing assumption, validated).

> **Picker coupling — validated NOT a risk.** The session-picker (#679) reads
> chat history from disk (`list_all_sessions` → `parse_claude_sessions`), never
> from `workspace_tab_sessions`. Dropping the pinned-row write is safe.

---

## 10. Validation findings (Explore, 2026-06-08 — resolved)

1. **Restart-recovery source →** `workspace_tab_sessions.session_id`
   (`v2_spawn.rs:186-247`), NOT the SSOT. **This is the coupling to undo**
   (§4.3.1). Not an A/B decision — a mandatory repoint.
2. **Picker (#679) →** reads chat history **from disk**
   (`misc_routes.rs:795` → `chat_history::list_all_sessions` →
   `parse_claude_sessions`), never `workspace_tab_sessions`. Dropping the
   pinned-row write is **safe**.
3. **Reusable read-back →** yes, `wake_headless.rs:259-291` (deferred
   `std::thread::spawn` → sleep ~5s → `detect_active_session` at `:263` →
   `k2so_agents_save_session_id`). Factor into a shared
   `defer_stamp_adopted_session` and call from `ensure_pinned_chat` too.
4. **Writers to `workspace_sessions.session_id` →** four families:
   (i) the resolver (`resume_chat.rs` upserts — mint + converge);
   (ii) `WorkspaceSession::update_session_id` (`schema.rs:1217`) via
   `set-chat-session` (`workspace_routes.rs:102`, intentional user override),
   `workspace_msg.rs:542`, and the wake deferred-save (`workspace/session.rs`);
   (iii) `WorkspaceSession::upsert` with `COALESCE` (preserves on NULL);
   (iv) heartbeat does **NOT** touch this column (it writes
   `workspace_heartbeats.last_session_id`, a separate table). "Single writer"
   isn't literally achievable (the user override is legitimate), but **single
   *authority*** is: the resolver + deferred read-back own the auto path;
   `set-chat-session` is an explicit user act. Good enough.
5. **Bare-spawn origin →** on current main, NO path spawns the canonical pinned
   PTY without `--session-id`/`--resume` (`ensure`, heartbeat fire, workspace_
   msg, and renderer `/v2/spawn` all inject one). The bare
   `["--dangerously-skip-permissions"]` `args_json` in GH#24 was the *residue*
   of a pre-allocated-but-never-run session on a long-running host, not a live
   spawn path. The read-back self-corrects it regardless — so no separate fix
   needed (Option 2 from the original choice is moot, as predicted).
6. **Reused-path change →** keep calling `resolve_resume_chat_args` (converge
   fallback is battle-tested); rely on the eager read-back to keep the SSOT
   truthful. Explicit reused short-circuit is optional polish.

## 11. Implementation plan (phased; for a future 0.40.x cycle)

1. **Phase 1 — eager read-back (additive, no breakage).** Factor
   `wake_headless.rs:259-291` into a shared `defer_stamp_adopted_session`; call
   it from `ensure_pinned_chat` after a fresh (`!reused`) spawn.
2. **Phase 2 — repoint restart-recovery (the racy one).** `v2_spawn.rs:186-247`:
   for `agent_name == project_id`, read `workspace_sessions.session_id` for the
   `--resume` id. Test across a real daemon restart.
3. **Phase 3 — stop double-booking.** `v2_session_map::register` (`:99-131`):
   skip the `workspace_tab_sessions` stamp when `agent_name == project_id`.
   (Optional one-line cleanup of existing pinned rows.)
4. **Phase 4 — regression locks.** Extend `pinned_chat_ensure_integration.rs`:
   reused `ensure` returns a **stable** `claudeSessionId` across N calls
   (GH#24 lock); restart→reattach resumes from the SSOT; headless boot
   reconciliation stamps a stale workspace with no client attached.

### Key file:line map (from validation)

| Component | File | Lines |
|---|---|---|
| Resolver | `crates/k2so-core/src/workspace/resume_chat.rs` | 69-183 |
| `ensure_pinned_chat` | `crates/k2so-daemon/src/pinned_chat.rs` | 125-232 |
| Spawn + idempotency (reuse) | `crates/k2so-daemon/src/spawn.rs` | 112-259 |
| **Restart-recovery (repoint)** | `crates/k2so-daemon/src/v2_spawn.rs` | 186-247 |
| Register (tab-session writer) | `crates/k2so-daemon/src/v2_session_map.rs` | 99-131 |
| **Deferred read-back (reuse)** | `crates/k2so-daemon/src/wake_headless.rs` | 259-291 |
| Detect helpers | `crates/k2so-core/src/chat_history.rs` | 87-178, 337-384 |
| Schema upsert/update | `crates/k2so-core/src/db/schema.rs` | 1074-1080, 1217-1222 |
