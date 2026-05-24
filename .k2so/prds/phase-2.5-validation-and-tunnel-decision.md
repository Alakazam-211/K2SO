# Phase 2.5: Post-Phase-2 validation + tunnel-provider decision

**Status**: Drafted 2026-05-23 while Phase 2.1 implementation runs in parallel. Launches after Phase 2.1 merges to main. Blocks Phase 3 entry (specifically Workstream B — TLS + auth upgrade — which depends on the tunnel-provider decision).
**Internal version markers**: 0.39.0g (validation phase; no version bump expected).
**Owner**: Rosson + pod-leader
**Date**: 2026-05-23

---

## tl;dr

Phase 2 + 2.1 + 2.2 ship a massively-restructured daemon + thin client + CLI. Before Phase 3 (contract hardening for Mobile Companion + K2SO Connect) begins, two things have to happen:

1. **Validate the migrated state in a real running `.app`** — `cargo test` doesn't prove the bundled `.app` works; we need to install + exercise every migrated path interactively.
2. **Lock the tunnel-provider decision** — Phase 3's TLS + auth architecture depends on whether we keep ngrok, switch to Cloudflare Tunnel, or pick another option.

Phase 2.5 is the bridge between "Phase 2 done in code" and "Phase 3 contract hardening can start."

Five workstreams. Estimated ~1-2 days of focused work.

---

## Why this exists

The original sequencing went: Phase 2 (daemon-headless migration) → Phase 2.1 (CLI redesign) → Phase 2.2 (schema hygiene) → **Phase 3 (contract hardening + Mobile Companion + K2SO Connect)**.

Rosson flagged the gap (2026-05-23 mid-Phase-2.1-validation-loop):

> I think after Phase 2 we should probably make sure that the app works and that all the changes we made take when we build/launch the app and update all of our tests that were likely using many of those CLI tools. Then we can loop around and validate the contracts for desktop + mobile. We will also need to finalize our decision of ngrok vs alternatives like Cloudflare's Tunnel vs LocalXpose.

Three concerns hidden in that sentence:
1. The `.app` build hasn't been validated end-to-end against the migrated daemon. `cargo test` passed throughout Phase 2, but the bundled `.app` (daemon sidecar + Tauri frontend + renderer) has never been smoke-tested as a unit.
2. Tests that used the old CLI verbs (`k2so work *`, `k2so agents *`, `k2so delegate`, `k2so done`, `k2so feed`, `k2so signal`, etc.) are still around. Soft-deprecation aliases keep most of them working, but verification matters; hard-deprecated verbs MUST be migrated.
3. The tunnel-provider decision is a Phase 3 prerequisite that hasn't been settled. Phase 3 Workstream B (TLS + auth) makes assumptions about whether TLS terminates at the provider edge or passes through to the daemon — that fundamentally changes the cert-pinning strategy clients use.

Phase 2.5 closes all three.

---

## Entry criteria

Phase 2.5 begins when **all of Phase 2 + 2.1 + 2.2 has merged to main**:

- ✅ Wave A (Units 1, 2, 5, 6, 7a) merged
- ✅ Wave B (Unit 3) merged
- ✅ Units 7b, 7c, 7d merged
- ✅ Unit 4 merged
- ✅ Phase 2.2 schema hygiene merged
- ✅ Phase 2 close-out (state.rs deleted, rusqlite removed from src-tauri) merged
- ⏳ Phase 2.1 (CLI redesign) merged — **the blocker; in flight at draft time**

Once Phase 2.1 lands, Phase 2.5 starts.

---

## Workstream A — Build + install + integration smoke

**Goal**: prove the `.app` actually works with all of Phase 2's migrations applied.

### A.1 — Build the .app

Use the release script flow (per `scripts/release.sh`) OR a manual build:

