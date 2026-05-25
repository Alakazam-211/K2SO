//! Agent CRUD + work queue commands.
//!
//! This is the business logic behind the bulk of the `k2so agents *`
//! and `k2so work *` CLI surface. Pre-0.33.0 these were Tauri-only
//! (`#[tauri::command]` functions in `src-tauri/src/commands/
//! k2so_agents.rs`); in 0.33.0 they live here so the k2so-daemon can
//! serve the same routes headlessly when the Tauri app is quit.
//!
//! Covers:
//!
//! - **Agent CRUD**: [`list`], [`create`], [`delete`] (+ forced
//!   `delete_inner`), [`get_profile`], [`update_profile`],
//!   [`update_field`] (+ its pure helper [`update_agent_md_field`]).
//! - **Wakeup + backup helpers** used by the commands above:
//!   [`ensure_agent_wakeup`], [`cleanup_agent_backups`].
//!
//! Phase 2.1c Item 2 — removed the per-agent work-queue functions
//! (`work_list`, `work_create`, `work_move`) and the legacy workspace
//! `workspace_inbox_list`. They had no remaining callers after the
//! React frontend migrated to the workspace inbox primitive
//! (`k2so_core::inbox::*`).
//!
//! Phase 2.1 wrap-up (0.39.0f) — removed `workspace_inbox_create`
//! (the legacy `.k2so/work/inbox/` writer). Its sole caller, the
//! daemon's `workspace_msg::deliver_to_inbox`, was retired in the
//! same commit. New inbox-delivery callers should compose against
//! `k2so_core::inbox::compose` directly so they land in the
//! canonical `.k2so/inbox/` primitive the renderer reads.
//!
//! Every function is host-agnostic — uses `db::shared()` +
//! `fs_atomic::*` + core agent-system primitives, no AppHandle, no
//! Tauri command macros.







// Phase 2.5d: back-compat re-exports. The agent CRUD cluster moved to
// `crate::workspace::agent`; per-agent heartbeat control moved to
// `crate::heartbeats::control`. Existing call sites in src-tauri and
// the daemon still spell these paths through `crate::agents::commands`
// — the aliases keep them working through Tier B. Retire together with
// `agents/` in Tier C.
pub use crate::workspace::agent::{
    cleanup_agent_backups, create, delete, delete_inner, get_profile, list, log_agent_warning,
    update_agent_md_field, update_field, update_profile, K2soAgentInfo,
};
pub use crate::heartbeats::control::{
    ensure_agent_wakeup, get_heartbeat, heartbeat_action, heartbeat_noop, set_heartbeat,
};
pub use crate::workspace::agent_editor::{
    k2so_agents_get_editor_context, k2so_agents_preview_agent_context,
    k2so_agents_regenerate_agent_context, k2so_agents_save_agent_md,
};
pub use crate::workspace::relations::{
    workspace_relations_create, workspace_relations_delete, workspace_relations_list,
    workspace_relations_list_incoming, workspace_session_get,
};


// Phase 2.1 wrap-up (0.39.0f) — the `work_item_slug` + `body_preview`
// helpers (last used by `workspace_inbox_create`) were removed with
// the function itself. The post-Phase-2.1 inbox primitive has its own
// slug + preview helpers in `k2so_core::inbox`.

// Phase 2.1c Item 2 — `work_list`, `work_create`, `work_move`, and
// `workspace_inbox_list` removed (zero remaining callers; the
// renderer migrated to `k2so_core::inbox::*` via the new
// `commands::inbox::k2so_inbox_*` Tauri shims). The daemon CLI
// surface for these had already been hard-deprecated in Phase 2.1b.
//
// Phase 2.1 wrap-up (0.39.0f) — `workspace_inbox_create` removed.
// Its sole caller (the daemon's `workspace_msg::deliver_to_inbox`)
// was retired in the same commit. The function wrote to the legacy
// `.k2so/work/inbox/` layout, which the post-Phase-2.1 migration
// hook (in `inbox::migrate_work_to_inbox`) now sends to the macOS
// Recycle Bin. New inbox-delivery callers should use
// `k2so_core::inbox::compose` so they land in `.k2so/inbox/` where
// the renderer + CLI actually read from.
