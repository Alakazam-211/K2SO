//! 0.38.0 Commit 4 — daemon-owned session lifecycle broadcast.
//!
//! Push-driven counterpart to `/cli/sessions/list-for-workspace`.
//! Every `v2_session_map::register` / `unregister` call publishes a
//! `SessionEvent` onto a process-wide `tokio::sync::broadcast` channel.
//! `session_events_ws::serve_session_events_connection` fans those
//! events out to each `/cli/sessions/events?path=<workspace>` WS
//! subscriber, filtering by `cwd starts_with workspace_path` (same
//! rule the existing list-for-workspace endpoint uses in `cli.rs`).
//!
//! Why a broadcast bus instead of per-subscriber polling: the
//! renderer used to discover new daemon-owned PTYs only on workspace
//! switch (`loadLayoutForWorkspace` triggers `reconcileWithDaemon`).
//! That left every other connected window stale until the user
//! manually re-opened the workspace. Push-driven adoption + drop
//! means Cmd+T in window A surfaces the new tab in window B + the
//! mobile companion within one network hop.
//!
//! Initialization: a `OnceLock<Sender<SessionEvent>>` lazily creates
//! the channel on first `sender()` call. Test environments that
//! never call into `register`/`unregister` won't initialize it;
//! emit sites use `let _ = ` to ignore the case where the bus has
//! no live subscribers (broadcast::send returns an Err in that
//! case but we treat it as a no-op).

use std::sync::OnceLock;

use serde::Serialize;
use tokio::sync::broadcast;

/// Capacity for the broadcast channel. Small enough that a stalled
/// subscriber doesn't bloat memory, large enough that bursts (e.g.
/// opening 10 tabs back-to-back) don't drop events under realistic
/// network jitter. Lagged subscribers get `RecvError::Lagged` and
/// the WS handler in `session_events_ws.rs` treats that as a hint
/// to re-emit the current truth via a fresh snapshot.
pub const EVENT_CHANNEL_CAP: usize = 256;

/// One lifecycle event for a daemon-owned PTY session. Serialized
/// as JSON and pushed to every matching WS subscriber. Discriminated
/// by `kind` so callers can switch on the variant cheaply.
///
/// **Wire stability:** the renderer + mobile companion both consume
/// this shape. Field renames are breaking. Adding new variants is
/// safe — clients should ignore unknown `kind` values.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SessionEvent {
    /// A daemon-owned PTY session has been registered in the v2 map.
    /// Emitted from `v2_session_map::register`. `agent_name` is the
    /// canonical key under which the session was registered (e.g.
    /// `tab-<paneGroupId>` for Cmd+T tabs, bare project UUID for
    /// pinned chat, ad-hoc shapes for heartbeats).
    SessionAdded {
        /// Absolute cwd of the spawned PTY. Subscribers filter on
        /// `cwd starts_with subscriber's workspace_path` before
        /// forwarding to their socket.
        workspace_path: String,
        /// `tab-<paneGroupId>` extracted from `agent_name` when the
        /// prefix matches; absent otherwise (pinned chat, heartbeat).
        /// The renderer uses this as the canonical paneGroup id when
        /// adopting the session as a new tab.
        pane_group_id: Option<String>,
        /// Canonical `v2_session_map` key. The renderer filters on
        /// the `tab-` prefix to decide whether to surface the event
        /// as a tab; non-tab agent_names are forwarded so the mobile
        /// companion sees the full session inventory.
        agent_name: String,
        /// Spawn command. May be `null` when the daemon spawned the
        /// user's default login shell.
        command: Option<String>,
        /// Spawn args. Empty when none.
        args: Vec<String>,
        /// Daemon-side session UUID. Mirrors
        /// `list-for-workspace`'s `sessionId` field.
        session_id: String,
        /// Always `true` for v2 sessions today. Reserved for the
        /// post-cleanup phase that may add legacy events.
        #[serde(rename = "isV2")]
        is_v2: bool,
    },
    /// A session has been removed from the v2 map. Emitted from
    /// `v2_session_map::unregister` (which is the single chokepoint
    /// for "session goes away" — child exit observer + explicit
    /// close route + watchdog escalation all funnel through it).
    SessionRemoved {
        workspace_path: String,
        pane_group_id: Option<String>,
        agent_name: String,
    },
    /// Session label/title changed. Reserved variant — currently
    /// unused on the emit side because the daemon's existing label
    /// broadcast (`sessions_grid_ws::Outbound::LabelChanged`) is
    /// already wired into the per-session WS. Kept in the schema
    /// so a future caller (mobile companion: "what's the current
    /// title?") doesn't break wire compat.
    #[allow(dead_code)]
    SessionRenamed {
        workspace_path: String,
        pane_group_id: Option<String>,
        title: String,
    },
}

static SENDER: OnceLock<broadcast::Sender<SessionEvent>> = OnceLock::new();

/// Lazy accessor for the broadcast sender. First caller creates the
/// channel. Cheap — the OnceLock is a single atomic load on the hot
/// path.
pub fn sender() -> &'static broadcast::Sender<SessionEvent> {
    SENDER.get_or_init(|| broadcast::channel(EVENT_CHANNEL_CAP).0)
}

/// Take a fresh `Receiver`. One per WS subscriber. Drops cleanly
/// when the subscriber disconnects.
pub fn subscribe() -> broadcast::Receiver<SessionEvent> {
    sender().subscribe()
}

