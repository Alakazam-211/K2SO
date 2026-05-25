//! Per-agent heartbeat control (legacy adaptive-backoff API).
//!
//! Phase 2.5d: extracted from the monolithic `agents/commands.rs`. The
//! workspace-level multi-heartbeat surface lives in
//! [`crate::heartbeats`] (its `mod.rs`); this submodule hosts the
//! pre-multi-heartbeat per-agent control: `ensure_agent_wakeup`,
//! `get_heartbeat`, `set_heartbeat`, `heartbeat_noop`, `heartbeat_action`.
//!
//! Pre-Phase-2.5d these all lived under `crate::agents::commands::*`.
//! The audit clustered them here because they're heartbeat-flavored
//! even though `ensure_agent_wakeup` is called during agent creation —
//! moving with the heartbeat siblings keeps the heartbeat lifecycle
//! code together.


use crate::agents::scheduler::{
    read_heartbeat_config, write_heartbeat_config, AgentHeartbeatConfig,
};
use crate::agents::wake::{agent_wakeup_path, wakeup_template_for};
use crate::agents::{agent_dir, resolve_project_id};
use crate::db::schema::WorkspaceSession;
use crate::fs_atomic::{atomic_write_str, log_if_err};
use crate::workspace::agent::log_agent_warning;

// ── Wakeup scaffolding ────────────────────────────────────────────────

// ── Wakeup scaffolding ────────────────────────────────────────────────

/// Create `<agent_dir>/WAKEUP.md` from the matching template if it
/// doesn't exist. No-op when the agent type doesn't use wake-up or
/// when a heartbeat folder has already claimed ownership of the wake
/// source of truth.
pub fn ensure_agent_wakeup(project_path: &str, agent_name: &str, agent_type: &str) {
    let Some(template) = wakeup_template_for(agent_type) else {
        return;
    };
    let path = agent_wakeup_path(project_path, agent_name);
    if path.exists() {
        return;
    }
    // Multi-heartbeat lives at .k2so/heartbeats/<name>/WAKEUP.md
    // (post-0.37.0 workspace-level layout). If any heartbeat folder
    // already exists, we're past the legacy single-slot world and
    // the agent-root wakeup.md is no longer the source of truth.
    // Skip scaffolding to avoid tricking the repair pass into
    // clobbering real content.
    let hb_default = crate::agents::workspace_heartbeats_dir(project_path)
        .join("default")
        .join("WAKEUP.md");
    if hb_default.exists() {
        return;
    }
    log_if_err(
        "ensure_agent_wakeup",
        &path,
        atomic_write_str(&path, template),
    );
}

// ── Per-agent heartbeat control (adaptive backoff) ──────────────────

/// Read an agent's current heartbeat config (from
/// `<agent-dir>/heartbeat.json`). Returns the default config if the
/// file doesn't exist yet.
pub fn get_heartbeat(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }
    Ok(read_heartbeat_config(&project_path, &agent_name))
}

/// Partial update of an agent's heartbeat config. Any field left
/// `None` is preserved from the current on-disk config. `force_wake`
/// sets `next_wake` to now so the scheduler's next tick fires the
/// agent immediately; otherwise `next_wake` advances by the (new or
/// existing) interval.
pub fn set_heartbeat(
    project_path: String,
    agent_name: String,
    interval: Option<u64>,
    phase: Option<String>,
    mode: Option<String>,
    cost_budget: Option<String>,
    force_wake: Option<bool>,
) -> Result<AgentHeartbeatConfig, String> {
    let dir = agent_dir(&project_path, &agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }

    let mut config = read_heartbeat_config(&project_path, &agent_name);

    if let Some(interval) = interval {
        config.interval_seconds = interval
            .max(config.min_interval_seconds)
            .min(config.max_interval_seconds);
    }
    if let Some(phase) = phase {
        config.phase = phase;
    }
    if let Some(mode) = mode {
        config.mode = mode;
    }
    if let Some(budget) = cost_budget {
        config.cost_budget = budget;
    }
    config.updated_by = "agent".to_string();

    let now = chrono::Utc::now();
    if force_wake.unwrap_or(false) {
        config.next_wake = Some(now.to_rfc3339());
        config.updated_by = "user".to_string();
    } else {
        config.next_wake = Some(
            (now + chrono::Duration::seconds(config.interval_seconds as i64)).to_rfc3339(),
        );
    }

    write_heartbeat_config(&project_path, &agent_name, &config)?;
    Ok(config)
}

/// Record a no-op wake (agent had nothing to do). Applies auto-backoff
/// after 3 consecutive no-ops: multiplies interval by 1.5, clamped
/// between min and max. Also clears the DB's saved session_id so the
/// next wake starts a fresh session — there's no value in resuming a
/// conversation that was just "nothing to do here."
pub fn heartbeat_noop(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    let mut config = read_heartbeat_config(&project_path, &agent_name);
    config.consecutive_no_ops += 1;

    // Auto-backoff: 1.5x after 3 consecutive no-ops, integer math
    // to avoid float drift across repeated backoffs.
    if config.auto_backoff && config.consecutive_no_ops >= 3 {
        let new_interval = config.interval_seconds.saturating_mul(3) / 2;
        config.interval_seconds = new_interval
            .max(config.min_interval_seconds)
            .min(config.max_interval_seconds);
        log_agent_warning(
            &project_path,
            &agent_name,
            &format!(
                "Auto-backoff: {} consecutive no-ops, interval now {}s",
                config.consecutive_no_ops, config.interval_seconds
            ),
        );
    }

    // Clear saved session_id — the scheduler's next wake for this
    // agent should start a fresh session, not --resume the "I had
    // nothing to do" one.
    {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Some(project_id) = resolve_project_id(&conn, &project_path) {
            let _ = WorkspaceSession::clear_session_id(&conn, &project_id);
        }
    }

    write_heartbeat_config(&project_path, &agent_name, &config)?;
    Ok(config)
}

/// Record that an agent took meaningful action this wake. Resets the
/// consecutive-no-op counter so backoff doesn't trigger on the next
/// wake.
pub fn heartbeat_action(
    project_path: String,
    agent_name: String,
) -> Result<AgentHeartbeatConfig, String> {
    let mut config = read_heartbeat_config(&project_path, &agent_name);
    config.consecutive_no_ops = 0;
    write_heartbeat_config(&project_path, &agent_name, &config)?;
    Ok(config)
}
