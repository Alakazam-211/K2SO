# Audit: client-baked logic that should be daemon-canonical

**Date:** 2026-06-07 · **Trigger:** GH#22 (Active/reaper were renderer-owned). · **Status:** backlog seed.
**Rule:** see `feedback_daemon_first` memory + `.k2so/PROJECT.md` Conventions. Two smells:
**(A) headless-breaking** — loop/side-effect only runs while the webview is open (dead on the headless Linux daemon);
**(B) multi-client divergence** — renderer-derived/owned state that should be one canonical truth shared across all clients/users.

> Findings are point-in-time (file:line may drift) — verify against current code before acting.

## P0 — Headless-breaking (background loops with daemon-relevant effects)
| # | What | File:line | Why mis-placed | Effort |
|---|------|-----------|----------------|--------|
| 1 | **K2 Connect tunnel lease heartbeat** — `setInterval(claimSubdomain, 60s)` keeps the tunnel alive, but only while the **Settings → K2 Connect panel is mounted**. Close Settings → lease expires (server TTL ~3min) → remote access drops. Headless: never renews. | `components/Settings/sections/K2ConnectSection.tsx:413,420` | (A) Tunnel lifecycle is daemon-owned but its keep-alive is gated on a UI component. **Likely the most serious finding.** | L |
| 2 | **Assistant LLM model-status polling** — `setTimeout(pollModelStatus, 2s)` loop. | `stores/assistant.ts:139-159` | (A)+(B) Headless can't learn when model is ready; clients poll independently. | M |
| 3 | **Review queue polling** — `setInterval(fetchAll, 30s)`. | `stores/review-queue.ts:122-124` | (A) No review awareness headless/when closed. | M |
| 4 | **Active-agents polling** — `setInterval(pollOnce, ~2.5s)` drives "agent working" status/spinners. | `stores/active-agents.ts:637-639` | (A)+(B) Daemon can't emit agent status headless; clients diverge. (Partial hook infra exists.) | M |
| 5 | **Review panel polling** — `setInterval({fetchReviews;fetchChats}, 15s)`. | `components/ReviewPanel/ReviewPanel.tsx:201` | (A) Scoped to panel mount. | M |
| 6 | **Companion tunnel-status polling** — `setInterval(refresh, 5s)`. | `components/Settings/sections/CompanionSection.tsx:52` | (A) Status is daemon-canonical already; renderer poll is redundant + client-only. | S |
| — | **Age-out sweep/reaper** — already being fixed by the canonical-Active work (#672). | `components/Sidebar/ActiveBar.tsx:163,206-232` | (A)+(B) | (in progress) |

## P1 — Multi-client divergence (renderer-owned/derived shared state)
| # | What | File:line | Why mis-placed | Effort |
|---|------|-----------|----------------|--------|
| 7 | **Tab titles (manual rename, + future auto-naming #653)** — `setTabTitle` persists to per-renderer `workspaceLayouts`, NOT the daemon. Two clients on one workspace see different tab names. | `stores/tabs.ts:~2416-2441` | (B) Tab metadata should be daemon-canonical + broadcast `tab:renamed`. Ties to #653. | M |
| 8 | **Active-bar dismiss / 24h memory** — `_dismissedProjects` + `_activeBarMemory` module-level maps; lost on restart, invisible to other clients. | `components/Sidebar/ActiveBar.tsx:79,111,139-141` | (B) Dismiss intent should be daemon state. **Likely absorbed by #672** (dismiss route). | M |
| 9 | **Heartbeat "live" state** — each renderer derives live-ness vs `terminal/list-running` on its own refresh. | `stores/heartbeat-sessions.ts:87-112` | (B) Should be a daemon `heartbeat:state-changed` broadcast. | M |
| 10 | **Review checklist cache** — local component cache not invalidated on remote edits. | `components/ReviewPanel/ReviewPanel.tsx:232-242` | (B) Daemon should publish checklist updates. | M |
| 11 | **Tab order conflict** — local mutate + debounced persist; concurrent clients last-write-wins silently. | `stores/tabs.ts:~1045-1047` | (B) Needs daemon revision/timestamp conflict resolution. | M |

## Correctly daemon-owned already (verified — no action)
Session lifecycle/PTY reap (v2_session_map + session_events WS), agent scheduling, project metadata, git state, auth tokens, chat-session persistence, fs ops, terminal I/O, settings persistence, workspace-layout persistence, heartbeat schedules/cron, K2 Connect tunnel *lifecycle* (vs the lease-renewal in P0#1), the awareness/multi-user bus.

## Correctly client-owned (no action)
Terminal refocus interval (`App.tsx:421-442`), toast auto-dismiss (`stores/toast.ts:34-36`), panel-resize debounce (`stores/panels.ts:257`), Active-bar/IconRail 60s re-render ticks (cosmetic once the daemon owns age-out).

## Suggested sequencing
1. **P0#1 tunnel lease heartbeat → daemon** (real remote-access risk). 
2. Replace renderer polling with daemon broadcasts: model status (P0#2), agent status (P0#4 — extends existing hook infra), review queue/panel (P0#3/#5), companion (P0#6, trivial).
3. P1 multi-client: tab titles (#7, bundle with #653), dismiss state (#8, fold into #672), heartbeat live-state (#9), review checklist (#10), tab-order conflict (#11).