```bash
# Build daemon binary
cargo build --release -p k2so-daemon

# Build Tauri app
bun run tauri build

# Copy daemon sidecar into the .app bundle
cp target/release/k2so-daemon \
    target/release/bundle/macos/K2SO.app/Contents/MacOS/k2so-daemon
```

For an internal-marker version (not public 0.39.0), **skip the sign + notarize steps**. Use the dev .app launched in-place via `open target/release/bundle/macos/K2SO.app`.

### A.2 — Install + launch sequence (preserves production)

Per the existing memory `feedback_subagent_no_prod_reload`: **do not** drop the dev .app into `/Applications/` because the launchd plist signature pin won't match. Instead:

```bash
# Terminal 1 — stop production daemon, run new daemon foreground
launchctl bootout gui/$(id -u)/com.k2so.daemon
"/path/to/worktree/target/release/bundle/macos/K2SO.app/Contents/MacOS/k2so-daemon"

# Terminal 2 — launch dev .app (Gatekeeper: right-click → Open first time)
open "/path/to/worktree/target/release/bundle/macos/K2SO.app"

# When done testing:
# Ctrl-C the daemon in terminal 1; then restore production:
launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.k2so.daemon.plist
```

**Back up first**: `cp ~/.k2so/settings.json ~/.k2so/settings.json.preupgrade-backup` and `cp ~/.k2so/k2so.db ~/.k2so/k2so.db.preupgrade-backup`. The 2.2 schema migrations + Phase 2.1 filesystem migration will both run on first launch; backups give recovery.

### A.3 — Smoke checklist

Each item should pass before Phase 2.5 closes. Failures = real regressions; create followup tasks.

| Migration unit | Smoke check |
|---|---|
| Unit 1 (Companion + ngrok) | Settings → Companion. Toggle auto-start. Verify ngrok tunnel URL appears (e.g. `https://k2.ngrok.app`). Rotate password. Disconnect a live session. |
| Unit 2 (LLM) | Cmd+L (or whatever the Workspace Assistant keybinding is now). Ask a simple question. Response streams back. If model not downloaded, the download progress shows. **Resilience**: `pkill -9 -f 'k2so-daemon.*--llm-worker'` mid-flight → next chat respawns worker in <5s. |
| Unit 5 (Claude Auth) | Settings → Claude Auth. Status shows valid + scheduler-installed. "Refresh Now" works. Install/uninstall scheduler works (verify plist appears/disappears at `~/Library/LaunchAgents/com.k2so.claude-auth-refresh.plist`). |
| Unit 6 (FS + Chat + Themes + ...) | File tree pane: navigate, read, write a file → changes hit disk. Chat sidebar: list IDE sessions; pin one; rename one. Settings → Themes: list loads, switch theme. |
| Unit 7a (App Settings + F3) | Change a setting; restart the dev .app (quit, re-open); setting persists. Rotate companion password via settings; verify live companion sessions invalidated immediately (F3 verification). |
| Unit 3 (Terminal PTY) | Cmd+T to open a terminal; run `sleep 60 && echo done`. **Force-quit the dev Tauri app** while sleep is running. Reopen dev .app. Terminal session is still alive; sleep finishes; output visible. **This is the architectural keystone.** |
| Unit 4 (DB writes) | Create a project; rename it; delete it. Settings change persists. Workspace layout saves. Git status / branch / commit through the renderer works. |
| Unit 7b (SKILL scaffolding) | Open a project. Verify `.k2so/agent/SKILL.md` regenerates correctly. First-boot legacy heartbeat migrations fire without errors. |
| Unit 7c (heartbeat-launchd) | Settings → Heartbeats. Install a heartbeat schedule; verify `~/Library/LaunchAgents/com.k2so.agent-heartbeat*.plist` appears. Uninstall; verify it's gone. |
| Unit 7d (residual k2so_agents) | Triage summary works. Workspace session set-surfaced works. Workspace relations CRUD works. |
| Phase 2.2 (schema hygiene) | Run daemon; check `~/.k2so/k2so.db` migrations — `_migrations` table contains 0046/0047/0048. `sqlite3 ~/.k2so/k2so.db ".schema"` shows no `terminal_panes`, `terminal_tabs`, `workspace_sessions_legacy_archive`. |
| Phase 2.1 (CLI redesign) | `k2so help` shows 14 daily verbs. `k2so glossary` lists ≥16 terms. `k2so inbox` lists items. `k2so inbox compose --title "smoke" --body "..."` creates an item. `k2so delegate` shows hard-deprecation message with harness pointer. `k2so signal scout msg "hi"` shows soft-deprecation warning + forwards. |
| Phase 2.1 filesystem migration | Verify daemon migrated `.k2so/work/{inbox,active,done}/*.md` → `.k2so/inbox/{,active,done}/`. Old `.k2so/work/` is in macOS Trash. `.k2so/.work-to-inbox-migration-v1-done` marker exists. |
| Public 0.39.0 release gate (pre-Phase-3) | None of the smoke items above should require Phase 3 features (TLS, OpenAPI, Mobile Companion contract update, K2SO Connect build). If any do, that's a sequencing bug — flag it. |

