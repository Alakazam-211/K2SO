//! Helpers + route handlers shared between the Tauri app's agent-
//! lifecycle HTTP server (`src-tauri/src/agent_hooks.rs`) and the
//! k2so-daemon counterpart (`crates/k2so-daemon/src/main.rs`).
//!
//! Scope:
//! - Event-name canonicalization (`map_event_type`) + URL / query
//!   parsing.
//! - 50-slot recent-event ring buffer used by `k2so hooks status`.
//! - `AgentHookEventSink` trait + `set_sink`/`emit` ambient-singleton
//!   plumbing so both hosts' handlers emit through the same API.
//! - `handle_hook_complete` — the first migrated route handler. Any
//!   Tauri-specific HTTP parsing stays in the caller; the handler is
//!   pure logic (parses a pre-built params map, fires the event, writes
//!   to `db::shared()`).
//!
//! Both hosts call the same `handle_*` functions so the two paths can't
//! drift. The daemon's `AgentHookEventSink` impl publishes events onto
//! its `/events` WebSocket (see `crates/k2so-daemon/src/events.rs`);
//! src-tauri's routes back onto `AppHandle::emit` directly.

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

// ── Host event sink ─────────────────────────────────────────────────────
//
// Agent hooks fire 7 distinct host-facing events; enumerated here so a
// future migration can swap `app_handle.emit(...)` call sites in
// src-tauri/agent_hooks.rs for `sink.emit(HookEvent::...)` without any
// string-match typos. Matches the companion::event_sink shape: set-once
// ambient global with a silent no-op default for daemon / test contexts.

/// Canonical set of events src-tauri/agent_hooks.rs emits to the React
/// frontend. The variant names are kebab-cased in the wire format to
/// match the existing string keys the renderer already listens for.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookEvent {
    AgentLifecycle,
    AgentReply,
    SyncProjects,
    SyncSettings,
    CliTerminalSpawn,
    CliTerminalSpawnBackground,
    CliAiCommit,
    HookInjectionFailed,
}

impl HookEvent {
    /// The wire-format event name the React frontend listens for.
    pub fn event_name(&self) -> &'static str {
        match self {
            Self::AgentLifecycle => "agent:lifecycle",
            Self::AgentReply => "agent:reply",
            Self::SyncProjects => "sync:projects",
            Self::SyncSettings => "sync:settings",
            Self::CliTerminalSpawn => "cli:terminal-spawn",
            Self::CliTerminalSpawnBackground => "cli:terminal-spawn-background",
            Self::CliAiCommit => "cli:ai-commit",
            Self::HookInjectionFailed => "hook-injection-failed",
        }
    }
}

/// Abstraction for "how do agent-hook emissions reach the UI." The
/// Tauri app provides an impl that calls `AppHandle::emit`; the future
/// k2so-daemon provides one that fans out over the companion WS.
pub trait AgentHookEventSink: Send + Sync {
    fn emit(&self, event: HookEvent, payload: serde_json::Value);
}

static SINK: OnceLock<Mutex<Option<Box<dyn AgentHookEventSink>>>> = OnceLock::new();

fn sink_slot() -> &'static Mutex<Option<Box<dyn AgentHookEventSink>>> {
    SINK.get_or_init(|| Mutex::new(None))
}

/// Register the host's sink. Idempotent; last writer wins (tests).
pub fn set_sink(s: Box<dyn AgentHookEventSink>) {
    *sink_slot().lock() = Some(s);
}

/// Fire `event` through the registered sink, if any. Silent no-op if
/// unregistered — daemon smoke tests + early-startup paths don't need
/// to care.
pub fn emit(event: HookEvent, payload: serde_json::Value) {
    if let Some(s) = sink_slot().lock().as_ref() {
        s.emit(event, payload);
    }
}

// ── Recent-event ring buffer ────────────────────────────────────────────

const RECENT_EVENTS_CAP: usize = 50;

