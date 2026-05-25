//! Resume-chat argument resolver — daemon-owned per the daemon-first
//! architecture invariant (`feedback_daemon_first.md`).
//!
//! The pinned chat tab in K2SO and any other thin client that wants to
//! attach to a workspace's canonical claude session calls into this
//! helper to get the right `claude` command + args. Two cases:
//!
//! 1. **A saved session_id exists in `workspace_sessions` AND its JSONL
//!    is present on disk** — return `claude --resume <id>` so the
//!    same conversation continues. The user's chat history is intact.
//! 2. **No saved session_id, OR its JSONL is gone** (workspace
//!    remove+readd, claude pruning, manual SQL clears) — pre-allocate
//!    a fresh UUID, persist it via `workspace_sessions.session_id`
//!    BEFORE claude spawns (so v2_spawn's auto-stamp hook can match
//!    it against `--session-id <X>` in argv), then return
//!    `claude --session-id <X>`. The session is "pre-decided" — the
//!    pinned tab and any subsequent attach see the same UUID.
//!
//! Lifted from `src-tauri/src/commands/k2so_agents.rs::k2so_agents_resume_chat_args`
//! (which was a Tauri-side command pre-0.37.5). Moving it to k2so-core
//! means:
//!
//!   - The daemon's `/cli/workspace/resume-chat-args` route calls it
//!     directly (canonical thin-client surface — Tauri proxies through
//!     this route via HTTP, mobile companion + future MCP do the same).
//!   - CLI verb `k2so workspace resume-chat-args <ws>` calls the same
//!     route.
//!   - Tests can exercise the logic without booting Tauri.

use rusqlite::params;

/// Resolved launch config for a thin client to spawn `claude` and
/// attach to the workspace's canonical session.
#[derive(Debug, Clone)]
pub struct ResumeChatArgs {
    pub command: String,
    pub args: Vec<String>,
    pub cwd: String,
    /// The session UUID we're resuming OR pre-allocating. Always set;
    /// callers can persist it / display it / use it for tracking.
    pub resume_session: String,
    /// `true` when the saved session_id was usable (JSONL on disk),
    /// `false` when we pre-allocated a fresh UUID.
    pub resumed_existing: bool,
}

impl ResumeChatArgs {
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::json!({
            "command": self.command,
            "args": self.args,
            "cwd": self.cwd,
            "resumeSession": self.resume_session,
            "resumedExisting": self.resumed_existing,
        })
    }
}

/// Build the resume-chat args for a workspace.
///
/// **Daemon-first.** This helper has no Tauri / IPC dependency; both
/// the daemon's HTTP route and Tauri's thin proxy command call into
/// it. The CLI verb hits the same daemon route.
///
/// - `project_path`: filesystem path to the workspace (used as `cwd`
///   for the spawned claude and as the lookup key into `projects.path`).
/// - Returns `Err` only on DB I/O failures — the "no saved session"
///   case is a SUCCESS that returns pre-allocated args.
pub fn resolve_resume_chat_args(project_path: &str) -> Result<ResumeChatArgs, String> {
    let mut args: Vec<String> = vec!["--dangerously-skip-permissions".to_string()];

    let project_id: Option<String> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT id FROM projects WHERE path = ?1",
            params![project_path],
            |row| row.get(0),
        )
        .ok()
    };

    let saved_session: Option<String> = project_id
        .as_ref()
        .and_then(|pid| {
            let db = crate::db::shared();
            let conn = db.lock();
            crate::db::schema::WorkspaceSession::get(&conn, pid)
                .ok()
                .flatten()
                .and_then(|row| row.session_id.filter(|s| !s.is_empty()))
        });

    if let Some(ref sid) = saved_session {
        if crate::chat_history::claude_session_file_exists(sid, project_path) {
            args.push("--resume".to_string());
            args.push(sid.clone());
            return Ok(ResumeChatArgs {
                command: "claude".to_string(),
                args,
                cwd: project_path.to_string(),
                resume_session: sid.clone(),
                resumed_existing: true,
            });
        }
    }

    // No saved session (or its JSONL is gone) — pre-allocate the
    // session UUID and pin it via `--session-id <X>`. Persist to
    // workspace_sessions.session_id BEFORE claude spawns so v2_spawn's
    // auto-stamp hook sees the matching id in argv.
    let new_sid = uuid::Uuid::new_v4().to_string();
    if let Some(pid) = project_id.as_deref() {
        let db = crate::db::shared();
        let conn = db.lock();
        let row_id = uuid::Uuid::new_v4().to_string();
        let _ = conn.execute(
            "INSERT INTO workspace_sessions (id, project_id, session_id, harness, owner, status, created_at) \
             VALUES (?1, ?2, ?3, 'claude', 'user', 'running', unixepoch()) \
             ON CONFLICT(project_id) DO UPDATE SET session_id = ?3, last_activity_at = unixepoch()",
            params![row_id, pid, new_sid],
        );
    }
    args.push("--session-id".to_string());
    args.push(new_sid.clone());

    Ok(ResumeChatArgs {
        command: "claude".to_string(),
        args,
        cwd: project_path.to_string(),
        resume_session: new_sid,
        resumed_existing: false,
    })
}
