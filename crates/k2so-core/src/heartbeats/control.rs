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


use crate::workspace::agent_identity::{agent_dir, resolve_project_id};
use crate::workspace::scheduler::{
    read_heartbeat_config, write_heartbeat_config, AgentHeartbeatConfig,
};
use crate::workspace::wake_prompts::{agent_wakeup_path, wakeup_template_for};
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
    let hb_default = crate::workspace::agent_identity::workspace_heartbeats_dir(project_path)
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

#[cfg(test)]
mod tests {
    //! Pre-0.39.0 test-update PRD, Tier 1.6 — fills the inline-test
    //! gap that audit #555 flagged for this Phase-2.5d extraction.
    //!
    //! These tests build a throwaway "project root" under
    //! `std::env::temp_dir()` and create the legacy `.k2so/agents/<name>/`
    //! agent directory by hand — `agent_dir()` falls through to that
    //! path when no `AGENT.md` / `SKILL.md` probe matches (see
    //! `workspace::agent_identity::agent_dir`). All file I/O lives
    //! under the temp project root; no `HOME` or DB mutation needed.

    use super::*;
    use std::path::PathBuf;

    /// Throwaway project root: `<tmpdir>/k2so-hb-control-<label>-<pid>-<nanos>`.
    /// Dropping the guard removes the tree.
    struct TempProject {
        path: PathBuf,
    }

    impl TempProject {
        fn new(label: &str) -> Self {
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir()
                .join(format!("k2so-hb-control-{label}-{pid}-{nanos}"));
            std::fs::create_dir_all(&path).expect("create tempdir");
            Self { path }
        }

        /// Scaffold the legacy `<root>/.k2so/agents/<name>/` shape so
        /// `agent_dir(root, name)` returns this path. No AGENT.md /
        /// SKILL.md is written, so the layout probes in
        /// `workspace::agent_identity::agent_dir` fall through to the
        /// `agents_dir(...).join(agent_name)` final branch.
        fn make_agent(&self, name: &str) -> PathBuf {
            let dir = self.path.join(".k2so").join("agents").join(name);
            std::fs::create_dir_all(&dir).expect("create agent dir");
            dir
        }

        fn path_str(&self) -> String {
            self.path.to_string_lossy().to_string()
        }
    }

    impl Drop for TempProject {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn set_heartbeat_clamps_interval_below_min() {
        let tp = TempProject::new("clamp-below-min");
        tp.make_agent("scout");
        // Default min_interval_seconds is well above 1s; passing 1
        // MUST clamp up to the config's min.
        let cfg = set_heartbeat(
            tp.path_str(),
            "scout".to_string(),
            Some(1),
            None,
            None,
            None,
            None,
        )
        .expect("set_heartbeat");
        assert!(
            cfg.interval_seconds >= cfg.min_interval_seconds,
            "interval {} should clamp up to min {}",
            cfg.interval_seconds,
            cfg.min_interval_seconds
        );
        // And the clamped value must equal the floor — not some other
        // surprise number (e.g., the requested 1 leaking through).
        assert_eq!(
            cfg.interval_seconds, cfg.min_interval_seconds,
            "below-min request should land exactly at min"
        );
    }

    #[test]
    fn set_heartbeat_clamps_interval_above_max() {
        let tp = TempProject::new("clamp-above-max");
        tp.make_agent("scout");
        // u64::MAX is guaranteed above the configured max.
        let cfg = set_heartbeat(
            tp.path_str(),
            "scout".to_string(),
            Some(u64::MAX),
            None,
            None,
            None,
            None,
        )
        .expect("set_heartbeat");
        assert_eq!(
            cfg.interval_seconds, cfg.max_interval_seconds,
            "above-max request should land exactly at max"
        );
    }

    #[test]
    fn set_heartbeat_force_wake_sets_next_wake_to_now_and_records_user() {
        let tp = TempProject::new("force-wake");
        tp.make_agent("scout");
        let before = chrono::Utc::now();
        let cfg = set_heartbeat(
            tp.path_str(),
            "scout".to_string(),
            None,
            None,
            None,
            None,
            Some(true),
        )
        .expect("set_heartbeat force_wake");
        let after = chrono::Utc::now();

        let nw_str = cfg.next_wake.expect("force_wake must populate next_wake");
        let nw = chrono::DateTime::parse_from_rfc3339(&nw_str)
            .expect("next_wake must be valid RFC3339")
            .with_timezone(&chrono::Utc);
        // The function calls `Utc::now()` once and stamps that into
        // next_wake; it MUST fall between `before` and `after`.
        assert!(
            nw >= before && nw <= after,
            "next_wake {nw} not within [{before}, {after}]"
        );
        // force_wake also flips updated_by to "user" (vs. "agent"
        // for non-force-wake set_heartbeat calls).
        assert_eq!(
            cfg.updated_by, "user",
            "force_wake must record updated_by=user"
        );
    }

    #[test]
    fn set_heartbeat_without_force_wake_schedules_next_wake_into_future() {
        let tp = TempProject::new("future-wake");
        tp.make_agent("scout");
        // Explicit clamp-safe interval so we know the value the
        // function used to advance next_wake.
        let cfg = set_heartbeat(
            tp.path_str(),
            "scout".to_string(),
            Some(900),
            None,
            None,
            None,
            Some(false),
        )
        .expect("set_heartbeat normal");
        let nw_str = cfg.next_wake.expect("normal set must populate next_wake");
        let nw = chrono::DateTime::parse_from_rfc3339(&nw_str)
            .expect("next_wake must be valid RFC3339")
            .with_timezone(&chrono::Utc);
        let now = chrono::Utc::now();
        assert!(
            nw > now,
            "non-force-wake next_wake must be in the future; nw={nw} now={now}"
        );
        // updated_by stays "agent" for non-force-wake calls.
        assert_eq!(cfg.updated_by, "agent");
    }

    #[test]
    fn get_heartbeat_returns_defaults_when_config_missing() {
        let tp = TempProject::new("defaults");
        tp.make_agent("scout");
        // No heartbeat.json written yet → get_heartbeat returns the
        // default config (not Err).
        let cfg = get_heartbeat(tp.path_str(), "scout".to_string())
            .expect("get_heartbeat on agent with no config");
        let default = AgentHeartbeatConfig::default();
        assert_eq!(cfg.interval_seconds, default.interval_seconds);
        assert_eq!(cfg.min_interval_seconds, default.min_interval_seconds);
        assert_eq!(cfg.max_interval_seconds, default.max_interval_seconds);
        assert_eq!(cfg.consecutive_no_ops, 0);
        assert!(
            cfg.next_wake.is_none(),
            "default config has no next_wake (got: {:?})",
            cfg.next_wake
        );
    }

    #[test]
    fn get_heartbeat_errors_when_agent_does_not_exist() {
        let tp = TempProject::new("missing-agent");
        // No agent dir created.
        let err = get_heartbeat(tp.path_str(), "ghost".to_string()).unwrap_err();
        assert!(
            err.contains("does not exist"),
            "expected 'does not exist' in error, got: {err}"
        );
    }
}