/// Best-effort emit. Returns `Ok(n)` with the number of subscribers
/// the event was queued for; `Err` when there are zero subscribers
/// (treated as a no-op by callers via `let _ =`). The emit cost is
/// bounded — broadcast doesn't synchronize beyond a single atomic
/// per subscriber.
pub fn emit(event: SessionEvent) -> Result<usize, broadcast::error::SendError<SessionEvent>> {
    sender().send(event)
}

/// Extract `tab-<paneGroupId>` from a `v2_session_map` agent_name.
/// Returns `None` for non-tab-shaped keys (pinned chat = bare UUID,
/// heartbeats = bare workspace key, legacy `<pid>:<agent>` shapes).
/// The renderer uses this to decide whether to surface the event as
/// a new tab — non-tab events are forwarded so the mobile companion
/// can see the full inventory without the renderer adopting them.
pub fn pane_group_id_from_agent(agent_name: &str) -> Option<String> {
    agent_name
        .strip_prefix("tab-")
        .map(|rest| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_group_id_extracts_tab_prefix() {
        assert_eq!(
            pane_group_id_from_agent("tab-abc123"),
            Some("abc123".to_string()),
        );
        assert_eq!(
            pane_group_id_from_agent("tab-uuid-with-hyphens-here"),
            Some("uuid-with-hyphens-here".to_string()),
        );
    }

    #[test]
    fn pane_group_id_is_none_for_non_tab_agents() {
        assert_eq!(pane_group_id_from_agent(""), None);
        assert_eq!(
            pane_group_id_from_agent("c3c5b9d6-e123-4456-89ab-cdef01234567"),
            None,
        );
        assert_eq!(pane_group_id_from_agent("__lead__"), None);
        assert_eq!(
            pane_group_id_from_agent("project-id:agent-name"),
            None,
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn emit_with_no_subscribers_is_a_no_op() {
        // Fresh OnceLock is fine for this test only because no other
        // subscribers exist in this process at this moment. Best-effort
        // assertion: emit returns Err (zero subscribers) and we don't
        // care.
        let res = emit(SessionEvent::SessionRemoved {
            workspace_path: "/tmp".into(),
            pane_group_id: None,
            agent_name: "test-no-subs".into(),
        });
        // Either Err (no subs) or Ok(0+) — both fine; emitter callers
        // never inspect the result.
        let _ = res;
    }

    #[tokio::test(flavor = "current_thread")]
    async fn subscriber_receives_emitted_event() {
        // The broadcast channel is process-global, so concurrent
        // tests (especially `register`/`unregister` integrations on
        // a different test thread) can leak events into our
        // receiver. Use a unique session_id as a probe and drain
        // events until we find the one we emitted — anything else
        // is contamination and gets ignored.
        let probe_id = format!(
            "test-probe-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0),
        );
        let mut rx = subscribe();
        let _ = emit(SessionEvent::SessionAdded {
            workspace_path: "/x/foo".into(),
            pane_group_id: Some("pg-1".into()),
            agent_name: "tab-pg-1".into(),
            command: Some("zsh".into()),
            args: vec![],
            session_id: probe_id.clone(),
            is_v2: true,
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            if remaining.is_zero() {
                panic!("did not receive probe event in time");
            }
            let got = match tokio::time::timeout(remaining, rx.recv()).await {
                Ok(Ok(ev)) => ev,
                Ok(Err(_)) => panic!("receiver closed"),
                Err(_) => panic!("timed out waiting for probe event"),
            };
            if let SessionEvent::SessionAdded { ref session_id, .. } = got {
                if session_id == &probe_id {
                    match got {
                        SessionEvent::SessionAdded { workspace_path, pane_group_id, agent_name, .. } => {
                            assert_eq!(workspace_path, "/x/foo");
                            assert_eq!(pane_group_id.as_deref(), Some("pg-1"));
                            assert_eq!(agent_name, "tab-pg-1");
                        }
                        _ => unreachable!(),
                    }
                    break;
                }
            }
            // Otherwise: contamination from another test — keep
            // draining.
        }
    }

    #[test]
    fn workspace_path_filter_rules_match_cli_endpoint() {
        // Document the filter rule in a test. The actual filter is
        // applied in session_events_ws.rs's per-subscriber loop;
        // this test just pins the rule.
        fn matches(cwd: &str, workspace_path: &str) -> bool {
            let trimmed = workspace_path.trim_end_matches('/');
            let prefix_with_slash = if trimmed.is_empty() {
                "/".to_string()
            } else {
                format!("{}/", trimmed)
            };
            let cwd_trim = cwd.trim_end_matches('/');
            cwd_trim == trimmed || cwd.starts_with(&prefix_with_slash)
        }
        assert!(matches("/x/K2SO", "/x/K2SO"));
        assert!(matches("/x/K2SO/sub", "/x/K2SO"));
        assert!(matches("/x/K2SO/sub/deeper", "/x/K2SO"));
        // Sibling — must NOT match (the exact bug `list-for-workspace`
        // got bitten by before).
        assert!(!matches("/x/K2SO-website", "/x/K2SO"));
        assert!(!matches("/x/other", "/x/K2SO"));
        // Trailing-slash tolerance.
        assert!(matches("/x/K2SO/", "/x/K2SO"));
        assert!(matches("/x/K2SO", "/x/K2SO/"));
    }
}
