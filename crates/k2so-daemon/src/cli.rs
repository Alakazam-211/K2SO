//! Unified `/cli/*` route dispatch.
//!
//! Every authenticated request whose path starts with `/cli/` lands
//! here. The handler:
//!
//! 1. Parses query parameters via `k2so_core::agent_hooks::parse_query_params`.
//! 2. Validates the bearer token against the per-boot daemon token
//!    (same auth check as `/status` and `/hook/complete`).
//! 3. Dispatches on the full path to one of the per-route handler
//!    functions.
//! 4. Returns `(status_code, content_type, body)` which the caller
//!    renders as an HTTP response.
//!
//! Each per-route handler is a thin wrapper around a
//! `k2so_core::agents::*` or `k2so_core::agent_hooks` function —
//! effectively the "daemon-side invoke_handler" mirror of the
//! Tauri-side command registry in src-tauri.
//!
//! Routes that require a `project` / `project_path` query parameter
//! accept EITHER — see `project_param` in main.rs. Routes that
//! don't need a project path (`/cli/hooks/status`) extract the
//! params but skip the project check.
//!
//! Unknown `/cli/*` paths fall through to 404.

use std::collections::HashMap;

// CliResponse is shared with lib-side handler modules
// (terminal_routes, etc.) via the top-level cli_response module.
pub use crate::cli_response::CliResponse;

