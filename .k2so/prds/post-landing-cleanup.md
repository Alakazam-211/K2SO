# Post-Landing Cleanup

**Status:** Planned. Gated on Alacritty_v2 AND Kessel-T1 both shipping
and proving stable.
**Captured:** 2026-04-24
**Prerequisites:**
- `.k2so/prds/alacritty-v2.md` — fully landed, v1 retired, no parity regressions for ≥1 week of real use.
- `.k2so/prds/kessel-t1.md` — Claude adapter (K1-K5) shipped at minimum; additional adapters optional before cleanup.

## Why this PRD exists

The path from Phase 6 → v2 + Kessel-T1 leaves behind a meaningful
amount of T0-era Rust and TypeScript: multi-stream machinery,
grow-then-shrink choreography, byte rings, APC filters, dual-`Term`
plumbing, feature flags for experiments that are now settled.

Most of that code is *correct for what it was solving* — it just isn't
solving a problem we still have. Some of it contains non-obvious
lessons we might want again someday (APC filter pattern for T2; grow-
shrink as a reference for terminal virtualization tricks). The rest is
dead weight that slows compile times and muddies mental models.

This PRD is the **cleanup rulebook**: what gets deleted, what gets
mothballed (preserved out-of-workspace for later reference), what must
not be touched.

## Philosophy

1. **Preserve learning, shed weight.** Code with reference value gets
   mothballed. Code with no reference value gets deleted outright.
2. **Mothballed ≠ versioned.** A `.mothballed/` directory at repo root,
   listed in `.gitignore`. Team members who want the learning can keep
   a local copy; it doesn't ship in git history forward, doesn't
   burden the workspace Cargo build, doesn't appear in grep results
   for new contributors. (Commit history still has it if we ever need
   the actual diff.)
3. **One cleanup, not a rolling one.** Do this in a single branch after
   both PRDs land, not as we go. A rolling cleanup tempts us to
   prematurely remove things we end up needing.
4. **Every deletion is paired with a git tag.** Tag as
   `pre-cleanup-<date>` before starting, so history is trivially
   recoverable even for deleted files.
5. **Tests are load-bearing.** If a test still passes AND exercises a
   real code path, it stays regardless of which era it was written for.

## The mothball process

```bash
# 1. Tag the pre-cleanup state
git tag pre-cleanup-2026-XX-XX

# 2. Create the mothball directory (first time only)
mkdir -p .mothballed

# 3. Ensure it's git-ignored
grep -q '^.mothballed/$' .gitignore || echo '.mothballed/' >> .gitignore

# 4. Move files (or copy then delete) into .mothballed/<topic>/
mv src-tauri/src/commands/kessel_term.rs .mothballed/kessel-t0/
# ... etc.

# 5. Each mothball subdirectory gets a MOTHBALL.md explaining:
#    - What was this code for?
#    - Why is it preserved?
#    - What would I read it for?
#    - Last-commit SHA where it was live.
```

`.mothballed/` is intentionally ignored so:
- `cargo build` skips it (no compile burden).
- `rg` / `grep` doesn't surface it when searching live code.
- It's available on your machine as a reference library, but not
  synced via git to teammates unless they explicitly pull it across.

## What to mothball (reference value)

### `.mothballed/kessel-t0-bytestream/`

Everything Kessel-T0 invented that won't come back in its original form
but encodes real learning.

| Path (pre-cleanup) | Mothball reason |
|---|---|
| `src-tauri/src/commands/kessel_term.rs` | Dual-`Term` pattern, APC filter implementation, damage-to-delta serializer. The APC filter is the best reference for a future T2 parser. The snapshot/delta logic migrated to `k2so-core/src/terminal/grid_snapshot.rs` for v2, but this file has the full original with its APC-aware branches intact. |
| `crates/k2so-core/src/terminal/grow_settle.rs` | The settle-detection state machine (ModeChange → AltScreen / Bracketed / Focus; idle timer; ceiling fallback). Interesting pattern even if we never grow-shrink again. |
| `crates/k2so-daemon/src/sessions_bytes_ws.rs` | The subscribe-before-snapshot race pattern (and the bug it caused). Worth re-reading before anyone designs another broadcast protocol. |
| Grow-shrink branches from `crates/k2so-core/src/terminal/session_stream_pty.rs` | Only the grow-shrink portions; the rest of the file is deleted separately or survives in Alacritty_v2's `daemon_pty.rs`. Extract the grow branches into a standalone `grow_shrink.rs` in the mothball. |
| `src/renderer/kessel/SessionStreamViewTerm.tsx` + `KesselTerminal.tsx` (Phase 6 forms) | Reference for "React component that holds a local mirror of a remote Term snapshot." The rendering logic itself migrates into Alacritty_v2's thin client; these originals show the full byte-stream-consumer pattern. |

