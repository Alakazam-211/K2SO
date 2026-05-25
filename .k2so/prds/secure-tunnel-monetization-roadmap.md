# Secure Tunnel Monetization Roadmap

**Status**: Living document — update on phase transitions.
**Last updated**: 2026-05-25
**Owner**: Rosson + pod-leader
**Strategic goal**: launch the K2SO Hosted middle-tier ($2-?/mo `<sub>.k2.dev`) — the secure tunnel monetization product.
**Intermediate milestone**: Public 0.39.0 release (sign+notarize+DMG ship) — unblocks the audience that will subscribe.
**Release strategy update (2026-05-25)**: ship **0.39.0 EARLY** — right after Phase 2.5d closes + one more smoke pass. Phase 2.6 (tunnel decision) and Phase 3 (contract hardening) become the **0.40.x cycle**, deferred to the workweek. Rationale: weekend over; Phase 2.6+ work is additive (doesn't change existing-user behavior); current users can start validating the new infrastructure during the workweek; workweek focus shifts to creature comforts users have been requesting.

**4-cycle weekend roadmap (2026-05-25)**: the post-0.39.0 path is now a 4-weekend plan from 0.40.0 → 1.0.0. See **`.k2so/prds/0.40.x-to-1.0-weekend-roadmap.md`** for the cycle-by-cycle scope. Headline: K2SO → K2 rename at 0.40.0, Brain/Documentation at 0.41.0, TTS + Kessel v2 at 0.42.0, K2 graduates at 1.0.0.

**Estimated wall time remaining**: ~1 day to 0.39.0 public release; ~4 weeks to 1.0.0.

---

## tl;dr

K2SO's monetization play is the **K2SO Hosted secure-tunnel tier**: users pay ~$2/mo for a stable `<sub>.k2.dev` subdomain that exposes their local services (Mobile Companion daemon, demo on `localhost:3000`, etc.) to the internet without DIY networking. The free tier (Tailscale Funnel) and BYO tier (ngrok / LocalXpose affiliate) serve users who don't want to subscribe.

Getting there has three engineering layers:

1. **Public 0.39.0 release** (Phase 3) — ship the rebuilt daemon-headless architecture to the existing user base. This is the audience that will eventually subscribe.
2. **Tunnel-provider decision** (Phase 2.6) — pick the backbone for the K2SO Hosted tier (CF Tunnel / Pangolin / FRP / etc.). Blocks Phase 3's TLS+auth workstream.
3. **K2SO Hosted infrastructure** (Phase 4) — build the actual control plane: per-user subdomain provisioning, Stripe billing, admin UI. Post-launch work.

Phase 2 + 2.1 (architectural cleanup) and Phase 2.5* (validation + refactor) are prerequisites. They're either done or in flight.

This document tracks remaining decisions, blocks, and sequencing. Each phase has its own detailed PRD; this is the index + status board.

---

## Where we are (visual)

```
✅ Phase 2 — daemon-headless migration (DONE — 18.8k LoC → 3.2k in src-tauri)
✅ Phase 2.1 — CLI redesign + inbox primitive + __lead__ removal (DONE — 8 commits)
✅ Phase 2.5b — skills filesystem consolidation + UI rename (DONE)
✅ Phase 2.5c — k2so-core file/module rename (DONE — 25 commits)
✅ Phase 2.5d — agents/workspace.rs + commands.rs split (DONE — 10 commits)
🔄 Phase 2.5 main — build+smoke validation (IN FLIGHT — dev launch testing user-driven)
🚀 PUBLIC 0.39.0 RELEASE — ships after Phase 2.5d + final smoke pass (EARLY RELEASE, 2026-05-25 decision)
⏳ Workweek polish — creature comforts users have been requesting (0.39.x)
⏳ Phase 2.6 — tunnel-provider decision (PRD drafted; runs in 0.40.x cycle)
⏳ Phase 3 — contract hardening + Mobile Companion + K2SO Connect (7 workstreams; 0.40.x cycle)
🚀 0.40.0 release — ships at Phase 3 close
⏳ Phase 4 — K2SO Hosted middle tier infrastructure (no PRD yet; post-0.40.0)
```

---

## Phase 2.5 (in flight, not blocking) — Build + Smoke + Test Migration

**Purpose**: validate the migrated daemon + thin client + CLI in a real `.app` build, migrate the test suite off hard-deprecated verbs, and confirm no regressions slipped in across Phases 2 + 2.1.

**Status**: In flight as task #543 (user-driven dev launch + click-through). Several findings already surfaced and resolved:

| Finding | Status |
|---|---|
| #547 — settings-load race wipes user state on upgrade (multi-store) | ✅ Fixed (`29134ebb`) — 5 sub-fixes: daemon port stability, frontend HTTP retry, reconnect listener, persist-after-baseline gate, chat command fix |
| #548 — `chat_history_list_for_project` Tauri command missing | ✅ Fixed (in #547 commit) |
| #549 — Skills section relocated above Worktrees | ✅ Fixed (`e395ee23`) |
| #550 — Duplicate React keys at boot (gemini sessions × 4) | ✅ Fixed (`783c9327`) — daemon-side dedup in `parse_gemini_sessions` |
| #551 — Audit same dedup pattern in claude/cursor/pi/codex parsers | ⏳ Open |

**Sub-phases**:
- ✅ 2.5b (skills consolidation + UI)
- ✅ 2.5c (core file rename)
- 🔄 2.5d (in flight — audit running)

**Definition of done**: every smoke checklist item passes, every finding either fixed or deferred with a task #, test suite green.

---

## Phase 2.5d — agents/workspace.rs + commands.rs split

**Status**: Audit complete (`a88b497263ee9dc64`); PRD drafted at `.k2so/prds/phase-2.5d-workspace-commands-split.md`. Implementation subagent next. **Complexity: Medium**; ~1 day of subagent work.

**Audit findings**: workspace.rs splits into 4 files (`migrations`/`skill_writer`/`harness`/`teardown` under `workspace/`); commands.rs splits across 5 destinations (4 new files + `skills/crud.rs`). Zero dead code in either. Cross-file: only 1 direct import (workspace.rs → commands::ensure_agent_wakeup). All 6 audit open-questions resolved per the audit's defaults (consistent with Phase 2.5c direction).

**Why deferred from 2.5c**:
- `agents/workspace.rs` is 143 KB — too large to move as a single `git mv` without shattering git blame
- `agents/commands.rs` is 45 KB with 25+ live public functions across 5 conceptual homes — needs design-pass to map functions to new homes

**Approach**:
1. Audit (in flight) — produces function-by-function map with proposed homes
2. PRD draft — encodes the audit's decisions as a commit plan
3. Implementation subagent — executes per the PRD
4. Cherry-pick + push + retire `agents/mod.rs` back-compat shim

**Out of 2.5d scope**: full elimination of `agents/` folder (likely outcome but not the stated goal — incidental).

**Effort estimate**: 4-8 hour subagent run depending on cluster complexity.

**Blocks**: nothing directly; can run in parallel with Phase 2.6 re-spike.

---

## Phase 2.6 — Tunnel-provider decision

**Status**: PRD drafted (`.k2so/prds/phase-2.6-tunnel-decision.md`, last touched `6f156ef2`). Awaits focused re-spike on expanded scope.

**What's locked** (no re-debate needed):
1. Three-tier monetization model:
   - **Free**: Tailscale Funnel (user installs, K2SO operates nothing)
   - **K2SO Hosted ($2-?/mo)**: `<sub>.k2.dev` backbone (K2SO operates)
   - **BYO**: ngrok / LocalXpose (with affiliate) / Cloudflare (user-managed)
2. K2SO owns `k2.dev` (purchased 2026-05-23)
3. K2SO bearer-token auth end-to-end across all providers
4. No cert pinning in Phase 3 (would break provider swaps)
5. `TunnelProvider` trait architecture in daemon (pluggable)
6. LocalXpose 40% recurring affiliate confirmed; no other meaningful BYO affiliate
7. Tailscale Funnel cannot be the Hosted backbone (free-tier port-cap + can't use custom domain)

**What's open**:
1. Backbone for K2SO Hosted middle tier — Cloudflare Tunnel vs Pangolin (self-hosted CF clone) vs FRP self-hosted vs AWS-native vs custom
2. Per-port URL UX — `<sub>-3000.k2.dev` (per-port subdomains under the hood) vs `<sub>.k2.dev:3000` (TCP tunneling)
3. Pricing strategy — $2/mo flat vs tiered ($2 Companion-only / $5-10 Multi-port / $20+ Pro)
4. AWS-native viability — initial scan suggests AWS isn't simpler/cheaper but warrants full evaluation
5. Trademark posture for future K2SO → K2 product rebrand (separate cleanup; domain ownership is independent)

**Re-spike scope**: targeted research subagent (~30-60 min) on the 5 open questions. Focus areas: Pangolin architecture + scaling, FRP multi-port-per-user pattern, CF Spectrum pricing/limits, competitive pricing analysis (ngrok Pro at $20/mo reference), AWS-native primitives.

**Blocks**: Phase 3 Workstream B (TLS + auth architecture) cannot finalize until this decision lands.

---

## Phase 3 — Contract hardening + Mobile Companion + K2SO Connect

**Status**: PRD drafted (`.k2so/prds/phase-3-contract-hardening.md`). Awaits Phase 2.5 close + Phase 2.6 decision.

**Workstreams**:

| Workstream | Description | Blocks on |
|---|---|---|
| **A** | Typed router — replace string-match dispatcher with typed routes; OpenAPI-friendly | Nothing |
| **B** | TLS + auth upgrade — cert pinning strategy, auth handoff, Companion pairing flow, Connect address book schema | **Phase 2.6 decision** |
| **C** | OpenAPI codegen — formal contract surface for clients | Workstream A |
| **D** | Mobile Companion contract update — protocol refresh (CompactLine streaming, etc.) | Workstream C |
| **E** | K2SO Connect thin-client build — desktop-to-desktop remote workspace access | Workstream B + C |
| **F** | Versioning + handshake — protocol version negotiation; SemVer client/server | Workstream A |
| **G** | Rate limiting + observability — request budgets, log structuring, health endpoints | Workstream A |

**Parallelism**: Workstreams A, C, D, F, G can begin in parallel with Phase 2.6's re-spike. Only Workstream B genuinely blocks on Phase 2.6's decision.

**Real engineering**: Multi-week. Probably 6-10 subagent passes across the workstreams.

**Ships**: **Public 0.39.0 release** at Phase 3 close.

---

## Public 0.39.0 release gate

**Ships when**:
- ✅ Phase 2 (done)
- ✅ Phase 2.1 (done)
- ✅ Phase 2.5* sub-phases (done / in flight)
- ✅ Phase 2.5 main (smoke validation closes)
- ✅ Phase 2.6 (decision committed; Workstream B unblocked)
- ✅ Phase 3 (all 7 workstreams close)

**Phase 4 is post-launch** — K2SO Hosted infrastructure builds AFTER 0.39.0 ships.

**Release ceremony**:
- Sign + notarize the `.app` (existing build-sign-notarize workflow — see memory `reference_build_sign_notarize`)
- Build DMG
- Tag git release
- Draft release notes / changelog
- Mobile Companion app update (if Phase 3 Workstream D changed the protocol)
- Update marketing / docs / website

---

## Phase 4 — K2SO Hosted middle tier (post-launch)

**Status**: No PRD yet — would be authored AFTER Phase 2.6 backbone decision locks the architecture.

**Scope** (will be fleshed out post-2.6):
- Build and operate whatever backbone Phase 2.6 chose (CF Tunnel / Pangolin / FRP)
- Control plane: per-user subdomain provisioning via API
- Stripe billing integration: subscription lifecycle webhooks
- Admin UI: per-user tunnel status, revoke, billing state
- `k2.dev` DNS zone management (likely delegated to Cloudflare regardless of backbone)
- Customer-facing onboarding: signup → subscription → pairing code → first connection
- Documentation: pairing flow, troubleshooting, billing

**Effort estimate** (from Phase 2.6 research): 2-3 engineer-weeks (1 rust-eng + 1 frontend-eng + ~1 week devops/billing wiring).

**Blocks**: monetization launch ($2/mo Hosted tier).

**Not blocking 0.39.0** — Phase 4 ships AFTER 0.39.0 is in users' hands; 0.39.0 ships with BYO tier (ngrok + LocalXpose with affiliate link) and Free tier (Tailscale Funnel documented) only.

---

## Parked / side-track items (track but not phase-shaped)

### K2SO → K2 product rebrand
- `k2.dev` domain owned by Alakazam Labs (purchased 2026-05-23)
- USPTO Class 9 + Class 42 trademark NOT yet cleared (initial scan: K2 Advisors L.L.C. holds K2 in Class 36 financial services, does NOT block software)
- Bundle ID, marketing copy, README, docs, generated SKILL.md all still say K2SO
- Could ride along with Phase 4 launch OR be its own pass
- See memory `project_k2_dev_domain` for details

### Pre-existing test flakes (need owners)
- `session_stream_setting.rs` cargo test
- `terminal-id.test.ts` vitest (`window is not defined` — needs jsdom/happy-dom)
- `no_lead_sentinel_remains.sh` shell test (may be passing now post-#550 fix; verify)
- All flagged by Phase 2.5c subagent as pre-existing, not regressions

### Deferred UI polish
- `AgentSkillsSection.tsx` "Agent Template" tab decision — likely retire since `delegate` is hard-deprecated
- `AgentContextDiagram.tsx` diagram restructuring — beyond path label updates (deferred from #549 follow-up)
- `WakeSchedulerSection.tsx` per-row primary-agent name resolution — Phase 3 follow-up flagged by frontend `__lead__` cleanup subagent

### Phase 2.5d follow-on
- Back-compat aliases in `agents/mod.rs` (16 `pub use` exports) can be retired once Phase 2.5d lands cleanly
- `parse_*_sessions` audit for the dedup bug pattern (#551 — claude / cursor / pi / codex parsers)
- Full `agents/` folder removal if 2.5d makes it empty (incidental, not the goal)

### Phase 3 deferred items
- Frontend `__lead__` cleanup surfaced 3 items the audit caught but the implementation subagent didn't fix:
  1. `WakeSchedulerSection.tsx` slug-based agent naming
  2. Pre-existing `TerminalPane.tsx` typecheck errors
  3. Pre-existing `tabs.test.ts` vitest failure
- Worktree-bound task data design conversation (#537 noted the WorktreeDetailPane Task tab now reads worktree CLAUDE.md; if a more sophisticated worktree task surface is desired, Phase 3 work)

---

## Suggested sequencing

**Sequential (each waits on prior)**:
```
Phase 2.5d (in flight)
    ↓
Phase 2.5 main close (smoke + test migration + CI sweep finalize)
    ↓
Phase 2.6 re-spike + decision doc commit
    ↓
Phase 3 Workstream B unblocked (TLS + auth)
```

**Parallel possible** (once Phase 2.5 closes):
```
Phase 3 Workstream A (typed router)     ←  can start as soon as 2.5 closes
Phase 3 Workstream C (OpenAPI)          ←  needs A in flight
Phase 3 Workstream D (Mobile contract)  ←  needs C
Phase 3 Workstream E (K2SO Connect)     ←  needs B + C
Phase 3 Workstream F (versioning)       ←  needs A
Phase 3 Workstream G (rate limit + obs) ←  needs A
```

**Phase 4** runs after public 0.39.0 ships; not on the critical path to release.

---

## Active background work (as of 2026-05-25)

| Task ID | What | Status |
|---|---|---|
| #543 | Phase 2.5 Workstream A (dev mode smoke testing) | In progress (user-driven) |
| #546 | Phase 2.5d split (workspace.rs + commands.rs) | In progress (audit subagent running) |
| #551 | Audit dedup pattern in parse_claude/cursor/pi/codex_sessions | Pending |

---

## References

| PRD | Status |
|---|---|
| `phase-2-daemon-headless-migration.md` | Done |
| `phase-2.1-cli-redesign.md` | Done |
| `phase-2.5-validation-and-tunnel-decision.md` | In flight (smoke) |
| `phase-2.5b-skills-consolidation.md` | Done |
| `phase-2.5c-core-rename.md` | Done |
| `phase-2.5d-` (to be drafted) | In flight (audit) |
| `phase-2.6-tunnel-decision.md` | Drafted; awaits re-spike |
| `phase-3-contract-hardening.md` | Drafted; blocks on 2.6 (Workstream B only) |
| `phase-4-` (to be drafted) | Post-launch |

---

## How to read this document

- **When a phase changes status**, edit this file to reflect new state (move emoji between ✅ / 🔄 / ⏳)
- **When a new finding lands**, add to the relevant phase's status table
- **When a decision is made**, move it from "What's open" to "What's locked"
- **When a parked item gets picked up**, promote it to its own phase if scope justifies

Future-you (or the next session) can read this top-down and immediately know: where are we, what's blocked on what, what comes next.