/// Past-events ring buffer used by `k2so hooks status` and friends.
/// Keyed by insertion order; newest at the back.
static RECENT_EVENTS: OnceLock<Mutex<VecDeque<RecentEvent>>> = OnceLock::new();

#[derive(Clone, Debug, serde::Serialize)]
pub struct RecentEvent {
    pub timestamp: String,
    pub raw_event: String,
    pub canonical: Option<String>,
    pub pane_id: String,
    pub tab_id: String,
    pub matched: bool,
}

fn recent_events() -> &'static Mutex<VecDeque<RecentEvent>> {
    RECENT_EVENTS.get_or_init(|| Mutex::new(VecDeque::with_capacity(RECENT_EVENTS_CAP)))
}

/// Append a new hook event to the ring buffer. Oldest entry evicted
/// when at capacity.
pub fn record_recent_event(
    raw: &str,
    canonical: Option<&str>,
    pane_id: &str,
    tab_id: &str,
) {
    let event = RecentEvent {
        timestamp: chrono::Utc::now().to_rfc3339(),
        raw_event: raw.to_string(),
        canonical: canonical.map(String::from),
        pane_id: pane_id.to_string(),
        tab_id: tab_id.to_string(),
        matched: canonical.is_some(),
    };
    let mut buf = recent_events().lock();
    if buf.len() >= RECENT_EVENTS_CAP {
        buf.pop_front();
    }
    buf.push_back(event);
}

/// Snapshot the recent events (newest last). Exposed for the CLI's
/// `k2so hooks status` probe.
pub fn get_recent_events() -> Vec<RecentEvent> {
    recent_events().lock().iter().cloned().collect()
}

/// Test helper: clear the ring buffer. Available under cfg(test) and
/// when the `test-util` feature is on so downstream crates' test
/// binaries can reset the global state between assertions.
#[cfg(any(test, feature = "test-util"))]
pub fn clear_recent_events() {
    recent_events().lock().clear();
}

/// Canonical lifecycle event name that downstream code should switch on.
/// Surfaced to the frontend via `agent:lifecycle` emits (event_type = one
/// of these strings). Variants are lifted from the CLI hook scripts and
/// third-party agents (Claude, Cursor, Gemini) — any new value goes into
/// one of the three buckets below.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentLifecycleEvent {
    pub pane_id: String,
    pub tab_id: String,
    /// One of "start" / "stop" / "permission".
    pub event_type: String,
}

/// Map a raw hook event name onto the three-bucket canonical taxonomy.
/// `None` means the event isn't one we care about.
pub fn map_event_type(raw: &str) -> Option<&'static str> {
    match raw {
        // Start events
        "Start" | "UserPromptSubmit" | "PostToolUse" | "PostToolUseFailure"
        | "BeforeAgent" | "AfterTool" | "sessionStart" | "userPromptSubmitted"
        | "postToolUse" | "beforeSubmitPrompt" => Some("start"),

        // Stop events
        "Stop" | "agent-turn-complete" | "AfterAgent" | "sessionEnd" | "stop" => {
            Some("stop")
        }

        // Permission request events
        "PermissionRequest" | "Notification" | "preToolUse"
        | "beforeShellExecution" | "beforeMCPExecution" => Some("permission"),

        _ => None,
    }
}

/// Parse the query-string from a URL like
/// `/hook/complete?paneId=...&tabId=...&eventType=...` into a map.
/// Each value is URL-decoded via [`urldecode`].
pub fn parse_query_params(url: &str) -> HashMap<String, String> {
    let mut params = HashMap::new();
    if let Some(query) = url.split('?').nth(1) {
        for pair in query.split('&') {
            if let Some((key, value)) = pair.split_once('=') {
                let decoded = urldecode(value);
                params.insert(key.to_string(), decoded);
            }
        }
    }
    params
}

// ── Route handlers ──────────────────────────────────────────────────────
//
// Each `handle_*` function takes an already-parsed params map and runs
// the pure business logic for one route. Token auth happens in the
// calling HTTP layer (daemon or src-tauri) so these stay protocol-
// agnostic. Hosts wrap the return with HTTP serialization.

