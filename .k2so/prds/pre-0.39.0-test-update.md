# Pre-0.39.0 Test Update PRD

**Status**: Drafted 2026-05-25 from combined findings of audit #554 (integration/bash/vitest) + audit #555 (inline tests + doctests).
**Owner**: Rosson + pod-leader
**Goal**: ensure the test suite is fully aligned with the post-Phase-2.* refactor before 0.39.0 public release.
**Realistic complexity**: **LOW for blocking work, MEDIUM for nice-to-have coverage**. Combined audits found surprisingly few RED items.

---

## tl;dr

K2SO's test suite went through Phase 2.0 / 2.1 / 2.5b / 2.5c / 2.5d (and 2.5e is in flight). Two audits enumerated the full test surface:

| Layer | Files | Tests | Audit | Status |
|---|---|---|---|---|
| Daemon integration (`crates/k2so-daemon/tests/`) | 22 | ~50-80 | #554 | 13 stable / 5 path-update / 4 misc |
| Session stream (`crates/k2so-core/tests/session_stream_*.rs`) | 13 | 163 | #554 | All stable |
| Bash CLI (`tests/cli/*.sh`) | 21 | ~21 | #554 | 14 stable / 5 deprecation tests verified / 1 path-stale (`no_lead_sentinel_remains.sh`) |
| Vitest (React) (`src/renderer/**/*.test.ts`) | 3 | ~10 | #554 | 2 stable / 1 jsdom-env flake (`terminal-id.test.ts`) |
| Inline unit tests (k2so-core src/) | 56 | ~529 | #555 | All GREEN/YELLOW (no RED) |
| Inline unit tests (k2so-daemon src/) | 12 | ~55 | #555 | All GREEN |
| Inline unit tests (src-tauri src/) | 6 | ~49 | #555 | All GREEN |
| Doctests (`///` blocks) | 16 | 16 | #555 | All stable; no deprecation refs |

**Total**: ~860 tests across ~150 files. Both audits returned LOW/MEDIUM complexity. **No RED findings** in inline tests. The 5 RED items in audit #554 are mechanical path/import refreshes.

**Net**: the test suite is in genuinely good shape. The pre-release work is small + targeted.

---

## Tier 1 — Must-do before 0.39.0 ship (~2-3 hours)

### 1.1 Update stale path in `tests/cli/no_lead_sentinel_remains.sh:54`

**Issue**: Test excludes `crates/k2so-core/src/agents/unification.rs` from grep, but Phase 2.5c moved that file to `crates/k2so-core/src/migrations/unification_0_37_0.rs`.

**Action**: 1-line edit. Change exclusion path.

**Effort**: 1 minute.

---

### 1.2 Add jsdom env to `src/renderer/lib/terminal-id.test.ts`

**Issue**: Pre-existing `window is not defined` flake; vitest default Node env doesn't provide `window`.

**Action**: Add `// @vitest-environment jsdom` at top of file (or update `vitest.config.ts` globally).

**Effort**: 5 minutes.

---

### 1.3 Verify hard-deprecation test assertions match final CLI

**Issue**: Audit #554 flagged 5 bash tests that assert error messages from hard-deprecated verbs. The assertions need to match the FINAL Phase 2.1 implementation's error text.

**Files to spot-check**:
- `tests/cli/agents_create_hard_deprecated.sh`
- `tests/cli/agents_delete_hard_deprecated.sh`
- `tests/cli/agents_list_hard_deprecated.sh`
- `tests/cli/work_create_hard_deprecated.sh`
- `tests/cli/work_send_hard_deprecated.sh`

**Action**: Run each test against current CLI; verify grep assertions match actual error output. Update strings if mismatched.

**Effort**: 15 minutes.

---

### 1.4 Update module imports in `crates/k2so-daemon/tests/agents_routes_integration.rs`

**Issue**: After Phase 2.5d removed the `agents/mod.rs` back-compat shim AND Phase 2.5e relocates the residual `agents/*` files, any test importing via the shim will need updating. Compiler will catch this when 2.5e lands.

**Action**: Wait for Phase 2.5e cherry-pick; run `cargo check --workspace`; update any failing imports to canonical post-2.5e paths.

**Effort**: 20-30 minutes (compiler-guided).

---

### 1.5 Add Cursor/Pi/Codex dedup tests (closes #551 + audit #555 gap)

**Issue**: Phase 2.5 fix #550 added a dedup pass to `parse_gemini_sessions` in `chat_history.rs`. The analogous functions for Cursor/Pi/Codex (`detect_cursor_session`, `detect_pi_session`, `detect_codex_session` + their `parse_*_sessions` variants) lack inline tests confirming they're dedup-safe.

**Action**: Add 3-6 unit tests mirroring the gemini dedup test pattern. Verify each parser dedupes correctly when a session appears in multiple history files.

**File**: `crates/k2so-core/src/chat_history.rs`

**Effort**: 30-60 minutes.

**Reference test** (gemini, already in place): look at how `detect_claude_session_finds_latest_in_window` test is structured; replicate.

---

### 1.6 Add `heartbeats/control.rs` unit tests (audit #555 gap)

