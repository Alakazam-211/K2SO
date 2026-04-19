//! Scheduler tick — decides which agents are ready to wake.
//!
//! Called by launchd's `com.k2so.agent-heartbeat` plist (via
//! `/cli/scheduler-tick` → the daemon's route → this fn). Returns the
//! ordered list of agent names the caller should launch; the caller is
//! responsible for the actual PTY spawn + `--resume`/`--append-system-
//! prompt` argument assembly.
//!
//! Differentiates between:
//!
//! - The workspace-level `__lead__` agent (fires when `.k2so/work/inbox/`
//!   has items).
//! - Top-tier agents with type `manager`/`custom`/`k2so` under
//!   `.k2so/agents/<name>/` (fires when that agent's inbox has items OR
//!   its custom-timing `next_wake` has elapsed).
//!
//! All gating decisions write audit rows into `heartbeat_fires` so
//! `k2so heartbeat status` can show exactly why a specific tick did or
//! did not fire.
//!
//! Pure function — no PTY spawning, no emissions. Host process (daemon
//! or Tauri app) turns the returned names into real PTYs.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::agents::{
    agent_dir, agents_dir, parse_frontmatter, resolve_project_id,
};
use crate::db::schema::{AgentSession, HeartbeatFire, WorkspaceState};
use crate::fs_atomic::atomic_write_str;
use crate::scheduler::should_project_fire;

// ── Path helpers private to the scheduler path ──────────────────────────

/// `<project>/.k2so/work/inbox/`.
pub fn workspace_inbox_dir(project_path: &str) -> PathBuf {
    PathBuf::from(project_path)
        .join(".k2so")
        .join("work")
        .join("inbox")
}

/// `<project>/.k2so/agents/<agent>/work/<folder>/`.
pub fn agent_work_dir(project_path: &str, agent_name: &str, folder: &str) -> PathBuf {
    agent_dir(project_path, agent_name)
        .join("work")
        .join(folder)
}

/// Bounded count of `.md` files (cap 10k so an adversarial directory
/// with millions of entries can't OOM the scheduler).
pub fn count_md_files(dir: &Path) -> usize {
    const MAX_COUNT: usize = 10_000;
    fs::read_dir(dir)
        .map(|entries| {
            entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                .take(MAX_COUNT)
                .count()
        })
        .unwrap_or(0)
}

// ── Lock check ─────────────────────────────────────────────────────────

/// Check whether an agent currently has an active session. Tries the
/// `agent_sessions` DB row first (authoritative); falls back to the
/// legacy `.lock` file so pre-migration workspaces keep working.
pub fn is_agent_locked(project_path: &str, agent_name: &str) -> bool {
    {
        let db = crate::db::shared();
        let conn = db.lock();
        if let Some(project_id) = resolve_project_id(&conn, project_path) {
            if let Ok(Some(session)) = AgentSession::get_by_agent(&conn, &project_id, agent_name) {
                if session.status == "running" {
                    return true;
                }
            }
        }
    }
    agent_work_dir(project_path, agent_name, "")
        .join(".lock")
        .exists()
}

// ── Heartbeat config types (for custom-timing agents) ──────────────────