/// Serialize a `Result<T, String>` from core into either a 200 JSON
/// body or a 400 `{"error": "..."}`. The single biggest shape for
/// `/cli/*` handlers.
fn respond<T: serde::Serialize>(r: Result<T, String>) -> CliResponse {
    match r {
        Ok(v) => CliResponse::ok_json(
            serde_json::to_string(&v).unwrap_or_else(|e| format!("{{\"error\":\"{}\"}}", e)),
        ),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Wrap `Ok(())` success into `{"success":true}` JSON.
fn respond_unit(r: Result<(), String>) -> CliResponse {
    match r {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

/// Extract project path from `project` or `project_path` query
/// params; returns 400 response if missing/empty.
fn need_project(params: &HashMap<String, String>) -> Result<String, CliResponse> {
    for key in &["project_path", "project"] {
        if let Some(v) = params.get(*key) {
            if !v.is_empty() {
                return Ok(v.clone());
            }
        }
    }
    Err(CliResponse::bad_request(
        "Missing project (or project_path) parameter",
    ))
}

fn str_param(params: &HashMap<String, String>, key: &str) -> String {
    params.get(key).cloned().unwrap_or_default()
}

fn opt_param(params: &HashMap<String, String>, key: &str) -> Option<String> {
    params.get(key).cloned().filter(|s| !s.is_empty())
}

fn bool_param(params: &HashMap<String, String>, key: &str) -> bool {
    matches!(
        params.get(key).map(|v| v.as_str()),
        Some("1") | Some("true") | Some("on")
    )
}

// ── Main dispatch ─────────────────────────────────────────────────────

/// Route a single `/cli/*` path to its handler. Assumes the caller
/// has already validated the bearer token.
pub fn dispatch(path: &str, params: &HashMap<String, String>) -> CliResponse {
    match path {
        // ── Read-only: agent metadata ────────────────────────────────
        "/cli/agents/list" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::commands::list(p)),
            Err(r) => r,
        },
        "/cli/agents/profile" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                match k2so_core::agents::commands::get_profile(p, agent) {
                    Ok(content) => CliResponse::ok_json(
                        serde_json::json!({ "content": content }).to_string(),
                    ),
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },
        "/cli/agents/work" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                let folder = opt_param(params, "folder");
                respond(k2so_core::agents::commands::work_list(p, agent, folder))
            }
            Err(r) => r,
        },
        "/cli/work/inbox" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::commands::workspace_inbox_list(p)),
            Err(r) => r,
        },

        // ── State-mutating: agent CRUD ──────────────────────────────
        "/cli/agents/create" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::commands::create(
                p,
                str_param(params, "name"),
                str_param(params, "role"),
                opt_param(params, "prompt"),
                opt_param(params, "agent_type"),
            )),
            Err(r) => r,
        },
        "/cli/agents/delete" => match need_project(params) {
            Ok(p) => respond_unit(k2so_core::agents::commands::delete(
                p,
                str_param(params, "name"),
            )),
            Err(r) => r,
        },
        "/cli/agent/update" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::commands::update_field(
                p,
                str_param(params, "agent"),
                str_param(params, "field"),
                str_param(params, "value"),
            )
            .map(|content| serde_json::json!({ "success": true, "content": content }))),
            Err(r) => r,
        },

        // ── State-mutating: work queue ──────────────────────────────
        "/cli/agents/work/create" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::commands::work_create(
                p,
                opt_param(params, "agent"),
                str_param(params, "title"),
                str_param(params, "body"),
                opt_param(params, "priority"),
                opt_param(params, "type"),
                opt_param(params, "source"),
            )),
            Err(r) => r,
        },
        "/cli/agents/work/move" => match need_project(params) {
            Ok(p) => respond_unit(k2so_core::agents::commands::work_move(
                p,
                str_param(params, "agent"),
                str_param(params, "filename"),
                str_param(params, "from"),
                str_param(params, "to"),
            )),
            Err(r) => r,
        },
        "/cli/work/inbox/create" => {
            // workspace path may override `project` for cross-workspace
            // delegation — matches the Tauri-side behavior.
            let workspace = opt_param(params, "workspace")
                .or_else(|| need_project(params).ok())
                .unwrap_or_default();
            if workspace.is_empty() {
                return CliResponse::bad_request("Missing workspace (or project_path) parameter");
            }
            respond(k2so_core::agents::commands::workspace_inbox_create(
                workspace,
                str_param(params, "title"),
                str_param(params, "body"),
                opt_param(params, "priority"),
                opt_param(params, "type"),
                opt_param(params, "assigned_by"),
                opt_param(params, "source"),
            ))
        }

        // ── Agent lifecycle: lock + session ─────────────────────────
        "/cli/agents/lock" => match need_project(params) {
            Ok(p) => respond_unit(k2so_core::agents::session::k2so_agents_lock(
                p,
                str_param(params, "agent"),
                opt_param(params, "terminal_id"),
                opt_param(params, "owner"),
            )),
            Err(r) => r,
        },
        "/cli/agents/unlock" => match need_project(params) {
            Ok(p) => respond_unit(k2so_core::agents::session::k2so_agents_unlock(
                p,
                str_param(params, "agent"),
            )),
            Err(r) => r,
        },

        // ── Agent-hook channel events ───────────────────────────────
        "/cli/events" => match need_project(params) {
            Ok(p) => {
                let agent =
                    opt_param(params, "agent").unwrap_or_else(|| "__lead__".to_string());
                let events = k2so_core::agents::events::drain_agent_events(&p, &agent);
                CliResponse::ok_json(
                    serde_json::to_string(&events).unwrap_or_else(|_| "[]".to_string()),
                )
            }
            Err(r) => r,
        },
        "/cli/agent/reply" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                let message = str_param(params, "message");
                k2so_core::agent_hooks::emit(
                    k2so_core::agent_hooks::HookEvent::AgentReply,
                    serde_json::json!({
                        "agentName": agent,
                        "message": message,
                        "projectPath": p,
                        "timestamp": chrono::Utc::now().to_rfc3339(),
                    }),
                );
                CliResponse::ok_json(r#"{"success":true}"#.to_string())
            }
            Err(r) => r,
        },

        // ── Per-agent heartbeat control ─────────────────────────────
        "/cli/agents/heartbeat" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                let interval = opt_param(params, "interval").and_then(|v| v.parse::<u64>().ok());
                let phase = opt_param(params, "phase");
                let mode = opt_param(params, "mode");
                let cost_budget = opt_param(params, "cost_budget");
                // If ANY mutation param is present → update; else → read.
                if interval.is_some()
                    || phase.is_some()
                    || mode.is_some()
                    || cost_budget.is_some()
                {
                    let force_wake = if params.contains_key("force_wake") {
                        Some(bool_param(params, "force_wake"))
                    } else {
                        None
                    };
                    respond(k2so_core::agents::commands::set_heartbeat(
                        p,
                        agent,
                        interval,
                        phase,
                        mode,
                        cost_budget,
                        force_wake,
                    ))
                } else {
                    respond(k2so_core::agents::commands::get_heartbeat(p, agent))
                }
            }
            Err(r) => r,
        },
        "/cli/agents/heartbeat/noop" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::commands::heartbeat_noop(
                p,
                str_param(params, "agent"),
            )),
            Err(r) => r,
        },
        "/cli/agents/heartbeat/action" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::commands::heartbeat_action(
                p,
                str_param(params, "agent"),
            )),
            Err(r) => r,
        },

        // ── Per-project mode + settings toggles ─────────────────────
        "/cli/mode" => match need_project(params) {
            Ok(p) => {
                if let Some(mode) = opt_param(params, "set") {
                    match k2so_core::agents::settings::update_project_setting(&p, "agent_mode", &mode) {
                        Ok(()) => {
                            k2so_core::agent_hooks::emit(
                                k2so_core::agent_hooks::HookEvent::SyncProjects,
                                serde_json::Value::Null,
                            );
                            CliResponse::ok_json(
                                serde_json::json!({"success": true, "mode": mode}).to_string(),
                            )
                        }
                        Err(e) => CliResponse::bad_request(e),
                    }
                } else {
                    // Read current mode. Falls back to filesystem-
                    // detection if DB has no row.
                    match k2so_core::agents::settings::get_project_settings(&p) {
                        Ok(settings) => CliResponse::ok_json(
                            serde_json::to_string(&settings).unwrap_or_default(),
                        ),
                        Err(_) => {
                            let k2so_dir = std::path::PathBuf::from(&p).join(".k2so");
                            let agents_dir = k2so_dir.join("agents");
                            let has_agents = agents_dir.exists()
                                && std::fs::read_dir(&agents_dir)
                                    .map(|e| e.count() > 0)
                                    .unwrap_or(false);
                            let claude_md =
                                std::path::PathBuf::from(&p).join("CLAUDE.md");
                            let mode = if !claude_md.exists() {
                                "off"
                            } else if has_agents {
                                "manager"
                            } else {
                                "agent"
                            };
                            CliResponse::ok_json(
                                serde_json::json!({"mode": mode}).to_string(),
                            )
                        }
                    }
                }
            }
            Err(r) => r,
        },
        "/cli/settings" => match need_project(params) {
            Ok(p) => match k2so_core::agents::settings::get_project_settings(&p) {
                Ok(s) => CliResponse::ok_json(serde_json::to_string(&s).unwrap_or_default()),
                Err(e) => CliResponse::bad_request(e),
            },
            Err(r) => r,
        },
        "/cli/worktree" => match need_project(params) {
            Ok(p) => {
                let enable = bool_param(params, "enable");
                let value = if enable { "1" } else { "0" };
                match k2so_core::agents::settings::update_project_setting(&p, "worktree_mode", value) {
                    Ok(()) => {
                        k2so_core::agent_hooks::emit(
                            k2so_core::agent_hooks::HookEvent::SyncProjects,
                            serde_json::Value::Null,
                        );
                        CliResponse::ok_json(
                            serde_json::json!({"success": true, "worktreeMode": enable})
                                .to_string(),
                        )
                    }
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },
        "/cli/agentic" => {
            // Global toggle, not project-specific.
            if let Some(enable) = opt_param(params, "enable") {
                let on = enable == "1" || enable == "true" || enable == "on";
                match k2so_core::agents::settings::set_agentic_enabled(on) {
                    Ok(()) => {
                        k2so_core::agent_hooks::emit(
                            k2so_core::agent_hooks::HookEvent::SyncSettings,
                            serde_json::Value::Null,
                        );
                        CliResponse::ok_json(
                            serde_json::json!({"success": true, "agenticEnabled": on})
                                .to_string(),
                        )
                    }
                    Err(e) => CliResponse::bad_request(e),
                }
            } else {
                let enabled = k2so_core::agents::settings::get_agentic_enabled();
                CliResponse::ok_json(
                    serde_json::json!({"agenticEnabled": enabled}).to_string(),
                )
            }
        }

        // ── Review queue ────────────────────────────────────────────
        "/cli/reviews" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::reviews::review_queue(&p)),
            Err(r) => r,
        },
        "/cli/review/approve" => match need_project(params) {
            Ok(p) => {
                let branch = str_param(params, "branch");
                let agent = str_param(params, "agent");
                match k2so_core::agents::reviews::review_approve(p, branch, agent) {
                    Ok(msg) => CliResponse::ok_json(
                        serde_json::json!({"success": true, "message": msg}).to_string(),
                    ),
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },
        "/cli/review/reject" => match need_project(params) {
            Ok(p) => respond_unit(k2so_core::agents::reviews::review_reject(
                p,
                str_param(params, "agent"),
                opt_param(params, "reason"),
            )),
            Err(r) => r,
        },
        "/cli/review/feedback" => match need_project(params) {
            Ok(p) => respond_unit(k2so_core::agents::reviews::review_request_changes(
                p,
                str_param(params, "agent"),
                str_param(params, "feedback"),
            )),
            Err(r) => r,
        },

        // ── Companion tunnel + globals ──────────────────────────────
        "/cli/companion/start" => match k2so_core::companion::start_companion() {
            Ok(url) => CliResponse::ok_json(
                serde_json::json!({"ok": true, "url": url}).to_string(),
            ),
            Err(e) => CliResponse::bad_request(e),
        },
        "/cli/companion/stop" => match k2so_core::companion::stop_companion() {
            Ok(()) => CliResponse::ok_json(r#"{"ok":true}"#.to_string()),
            Err(e) => CliResponse::bad_request(e),
        },
        "/cli/companion/status" => {
            CliResponse::ok_json(k2so_core::companion::companion_status().to_string())
        }
        "/cli/companion/presets" => match k2so_core::companion::cli_routes::list_presets() {
            Ok(body) => CliResponse::ok_json(body),
            Err(e) => CliResponse::bad_request(e),
        },
        "/cli/companion/projects" => match k2so_core::companion::cli_routes::list_projects() {
            Ok(body) => CliResponse::ok_json(body),
            Err(e) => CliResponse::bad_request(e),
        },

        // ── Aggregated agent check-in ───────────────────────────────
        "/cli/checkin" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                match k2so_core::agents::checkin::checkin(&p, &agent) {
                    Ok(body) => CliResponse::ok_json(body),
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // ── Workspace lifecycle ─────────────────────────────────────
        "/cli/workspace/create" => {
            let target = str_param(params, "path");
            match k2so_core::agents::workspaces::create_workspace(&target) {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => CliResponse::bad_request(e),
            }
        }
        "/cli/workspace/open" => {
            let target = str_param(params, "path");
            match k2so_core::agents::workspaces::open_workspace(&target) {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => CliResponse::bad_request(e),
            }
        }
        "/cli/workspace/cleanup" => {
            match k2so_core::agents::workspaces::cleanup_stale_workspaces() {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => CliResponse::bad_request(e),
            }
        }
        "/cli/workspace/remove" => {
            // Teardown modes (keep_current / restore_original) still
            // live in src-tauri because they depend on
            // HARNESS_WORKSPACE_FILES + find_latest_archive. The
            // daemon serves the DB-only path; callers that pass a
            // `mode` get a 400 telling them to run from the Tauri
            // app until that helper is migrated.
            if params.contains_key("mode") {
                return CliResponse::bad_request(
                    "Workspace teardown modes (keep_current/restore_original) must be run from the Tauri app — daemon serves DB-only remove.",
                );
            }
            let target = str_param(params, "path");
            match k2so_core::agents::workspaces::remove_workspace_db_only(&target) {
                Ok(body) => CliResponse::ok_json(body),
                Err(e) => CliResponse::bad_request(e),
            }
        }

        // ── Sub-agent completion ────────────────────────────────────
        "/cli/agent/complete" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                let file = str_param(params, "file");
                match k2so_core::agents::reviews::agent_complete(p, agent, file) {
                    Ok(body) => CliResponse::ok_json(body),
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // ── Agent CLAUDE.md regen ───────────────────────────────────
        "/cli/agents/generate-claude-md" => match need_project(params) {
            Ok(p) => {
                let agent = str_param(params, "agent");
                if agent.is_empty() {
                    return CliResponse::bad_request("Missing 'agent' parameter");
                }
                match k2so_core::agents::skill_content::generate_agent_claude_md_content(
                    &p, &agent, None,
                ) {
                    Ok(md) => {
                        let claude_md_path =
                            k2so_core::agents::agent_dir(&p, &agent).join("CLAUDE.md");
                        if let Err(e) =
                            k2so_core::agents::work_item::atomic_write(&claude_md_path, &md)
                        {
                            return CliResponse::bad_request(e);
                        }
                        CliResponse::ok_json(
                            serde_json::json!({"success": true, "length": md.len()})
                                .to_string(),
                        )
                    }
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // ── Workspace connections ───────────────────────────────────
        "/cli/connections" => match need_project(params) {
            Ok(p) => {
                let action = params
                    .get("action")
                    .cloned()
                    .unwrap_or_else(|| "list".to_string());
                let target = opt_param(params, "target");
                let rel_type = opt_param(params, "type");
                match k2so_core::agents::connections::connections(
                    &p,
                    &action,
                    target.as_deref(),
                    rel_type.as_deref(),
                ) {
                    Ok(body) => CliResponse::ok_json(body),
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // ── Workspace states ────────────────────────────────────────
        "/cli/states/list" => {
            let db = k2so_core::db::shared();
            let conn = db.lock();
            match k2so_core::db::schema::WorkspaceState::list(&conn) {
                Ok(rows) => CliResponse::ok_json(
                    serde_json::to_string(&rows).unwrap_or_else(|_| "[]".to_string()),
                ),
                Err(e) => CliResponse::bad_request(e.to_string()),
            }
        }
        "/cli/states/get" => {
            let id = str_param(params, "id");
            let db = k2so_core::db::shared();
            let conn = db.lock();
            match k2so_core::db::schema::WorkspaceState::get(&conn, &id) {
                Ok(s) => CliResponse::ok_json(serde_json::to_string(&s).unwrap_or_default()),
                Err(_) => CliResponse::bad_request(format!("State '{}' not found", id)),
            }
        }
        "/cli/states/set" => match need_project(params) {
            Ok(p) => {
                let state_id = str_param(params, "state_id");
                match k2so_core::agents::settings::update_project_setting(&p, "tier_id", &state_id)
                {
                    Ok(()) => {
                        k2so_core::agent_hooks::emit(
                            k2so_core::agent_hooks::HookEvent::SyncProjects,
                            serde_json::Value::Null,
                        );
                        CliResponse::ok_json(
                            serde_json::json!({"success": true, "stateId": state_id})
                                .to_string(),
                        )
                    }
                    Err(e) => CliResponse::bad_request(e),
                }
            }
            Err(r) => r,
        },

        // ── Agent channel ops (status / done / reserve / release) ──
        "/cli/status" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::channel::status(
                p,
                str_param(params, "agent"),
                str_param(params, "message"),
            )),
            Err(r) => r,
        },
        "/cli/done" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::channel::done(
                p,
                str_param(params, "agent"),
                opt_param(params, "blocked"),
            )),
            Err(r) => r,
        },
        "/cli/reserve" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::channel::reserve(
                p,
                str_param(params, "agent"),
                str_param(params, "paths"),
            )),
            Err(r) => r,
        },
        "/cli/release" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::channel::release(
                p,
                str_param(params, "agent"),
                str_param(params, "paths"),
            )),
            Err(r) => r,
        },

        // ── Skill fan-out ───────────────────────────────────────────
        "/cli/skills/regenerate" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::commands::regenerate_skills(p)),
            Err(r) => r,
        },

        // ── Activity feed ───────────────────────────────────────────
        "/cli/feed" => match need_project(params) {
            Ok(p) => {
                let limit = params
                    .get("limit")
                    .and_then(|s| s.parse::<i64>().ok())
                    .unwrap_or(20);
                let agent = opt_param(params, "agent");

                let db = k2so_core::db::shared();
                let conn = db.lock();

                let project_id: String = match conn.query_row(
                    "SELECT id FROM projects WHERE path = ?1",
                    rusqlite::params![p],
                    |row| row.get(0),
                ) {
                    Ok(id) => id,
                    Err(e) => {
                        return CliResponse::bad_request(format!("Project not found: {}", e))
                    }
                };

                let entries = match agent {
                    Some(agent_name) => k2so_core::db::schema::ActivityFeedEntry::list_by_agent(
                        &conn, &project_id, &agent_name, limit,
                    ),
                    None => k2so_core::db::schema::ActivityFeedEntry::list_by_project(
                        &conn, &project_id, limit, 0,
                    ),
                };

                match entries {
                    Ok(entries) => {
                        let items: Vec<serde_json::Value> = entries
                            .iter()
                            .map(|e| {
                                serde_json::json!({
                                    "id": e.id,
                                    "agent": e.agent_name,
                                    "type": e.event_type,
                                    "from": e.from_agent,
                                    "to": e.to_agent,
                                    "summary": e.summary,
                                    "at": e.created_at,
                                })
                            })
                            .collect();
                        CliResponse::ok_json(serde_json::json!({ "feed": items }).to_string())
                    }
                    Err(e) => CliResponse::bad_request(e.to_string()),
                }
            }
            Err(r) => r,
        },

        // ── AI-assisted commit (emit-only) ──────────────────────────
        // /cli/commit and /cli/commit-merge both emit HookEvent::CliAiCommit
        // — Tauri-side sink spawns the commit terminal. Daemon has no PTY
        // of its own to spawn, so emission is the whole job.
        "/cli/commit" | "/cli/commit-merge" => match need_project(params) {
            Ok(p) => {
                let include_merge = path == "/cli/commit-merge";
                let message = str_param(params, "message");
                let git_context = k2so_core::git::gather_git_context(&p);
                let event_payload = serde_json::json!({
                    "projectPath": p,
                    "includeMerge": include_merge,
                    "message": message,
                    "gitContext": git_context,
                });
                k2so_core::agent_hooks::emit(
                    k2so_core::agent_hooks::HookEvent::CliAiCommit,
                    event_payload,
                );
                CliResponse::ok_json(
                    serde_json::json!({
                        "success": true,
                        "action": if include_merge { "commit-merge" } else { "commit" },
                        "note": "AI commit terminal session will be launched by K2SO"
                    })
                    .to_string(),
                )
            }
            Err(r) => r,
        },

        // ── Per-project heartbeat schedule (distinct from per-agent) ─
        "/cli/heartbeat/schedule" => match need_project(params) {
            Ok(p) => {
                let db = k2so_core::db::shared();
                let conn = db.lock();

                if let Some(mode) = opt_param(params, "mode") {
                    let schedule = opt_param(params, "schedule");
                    let hb_enabled = if mode == "off" { "0" } else { "1" };

                    let res = conn
                        .execute(
                            "UPDATE projects SET heartbeat_mode = ?1, heartbeat_schedule = ?2, heartbeat_enabled = ?3 WHERE path = ?4",
                            rusqlite::params![mode, schedule, hb_enabled, p],
                        )
                        .map(|_| ())
                        .map_err(|e| format!("DB update failed: {}", e));
                    drop(conn);

                    match res {
                        Ok(()) => {
                            // Nudge the Tauri side to refresh its
                            // launchd/cron installer via SyncProjects.
                            k2so_core::agent_hooks::emit(
                                k2so_core::agent_hooks::HookEvent::SyncProjects,
                                serde_json::Value::Null,
                            );
                            CliResponse::ok_json(
                                serde_json::json!({
                                    "success": true,
                                    "mode": mode,
                                    "schedule": schedule,
                                })
                                .to_string(),
                            )
                        }
                        Err(e) => CliResponse::bad_request(e),
                    }
                } else {
                    let res = conn.query_row(
                        "SELECT heartbeat_mode, heartbeat_schedule, heartbeat_last_fire FROM projects WHERE path = ?1",
                        rusqlite::params![p],
                        |row| {
                            Ok((
                                row.get::<_, String>(0)?,
                                row.get::<_, Option<String>>(1)?,
                                row.get::<_, Option<String>>(2)?,
                            ))
                        },
                    );
                    drop(conn);
                    match res {
                        Ok((mode, schedule, last_fire)) => CliResponse::ok_json(
                            serde_json::json!({
                                "mode": mode,
                                "schedule": schedule,
                                "lastFire": last_fire,
                            })
                            .to_string(),
                        ),
                        Err(e) => CliResponse::bad_request(format!("Project not found: {}", e)),
                    }
                }
            }
            Err(r) => r,
        },

        // ── Hook diagnostic ─────────────────────────────────────────
        "/cli/hooks/status" => {
            let limit = params
                .get("limit")
                .and_then(|s| s.parse::<usize>().ok())
                .unwrap_or(20)
                .min(50);
            let mut events: Vec<_> = k2so_core::agent_hooks::get_recent_events();
            events.reverse();
            events.truncate(limit);
            CliResponse::ok_json(
                serde_json::json!({
                    "port": k2so_core::hook_config::get_port(),
                    "notify_script": dirs::home_dir()
                        .map(|h| h.join(".k2so/hooks/notify.sh").to_string_lossy().to_string())
                        .unwrap_or_default(),
                    // H7.1: scan per-CLI config files for notify.sh
                    // injection so `k2so hooks status` reports the
                    // full pipeline state (claude/cursor/gemini). Core
                    // helper moved from src-tauri as part of H7.
                    "injections": k2so_core::agent_hooks::check_hook_injections(),
                    "recent_events": events,
                    "recent_events_cap": 50,
                })
                .to_string(),
            )
        }

        // ── Scheduler / triage ──────────────────────────────────────
        // `/cli/agents/triage` is READ-ONLY (plain-text summary for
        // `k2so agents triage`). `/cli/scheduler-tick` is the
        // DESTRUCTIVE heartbeat fire path — `~/.k2so/heartbeat.sh`
        // invokes it on launchd's schedule and parses `"count":N`
        // to log what fired. Pre-Phase-4 Tauri's agent_hooks
        // listener served them with these same semantics; H7
        // preserves the contract.
        "/cli/agents/triage" => match need_project(params) {
            Ok(p) => CliResponse::ok_text(crate::handle_agents_triage(&p)),
            Err(r) => r,
        },
        "/cli/scheduler-tick" => match need_project(params) {
            Ok(p) => CliResponse::ok_json(crate::triage::handle_scheduler_fire(&p)),
            Err(r) => r,
        },

        // P5.6: DB-as-source-of-truth replacement for the legacy
        // ~/.k2so/heartbeat-projects.txt file. heartbeat.sh now calls
        // this once per cron tick and iterates the response, calling
        // /cli/scheduler-tick per project. Newline-delimited plain
        // text so bash can `while read` without a JSON parser.
        // Returns every project path with at least one enabled,
        // non-archived agent_heartbeats row — derived state, never
        // stale.
        "/cli/heartbeat/active-projects" => {
            CliResponse::ok_text(crate::triage::handle_active_projects())
        }

        // ── Heartbeat CRUD + fires ──────────────────────────────────
        p if p.starts_with("/cli/heartbeat/") || p == "/cli/heartbeat-log" => {
            match need_project(params) {
                Ok(pp) => {
                    let result = if p == "/cli/heartbeat-log" {
                        crate::handle_cli_heartbeat_log(&pp, params)
                    } else {
                        crate::handle_cli_heartbeat(p, &pp, params)
                    };
                    match result {
                        Ok(body) => CliResponse::ok_json(body),
                        Err(msg) => CliResponse::bad_request(msg),
                    }
                }
                Err(r) => r,
            }
        }

        // ── Phase 4 H1: daemon-side terminal IO ─────────────────────
        // Session-stream-aware read + write against daemon-owned
        // sessions. `id` is a SessionId UUID. See
        // `terminal_routes` for behavior details.
        "/cli/terminal/read" => crate::terminal_routes::handle_read(params),
        "/cli/terminal/write" => crate::terminal_routes::handle_write(params),

        // ── Phase 4 H2: live-session enumeration ────────────────────
        // Replaces the Tauri endpoint that walked AppState's
        // terminal_manager. Now a walk of session_map + registry.
        "/cli/agents/running" => crate::terminal_routes::handle_agents_running(params),

        // ── Phase 4.5 I7: resize a live session ─────────────────────
        // Resizes both the PTY and the alacritty Term so the child
        // re-flows for the new dimensions. Called by Kessel's
        // ResizeObserver on DOM pane resize.
        "/cli/sessions/resize" => crate::terminal_routes::handle_sessions_resize(params),

        // ── Phase 4 H3: daemon-side terminal spawn ──────────────────
        // Thin wrappers over `spawn::spawn_agent_session` (the same
        // helper /cli/sessions/spawn uses). Emits HookEvents so
        // attached UIs can react, matching the legacy Tauri
        // endpoint shape.
        "/cli/terminal/spawn" => match need_project(params) {
            Ok(p) => crate::terminal_routes::handle_terminal_spawn(params, &p),
            Err(r) => r,
        },
        "/cli/terminal/spawn-background" => match need_project(params) {
            Ok(p) => crate::terminal_routes::handle_terminal_spawn_background(params, &p),
            Err(r) => r,
        },

        // ── Phase 4 H4: companion cross-workspace enumeration ──────
        // Global session list + per-project summary. No project
        // param — these are intentionally cross-workspace (the
        // companion UI shows every workspace at once).
        "/cli/companion/sessions" => crate::companion_routes::handle_companion_sessions(params),
        "/cli/companion/projects-summary" => {
            crate::companion_routes::handle_companion_projects_summary(params)
        }

        // ── Phase 4 H5: agent launch + delegate ─────────────────────
        // Daemon-owned Session Stream replacement for Tauri's
        // `spawn_wake_pty`-backed handlers. Core still builds the
        // launch JSON (three wake branches for launch; worktree +
        // task CLAUDE.md for delegate) — the difference is the
        // spawn lands in daemon session_map, not in Tauri's
        // TerminalManager.
        "/cli/agents/launch" => match need_project(params) {
            Ok(p) => crate::agents_routes::handle_agents_launch(params, &p),
            Err(r) => r,
        },
        "/cli/agents/delegate" => match need_project(params) {
            Ok(p) => crate::agents_routes::handle_agents_delegate(params, &p),
            Err(r) => r,
        },

        // R3 — session diagnostic. Returns ring stats for a live
        // session so callers can verify the preload flow worked
        // ("did the full conversation history make it into the
        // ring?"). Query param: session=<uuid>.
        "/cli/sessions/diagnose" => diagnose_session(params),

        // ── Onboarding (workspace-add three-option flow) ────────
        //
        // Logic lives in `k2so_core::agents::onboarding`. Daemon
        // exposes the four ops over HTTP so the `k2so onboarding`
        // CLI subcommand and any other headless caller can drive
        // the same flow as the Tauri `WorkspaceOnboardingModal`.
        // Adopt + Start Fresh fire the workspace-regen bridge —
        // a no-op when the host hasn't registered a regen impl
        // (next Tauri launch picks up the staged PROJECT.md).
        "/cli/onboarding/scan" => match need_project(params) {
            Ok(p) => respond(Ok::<_, String>(
                k2so_core::agents::onboarding::scan_harness_files(&p),
            )),
            Err(r) => r,
        },
        "/cli/onboarding/adopt" => match need_project(params) {
            Ok(p) => {
                let source = str_param(params, "source");
                if source.is_empty() {
                    CliResponse::bad_request("Missing source parameter")
                } else {
                    match k2so_core::agents::onboarding::adopt_harness_as_project_md(
                        &p,
                        std::path::Path::new(&source),
                    ) {
                        Ok(outcome) => {
                            let _ = k2so_core::agents::workspace_regen::regen_workspace_skill(&p);
                            respond(Ok::<_, String>(outcome))
                        }
                        Err(e) => CliResponse::bad_request(e),
                    }
                }
            }
            Err(r) => r,
        },
        "/cli/onboarding/skip" => match need_project(params) {
            Ok(p) => respond_unit(k2so_core::agents::onboarding::skip_harness_management(&p)),
            Err(r) => r,
        },
        "/cli/onboarding/start-fresh" => match need_project(params) {
            Ok(p) => {
                if let Err(e) = k2so_core::agents::onboarding::unskip_harness_management(&p) {
                    return CliResponse::bad_request(e);
                }
                let _ = k2so_core::agents::workspace_regen::regen_workspace_skill(&p);
                CliResponse::ok_json(r#"{"success":true}"#.to_string())
            }
            Err(r) => r,
        },

        // Note: `/cli/heartbeat/active-session` lives in main.rs's
        // `handle_cli_heartbeat` dispatcher (alongside the rest of the
        // heartbeat CRUD), not here — main.rs intercepts /cli/heartbeat/*
        // before this fallthrough dispatcher runs.

        _ => CliResponse::not_found(),
    }
}

