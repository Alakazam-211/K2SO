pub mod projects;
pub mod workspaces;
pub mod focus_groups;
pub mod workspace_sections;
pub mod agents;
pub mod terminal;
pub mod git;
pub mod settings;
pub mod workspace_ops;
// Phase 2 Unit 2 — `assistant` command shims deleted. The renderer
// hits `/cli/llm/*` on k2so-daemon directly; the daemon owns the
// LLM worker subprocess + supervisor (timeout/RSS/queue/crash
// isolation). No Tauri-side LLM surface remains.
//
// Phase 2 Unit 6 — `chat_history` command shims deleted alongside
// the rest of Unit 6's batch. The renderer hits `/cli/chat/*` on
// k2so-daemon directly.
pub mod timer;
pub mod updater;
pub mod workspace_layouts;
// Phase 2 Unit 5 — `claude_auth` command shims deleted. The renderer
// hits `/cli/claude-auth/*` on k2so-daemon directly; no Tauri-side
// command surface for it remains. Scheduler ownership (the launchd
// plist `com.k2so.claude-auth-refresh.plist`) now lives in the
// daemon's `claude_auth_host` module so refresh keeps working when
// the Tauri app is closed.
pub mod k2so_agents;
pub mod format;
pub mod states;
// Phase 2 Unit 1 — `companion` command shims deleted. The renderer
// hits `/cli/companion/*` on k2so-daemon directly; no Tauri-side
// command surface for it remains.
//
// Phase 2 Unit 6 — `filesystem`, `chat_history`, `themes`,
// `skill_layers`, `review_checklist`, `project_config`, `whats_new`
// command shims deleted. The renderer hits `/cli/fs/*`, `/cli/chat/*`,
// `/cli/themes/*`, `/cli/skill-layers/*`, `/cli/review-checklist/*`,
// `/cli/project-config/*`, and `/cli/whats_new` on k2so-daemon
// directly.
pub mod daemon;
pub mod permissions;
pub mod memory_watcher;