fn default_heartbeat_mode() -> String {
    "heartbeat".to_string()
}
fn default_interval() -> u64 {
    300
}
fn default_phase() -> String {
    "monitoring".to_string()
}
fn default_max_interval() -> u64 {
    3600
}
fn default_min_interval() -> u64 {
    60
}
fn default_cost_budget() -> String {
    "low".to_string()
}
fn default_true() -> bool {
    true
}
fn default_updated_by() -> String {
    "user".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentHeartbeatConfig {
    #[serde(default = "default_heartbeat_mode")]
    pub mode: String,
    #[serde(default = "default_interval")]
    pub interval_seconds: u64,
    #[serde(default = "default_phase")]
    pub phase: String,
    #[serde(default)]
    pub active_hours: Option<ActiveHours>,
    #[serde(default = "default_max_interval")]
    pub max_interval_seconds: u64,
    #[serde(default = "default_min_interval")]
    pub min_interval_seconds: u64,
    #[serde(default = "default_cost_budget")]
    pub cost_budget: String,
    #[serde(default)]
    pub consecutive_no_ops: u32,
    #[serde(default = "default_true")]
    pub auto_backoff: bool,
    #[serde(default)]
    pub last_wake: Option<String>,
    #[serde(default)]
    pub next_wake: Option<String>,
    #[serde(default = "default_updated_by")]
    pub updated_by: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActiveHours {
    pub start: String,
    pub end: String,
    pub timezone: String,
}

impl Default for AgentHeartbeatConfig {
    fn default() -> Self {
        Self {
            mode: default_heartbeat_mode(),
            interval_seconds: default_interval(),
            phase: default_phase(),
            active_hours: None,
            max_interval_seconds: default_max_interval(),
            min_interval_seconds: default_min_interval(),
            cost_budget: default_cost_budget(),
            consecutive_no_ops: 0,
            auto_backoff: true,
            last_wake: None,
            next_wake: None,
            updated_by: default_updated_by(),
        }
    }
}

/// Read `<project>/.k2so/agents/<name>/heartbeat.json`. Missing or
/// corrupt file yields `Default`.
pub fn read_heartbeat_config(project_path: &str, agent_name: &str) -> AgentHeartbeatConfig {
    let path = agent_dir(project_path, agent_name).join("heartbeat.json");
    if path.exists() {
        fs::read_to_string(&path)
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default()
    } else {
        AgentHeartbeatConfig::default()
    }
}

/// Write `<project>/.k2so/agents/<name>/heartbeat.json` atomically.
pub fn write_heartbeat_config(
    project_path: &str,
    agent_name: &str,
    config: &AgentHeartbeatConfig,
) -> Result<(), String> {
    let dir = agent_dir(project_path, agent_name);
    if !dir.exists() {
        return Err(format!("Agent '{}' does not exist", agent_name));
    }
    let path = dir.join("heartbeat.json");
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("Failed to serialize heartbeat config: {}", e))?;
    atomic_write_str(&path, &json).map_err(|e| format!("atomic write failed: {}", e))
}

/// Is the current wall clock within an [`ActiveHours`] window?
///
/// Note: `timezone` is accepted but currently compared against local
/// system time. Full timezone support (chrono-tz) is planned.
pub fn is_within_active_hours(
    hours: &ActiveHours,
    _now: &chrono::DateTime<chrono::Utc>,
) -> bool {
    let parse_hhmm = |s: &str| -> Option<(u32, u32)> {
        let parts: Vec<&str> = s.split(':').collect();
        if parts.len() == 2 {
            let h: u32 = parts[0].parse().ok()?;
            let m: u32 = parts[1].parse().ok()?;
            if h > 23 || m > 59 {
                return None;
            }
            Some((h, m))
        } else {
            None
        }
    };
    let Some((start_h, start_m)) = parse_hhmm(&hours.start) else {
        return true;
    };
    let Some((end_h, end_m)) = parse_hhmm(&hours.end) else {
        return true;
    };
    use chrono::Timelike;
    let local = chrono::Local::now();
    let cur_min = local.hour() * 60 + local.minute();
    let start_min = start_h * 60 + start_m;
    let end_min = end_h * 60 + end_m;
    if start_min <= end_min {
        cur_min >= start_min && cur_min < end_min
    } else {
        // Overnight window (e.g. 22:00-06:00).
        cur_min >= start_min || cur_min < end_min
    }
}

// ── Workspace state + inbox priority ───────────────────────────────────

pub fn get_workspace_state(project_path: &str) -> Option<WorkspaceState> {
    let db = crate::db::shared();
    let conn = db.lock();
    let state_id: Option<String> = conn
        .query_row(
            "SELECT tier_id FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| row.get(0),
        )
        .ok()?;
    let sid = state_id?;
    WorkspaceState::get(&conn, &sid).ok()
}

/// Priority rank (lower = higher priority). Matches the frontend's
/// priority ordering for inbox sorting.
pub fn priority_rank(priority: &str) -> u8 {
    match priority {
        "critical" => 0,
        "high" => 1,
        "normal" => 2,
        "low" => 3,
        _ => 2,
    }
}

pub fn priority_label(rank: u8) -> &'static str {
    match rank {
        0 => "critical",
        1 => "high",
        2 => "normal",
        _ => "low",
    }
}

