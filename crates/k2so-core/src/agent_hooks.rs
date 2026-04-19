//! Pure helpers shared between the Tauri app's agent-lifecycle HTTP
//! server (`src-tauri/src/agent_hooks.rs`) and the future k2so-daemon
//! counterpart that will grow its own HTTP routes.
//!
//! Scope deliberately tight: event-name canonicalization, URL / query
//! parsing, and the 50-slot ring buffer of recent hook events. Any
//! Tauri-specific emission, DB writes, or command routing stays in the
//! host's server module. The daemon will import these same helpers so
//! the two paths can't drift.

use parking_lot::Mutex;
use std::collections::{HashMap, VecDeque};
use std::sync::OnceLock;

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

    static TEST_LOCK: PLMutex<()> = PLMutex::new(());

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
