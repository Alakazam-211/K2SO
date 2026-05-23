pub mod projects;
pub mod workspaces;
pub mod focus_groups;
pub mod workspace_sections;
pub mod agents;
pub mod terminal;
pub mod git;
pub mod filesystem;
pub mod settings;
pub mod project_config;
pub mod workspace_ops;
// Phase 2 Unit 2 — `assistant` command shims deleted. The renderer
// hits `/cli/llm/*` on k2so-daemon directly; the daemon owns the
// LLM worker subprocess + supervisor (timeout/RSS/queue/crash
// isolation). No Tauri-side LLM surface remains.
pub mod chat_history;
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
pub mod review_checklist;
pub mod format;
pub mod themes;
pub mod states;
// Phase 2 Unit 1 — `companion` command shims deleted. The renderer
// hits `/cli/companion/*` on k2so-daemon directly; no Tauri-side
// command surface for it remains.
pub mod daemon;
pub mod skill_layers;
pub mod permissions;
pub mod whats_new;
pub mod memory_watcher;