### A.4 — Smoke pass/fail discipline

- Use a fresh sandbox workspace (`mkdir /tmp/k2so-2.5-smoke && cd /tmp/k2so-2.5-smoke`) for any test that creates/destroys workspace state, so production workspaces aren't touched.
- After smoke is done, restore production launchd: `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.k2so.daemon.plist`.
- Any failure that blocks core functionality → create a task + halt Phase 2.5 → fix → re-smoke. Don't move to Phase 3 with broken Phase 2 paths.

---

## Workstream B — Test suite migration

**Goal**: every test in the repo that invokes a deprecated CLI verb runs cleanly (either via soft-deprecation alias OR migrated to the new verb).

### B.1 — Inventory test files using deprecated verbs

```bash
# Run from repo root
grep -rE "k2so (work|agents|delegate|signal|done|feed|roster|whatsnew|app-update|commit-merge|agentic|state|mode|companion) " \
    crates/ src-tauri/ src/ tests/ scripts/ .k2so/ 2>/dev/null | \
    grep -v target/ | grep -v node_modules/ | grep -v ".git/" | \
    sort -u > /tmp/k2so-deprecated-verb-callers.txt
```

Each line in `/tmp/k2so-deprecated-verb-callers.txt` is a call site. Triage per file:
- **Soft-deprecated verb** (e.g., `k2so work create`, `k2so agents list`): can stay as-is; soft-deprecation warning is expected. Optionally migrate to new verb for cleanliness.
- **Hard-deprecated verb** (e.g., `k2so signal`, `k2so delegate`, `k2so commit-merge`, `k2so app-update`): MUST migrate to the new equivalent. Use `k2so help-deprecated` as the migration map.

### B.2 — Migrate callers

Per-file edit:
- `k2so work create --title "x"` → `k2so inbox compose --title "x"`
- `k2so work send <ws> --title --body` → `k2so msg <ws> --inbox --title --body`
- `k2so done` → `k2so done` (alias preserved — no change needed)
- `k2so signal <ws> <kind> <payload>` → `k2so msg <ws> --signal <kind> [--payload <json>]`
- `k2so delegate <skill> <file>` → **MUST update** — `delegate` is hard-deprecated. Either use harness directly OR if the test was just exercising the CLI surface, replace with `k2so skills profile <skill>` + manual worktree setup.
- `k2so app-update` → `k2so update --app`
- `k2so commit-merge` → `k2so commit --merge`
- `k2so feed` → `k2so activity`
- `k2so roster` → `k2so who`
- `--agent <name>` flag → `--workspace <path>` (soft-deprecated; can leave for one release)

### B.3 — Bash test files Phase 2.1 spec'd

