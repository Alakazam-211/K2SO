---
title: Implement open-core split: Community IDE vs Proprietary Agent Orchestration
priority: normal
assigned_by: external
created: 2026-04-01
type: task
source: manual
---

## Priority: HIGH
## Date Initiated: April 1, 2026

This work item marks the formal beginning of splitting K2SO into an open-core model with a clear boundary between the MIT-licensed community IDE and the proprietary Alakazam Engine orchestration layer.

---

## The Line

**Community (MIT — public GitHub):** Manual agent usage.
- Terminal multiplexer with GPU-accelerated rendering
- Workspace/project management, focus groups, layout persistence
- Document viewers (Markdown, PDF, DOCX)
- Git worktree management (basic create/remove/switch)
- Agent presets (launch Claude, Codex, Gemini, Copilot, Aider, etc.)
- Agent terminal panes (display, input/output, basic lifecycle tracking)
- Code editor integration, themes, settings
- Chat history viewer
- Timer

**Proprietary (All Rights Reserved — internal only):** Autonomous agent coordination.
- **K2SO Agent** — top-level autonomous planner/orchestrator
- **Pod system** — Pod Leaders (per-workspace orchestrators), Pod Members (specialized agents in isolated worktrees), agent hierarchy and coordination
- **Heartbeat system** — adaptive timing, auto-backoff, local LLM triage, phase management, cost budgeting
- **Work item pipeline** — inbox → active → done queue, work creation/movement, workspace-level inbox, cross-workspace work sending
- **Delegation** — atomic worktree + agent launch, CLAUDE.md generation, task context assembly
- **Review workflow** — approve/reject/feedback gates, diff summaries, branch merge on approval
- **MCP channel integration** — event-driven agent-to-K2SO communication bridge
- **Local LLM inference** — llama-cpp-2 with Metal GPU acceleration for on-device triage decisions
- **Workspace states** — Build/Managed Service/Maintenance/Locked capability profiles
- **Agentic systems toggle** — global on/off for autonomous operations
- **AI-assisted commits** — `k2so commit` / `k2so commit-merge`

---

## Implementation Plan

### Phase 1: Cargo Feature Flag

Add feature flag to `src-tauri/Cargo.toml`:

```toml
[features]
default = []
agent-orchestration = ["llama-cpp-2"]
```

Gate the following behind `#[cfg(feature = "agent-orchestration")]`:
- `src-tauri/src/commands/k2so_agents.rs` (entire module)
- `src-tauri/src/llm/` (entire module)
- Agent-related Tauri commands in `lib.rs` invoke_handler registration
- Agent lifecycle event handling in `agent_hooks.rs` (the `/cli/agents/*`, `/cli/work/*`, `/cli/review/*`, `/cli/heartbeat/*`, `/cli/delegate/*`, `/cli/agentic/*`, `/cli/state/*` endpoints)
- `review_checklist.rs` commands

Keep UNGATED (community features):
- `/cli/terminal/spawn` endpoint (agents need to launch in terminals)
- Basic agent preset CRUD (presets are just terminal commands)
- Agent pane display components (showing a terminal running Claude is free)

### Phase 2: CLI Gating

Update `cli/k2so` script to check for orchestration availability:
- Commands like `agents`, `work`, `delegate`, `reviews`, `heartbeat`, `agentic`, `state` should return a clear message: "This feature requires K2SO Pro. Visit alakazamlabs.com for details." when orchestration is not available.
- Commands like `terminal spawn`, `commit` (basic git commit, not AI-assisted) remain available.

### Phase 3: Frontend Gating

Add a build-time or runtime constant (e.g., `NEXT_PUBLIC_K2SO_PRO=true`):
- Gate `AgentPane/` kanban work item view (community shows terminal only, not work queue)
- Gate `AgentsPanel/` full agent management UI
- Gate `ReviewPanel/` and `ReviewQueueModal/`
- Gate workspace state selector in Settings
- Gate "Agentic" toggle
- Keep: agent preset bar, agent terminal panes, basic agent lifecycle indicators (working/idle)

### Phase 4: License Update

Update LICENSE file in public repo:
```
MIT License — K2SO Community Edition

Copyright (c) 2025 Alakazam Labs, LLC

[standard MIT text]

Note: This license applies to the K2SO Community Edition (IDE).
The agent orchestration capabilities available in K2SO Pro,
including but not limited to autonomous agent coordination,
pod management, heartbeat systems, work item pipelines, and
review workflows, are proprietary to Alakazam Labs, LLC and
are not covered by this license.
```

Update README.md to reflect the open-core model:
- "K2SO Community: The AI workspace IDE. Free and open source."
- "K2SO Pro: Autonomous agent orchestration. Contact Alakazam Labs."

### Phase 5: Repository Hygiene

Ensure NO proprietary code is in the public repo:
- Verify that agent orchestration source files are excluded from public builds
- Confirm `.k2so/agents/` directories are in `.gitignore`
- Confirm heartbeat configs, work items, and agent profiles are never committed
- Audit git history for any proprietary logic that was previously committed publicly

---

## Why This Matters

This split establishes K2SO's open-core model as the foundation of the Alakazam Engine — our proprietary AI development platform. The community IDE provides marketing reach and community goodwill. The proprietary orchestration layer is the licensable IP that powers client engagements and is referenced in service contracts (including the active NSI/Cherokee Federal negotiation).

The boundary is: **using agents is free. Agents orchestrating themselves is the Engine.**

This work was initiated on April 1, 2026 to establish the date that proprietary development of the orchestration layer formally began as a separate, non-MIT-licensed component.