### `.mothballed/t05-recognizers/`

The pattern-match-the-TUI approach is deferred but not dead. Three tools
might want it later (Aider, Copilot CLI, Code Puppy).

| Path | Mothball reason |
|---|---|
| `crates/k2so-core/src/term/recognizers/claude_code.rs` | Phase 1 T0.5 recognizer, written before we pivoted to T1. Demonstrates the `Recognizer` trait shape and box-pattern matching. If anyone revives T0.5, this is the starting point. |
| `crates/k2so-core/src/term/recognizers/mod.rs` | The recognizer registry. Same reasoning. |
| `crates/k2so-core/src/term/line_mux.rs::with_recognizer()` method (if LineMux is also retired) | Moves with LineMux itself, see below. |

### `.mothballed/linemux/` *(conditional)*

LineMux's fate depends on whether any live code still uses it post-v2
+ Kessel-T1:

- **If nothing uses it:** mothball entirely. The VTE-to-Frame tokenizer
  is a clean pattern worth preserving.
- **If heartbeat activity tracking / idle detection still uses it:**
  it stays in `k2so-core`. Skip this mothball entry.

Verify with:
```bash
rg 'LineMux|line_mux::' crates/ src/ src-tauri/ --type rust --type ts
```

If the only hits are within LineMux itself + its tests, it's safe to
mothball.

### `.mothballed/session-stream-prds/`

Old strategic docs that are now superseded.

| Path | Mothball reason |
|---|---|
| `.k2so/prds/session-stream-and-awareness-bus.md` (the multi-stream byte-replay sections) | The Awareness Bus parts are still live; split the doc — keep Awareness Bus in `.k2so/prds/awareness-bus.md` (new), mothball the rest. Or: add a "SUPERSEDED BY" header and leave in place. |
| `.k2so/prds/canvas-plan.md` | Entire PRD was T0's canvas-plan. Superseded by Alacritty_v2 and Kessel-T1. Mothball whole. |
| `.k2so/notes/kessel-launch-perf-plan.md` + `phase-3.2-hardening-plan.md` + `phase-4.6-kessel-parity-plan.md` + other Phase-N notes | Historical Kessel-T0 phase notes. Mothball, but add a `.k2so/notes/README.md` pointing at the archive so the trail is discoverable. |

## What to delete outright (no reference value)

### Code paths

| Path | Why delete |
|---|---|
| `src/renderer/kessel/daemon-ws.ts` (Kessel-T0 daemon-ws client) | Replaced by v2's simpler WS client. The T0 version handled byte-stream replay + APC, which no one does post-cleanup. No instructive value. |
| Phase 6/7/8 perf instrumentation scaffolding (already partially stripped) | Dead branches, dead logging. |
| `kessel_term_resize` Tauri command (if still present) | Was only used by T0's ResizeObserver; A3's WS protocol replaces it. |
| Feature flags: `use_session_stream` (project setting + code branches) | If the setting exists in the DB schema, add a migration to drop the column; code branches go. |
| Any `grow_rows` / `grow_boundary` references outside `session_stream_pty.rs` | Dead with grow-shrink gone. |

### Cargo.toml entries

`src-tauri/Cargo.toml`:
- Drop `alacritty_terminal = "0.26.0-rc1"` — Tauri no longer hosts a Term.
- Drop `vte = "0.15"` — Tauri no longer parses ANSI.
- Keep `portable-pty` ONLY if Tauri still spawns non-daemon PTYs anywhere. If not, drop.
- Keep `tokio-tungstenite` — still used for WS client.

`crates/k2so-core/Cargo.toml`:
- Keep everything — daemon-side Terms + adapters still need these.

### Tests

| Path | Why delete |
|---|---|
| `crates/k2so-core/tests/session_stream_grow_shrink.rs` | Grow-shrink gone. |
| `crates/k2so-core/tests/session_stream_archive.rs` | On-disk archive was T0-adjacent; unused. |
| Any test that exercises `kessel_term_*` Tauri commands end-to-end | Those commands are gone. |

### Database migrations

- If migration added `use_session_stream` to `projects` or similar:
  add a new migration to drop it after cleanup lands. Do NOT retro-
  edit the original migration — that breaks anyone on an older DB.

## What must NOT be touched

Guard rails to avoid overshooting:

| Path | Why it stays |
|---|---|
| `crates/k2so-core/src/session/` (registry, entry, frame) | Multi-subscriber broadcast, Frame schema, session lifecycle. All still load-bearing for Kessel-T1 and for heartbeat targeting. |
| `crates/k2so-core/src/awareness/` | Awareness Bus, cross-agent signaling. Completely independent of the byte-stream experiments. |
| `crates/k2so-daemon/src/sessions_ws.rs` | Frame-stream WS endpoint. Kessel-T1's wire. |
| `portable-pty` + `alacritty_terminal` + `vte` in `crates/k2so-core/Cargo.toml` | Alacritty_v2's daemon-side Term needs all three. |
| Heartbeat + scheduler paths (`crates/k2so-daemon/src/scheduler*`, `awareness_ws.rs`) | Orthogonal to renderer choice; both renderers consume from these. |
| `session::registry::lookup_by_agent_name` (or equivalent) | Used by Alacritty_v2's find-or-spawn AND heartbeat. |
| `.k2so/prds/alacritty-v2.md` + `.k2so/prds/kessel-t1.md` | These are the active PRDs. |
| `.k2so/notes/renderer-roadmap-post-t0.md` | Historical context for why the pivot happened. Keep. |

## Cleanup sequence

1. **Confirm the two PRDs are landed** and stable. Defined as: ≥1 week
   of real use on Alacritty_v2 with no regressions; Kessel-T1 Claude
   adapter demonstrably rendering tool calls, messages, and thinking
   blocks correctly.
2. **Tag: `git tag pre-cleanup-<yyyy-mm-dd>`**. Push the tag.
3. **Branch: `chore/post-landing-cleanup`**.
4. **Create `.mothballed/`** and add to `.gitignore`.
5. **Mothball first, delete second.** Do one subdirectory at a time,
   verify `cargo check` + `cargo test` + `npm run build` + `npx tsc --noEmit`
   all still green after each.
6. **Drop Cargo.toml deps** last (requires no code references first).
7. **Write the drop-column migration** if needed.
8. **Update README + any `.k2so/prds/` index** to reflect the final
   two-renderer landscape.
9. **Run the UX parity checklists** from both PRDs one more time on
   the cleanup branch.
10. **Merge. Done.**

## Estimated scope

| Phase | Work | Effort |
|---|---|---|
| Mothball moves | ~15-20 files, careful commit-per-topic | 0.5 day |
| Source deletions (code paths, feature flags) | ~10-15 files | 0.5 day |
| Cargo.toml cleanup + dep resolve | 1 file per crate | 0.25 day |
| Test deletions | ~5 files | 0.25 day |
| DB migration for dropped columns (if any) | 1 migration + forward-compatibility check | 0.25 day |
| UX parity re-validation | Re-run both PRD checklists | 0.5 day |
| Documentation updates (README, PRD index) | | 0.25 day |
| **Total** | | **~2.5 days focused work** |

## Verification gates

After cleanup, the following must hold:

- `cargo build --workspace --release` succeeds with no warnings about
  missing files or unused deps.
- `cargo test --workspace` all green.
- `npx tsc --noEmit` clean.
- `npm run build` produces a Tauri bundle. Bundle size should be
  measurably smaller (target: `src-tauri` binary at least 2 MB
  smaller, reflecting dropped `alacritty_terminal` + `vte` deps —
  they're large crates).
- `rg` for any grep-target name of a mothballed file returns nothing
  in live code (only in tests/docs that reference history, if any).
- Both renderer parity checklists (from `alacritty-v2.md` and
  `kessel-t1.md`) pass on the cleanup branch.

## The "two years from now" test

If someone reading the codebase in 2028 asks "why was this removed?",
they should find:

- A `git log --follow` that shows the file's full history (not amended
  away).
- A `pre-cleanup-<date>` tag they can check out to restore it.
- This PRD in `.k2so/prds/` explaining the deletion + mothball rules.
- A `MOTHBALL.md` inside the (locally-preserved) mothball subdirectory
  explaining what each file was for.

If all four are true, the deletion was responsible. If any is false,
we lost something.

## Sign-off criteria

This PRD is complete when:

1. Alacritty_v2 + Kessel-T1 (Claude at minimum) are both shipping and
   stable.
2. The cleanup branch merged to main.
3. The `pre-cleanup-<date>` git tag is pushed and discoverable.
4. `.mothballed/` exists on at least one developer's machine with
   per-topic `MOTHBALL.md` files.
5. Binary size and compile times are measurably improved (record the
   numbers somewhere).
6. No reported regressions traced to the cleanup for 2 weeks post-merge.

## Add-on: Unify renderer terminalId with daemon session_id

**Captured:** 2026-04-30
**Trigger:** `.k2so/prds/heartbeat-active-session-tracking.md` shipped
with an `agent_name` bridge (`tab-<rendererId>`) joining renderer-side
tab ids and daemon-side session ids. The bridge works but every consumer
that wants to join the two has to remember it exists. Forgetting is
silent — cross-references just fail to match, no exception thrown. We
already hit this twice while landing the heartbeat-active-session
fix.

### Problem

Two id spaces today:

- **Renderer's `terminalId`** — UUID generated locally at tab creation
  (Cmd+T, openHeartbeatTab, drag-drop). Used as the tab key in
  `useTabsStore` and as the prefix in `agent_name = "tab-${terminalId}"`
  for v2 spawns.