Phase 2.1's PRD (A8 + A22.5 + A23 + A24 + A27) specified ~20 new bash test files (`tests/cli/*.sh`). Phase 2.1's implementation may or may not have created them; verify.

If not yet created, **create them in Workstream B**:
- `tests/cli/inbox_compose_subsumes_work_create.sh`
- `tests/cli/msg_inbox_lands_in_recipient_inbox.sh`
- `tests/cli/delegate_hard_deprecated_with_harness_pointer.sh`
- `tests/cli/skills_launch_hard_deprecated.sh`
- `tests/cli/glossary_lists_all_terms.sh`
- `tests/cli/help_three_tiers.sh`
- `tests/cli/heartbeat_default_lists_schedules.sh`
- `tests/cli/inbox_delete_uses_trash.sh`
- `tests/cli/inbox_move_creates_folder.sh`
- `tests/cli/work_create_subsumed_with_warning.sh`
- ... and any other parity tests Phase 2.1's PRD listed

Each test exits 0 on parity success, non-zero on failure. Wire into the existing test runner (or create one if there isn't one for bash tests).

### B.4 — Verification

`cargo test --workspace` + `bun run typecheck` + `bash tests/cli/*.sh` all pass. Deprecation warnings appear in expected stderr captures. Phase 2.5 doesn't close until this is green.

---

## Workstream C — CI / regression detection

**Goal**: confirm no regressions slipped in across the massive Phase 2 + 2.1 + 2.2 work.

### C.1 — Full test sweep

```bash
cargo build --workspace --release
cargo test --workspace
bun run typecheck
bash tests/cli/*.sh
# If there's a separate integration suite, run it too
```

Baselines (as of pre-Phase-2.1 main):
- `cargo test -p k2so-daemon --lib` — 57 passing
- `cargo test -p k2so-core --lib` — 492 passing (post-Unit-7d + post-test backfill)
- `cargo test -p k2so --lib` — 40 passing
- `bun run typecheck` — 47 errors (pre-existing baseline)

Phase 2.5 verifies these counts stayed level or went up after Phase 2.1 landed. Any decrease = regression to investigate.

### C.2 — Specific regression checks

- **Test for `rusqlite` direct usage in `src-tauri/src/`**: `grep -rE "rusqlite|conn\.execute|conn\.prepare|conn\.query_row|db::shared" src-tauri/src/ | grep -v test` should return ZERO matches. Phase 2 close-out (commit `c16109f3`) achieved this; Phase 2.1 must not have re-introduced any.
- **Test for `AppState` survival**: `grep -rn "tauri::State<.*AppState" src-tauri/src/` should return ZERO (deleted in Phase 2 close-out).
- **Test for old verb dispatch in `cli/k2so`**: hard-deprecated verbs should be present but only as error stubs; verify they all `exit 1` with a `help-deprecated` pointer.
- **Test for `.k2so/work/` references in daemon code**: should ONLY appear in the migration helper (`migrate_work_to_inbox`); not in any read path.

### C.3 — `cargo tree` audit for src-tauri

```bash
cargo tree -p k2so --edges normal | grep -E "rusqlite|llama-cpp" | head -5
```

Should return zero direct edges for `rusqlite` and `llama-cpp-2`. Both still appear transitively (via `k2so-core` and the daemon's LLM subprocess code), but src-tauri shouldn't be a direct consumer.

---

## Workstream D — Tunnel provider spike

**Goal**: settle ngrok vs Cloudflare Tunnel vs LocalXpose vs Tailscale Funnel vs self-hosted. The choice locks Phase 3 Workstream B's TLS architecture.

### D.1 — Research subagent (in flight)

A research subagent was launched on 2026-05-23 in parallel with this PRD draft. It's evaluating providers against K2SO's specific use cases (Mobile Companion + K2SO Connect) using 10 weighted criteria:

1. End-user friction (account ceremony, signup)
2. TLS handling (terminate-at-edge vs pass-through; affects mTLS feasibility)
3. Free-tier headroom
4. Account-binding (anonymous tunnels possible? account required?)
5. Auth model (built-in basic-auth/OAuth/IP allowlist or BYO-auth)
6. CLI/SDK quality for Rust (crate available? maintained?)
7. Stable longevity (will this provider be around in 5 years?)
8. Domain branding (custom domains supported? at what tier?)
9. Latency (anecdotal from mobile/remote clients)
10. Offline-friendly observability (can daemon detect degraded tunnel?)

Output: comparison matrix + per-provider summary + top-3 ranking + implementation complexity to switch + Phase 3 TLS implications.

### D.2 — Decision criteria

The recommendation will land in Workstream E (decision doc). Spike output is INPUT, not auto-accepted; pod-leader + Rosson make the call.

Key decision pivots:
- **TLS strategy**: if we pick a pass-through provider (Cloudflare Tunnel via cloudflared with `--no-tls-verify` removed, or self-hosted), Phase 3 can do real mTLS cert pinning end-to-end. If we pick an edge-terminating provider (default ngrok), clients can only verify the provider's cert + bearer tokens.
- **Account ceremony**: K2SO targets developers but ideally onboarding is "install + run." If the chosen provider requires account creation, the K2SO setup flow grows.
- **Cost predictability**: a typical K2SO user runs 1 tunnel; if free tier handles that comfortably, no concern. If hitting paid limits is likely under normal use, that's a barrier.

### D.3 — Possible outcomes

1. **Stay with ngrok** — incumbent, works, no code change. Phase 3 Workstream B accepts ngrok's edge TLS termination and builds bearer-token auth on top.
2. **Switch to Cloudflare Tunnel** — better TLS story (pass-through possible), more robust edge, but adds Cloudflare account requirement. Daemon code change: ~200-400 LoC.
3. **Switch to Tailscale Funnel** — uses Tailscale's mesh; ties users to Tailscale account. Best fit if users are already Tailscale-native.
4. **Self-hosted** (FRP or bore) — full control, no third-party dependency, but the user has to run their own relay server. K2SO would have to ship + manage one, or be BYO-relay (heavy burden).

### D.4 — Decision deadline

Phase 2.5 doesn't close until the decision is committed. Phase 3 Workstream B is blocked on it.

---

## Workstream E — Decision document committed

**Goal**: write `.k2so/prds/tunnel-provider-decision.md` capturing the choice + rationale + Phase 3 implications. Future-you (and Mobile Companion team + K2SO Connect implementation) reads this to understand the constraints.

### E.1 — Decision doc shape

```
# Tunnel Provider Decision

**Decision date**: <ISO date>
**Decision**: <provider name>
**Decided by**: Rosson + pod-leader

## Why
[3-4 paragraphs summarizing the trade-offs that drove the choice]

## What this means for Phase 3 Workstream B (TLS + auth)
- Cert pinning strategy: [end-to-end mTLS / edge-only / both]
- Auth handoff: [provider auth + K2SO bearer / K2SO bearer only / OAuth flow]
- Mobile Companion impact: [pairing flow, what the phone sees]
- K2SO Connect impact: [address book token storage shape]

## What this means for the daemon
- Code change: [Rust crate to add/remove]
- Provider account requirement at install time: [yes/no/optional]
- Migration from ngrok: [how existing installs transition]

## Rejected alternatives
- [provider]: rejected because [reason]
- [provider]: rejected because [reason]

## Open questions deferred to Phase 3
- [...]
```

### E.2 — Tie-in to Phase 3 PRD

Add a one-line reference in `.k2so/prds/phase-3-contract-hardening.md` Workstream B: "See `.k2so/prds/tunnel-provider-decision.md` for the TLS strategy this workstream implements."

---

## Workstream order

Sequential (workstreams depend on prior ones in some cases):

1. **A — Build + smoke** (after Phase 2.1 merges; ~2 hours)
2. **B — Test migration** (can run in parallel with A; ~3-4 hours)
3. **C — CI sweep** (after A + B; ~30 minutes)
4. **D — Tunnel research** (running in parallel with PRD draft — already in flight)
5. **E — Decision doc** (after D returns; ~30 minutes of human deliberation + writing)

Total estimated effort: **1-2 days** depending on smoke surprises.

---

## Definition of done (Phase 2.5 → Phase 3 gate)

Phase 2.5 is complete when:

1. ✅ Full smoke checklist passes (or surprises documented as Phase 3 followups, not blockers)
2. ✅ `cargo test --workspace` + `bun run typecheck` + `bash tests/cli/*.sh` all green
3. ✅ Test suite migrated off hard-deprecated verbs
4. ✅ Tunnel provider decision committed at `.k2so/prds/tunnel-provider-decision.md`
5. ✅ Phase 3 PRD updated with the tunnel-provider reference in Workstream B
6. ✅ Production launchd state restored (no leftover dev daemon)
7. ✅ Production `.app` left untouched (per the hard rules — internal-marker builds don't replace the production install)

After done: Phase 3 starts. **Public 0.39.0 ships at the end of Phase 3.**

---

## Out of scope (explicit non-goals for Phase 2.5)

- **Sign + notarize the dev .app** — Phase 2.5 uses an unsigned dev build for smoke. Sign + notarize happens at the actual public 0.39.0 release after Phase 3.
- **Update Mobile Companion / K2SO Connect clients** — that's Phase 3 Workstream D (Mobile Companion contract update) + Workstream E (K2SO Connect thin-client build). Phase 2.5 only validates the daemon side + CLI surface.
- **Net-new features** — Phase 2.5 is validation, not new work. Anything that needs new functionality goes to Phase 3 or beyond.
- **Performance tuning** — same. Phase 2.5 confirms parity, not improvements.
- **Public release announcements / changelog drafting** — Phase 3 close-out.

---

## Open questions

1. **`bun tauri dev` vs `bun tauri build` for smoke?** Dev mode is faster iteration but doesn't bundle the daemon sidecar the same way; release build gives more confidence but takes longer. Recommend release build for the smoke pass; dev for any followup iteration.
2. **Should the migration smoke (`.k2so/work/` → `.k2so/inbox/`) run against the user's real workspace** or a sandbox? Real workspace tests the actual migration but risks data loss if there's a bug. Sandbox is safer but tests fewer real-world scenarios. **Recommend**: do both. Sandbox first to verify; if green, allow user to opt-in for their real workspace.
3. **Old `.k2so/work/` folder in Trash — when does it auto-empty?** macOS Trash default = 30 days. Document this in the migration completion message so users know they have a recovery window.
4. **Tunnel decision urgency**: if Phase 2.5's spike comes back ambiguous (no clear winner), do we defer to Phase 3 or pick a winner anyway? Recommend: pick a winner with a documented fallback. "We're going with X; if X turns out to be a bad fit, the fallback is Y because Z."
5. **Tauri auto-updater + the unsigned dev build**: will the auto-updater try to update the dev .app? Document expected behavior.

---

## References

- `.k2so/prds/phase-2-daemon-headless-migration.md` — Phase 2 context
- `.k2so/prds/phase-2.1-cli-redesign.md` — Phase 2.1 CLI redesign (deprecation map)
- `.k2so/prds/phase-2.1-mock-cli.md` — Phase 2.1 mock (the validated CLI surface)
- `.k2so/prds/phase-3-contract-hardening.md` — Phase 3 (Workstream B depends on Workstream D's tunnel decision)
- Memory: `feedback_subagent_no_prod_reload` — applies throughout Workstream A (smoke)
- Memory: `feedback_test_discipline` — applies to Workstream B + C (tests must fail loudly)
- Memory: `feedback_recycle_bin_tests` — relevant for verifying the Phase 2.1 filesystem migration (which uses `safe_delete::trash`)
