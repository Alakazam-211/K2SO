use std::collections::{HashMap, HashSet};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::Instant;

/// Core state for the companion API proxy.
/// Stored in a module-level OnceLock, accessed by the proxy listener thread.
pub struct CompanionState {
    /// The public ngrok tunnel URL (e.g., "https://k2.ngrok.app")
    pub tunnel_url: Mutex<Option<String>>,
    /// Active authenticated sessions (token → session)
    pub sessions: Mutex<HashMap<String, Session>>,
    /// Connected WebSocket clients
    pub ws_clients: Mutex<Vec<WsClient>>,
    /// Shutdown signal — set to true to stop the proxy listener
    pub shutdown: AtomicBool,
    /// Internal hook server port (127.0.0.1:{port})
    pub hook_port: u16,
    /// Internal hook server auth token
    pub hook_token: String,
    /// Keeps the ngrok runtime thread alive — drop this to stop the tunnel
    pub _tunnel_keepalive: Mutex<Option<std::sync::mpsc::Sender<()>>>,
}

/// An authenticated companion session (24hr TTL).
pub struct Session {
    pub token: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub last_active: chrono::DateTime<chrono::Utc>,
    pub remote_addr: String,
    /// Rate limiting: request count in current window
    pub request_count: u32,
    /// Rate limiting: window start time
    pub window_start: Instant,
}

impl Session {
    /// Check if the session has expired.
    pub fn is_expired(&self) -> bool {
        chrono::Utc::now() > self.expires_at
    }

    /// Check rate limit (60 requests per 60 seconds).
    /// Returns true if within limit, false if exceeded.
    pub fn check_rate_limit(&mut self) -> bool {
        let now = Instant::now();
        let window_duration = std::time::Duration::from_secs(60);

        if now.duration_since(self.window_start) > window_duration {
            // Reset window
            self.window_start = now;
            self.request_count = 1;
            true
        } else if self.request_count < 60 {
            self.request_count += 1;
            true
        } else {
            false // Rate limit exceeded
        }
    }
}

/// A connected WebSocket client.
pub struct WsClient {
    /// Unique client ID (UUID)
    pub client_id: String,
    pub session_token: String,
    /// Whether this client has authenticated via the WS auth message
    pub authenticated: bool,
    pub subscribed_terminals: HashSet<String>,
    /// Mobile screen dimensions for shadow terminal reflow.
    /// If set, grid updates are reflowed to these dimensions before sending.
    pub mobile_dims: Option<(u16, u16)>, // (cols, rows)
    /// Channel to send messages to the WS writer thread
    pub sender: std::sync::mpsc::Sender<String>,
    /// Last time we received any message from this client
    pub last_seen: Instant,
}