- **Daemon's `session.session_id`** — UUID generated by
  `DaemonPtySession::spawn`. Stored on the v2 session, returned in the
  `/cli/sessions/v2/spawn` response, persisted as `active_terminal_id`
  on `agent_heartbeats`, used as the route key in
  `/cli/terminal/{write,resize}`, and as the inject target by
  `awareness::send_signal` and `find_live_for_resume`.

The two are joined via `agent_name` — the daemon registers under
`"tab-<rendererId>"` and the renderer derives that name on attach.
Every cross-stack call site has to encode the join.

User-facing consequence: right-click → **Copy Terminal ID** today gives
the renderer's local id. Pasting that into `k2so msg`-style "talk to
this conversation" tooling fails because the daemon doesn't know that
id — the actual address is the daemon's session_id.

### Goal

One canonical id per session. The id you copy is the id every push /
inject / write / cross-reference uses. The renderer keeps a local
"tab ordering" key for UI bookkeeping if it needs one, but the
*session identity* is single-sourced from the daemon.

### Two flavors

**Flavor A — daemon-assigned, renderer-mirrors.**
`/cli/sessions/v2/spawn` already returns `{ sessionId, agentName }`.
Renderer reads `sessionId` from the response and replaces the local
UUID it pre-generated, so the tab's `terminalId` field becomes the
daemon's id from then on. Brief id-swap window between tab creation
and spawn response — manageable, but a real moment of dual-state.

**Flavor B — renderer-supplies-id, daemon-validates.**
Renderer pre-generates the UUID and sends it as the desired
`session_id` in the spawn request body. Daemon validates uniqueness
against `v2_session_map` + `session_map` and uses it verbatim as the
session's id. No swap window: terminalId === session_id from the
moment the tab is created.

Lean toward **Flavor B** — fewer moving parts, no transient state,
and it cleanly handles offline / queued tab creation (renderer can
generate the id without the daemon being reachable yet).

### Daemon-spawned sessions

Heartbeats and awareness wakes spawn without a renderer in the loop.
Daemon generates the id and writes it into `agent_heartbeats.active_terminal_id`
(already does). When the renderer surfaces one of those, it reads the
id from the active-session endpoint and uses it as the tab's
`terminalId` directly — no synthesis of a separate local id.

### Addressing conventions

After unification, two address forms:

- **Precise** — `<sessionId>` UUID. "Talk to **this exact**
  conversation." The Copy Terminal ID context-menu yields this.
  Stable across reconnects, single-source-of-truth.
- **Friendly** — `<workspace>:<agent>` (as `k2so msg`-style). "Send
  to whoever is the cortana agent in the Cortana workspace." Used
  when the user doesn't care which specific session — the daemon
  picks the live one (or spawns).

Document both in CLAUDE.md / agent skill output so agents copying
ids know which they have and what each does.

### Migration

- `/cli/sessions/v2/spawn` body gains optional `session_id`.
  Daemon validates uniqueness, falls back to generating one when
  omitted (existing callers continue working).
- `workspace_sessions` table reconciliation: on first-load after
  the change, walk rows and replace renderer-only ids with the
  daemon's id (if known); orphan rows (id not in any map and no
  matching last-known) get archived.
- `Copy Terminal ID` returns `daemon-session-id` instead of the
  renderer-local one.
- Cross-references throughout (closeTerminalForRenderer's heartbeat
  detection, openHeartbeatTab's tab-find loop, the heartbeats sidebar
  panel's live indicator) collapse to single equality checks.

### Touched files (rough)

- `src/renderer/terminal-v2/TerminalPane.tsx` — read response, swap id
- `src/renderer/stores/tabs.ts` — terminalId becomes daemon-sourced
- `src/renderer/components/PaneLayout/PaneGroupView.tsx` — use single id
- `crates/k2so-daemon/src/v2_spawn.rs` — accept optional session_id
- `crates/k2so-core/src/db/schema.rs` — `workspace_sessions` reconcile
- Every test that hardcodes a tab id format

### Definition of done

1. Right-click → Copy Terminal ID returns the daemon's session_id.
2. The id pasted as `k2so terminal write <id>` writes to the same
   PTY the user is looking at.
3. `agent_name` in `v2_session_map` is no longer prefixed (or the
   prefix is structural-only metadata, not part of the id space).
4. `find_live_for_resume`, `closeTerminalForRenderer`'s heartbeat
   detection, and `openHeartbeatTab`'s tab-find loop all do simple
   equality on terminalId — no string-prefix manipulation.
5. The agent_name bridge in `.k2so/prds/heartbeat-active-session-tracking.md`
   is retired (or kept only for legacy session_map / Kessel sessions).