**Issue**: Phase 2.5d extracted heartbeats control logic; module has NO inline tests.

**Action**: Add 2-3 priority tests:
- `set_heartbeat_clamps_interval` — interval < min_interval_seconds should clamp up
- `force_wake_updates_next_wake_to_now` — force_wake=true should trigger immediately
- (Optional) `get_heartbeat_returns_defaults_on_missing_config`

**File**: `crates/k2so-core/src/heartbeats/control.rs`

**Effort**: 30-45 minutes.

---

### 1.7 Full test suite verification

**Action**: After Tier 1 changes land, run:
- `cargo test --release --workspace` — verify all 907+ pass
- `cargo test --doc -p k2so-core` + `cargo test --doc -p k2so-daemon` — verify doctests clean
- `bun run typecheck` — verify baseline 47 errors (no growth)
- `bash tests/cli/*.sh` — verify all bash CLI tests pass

**Effort**: 10-15 minutes (run + observe).

---

## Tier 2 — Defer to 0.39.x or 0.40.x (~10-12 hours when scheduled)

These are coverage gaps for Phase 2.5d-extracted modules. NOT blocking 0.39.0 release.

### 2.1 Coverage gaps for Phase 2.5d extractions (6 modules)

The following modules were extracted from `agents/workspace.rs` or `agents/commands.rs` but have NO inline tests:

| Module | What it does | Suggested tests |
|---|---|---|
| `workspace/relations.rs` | Parent workspace + relations resolution | symlink + non-symlink path resolution; circular-ref detection |
| `workspace/agent_launch.rs` | Launch orchestration (cold → alive transitions) | state transition tests; missing-agent graceful handling |
| `workspace/agent_editor.rs` | AIFileEditor surface (AGENT.md editing) | save flow; backup creation |
| `workspace/harness.rs` | Harness file discovery (CLAUDE.md, AGENTS.md, etc.) | discovery in clean workspace; ingest preview shape |
| `workspace/skill_writer.rs` | SKILL.md serialization + harness fanout | write produces expected content; symlink updates |
| `workspace/settings.rs` | Workspace settings reader | round-trip serialize/deserialize |

**Effort**: ~6-8 hours total (1-1.5 hr each).

**Priority**: MEDIUM. Coverage rises ~10% relative; no current regressions.

---

### 2.2 `skills/content.rs` generator snapshot tests