/// Highest-priority rank found in an agent's inbox. Returns 3 (low) if
/// the inbox is missing or empty.
pub fn get_highest_inbox_priority(project_path: &str, agent_name: &str) -> u8 {
    let inbox = agent_work_dir(project_path, agent_name, "inbox");
    if !inbox.exists() {
        return 3;
    }
    fs::read_dir(&inbox)
        .ok()
        .map(|entries| {
            entries
                .flatten()
                .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                .filter_map(|e| {
                    let content = fs::read_to_string(e.path()).ok()?;
                    let fm = parse_frontmatter(&content);
                    Some(priority_rank(
                        fm.get("priority").map(|s| s.as_str()).unwrap_or("normal"),
                    ))
                })
                .min()
                .unwrap_or(3)
        })
        .unwrap_or(3)
}

// ── The scheduler tick itself ──────────────────────────────────────────

/// Iterate the workspace + each top-tier agent and return the names
/// ready to launch. The caller is responsible for spawning PTYs. All
/// gating decisions write `heartbeat_fires` audit rows so
/// `k2so heartbeat status` can show what happened.
pub fn k2so_agents_scheduler_tick(project_path: String) -> Result<Vec<String>, String> {
    let tick_start = std::time::Instant::now();

    let project_row: Option<(String, String, Option<String>, Option<String>)> = {
        let db = crate::db::shared();
        let conn = db.lock();
        conn.query_row(
            "SELECT id, heartbeat_mode, heartbeat_schedule, heartbeat_last_fire \
             FROM projects WHERE path = ?1",
            rusqlite::params![project_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        )
        .ok()
    };

    let resolved_project_id: Option<String> = project_row.as_ref().map(|r| r.0.clone());
    let audit = |agent: Option<&str>,
                 mode: &str,
                 decision: &str,
                 reason: Option<&str>,
                 inbox_priority: Option<&str>,
                 inbox_count: Option<i64>| {
        if let Some(pid) = resolved_project_id.as_deref() {
            let db = crate::db::shared();
            let conn = db.lock();
            let _ = HeartbeatFire::insert(
                &conn,
                pid,
                agent,
                mode,
                decision,
                reason,
                inbox_priority,
                inbox_count,
                Some(tick_start.elapsed().as_millis() as i64),
            );
        }
    };

    let mode_str = project_row
        .as_ref()
        .map(|r| r.1.clone())
        .unwrap_or_else(|| "heartbeat".to_string());

    // Gate 1: workspace-level state. Locked workspaces halt all agents.
    if let Some(ws_state) = get_workspace_state(&project_path) {
        if ws_state.heartbeat == 0 {
            audit(
                None,
                &mode_str,
                "skipped_locked",
                Some("workspace state has heartbeat=0"),
                None,
                None,
            );
            return Ok(vec![]);
        }
    }

    // Gate 2: project-level schedule.
    if let Some((project_id, mode, schedule, last_fire)) = project_row.clone() {
        if mode == "off" {
            audit(None, &mode, "skipped_schedule", Some("heartbeat_mode=off"), None, None);
            return Ok(vec![]);
        }
        if mode == "scheduled" || mode == "hourly" {
            if !should_project_fire(&mode, schedule.as_deref(), last_fire.as_deref()) {
                audit(
                    None,
                    &mode,
                    "skipped_schedule",
                    Some("schedule window not open"),
                    None,
                    None,
                );
                return Ok(vec![]);
            }
            let db = crate::db::shared();
            let conn = db.lock();
            let _ = conn.execute(
                "UPDATE projects SET heartbeat_last_fire = ?1 WHERE id = ?2",
                rusqlite::params![chrono::Local::now().to_rfc3339(), project_id],
            );
        }
    }

    let mut launchable = Vec::new();
    let now = chrono::Utc::now();

    // Step 1: workspace inbox → __lead__
    let ws_inbox = workspace_inbox_dir(&project_path);
    let ws_inbox_count = if ws_inbox.exists() {
        fs::read_dir(&ws_inbox)
            .map(|e| {
                e.flatten()
                    .filter(|e| e.path().extension().map_or(false, |ext| ext == "md"))
                    .count() as i64
            })
            .unwrap_or(0)
    } else {
        0
    };
    let has_workspace_inbox = ws_inbox_count > 0;

    if has_workspace_inbox {
        if is_agent_locked(&project_path, "__lead__") {
            audit(
                Some("__lead__"),
                &mode_str,
                "skipped_locked",
                Some("lead already running"),
                None,
                Some(ws_inbox_count),
            );
        } else {
            launchable.push("__lead__".to_string());
            audit(
                Some("__lead__"),
                &mode_str,
                "fired",
                Some("workspace inbox has items"),
                None,
                Some(ws_inbox_count),
            );
        }
    }

    // Step 2: per-agent evaluation
    let dir = agents_dir(&project_path);
    if !dir.exists() {
        return Ok(launchable);
    }

    if let Ok(entries) = fs::read_dir(&dir) {
        for entry in entries.flatten() {
            if !entry.file_type().map_or(false, |ft| ft.is_dir()) {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();

            if is_agent_locked(&project_path, &name) {
                audit(
                    Some(&name),
                    &mode_str,
                    "skipped_locked",
                    Some("agent is already running"),
                    None,
                    None,
                );
                continue;
            }

            // Safety: skip agents whose terminal is being used interactively.
            if let Some(ref pid) = resolved_project_id {
                let db = crate::db::shared();
                let conn = db.lock();
                if let Ok(Some(session)) = AgentSession::get_by_agent(&conn, pid, &name) {
                    if session.owner == "user" && session.status == "running" {
                        audit(
                            Some(&name),
                            &mode_str,
                            "skipped_user_session",
                            Some("user is driving this agent's terminal"),
                            None,
                            None,
                        );
                        continue;
                    }
                }
            }

            let agent_md = entry.path().join("AGENT.md");
            let agent_type = if agent_md.exists() {
                let content = fs::read_to_string(&agent_md).unwrap_or_default();
                let fm = parse_frontmatter(&content);
                fm.get("type")
                    .cloned()
                    .unwrap_or_else(|| "agent-template".to_string())
            } else {
                "agent-template".to_string()
            };

            if agent_type == "custom" {
                let config = read_heartbeat_config(&project_path, &name);

                if config.mode == "persistent" {
                    audit(
                        Some(&name),
                        &mode_str,
                        "skipped_custom_timing",
                        Some("persistent mode — always running"),
                        None,
                        None,
                    );
                    continue;
                }

                if let Some(ref hours) = config.active_hours {
                    if !is_within_active_hours(hours, &now) {
                        audit(
                            Some(&name),
                            &mode_str,
                            "skipped_custom_timing",
                            Some("outside active hours"),
                            None,
                            None,
                        );
                        continue;
                    }
                }

                let should_wake = match &config.next_wake {
                    Some(next) => chrono::DateTime::parse_from_rfc3339(next)
                        .map(|t| now >= t)
                        .unwrap_or(true),
                    None => true,
                };

                if should_wake {
                    let mut updated = config.clone();
                    updated.last_wake = Some(now.to_rfc3339());
                    updated.next_wake = Some(
                        (now + chrono::Duration::seconds(updated.interval_seconds as i64))
                            .to_rfc3339(),
                    );
                    let _ = write_heartbeat_config(&project_path, &name, &updated);
                    audit(
                        Some(&name),
                        &mode_str,
                        "fired",
                        Some(&format!(
                            "custom agent next_wake elapsed (interval {}s)",
                            updated.interval_seconds
                        )),
                        None,
                        None,
                    );
                    launchable.push(name);
                } else {
                    audit(
                        Some(&name),
                        &mode_str,
                        "skipped_custom_timing",
                        Some("next_wake not elapsed"),
                        None,
                        None,
                    );
                }
            } else {
                // Manager / coordinator / agent-template / k2so: inbox-based.
                let inbox = agent_work_dir(&project_path, &name, "inbox");
                let inbox_count = if inbox.exists() {
                    fs::read_dir(&inbox)
                        .map(|e| {
                            e.flatten()
                                .filter(|e| {
                                    e.path().extension().map_or(false, |ext| ext == "md")
                                })
                                .count() as i64
                        })
                        .unwrap_or(0)
                } else {
                    0
                };

                if inbox_count == 0 {
                    audit(Some(&name), &mode_str, "no_work", Some("empty inbox"), None, Some(0));
                    continue;
                }

                let highest_prio = get_highest_inbox_priority(&project_path, &name);
                let prio_label = priority_label(highest_prio);

                // Quality gate: skip low-priority inbox when active work is in progress.
                let active_count = count_md_files(&agent_work_dir(&project_path, &name, "active"));
                if active_count > 0 && highest_prio > priority_rank("high") {
                    audit(
                        Some(&name),
                        &mode_str,
                        "skipped_quality_gate",
                        Some(&format!("active work in progress, inbox only {}", prio_label)),
                        Some(prio_label),
                        Some(inbox_count),
                    );
                    continue;
                }
                audit(
                    Some(&name),
                    &mode_str,
                    "fired",
                    Some(&format!("inbox has items at priority {}", prio_label)),
                    Some(prio_label),
                    Some(inbox_count),
                );
                launchable.push(name);
            }
        }
    }

    // Sort by highest-priority inbox item (critical > high > normal > low).
    // __lead__ always sorts first if present.
    launchable.sort_by(|a, b| {
        if a == "__lead__" {
            return std::cmp::Ordering::Less;
        }
        if b == "__lead__" {
            return std::cmp::Ordering::Greater;
        }
        let prio_a = get_highest_inbox_priority(&project_path, a);
        let prio_b = get_highest_inbox_priority(&project_path, b);
        prio_a.cmp(&prio_b)
    });

    Ok(launchable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority_rank_standard_labels() {
        assert_eq!(priority_rank("critical"), 0);
        assert_eq!(priority_rank("high"), 1);
        assert_eq!(priority_rank("normal"), 2);
        assert_eq!(priority_rank("low"), 3);
        assert_eq!(priority_rank("bogus"), 2); // default-to-normal
    }

    #[test]
    fn priority_label_inverts_rank() {
        assert_eq!(priority_label(0), "critical");
        assert_eq!(priority_label(1), "high");
        assert_eq!(priority_label(2), "normal");
        assert_eq!(priority_label(3), "low");
        assert_eq!(priority_label(99), "low");
    }

    #[test]
    fn active_hours_same_day_window() {
        let hours = ActiveHours {
            start: "09:00".into(),
            end: "17:00".into(),
            timezone: "America/Los_Angeles".into(),
        };
        // We can't freeze chrono::Local::now from here without a heavier
        // scaffold, but we can at least exercise malformed input.
        let bad = ActiveHours {
            start: "bogus".into(),
            end: "17:00".into(),
            timezone: "UTC".into(),
        };
        let _ = is_within_active_hours(&hours, &chrono::Utc::now());
        assert!(is_within_active_hours(&bad, &chrono::Utc::now()));
    }

    #[test]
    fn count_md_files_handles_missing_dir() {
        let missing = PathBuf::from("/tmp/k2so-does-not-exist-xyz-12345");
        assert_eq!(count_md_files(&missing), 0);
    }

    #[test]
    fn agent_heartbeat_config_default_shape() {
        let c = AgentHeartbeatConfig::default();
        assert_eq!(c.mode, "heartbeat");
        assert_eq!(c.interval_seconds, 300);
        assert_eq!(c.cost_budget, "low");
        assert!(c.auto_backoff);
    }
}