/// POST a canonicalized lifecycle event to the hook sink + update the
/// `agent_sessions` row keyed by `pane_id` (= terminal_id = the
/// `K2SO_PANE_ID` env var the PTY is spawned with).
///
/// Side effects:
/// - Appends to the recent-events ring buffer.
/// - Emits `HookEvent::AgentLifecycle` via the registered sink (Tauri
///   bus or daemon WS broadcast).
/// - If the mapped canonical is `start`/`stop`/`permission` and the
///   `agent_sessions` row has a different status, updates the row so
///   the scheduler's `is_agent_locked` check reflects reality.
///
/// Returns the JSON response body (`{"success":true}`) so the HTTP
/// layer doesn't need to know the wire format. Always 200; no error
/// paths — unknown event types are recorded in the ring buffer with
/// `canonical = None` and otherwise ignored.
pub fn handle_hook_complete(params: &HashMap<String, String>) -> &'static str {
    let pane_id = params.get("paneId").cloned().unwrap_or_default();
    let tab_id = params.get("tabId").cloned().unwrap_or_default();
    let raw_event = params.get("eventType").cloned().unwrap_or_default();

    let canonical_opt = map_event_type(&raw_event);
    record_recent_event(&raw_event, canonical_opt, &pane_id, &tab_id);

    if let Some(canonical) = canonical_opt {
        let event = AgentLifecycleEvent {
            pane_id: pane_id.clone(),
            tab_id: tab_id.clone(),
            event_type: canonical.to_string(),
        };

        crate::log_debug!(
            "[agent-hooks] {} → {} (pane={}, tab={})",
            raw_event,
            canonical,
            pane_id,
            tab_id
        );
        emit(
            HookEvent::AgentLifecycle,
            serde_json::to_value(&event).unwrap_or(serde_json::Value::Null),
        );

        // Sync AgentSession.status so the scheduler's is_agent_locked
        // check reflects reality. Without this, a single wake leaves
        // status='running' forever and every subsequent heartbeat
        // silently skips the agent. Pane_id is the K2SO_PANE_ID env
        // var we set at PTY creation.
        let new_status: Option<&str> = match canonical {
            "start" => Some("running"),
            "stop" => Some("sleeping"),
            "permission" => Some("permission"),
            _ => None,
        };
        if let Some(new_status) = new_status {
            let db = crate::db::shared();
            let conn = db.lock();
            if let Ok(Some(s)) =
                crate::db::schema::AgentSession::get_by_terminal_id(&conn, &pane_id)
            {
                if s.status != new_status {
                    let _ = crate::db::schema::AgentSession::update_status(
                        &conn,
                        &s.project_id,
                        &s.agent_name,
                        new_status,
                    );
                }
            }
        }
    }

    r#"{"success":true}"#
}