**Issue**: 4 generators (`generate_manager_skill_content`, `generate_custom_agent_skill_content`, `generate_k2so_agent_skill_content`, `generate_template_skill_content`) have NO tests. With template content rewrite (#552) landing, having snapshot tests would catch future content drift.

**Action**: Add snapshot-style tests:
- `generate_custom_agent_skill_content_includes_heartbeat_docs`
- `compose_agent_wake_context_writes_skill_md`
- Per-generator: verify output is non-empty + contains expected section headers

**File**: `crates/k2so-core/src/skills/content.rs`

**Effort**: ~3-4 hours.

**Priority**: MEDIUM-LOW. Catches regressions but not blocking; templates were just rewritten so content is fresh.

---

### 2.3 `hasLoadedFromDaemon` gate test (audit #554's coverage gap #C)

**Issue**: Phase 2.5 fix #547 added a `hasLoadedFromDaemon` gate to prevent settings overwrite on slow daemon boot. Behavior is verified end-to-end via dev mode smoke but lacks a unit test.

**Action**: Add vitest test (or React Testing Library) asserting that:
- `useSettingsStore` initialState has `hasLoadedFromDaemon: false`
- Store does NOT render default values until `loadFromDaemon()` resolves
- After resolve, `hasLoadedFromDaemon: true`

**File**: `src/renderer/stores/settings.test.ts` (new)

**Effort**: ~1-2 hours.

**Priority**: MEDIUM. Real bug fix that should have test coverage.

---

### 2.4 `inbox_heartbeat_interaction.sh` integration test (audit #554's coverage gap #A)

**Issue**: Phase 2.1a inbox primitive + heartbeats coexist but interaction not tested.

**Action**: Add bash test: seed inbox with N items → trigger heartbeat → verify items drain into agent session → inbox empty.

**File**: `tests/cli/inbox_heartbeat_interaction.sh` (new)

**Effort**: ~1-2 hours.

**Priority**: MEDIUM-HIGH. Inbox is a new primitive; cross-system test would catch real bugs.

---

## Tier 3 — Defer indefinitely (low priority)

### 3.1 `session_stream_setting.rs` pre-existing flake

**Status**: Failed before refactor; root cause is DB initialization race in `init_for_tests()`. Not related to Phase 2 refactor.

**Action**: Defer to 0.40.x or 0.41.x cleanup pass.

---

### 3.2 Untested daemon modules (auth.rs, routes.rs)

**Status**: Heavily integration-tested in `tests/`; inline test gaps don't affect confidence.

**Action**: Skip unless coverage metric matters.

---

### 3.3 CI doctest check

**Action**: Add a CI step that runs `cargo test --doc -p k2so-core -p k2so-daemon` on every PR to catch doctest drift early.

**Priority**: Useful but not urgent. Defer until 0.40.x infrastructure work.

---

## Implementation plan

### Option A — Single coordinated subagent (recommended)

Brief a subagent with this PRD as the spec. Subagent executes Tier 1 in order:
1. Path fix to `no_lead_sentinel_remains.sh`
2. jsdom env to `terminal-id.test.ts`
3. Verify hard-deprecation test assertions (read each .sh + compare to current CLI output)
4. Wait for Phase 2.5e cherry-pick; run `cargo check --workspace`; update any failing imports
5. Add Cursor/Pi/Codex dedup tests to `chat_history.rs`
6. Add `heartbeats/control.rs` unit tests
7. Run full test suite verification

**Single commit OR per-item commits** at subagent's discretion.

**Effort**: ~2-3 hours subagent run.

### Option B — Inline execution by pod-leader

Pod-leader runs items 1-3 + 7 inline (~30 min), then briefs a subagent for items 4-6 (cargo work + new tests). Splits effort.

**Tradeoff**: Inline saves wall time but burns context; subagent isolates.

---

## Hard rules (CRITICAL)

1. **Build only from worktree's `target/`** — no production touches.
2. **`git mv` if any files move** — preserves blame.
3. **No `git commit --no-verify`, no `--amend`, no `Co-Authored-By` lines.** Per memory `feedback_commit_attribution`.
4. **Do NOT touch version strings.** Per memory `feedback_no_version_bump`.
5. **Tests must fail loudly.** Per memory `feedback_test_discipline`. New tests should assert specific outcomes, not "didn't crash."
6. **`safe_delete::trash` if any test cleanup deletes folders.** Per memory `feedback_recycle_bin_tests`. (Probably N/A for this PRD's scope.)

---

## Definition of done

### For 0.39.0 release gate

1. ✅ Tier 1.1 — `no_lead_sentinel_remains.sh` path updated
2. ✅ Tier 1.2 — `terminal-id.test.ts` jsdom env added
3. ✅ Tier 1.3 — Hard-deprecation tests verified against current CLI
4. ✅ Tier 1.4 — `agents_routes_integration.rs` imports updated post-Phase-2.5e
5. ✅ Tier 1.5 — Cursor/Pi/Codex dedup tests added (closes #551)
6. ✅ Tier 1.6 — `heartbeats/control.rs` tests added
7. ✅ Tier 1.7 — Full test suite green: `cargo test --workspace` 907+ passing, doctest clean, typecheck baseline 47, bash CLI green

### For 0.39.x / 0.40.x cycles

Tier 2 items get their own micro-PRs as time permits. Track in roadmap.

---

## What's NOT in scope

- **Phase 2.5e source code work** — handled by subagent `a08e047aec624175b` (separate PRD). This PRD just covers TEST updates.
- **Template content rewrite** — handled by subagent `acd7b45bf18beeee8` (task #552). This PRD just adds tests, not template content.
- **Release ceremony** — version bump, sign, notarize, DMG, release notes. Separate workflow.
- **Phase 2.5 dev mode smoke testing** — user-driven manual click-through (task #543). Complements but doesn't replace the test suite.

---

## Risks + open questions

### A. Phase 2.5e cherry-pick timing

The Phase 2.5e subagent is still running. Tier 1.4 (update `agents_routes_integration.rs` imports) waits on its completion. If 2.5e introduces NEW path changes beyond what audit #554 anticipated, this PRD may need a small update.

**Mitigation**: Tier 1.4 is the only item gated on 2.5e. Other Tier 1 items can proceed immediately.

### B. Template content rewrite affecting `skills/content.rs` tests

Subagent `acd7b45bf18beeee8` is rewriting baseline content in `skills/content.rs`. If they ALSO add tests as part of the rewrite, Tier 2.2 effort might be partially absorbed.

**Mitigation**: After their work lands, re-check `skills/content.rs` test coverage. Adjust Tier 2.2 scope.

### C. Hard-deprecation test assertion drift

The exact error text emitted by hard-deprecated verbs may have shifted across Phase 2.1's many commits. If a test grep'd for "Use `k2so workspace launch` instead" but the actual error says "Use `k2so workspace launch [--workspace <path>]` instead", the test fails.

**Mitigation**: Tier 1.3 explicitly spot-checks each test against current CLI output. Update grep strings if mismatched.

---

## References

- Audit #554 output: `/private/tmp/claude-501/.../tasks/a60f7edc6499521f0.output` — first-pass audit (integration/bash/vitest)
- Audit #555 output: `/private/tmp/claude-501/.../tasks/af7ee20653279bae7.output` — follow-up audit (inline + doctests)
- `.k2so/prds/phase-2.1-cli-redesign.md` — Phase 2.1 PRD, especially A25 final verb taxonomy
- `.k2so/prds/phase-2.5d-workspace-commands-split.md` — Phase 2.5d which created the new extracted modules
- `.k2so/prds/secure-tunnel-monetization-roadmap.md` — master roadmap
- Memory `feedback_test_discipline` — tests must fail loudly
- Memory `feedback_no_version_bump` — version strings handled by release script
- Memory `feedback_recycle_bin_tests` — Touch ID issue with trash tests on macOS
