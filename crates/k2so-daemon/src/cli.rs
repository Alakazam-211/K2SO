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

/// Final HTTP response body + status line the caller emits. Keeping
/// it as an owned struct (rather than returning the raw TcpStream
/// write) lets the caller attach the `Content-Length` / `Connection:
/// close` boilerplate once at the top of the dispatch.
pub struct CliResponse {
    pub status: &'static str,
    pub content_type: &'static str,
    pub body: String,
}

impl CliResponse {
    pub fn ok_json(body: String) -> Self {
        Self {
            status: "200 OK",
            content_type: "application/json",
            body,
        }
    }
    pub fn ok_text(body: String) -> Self {
        Self {
            status: "200 OK",
            content_type: "text/plain; charset=utf-8",
            body,
        }
    }
    pub fn bad_request(err: impl std::fmt::Display) -> Self {
        Self {
            status: "400 Bad Request",
            content_type: "application/json",
            body: serde_json::json!({ "error": err.to_string() }).to_string(),
        }
    }
    pub fn not_found() -> Self {
        Self {
            status: "404 Not Found",
            content_type: "application/json",
            body: r#"{"error":"route not found"}"#.to_string(),
        }
    }
    pub fn forbidden() -> Self {
        Self {
            status: "403 Forbidden",
            content_type: "application/json",
            body: r#"{"error":"Invalid or missing auth token"}"#.to_string(),
        }
    }
}

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
                    "injections": serde_json::Value::Array(vec![]),
                    "recent_events": events,
                    "recent_events_cap": 50,
                })
                .to_string(),
            )
        }

        // ── Scheduler / triage ──────────────────────────────────────
        "/cli/agents/triage" => match need_project(params) {
            Ok(p) => CliResponse::ok_json(crate::handle_agents_triage(&p)),
            Err(r) => r,
        },
        "/cli/scheduler-tick" => match need_project(params) {
            Ok(p) => respond(k2so_core::agents::scheduler::k2so_agents_scheduler_tick(p)),
            Err(r) => r,
        },

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

        _ => CliResponse::not_found(),
    }
}