/// Percent-decode a URL-encoded string. Handles multi-byte UTF-8 sequences
/// (e.g. `%E2%80%94` → `—`) by decoding bytes into a buffer first, then
/// converting to UTF-8.
pub fn urldecode(s: &str) -> String {
    let mut bytes = Vec::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '%' {
            let hex: String = chars.by_ref().take(2).collect();
            if hex.len() == 2 && hex.chars().all(|c| c.is_ascii_hexdigit()) {
                if let Ok(byte) = u8::from_str_radix(&hex, 16) {
                    bytes.push(byte);
                } else {
                    bytes.push(b'%');
                    bytes.extend_from_slice(hex.as_bytes());
                }
            } else {
                bytes.push(b'%');
                bytes.extend_from_slice(hex.as_bytes());
            }
        } else if c == '+' {
            bytes.push(b' ');
        } else {
            let mut buf = [0u8; 4];
            let encoded = c.encode_utf8(&mut buf);
            bytes.extend_from_slice(encoded.as_bytes());
        }
    }
    String::from_utf8(bytes).unwrap_or_else(|e| {
        String::from_utf8_lossy(e.into_bytes().as_slice()).into_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex as PLMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    static TEST_LOCK: PLMutex<()> = PLMutex::new(());

    #[test]
    fn hook_event_name_matches_wire_format() {
        assert_eq!(HookEvent::AgentLifecycle.event_name(), "agent:lifecycle");
        assert_eq!(HookEvent::AgentReply.event_name(), "agent:reply");
        assert_eq!(HookEvent::SyncProjects.event_name(), "sync:projects");
        assert_eq!(HookEvent::SyncSettings.event_name(), "sync:settings");
        assert_eq!(
            HookEvent::CliTerminalSpawn.event_name(),
            "cli:terminal-spawn"
        );
        assert_eq!(
            HookEvent::CliTerminalSpawnBackground.event_name(),
            "cli:terminal-spawn-background"
        );
        assert_eq!(HookEvent::CliAiCommit.event_name(), "cli:ai-commit");
    }

    #[test]
    fn emit_without_sink_is_silent_noop() {
        let _g = TEST_LOCK.lock();
        *sink_slot().lock() = None;
        emit(HookEvent::AgentLifecycle, serde_json::json!({}));
    }

    #[test]
    fn registered_sink_receives_emit() {
        let _g = TEST_LOCK.lock();
        let count = Arc::new(AtomicUsize::new(0));
        struct Fake(Arc<AtomicUsize>);
        impl AgentHookEventSink for Fake {
            fn emit(&self, _e: HookEvent, _p: serde_json::Value) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        set_sink(Box::new(Fake(count.clone())));
        emit(HookEvent::SyncProjects, serde_json::json!({}));
        emit(HookEvent::AgentReply, serde_json::json!({"x": 1}));
        assert_eq!(count.load(Ordering::SeqCst), 2);
        *sink_slot().lock() = None;
    }

    #[test]
    fn map_event_type_buckets() {
        assert_eq!(map_event_type("Start"), Some("start"));
        assert_eq!(map_event_type("UserPromptSubmit"), Some("start"));
        assert_eq!(map_event_type("Stop"), Some("stop"));
        assert_eq!(map_event_type("agent-turn-complete"), Some("stop"));
        assert_eq!(map_event_type("PermissionRequest"), Some("permission"));
        assert_eq!(map_event_type("beforeShellExecution"), Some("permission"));
        assert_eq!(map_event_type("unknown"), None);
    }

    #[test]
    fn parse_query_params_basic() {
        let params = parse_query_params("/hook/complete?paneId=t-1&tabId=x&eventType=Start");
        assert_eq!(params.get("paneId"), Some(&"t-1".to_string()));
        assert_eq!(params.get("tabId"), Some(&"x".to_string()));
        assert_eq!(params.get("eventType"), Some(&"Start".to_string()));
    }

    #[test]
    fn parse_query_params_url_decodes_values() {
        let params = parse_query_params("/hook?message=hello%20world&symbol=%E2%80%94");
        assert_eq!(params.get("message"), Some(&"hello world".to_string()));
        assert_eq!(params.get("symbol"), Some(&"—".to_string()));
    }

    #[test]
    fn parse_query_params_no_query_is_empty() {
        let params = parse_query_params("/hook/status");
        assert!(params.is_empty());
    }

    #[test]
    fn urldecode_handles_multibyte_utf8() {
        // "café" with é = %C3%A9
        assert_eq!(urldecode("caf%C3%A9"), "café");
        // em dash
        assert_eq!(urldecode("a%E2%80%94b"), "a—b");
    }

    #[test]
    fn urldecode_preserves_invalid_percent_sequences() {
        // "%GG" is not a valid hex escape — kept as-is.
        assert_eq!(urldecode("a%GGb"), "a%GGb");
    }

    #[test]
    fn urldecode_plus_becomes_space() {
        assert_eq!(urldecode("hello+world"), "hello world");
    }

    #[test]
    fn recent_events_ring_buffer_fifo_eviction() {
        let _g = TEST_LOCK.lock();
        // Clear between tests.
        recent_events().lock().clear();

        for i in 0..(RECENT_EVENTS_CAP + 5) {
            record_recent_event(&format!("Event{i}"), Some("start"), "pane", "tab");
        }

        let events = get_recent_events();
        assert_eq!(events.len(), RECENT_EVENTS_CAP);
        // Oldest 5 should be evicted; first retained is Event5.
        assert_eq!(events.first().unwrap().raw_event, "Event5");
        // Newest at the back.
        assert_eq!(
            events.last().unwrap().raw_event,
            format!("Event{}", RECENT_EVENTS_CAP + 4)
        );
    }

    #[test]
    fn handle_hook_complete_records_and_emits_on_known_event() {
        let _g = TEST_LOCK.lock();
        recent_events().lock().clear();

        let captured: Arc<PLMutex<Vec<(String, serde_json::Value)>>> =
            Arc::new(PLMutex::new(Vec::new()));
        struct Fake(Arc<PLMutex<Vec<(String, serde_json::Value)>>>);
        impl AgentHookEventSink for Fake {
            fn emit(&self, event: HookEvent, payload: serde_json::Value) {
                self.0.lock().push((event.event_name().to_string(), payload));
            }
        }
        set_sink(Box::new(Fake(captured.clone())));

        let params: HashMap<String, String> = [
            ("paneId", "pane-9"),
            ("tabId", "tab-9"),
            ("eventType", "UserPromptSubmit"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();

        let body = handle_hook_complete(&params);
        assert_eq!(body, r#"{"success":true}"#);

        // Emitted exactly once with canonical "start".
        let events = captured.lock();
        assert_eq!(events.len(), 1, "expected one emit, got {events:?}");
        assert_eq!(events[0].0, "agent:lifecycle");
        assert_eq!(events[0].1["eventType"], "start");
        assert_eq!(events[0].1["paneId"], "pane-9");
        drop(events);

        // Ring buffer captured the raw name + canonical.
        let recent = get_recent_events();
        let last = recent.last().expect("ring buffer has entry");
        assert_eq!(last.raw_event, "UserPromptSubmit");
        assert_eq!(last.canonical.as_deref(), Some("start"));
        assert!(last.matched);

        *sink_slot().lock() = None;
    }

    #[test]
    fn handle_hook_complete_ignores_unknown_event_type_but_records() {
        let _g = TEST_LOCK.lock();
        recent_events().lock().clear();

        let captured: Arc<AtomicUsize> = Arc::new(AtomicUsize::new(0));
        struct Fake(Arc<AtomicUsize>);
        impl AgentHookEventSink for Fake {
            fn emit(&self, _e: HookEvent, _p: serde_json::Value) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        set_sink(Box::new(Fake(captured.clone())));

        let params: HashMap<String, String> = [
            ("paneId", "p"),
            ("tabId", "t"),
            ("eventType", "MadeUpEvent"),
        ]
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();

        let body = handle_hook_complete(&params);
        assert_eq!(body, r#"{"success":true}"#);
        assert_eq!(
            captured.load(Ordering::SeqCst),
            0,
            "unknown event should not emit"
        );

        // But ring buffer still holds the raw observation (matched=false).
        let recent = get_recent_events();
        let last = recent.last().expect("ring buffer has entry");
        assert_eq!(last.raw_event, "MadeUpEvent");
        assert_eq!(last.canonical, None);
        assert!(!last.matched);

        *sink_slot().lock() = None;
    }

    #[test]
    fn recent_event_records_canonical_and_matched() {
        let _g = TEST_LOCK.lock();
        recent_events().lock().clear();
        record_recent_event("Start", Some("start"), "p1", "t1");
        record_recent_event("GarbageEvent", None, "p2", "t2");
        let events = get_recent_events();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].canonical.as_deref(), Some("start"));
        assert!(events[0].matched);
        assert_eq!(events[1].canonical, None);
        assert!(!events[1].matched);
    }
}