/// R3 — handler for `/cli/sessions/diagnose?session=<uuid>`.
/// Counts each Frame variant in the replay ring, emits the first
/// and last frames as full JSON (for sanity-checking), and reports
/// the ring's cap so the caller knows how close to the limit the
/// session is. Used by the daemon to answer "what got preloaded?"
/// without dumping an entire conversation's worth of frames back
/// over the wire.
fn diagnose_session(params: &HashMap<String, String>) -> CliResponse {
    use k2so_core::session::{registry, Frame, SessionId};

    let raw = str_param(params, "session");
    let session_id = match SessionId::parse(&raw) {
        Some(id) => id,
        None => {
            return CliResponse::bad_request(
                "missing or malformed 'session' query param",
            );
        }
    };
    let entry = match registry::lookup(&session_id) {
        Some(e) => e,
        None => {
            return CliResponse::bad_request(format!(
                "session {session_id} not found in registry"
            ));
        }
    };

    let ring = entry.replay_snapshot();
    // Count frame variants. Matches the preload filter in
    // `session::preload::should_replay` so callers can eyeball
    // whether the filter is working.
    let mut text_count = 0usize;
    let mut cursor_op_count = 0usize;
    let mut mode_change_count = 0usize;
    let mut raw_pty_count = 0usize;
    let mut bell_count = 0usize;
    let mut semantic_count = 0usize;
    let mut agent_signal_count = 0usize;
    let mut other_count = 0usize;
    for f in ring.iter() {
        match f {
            Frame::Text { .. } => text_count += 1,
            Frame::CursorOp(_) => cursor_op_count += 1,
            Frame::ModeChange { .. } => mode_change_count += 1,
            Frame::RawPtyFrame(_) => raw_pty_count += 1,
            Frame::Bell => bell_count += 1,
            Frame::SemanticEvent { .. } => semantic_count += 1,
            Frame::AgentSignal(_) => agent_signal_count += 1,
            // Frame is #[non_exhaustive]; catch-all for future
            // variants so this route keeps compiling past new
            // additions. Diagnostic only — future work should add
            // the new variant to the struct's field list so it
            // shows up separately.
            _ => other_count += 1,
        }
    }
    let first = ring.first().cloned();
    let last = ring.last().cloned();

    let body = serde_json::json!({
        "sessionId": session_id.to_string(),
        "ringLen": ring.len(),
        "replayCap": entry.replay_cap(),
        "subscribers": entry.subscriber_count(),
        "frameCounts": {
            "text": text_count,
            "cursorOp": cursor_op_count,
            "modeChange": mode_change_count,
            "rawPty": raw_pty_count,
            "bell": bell_count,
            "semanticEvent": semantic_count,
            "agentSignal": agent_signal_count,
            "other": other_count,
        },
        // First + last frames — spot check whether the preloaded
        // content looks like the expected prior conversation (a
        // shell greeting, a Claude banner, etc.) and whether the
        // tail is where live output has been arriving.
        "firstFrame": first,
        "lastFrame": last,
    });
    CliResponse::ok_json(body.to_string())
}
