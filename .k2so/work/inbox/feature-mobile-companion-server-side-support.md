---
title: "Feature: Mobile Companion Server-Side Support"
priority: high
assigned_by: user
created: 2026-04-07
type: feature
source: manual
---

## Description

Add server-side support for the K2SO Mobile Companion app. This enables users to monitor and interact with their AI agents from their phone via a native mobile app (built separately in the K2SO-companion repo).

The feature adds a new **Settings page** ("Mobile Companion") and a new Rust module that exposes a curated subset of K2SO's HTTP API through an ngrok tunnel with username/password authentication.

## Why

Users need to interact with running agents without being at their laptop. The killer use case is sending messages to PTY LLM sessions from mobile to keep agents progressing. There is no cloud service layer — K2SO exposes itself directly via ngrok.

## New Settings Page: "Mobile Companion"

Add a new page in K2SO's settings UI with:

- **Enable/Disable toggle** — starts/stops the ngrok tunnel
- **Username field** — required when enabling
- **Password field** — required when enabling (hashed with argon2 before storage)
- **ngrok Auth Token field** — user's ngrok token (stored encrypted in settings)
- **ngrok URL display** — shows the generated URL with a copy button (read-only, visible when enabled)
- **Connected clients count** — shows how many mobile devices are currently connected
- **Active sessions list** — shows connected devices with "Disconnect" option per session

## New Rust Module: `companion.rs`

### Dependencies (add to Cargo.toml)
- `ngrok = "0.14"` — Rust SDK for programmatic ngrok tunnels
- `tungstenite = "0.21"` — WebSocket support
- `argon2 = "0.5"` — password hashing

### Architecture

The companion module is a **reverse proxy** that sits in front of K2SO's existing HTTP server. It does NOT modify `agent_hooks.rs`. Instead it:

1. Starts an ngrok HTTP tunnel using the user's ngrok auth token
2. Accepts incoming connections from ngrok
3. Validates auth (Basic Auth for login, Bearer session token for subsequent requests)
4. Maps companion endpoints to internal `/cli/*` routes
5. Proxies validated requests to `127.0.0.1:{HOOK_PORT}` (rewriting POST bodies to GET query params)
6. Handles WebSocket upgrades for real-time push

### State

```rust
pub struct CompanionState {
    pub enabled: AtomicBool,
    pub ngrok_url: RwLock<Option<String>>,
    pub sessions: RwLock<HashMap<String, SessionInfo>>,  // session_token -> info
    pub ws_clients: RwLock<Vec<WebSocketSender>>,
    pub username: String,
    pub password_hash: String,  // argon2 hashed
}
```

Add to `AppState` in `state.rs`:
```rust
pub companion: Mutex<Option<CompanionState>>,
```

### Tauri Commands (new file: `commands/companion.rs`)

- `companion_enable(username, password, ngrok_token)` — starts tunnel, returns ngrok URL
- `companion_disable()` — tears down tunnel, invalidates all sessions
- `companion_status()` — returns { enabled, ngrok_url, connected_clients_count }
- `companion_set_credentials(username, password)` — updates credentials, invalidates sessions

### API Endpoints (exposed through ngrok)

| Companion Endpoint | Maps to Internal Route | Method | Purpose |
|---|---|---|---|
| `/companion/auth` | New | POST | Login with Basic Auth, returns session token |
| `/companion/ws` | New | GET | WebSocket for real-time push events |
| `/companion/agents` | `/cli/agents/list` | GET | List all agents with status |
| `/companion/agents/running` | `/cli/agents/running` | GET | Running terminal sessions |
| `/companion/agents/work` | `/cli/agents/work` | GET | Agent work items |
| `/companion/reviews` | `/cli/reviews` | GET | Pending review queue |
| `/companion/review/approve` | `/cli/review/approve` | POST | Approve agent work |
| `/companion/review/reject` | `/cli/review/reject` | POST | Reject agent work |
| `/companion/review/feedback` | `/cli/review/feedback` | POST | Request changes with message |
| `/companion/terminal/read` | `/cli/terminal/read` | GET | Read terminal buffer |
| `/companion/terminal/write` | `/cli/terminal/write` | POST | Send input to PTY |
| `/companion/status` | `/cli/status` | GET | Workspace dashboard status |
| `/companion/agents/wake` | `/cli/agents/launch` | POST | Wake an idle agent — launches a terminal session for the named agent, picks work from its inbox |

**Auth flow:**
1. Mobile sends `Authorization: Basic base64(user:pass)` to `/companion/auth`
2. K2SO validates, returns `{ "token": "random-hex-string", "expires_at": "..." }`
3. All subsequent requests use `Authorization: Bearer {token}`
4. Tokens expire after 24 hours

**Security:**
- Only curated endpoints exposed (no destructive operations like agent delete)
- POST for mutations (not GET with query params like internal API)
- JSON request/response bodies with consistent envelope: `{ "ok": bool, "data": ..., "error": ... }`
- Rate limiting: 60 requests/minute
- Password hashed with argon2, never stored in plaintext

### WebSocket Events

Hook into existing `app_handle.emit()` calls in `agent_hooks.rs` to also broadcast to WebSocket clients:

- `agent:lifecycle` — agent start/stop/permission events
- `agent:reply` — agent sends a message back
- `terminal:output` — new terminal output for subscribed terminals (server-side polling of `read_lines()` every 500ms, diff against previous snapshot, push only new lines)
- `sync:projects` — workspace state changes

Mobile clients subscribe to specific terminals by sending `{ "type": "subscribe", "terminalId": "..." }` over the WebSocket.

### Frontend Settings Panel

New component in K2SO's React frontend (likely `src/renderer/components/Settings/CompanionSettings.tsx` or as a new section in the existing settings UI):

- Wired to the Tauri commands above
- ngrok URL displayed with copy-to-clipboard button
- Visual indicator when companion is active (could also show in the status bar)

## Acceptance Criteria

- [ ] New "Mobile Companion" settings page accessible from K2SO settings
- [ ] User can set username, password, and ngrok auth token
- [ ] Enabling starts an ngrok tunnel and displays the public URL
- [ ] Disabling tears down the tunnel and invalidates all sessions
- [ ] All 13 companion endpoints work correctly through the ngrok tunnel (including /companion/agents/wake)
- [ ] WebSocket connection works through ngrok with real-time event push
- [ ] Terminal read/write works through the companion API (chat feature)
- [ ] Password is hashed with argon2 before storage
- [ ] Rate limiting is enforced
- [ ] Connected clients count is displayed in settings
