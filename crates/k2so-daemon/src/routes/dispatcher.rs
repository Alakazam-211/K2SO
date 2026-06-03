//! HTTP route dispatcher for the k2so-daemon binary.
//!
//! Pre-0.39.0 this entire function body lived inline in `main.rs`'s
//! `handle_connection`. Extraction is a mechanical move — every match
//! arm, every per-route comment, every method-gating guard is preserved
//! verbatim. HTTP framing helpers (`send_response`, `send_cors_preflight`,
//! `require_post`, `read_post_body`, `token_ok`, `parse_params`) live in
//! the sibling `routes::http` module; the dispatch sub-helpers
//! (`handle_archive_orphans`, `dispatch_unit6_post`) live at the bottom
//! of this file.
//!
//! `/cli/*` POST routes use `super::http::require_post` to enforce
//! method gating per [[feedback_post_only_route_guards]] memory; the
//! starts_with arms (`/cli/git/`, `/cli/states/`, `/cli/workspaces/`,
//! `/cli/focus-groups/`, `/cli/sections/`, `/cli/workspace-layouts/`,
//! `/cli/timer/`, `/cli/presets/`, `/cli/window-state/`,
//! `/cli/projects/`, `/cli/fs/`, `/cli/chat/`, `/cli/themes/`,
//! `/cli/skill-layers/`, `/cli/review-checklist/`, `/cli/inbox/`)
//! inherit the gate from the top-level `method != "GET" && !(is_post &&
//! post_allowed)` 405 short-circuit. See `feedback_post_only_route_guards`
//! for the full rationale.

use std::time::Duration;

use tokio::io::AsyncReadExt;
use tokio::net::TcpStream;

/// Outcome of [`handle_one_request`] — tells the outer keep-alive loop
/// whether to wait for the next request on this socket or tear it down.
///
/// **`KeepAlive`** — a regular HTTP response was sent and the client
/// didn't request close. Loop and serve the next request on the same
/// socket.
///
/// **`Done`** — close the socket. Reasons:
/// - WS upgrade handed off; the WS handler owned read/write semantics
///   for the lifetime of the upgraded connection.
/// - Client sent `Connection: close` in the request headers.
/// - Auth failure, malformed request, or any other early-exit path
///   (closing is the safe default — if a client is sending broken
///   requests we don't want to amplify the problem by keeping the
///   connection alive).
/// - Idle-timeout while waiting for the next request.
enum DispatchOutcome {
    /// HTTP request handled; outer loop should poll for the next one.
    KeepAlive,
    /// Close the socket — WS handoff, client close request, error, or
    /// idle timeout.
    Done,
}

/// How long to wait for the next request on an idle keep-alive socket
/// before closing it. 60 s is comfortably longer than the renderer's
/// slowest poll cycle (~30 s) so idle connections recycle without
/// hoarding fds, but short enough that abandoned sockets don't sit
/// indefinitely.
const KEEP_ALIVE_IDLE_TIMEOUT: Duration = Duration::from_secs(60);

/// Cap on requests served per TCP connection. A pathological client
/// loop can't infinitely hold a single socket open. 10 000 is plenty
/// for any sane session; at 2 s/poll that's ~5.5 hours of nonstop
/// polling on one socket before recycle.
const KEEP_ALIVE_MAX_REQUESTS: u32 = 10_000;

/// Serve one TCP connection, looping over requests on the same socket
/// (HTTP/1.1 keep-alive).
///
/// **0.39.7 (Issue #2):** pre-0.39.7 this function served exactly ONE
/// request per connection because [`super::http::send_response`] hard-
/// coded `Connection: close`. Combined with the renderer's ~12 different
/// `setInterval` HTTP polls, every fetch was a fresh TCP socket. macOS's
/// WKWebView Networking process has a soft `RLIMIT_NOFILE = 256`; over
/// ~50 min the trickle of `CLOSE_WAIT` sockets the WebView didn't
/// close fast enough filled the fd table, and the UI progressively
/// locked up.
///
/// Now: the dispatcher loops, calling [`handle_one_request`] until the
/// client sends `Connection: close`, the connection idles out
/// ([`KEEP_ALIVE_IDLE_TIMEOUT`]), or a WS handoff or fatal error closes
/// it.
///
/// `/events` (and the other WS endpoints) are handled the same way as
/// before — on the FIRST request only. After a WS handoff the loop
/// exits because the WS handler now owns read/write semantics on the
/// stream; there is no concept of "another request" on an upgraded
/// connection. The mid-keep-alive WS upgrade case is forbidden by
/// HTTP/1.1 anyway.
pub async fn dispatch(mut stream: TcpStream, state: crate::DaemonState) {
    let mut requests_served: u32 = 0;
    loop {
        let outcome = handle_one_request(&mut stream, &state).await;
        match outcome {
            DispatchOutcome::KeepAlive => {
                requests_served = requests_served.saturating_add(1);
                if requests_served >= KEEP_ALIVE_MAX_REQUESTS {
                    return;
                }
                // Continue: loop body re-enters `handle_one_request`
                // which awaits the next request (with idle timeout).
            }
            DispatchOutcome::Done => return,
        }
    }
}

/// Serve exactly one HTTP request on `stream`, returning whether the
/// outer keep-alive loop should poll for another request.
///
/// `/events` is the one exception: on a valid token we hand off to
/// [`crate::events::serve_events_connection`] which performs the
/// WebSocket upgrade via `tokio_tungstenite::accept_async` — that
/// function consumes the handshake bytes itself, so we DO NOT read
/// the request body here for that route. The WS handler now takes
/// `&mut TcpStream` so we keep ownership of the socket; on return
/// we exit with [`DispatchOutcome::Done`] because the upgraded
/// connection has no concept of a "next request."
async fn handle_one_request(
    stream: &mut TcpStream,
    state: &crate::DaemonState,
) -> DispatchOutcome {
    // Peek just the request line + headers so we can route on path
    // without consuming the body. Enough for WS handshakes (which
    // tokio-tungstenite will re-read) and the small GET bodies (which
    // have no body).
    //
    // 0.39.7: wrap the peek in an idle-timeout so a client that
    // opened a connection and went silent doesn't hold an fd forever.
    // The timeout only covers the wait-for-next-request window; once
    // bytes arrive the request is fully served without further time
    // pressure (LLM inference et al. can legitimately take tens of
    // seconds).
    let mut buf = [0u8; 4096];
    let n = match tokio::time::timeout(
        KEEP_ALIVE_IDLE_TIMEOUT,
        stream.peek(&mut buf),
    )
    .await
    {
        Ok(Ok(n)) if n > 0 => n,
        // Idle timeout, EOF (peer closed), or read error → close.
        _ => return DispatchOutcome::Done,
    };
    let req = String::from_utf8_lossy(&buf[..n]);

    // 0.39.7: parse the request's `Connection:` header. If the client
    // requested close, we still serve THIS request normally, but
    // we return `DispatchOutcome::Done` at the end instead of
    // keep-alive. HTTP/1.0 clients send `close` explicitly; HTTP/1.1
    // clients can send it to opt out of the default keep-alive.
    let client_wants_close = super::http::request_wants_close(&req);

    let first_line = req.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    let (method, path_and_query) = match parts.as_slice() {
        [m, p, ..] => (*m, *p),
        _ => {
            // Consume what we peeked so the error gets delivered.
            let _ = stream.read(&mut buf).await;
            super::http::send_response(&mut *stream, "400 Bad Request", "text/plain", "bad request\n").await;
            return DispatchOutcome::Done;
        }
    };

    // Phase 4.5: handle CORS preflight before the method allowlist.
    // The Tauri WebView origin (tauri://localhost or http://localhost:5173
    // in dev) is cross-origin relative to http://127.0.0.1:<port>, so
    // the browser sends an OPTIONS preflight before every POST. We
    // answer it with permissive CORS headers — token auth still
    // gates every real request, so `Access-Control-Allow-Origin: *`
    // adds no security risk and avoids hard-coding every possible
    // Tauri dev-server port.
    if method == "OPTIONS" {
        let _ = stream.read(&mut buf).await;
        super::http::send_cors_preflight(&mut *stream).await;
        return DispatchOutcome::Done;
    }

    // Most routes are GET. Specific POST-accepting routes are
    // allowlisted here so non-GET hits other paths get a clean 405.
    let is_post = method == "POST";
    let post_allowed = matches!(
        path_and_query.split_once('?').map(|(p, _)| p).unwrap_or(path_and_query),
        "/cli/awareness/publish"
            | "/cli/sessions/v2/spawn"
            | "/cli/sessions/v2/close"
            // Phase 2 Unit 1 — body-bearing companion control routes.
            // Password and session-token live in the body so they
            // don't end up in URL-logged form on shared/loopback
            // intermediaries.
            | "/cli/companion/set-password"
            | "/cli/companion/disconnect-session"
            // K2 Connect tunnel — mutating control routes. Method-gated
            // per-handler below (the top-level dispatch lets a GET
            // through on POST-allowlisted routes; see
            // feedback_post_only_route_guards). Status is a GET via
            // crate::cli::dispatch.
            | "/cli/tunnel/start"
            | "/cli/tunnel/stop"
            // GET reads the redacted config (tokenSet bool, never the
            // secret); POST sets token/subdomain/server. Claimed here for
            // both methods; the handler branches on `is_post`.
            | "/cli/tunnel/config"
            // Phase 2 Unit 5 — Claude Auth mutating routes. POST
            // (not GET) so they're not idempotent-cached by any
            // future proxy and so they parallel Unit 1's pattern
            // for "this writes state". The status read-side stays
            // a GET and goes through `crate::cli::dispatch`.
            | "/cli/claude-auth/refresh-now"
            | "/cli/claude-auth/install-scheduler"
            | "/cli/claude-auth/uninstall-scheduler"
            // Phase 2 Unit 2 — LLM control + chat. Chat body
            // carries the user message + workspace context. Load
            // takes a path. Download-default takes no body but is
            // a write-side operation so we accept POST for it too.
            | "/cli/llm/chat"
            | "/cli/llm/load-model"
            | "/cli/llm/download-default"
            // Phase 2 Unit 7a — settings writes. Partial settings
            // payloads live in the body; `reset` is POST so it can't
            // be reached via a stray GET / browser refresh.
            | "/cli/settings/update"
            | "/cli/settings/reset"
            // Phase 2 Unit 6 — filesystem mutations + chat history
            // mutations + theme/skill-layer/review-checklist
            // mutations. JSON bodies carry the arguments (paths,
            // file contents, source/destination tuples) so they
            // aren't URL-encoded in proxy logs.
            | "/cli/fs/search-tree"
            | "/cli/fs/write-file"
            | "/cli/fs/move"
            | "/cli/fs/copy"
            | "/cli/fs/delete"
            | "/cli/fs/rename"
            | "/cli/fs/create"
            | "/cli/fs/duplicate"
            | "/cli/fs/open-finder"
            | "/cli/fs/open-external"
            | "/cli/chat/rename"
            | "/cli/chat/toggle-pin"
            | "/cli/chat/migrate-ide"
            | "/cli/themes/create-template"
            | "/cli/themes/delete"
            | "/cli/skill-layers/create"
            | "/cli/skill-layers/delete"
            | "/cli/review-checklist/write"
            | "/cli/review-checklist/toggle"
            | "/cli/review-checklist/init"
            // Phase 2 Unit 3 — terminal PTY lifecycle. JSON-bodied
            // mutating routes; method-gated per-handler below.
            | "/cli/terminal/create"
            | "/cli/terminal/kill"
            | "/cli/terminal/resize"
            | "/cli/terminal/kill-foreground"
            | "/cli/terminal/scroll"
            | "/cli/terminal/log"
            | "/cli/terminal/lifecycle-write"
            | "/cli/terminal/set-focus"
            // Phase 2 Unit 7c — heartbeat-launchd installer + orphan-
            // agents sweep. Body-bearing writes; method-gated below.
            | "/cli/heartbeat/install-launchd"
            | "/cli/heartbeat/uninstall-launchd"
            | "/cli/heartbeat/apply-wake-scheduler"
            | "/cli/agents/archive-orphans"
            // Phase 2 Unit 4 — DB-writing routes (states / workspaces /
            // focus-groups / sections / workspace-layouts / timer /
            // presets / window-state / projects / git). JSON-bodied
            // writes — implicit method gate via the `starts_with`
            // dispatch arm in handle_connection that runs Unit 4's
            // POST dispatch. Listed explicitly here so the top-level
            // 405 guard never short-circuits them.
            | "/cli/states/create" | "/cli/states/update" | "/cli/states/delete"
            | "/cli/workspaces/create" | "/cli/workspaces/delete" | "/cli/workspaces/set-nav-visible"
            | "/cli/focus-groups/create" | "/cli/focus-groups/update" | "/cli/focus-groups/delete"
            | "/cli/focus-groups/assign" | "/cli/focus-groups/reconcile"
            | "/cli/sections/create" | "/cli/sections/update" | "/cli/sections/delete"
            | "/cli/sections/reorder" | "/cli/sections/assign"
            | "/cli/workspace-layouts/save" | "/cli/workspace-layouts/delete"
            | "/cli/timer/create" | "/cli/timer/delete"
            | "/cli/presets/create" | "/cli/presets/update" | "/cli/presets/delete"
            | "/cli/presets/reorder" | "/cli/presets/reset"
            | "/cli/window-state/set"
            | "/cli/projects/create" | "/cli/projects/update" | "/cli/projects/delete"
            | "/cli/projects/reorder" | "/cli/projects/touch-interaction"
            | "/cli/projects/touch-interaction-clear" | "/cli/projects/add-from-path"
            | "/cli/projects/add-without-git" | "/cli/projects/init-git-and-open"
            | "/cli/projects/enable-worktrees" | "/cli/projects/detect-icon"
            | "/cli/projects/set-icon" | "/cli/projects/clear-icon"
            | "/cli/projects/open-in-finder" | "/cli/projects/open-in-editor"
            | "/cli/projects/open-in-terminal" | "/cli/projects/refresh-editors"
            | "/cli/git/create-worktree" | "/cli/git/remove-worktree"
            | "/cli/git/reopen-worktree" | "/cli/git/stage" | "/cli/git/unstage"
            | "/cli/git/stage-all" | "/cli/git/commit" | "/cli/git/merge-branch"
            | "/cli/git/abort-merge" | "/cli/git/resolve" | "/cli/git/delete-branch"
            | "/cli/git/prune-worktrees"
            // Phase 2.1 — workspace inbox mutating routes (A22.1).
            // Query-string POSTs (no JSON body); dispatched via
            // `inbox_routes::dispatch_post`. The `/cli/inbox/migrate`
            // route is a one-shot helper for tests / explicit
            // re-migration triggers — daemon first-boot also auto-
            // invokes (Phase 2.1b wiring).
            | "/cli/inbox/compose"
            | "/cli/inbox/move"
            | "/cli/inbox/archive"
            | "/cli/inbox/delete"
            | "/cli/inbox/respond"
            | "/cli/inbox/migrate"
    );
    if method != "GET" && !(is_post && post_allowed) {
        let _ = stream.read(&mut buf).await;
        super::http::send_response(
            &mut *stream,
            "405 Method Not Allowed",
            "application/json",
            r#"{"error":"method not allowed for this route"}"#,
        )
        .await;
        return DispatchOutcome::Done;
    }

    let (path, query) = match path_and_query.split_once('?') {
        Some((p, q)) => (p, q),
        None => (path_and_query, ""),
    };
    // Copy out of the lossy Cow so we can consume the read buffer below
    // without extending the immutable borrow.
    let path = path.to_string();
    let query = query.to_string();
    drop(req);

    // 0.39.5 readiness gate. While the daemon is still completing
    // first-boot migrations (phase != ready) it has bound its port and
    // answers liveness + the /boot-status handshake — so the renderer's
    // ConnectionGate can SEE us booting and read our version — but every
    // real route returns 503 so no handler runs against half-migrated
    // state. This preserves the pre-0.39.5 "handlers always see migrated
    // state" invariant now that migrations run AFTER the listener binds.
    // See `crate::boot_status`.
    if !crate::boot_status::is_ready()
        && !matches!(path.as_str(), "/ping" | "/health" | "/boot-status")
    {
        let _ = stream.read(&mut buf).await;
        super::http::send_response(
            &mut *stream,
            "503 Service Unavailable",
            "application/json",
            r#"{"state":"migrating","error":"daemon is completing first-boot migrations"}"#,
        )
        .await;
        return DispatchOutcome::Done;
    }

    match path.as_str() {
        "/ping" => {
            let _ = stream.read(&mut buf).await;
            // Unauthenticated. Smallest liveness check.
            super::http::send_response(&mut *stream, "200 OK", "text/plain; charset=utf-8", crate::BANNER).await;
        }
        "/health" => {
            // Unauthenticated liveness probe the behavior test suite
            // polls before it does anything. Mirrors the body shape
            // src-tauri's agent_hooks server returns so tests can talk
            // to either process without branching.
            let _ = stream.read(&mut buf).await;
            super::http::send_response(
                &mut *stream,
                "200 OK",
                "application/json",
                r#"{"status":"ok"}"#,
            )
            .await;
        }
        "/boot-status" => {
            let _ = stream.read(&mut buf).await;
            // 0.39.5: unauthenticated daemon-identity + readiness
            // handshake. The renderer's ConnectionGate polls this to
            // decide whether to mount the app against THIS daemon.
            //
            // - `version`  — exact build string. The LocalPaired policy
            //   (auto-update path) requires it to equal the app's bundled
            //   version, so the renderer never binds to an OUTGOING old
            //   daemon during an update.
            // - `protocol` — daemon↔client API version K2 Connect
            //   range-checks for remote daemons (decoupled from the
            //   marketing version).
            // - `phase`    — starting | migrating | ready (+ reserved
            //   error). Clients treat anything but `ready` as not-ready.
            // - `detail`   — free-text for the UI only; never parsed.
            //
            // Pre-0.39.5 daemons have no such route and return 404, so an
            // outgoing old daemon fails the gate without special-casing.
            // See `crate::boot_status` + `[[project_daemon_handshake_contract]]`.
            let body = serde_json::json!({
                "version": env!("CARGO_PKG_VERSION"),
                "protocol": crate::boot_status::PROTOCOL,
                "phase": crate::boot_status::phase_str(),
                "detail": crate::boot_status::detail(),
            })
            .to_string();
            super::http::send_response(&mut *stream, "200 OK", "application/json", &body).await;
        }
        "/status" => {
            let _ = stream.read(&mut buf).await;
            // Token-gated. Returns a small JSON blob describing the
            // daemon's state so the Tauri app can verify it's talking to
            // the right process.
            if !super::http::token_ok(&query, state.token.as_str()) {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let uptime_secs = state.started_at.elapsed().as_secs();
            let pid = std::process::id();
            let body = format!(
                r#"{{"version":"{}","uptime_secs":{},"pid":{},"port":{}}}"#,
                env!("CARGO_PKG_VERSION"),
                uptime_secs,
                pid,
                state.port,
            );
            super::http::send_response(&mut *stream, "200 OK", "application/json", &body).await;
        }
        "/hook/complete" => {
            // Agent-lifecycle hook endpoint. URL-encoded query params
            // carry paneId / tabId / eventType / token. Business logic
            // (ring buffer, emit, WorkspaceSession.status sync) lives in
            // k2so_core so src-tauri's existing server hits the same
            // code path.
            let _ = stream.read(&mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let req_token = params.get("token").cloned().unwrap_or_default();
            if req_token != *state.token {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"Invalid or missing auth token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body = k2so_core::agent_hooks::handle_hook_complete(&params);
            super::http::send_response(&mut *stream, "200 OK", "application/json", body).await;
        }
        // Session Stream WS subscribe endpoint (0.34.0 Phase 2).
        // Lives on a /cli/ path but routes to the WS handler rather
        // than crate::cli::dispatch because it's an HTTP upgrade, not a
        // JSON request. Branch must precede the generic /cli/
        // catchall below or the dispatch would swallow it.
        "/cli/sessions/subscribe" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            crate::sessions_ws::serve_session_subscribe_connection(stream, params).await;
            // WS handler took read/write semantics for the upgraded
            // connection's lifetime. Don't loop — close the dispatch.
            return DispatchOutcome::Done;
        }
        // Canvas Plan Phase 2: raw-byte stream subscribe. Parallel
        // to /cli/sessions/subscribe but streams PTY bytes as
        // binary WS frames for clients running their own vte.
        "/cli/sessions/bytes" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            crate::sessions_bytes_ws::serve_session_bytes_connection(stream, params).await;
            return DispatchOutcome::Done;
        }
        // Alacritty_v2 (A3): grid snapshot + delta WS endpoint.
        // Serves one Tauri thin client per session. Single-subscriber
        // by design. See `.k2so/prds/alacritty-v2.md`.
        "/cli/sessions/grid" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            crate::sessions_grid_ws::serve_session_grid_connection(stream, params).await;
            return DispatchOutcome::Done;
        }
        // 0.38.0 Commit 4: daemon-authoritative session lifecycle
        // stream. Pushes `session_added`/`session_removed` JSON
        // frames to subscribers whose `path=` matches the affected
        // session's cwd. Renderer + mobile companion consume the
        // same wire format. See `.k2so/prds/daemon-authoritative-tabs.md`.
        "/cli/sessions/events" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let params = super::http::parse_params(&path, &query);
            crate::session_events_ws::serve_session_events_connection(stream, params).await;
            return DispatchOutcome::Done;
        }
        // Awareness Bus endpoints (0.34.0 Phase 3).
        // `/cli/awareness/publish` — POST JSON body → egress::deliver
        // `/cli/awareness/subscribe` — WS, streams bus signals out
        "/cli/awareness/publish" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::awareness_ws::handle_publish(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        "/cli/awareness/subscribe" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            crate::awareness_ws::serve_awareness_subscribe_connection(stream).await;
            return DispatchOutcome::Done;
        }
        // POST /cli/sessions/v2/spawn — Alacritty_v2 find-or-spawn
        // (A4). Parallel to /cli/sessions/spawn but produces a
        // DaemonPtySession (registered in v2_session_map) instead
        // of a SessionStreamSession. Idempotent on agent_name: same
        // name → same session, suitable for remount reattach.
        "/cli/sessions/v2/spawn" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::v2_spawn::handle_v2_spawn(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/sessions/v2/close — explicit teardown of a v2
        // session. Called only from `tabs.ts::removeTab` (A6).
        "/cli/sessions/v2/close" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::v2_spawn::handle_v2_close(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/companion/set-password — Phase 2 Unit 1.
        // Body: `{"password": "..."}`. Hashes argon2id, stores in
        // macOS Keychain (preferred) or settings.json (fallback),
        // then invalidates every live companion session so the old
        // token can't be replayed.
        //
        // Method gate: see the long-form note on /cli/claude-auth/
        // refresh-now below — the top-level dispatch lets a GET
        // through on POST-allowlisted routes. Mirror Unit 5's
        // explicit `if !is_post` guard so a GET against this route
        // can't trigger the password rotation. Especially important
        // here because this is one of the routes Mobile Companion
        // and K2SO Connect will hit over the ngrok tunnel.
        "/cli/companion/set-password" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::companion_routes::handle_companion_set_password(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // ── Phase 2 Unit 2 — /cli/llm/* ────────────────────────────
        // GET routes are cheap; POST routes go through llm_routes
        // which owns the supervisor's subprocess machinery. All
        // five routes are token-gated by the standard query-string
        // check so callers must pass `?token=<token>` like every
        // other /cli/* endpoint.
        "/cli/llm/check" => {
            let _ = stream.read(&mut buf).await;
            if !super::http::token_ok(&query, state.token.as_str()) {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let r = crate::llm_routes::handle_check();
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/status" => {
            let _ = stream.read(&mut buf).await;
            if !super::http::token_ok(&query, state.token.as_str()) {
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let r = crate::llm_routes::handle_status();
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/chat" => {
            // Method gate (see feedback_post_only_route_guards memory + the
            // /cli/claude-auth/refresh-now comment): the top-level dispatch
            // lets a GET through on POST-allowlisted routes. Reject explicitly.
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // Inference is CPU/GPU heavy and may block for tens of
            // seconds. Run on a blocking worker so the runtime's
            // accept-loop threads stay free.
            let r = tokio::task::spawn_blocking(move || {
                crate::llm_routes::handle_chat(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/load-model" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = tokio::task::spawn_blocking(move || {
                crate::llm_routes::handle_load_model(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/llm/download-default" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Body is currently empty; read+drop to flush.
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::llm_routes::handle_download_default(&state.event_tx);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/companion/disconnect-session — Phase 2 Unit 1.
        // Body: `{"sessionToken": "..."}`. Removes the session row
        // and any WS clients still attached to it.
        //
        // Method gate: same rationale as /cli/companion/set-password
        // above. Don't let a GET disconnect a live session.
        "/cli/companion/disconnect-session" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result =
                crate::companion_routes::handle_companion_disconnect_session(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/tunnel/start — K2 Connect tunnel.
        //
        // Spawns/supervises the `frpc` child that dials the hosted frps
        // server, exposing THIS daemon at https://<user>.k2.dev. The
        // optional `subdomain` query param overrides the stored config's
        // requested label; the live daemon port (`state.port`) is the
        // exposed `localPort` when the config doesn't pin one.
        //
        // Method gate: explicit `require_post` — the top-level dispatch
        // lets a GET through on POST-allowlisted routes, and we must
        // never let a curl GET launch a tunnel (see
        // feedback_post_only_route_guards).
        "/cli/tunnel/start" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // No JSON body — params ride the query string. Drain to flush.
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let subdomain = params
                .get("subdomain")
                .map(|s| s.to_string())
                .filter(|s| !s.is_empty());
            let daemon_port = state.port;
            let result = tokio::task::spawn_blocking(move || {
                k2so_core::tunnel::start_tunnel(subdomain, daemon_port)
            })
            .await
            .unwrap_or_else(|e| Err(format!("worker join: {e}")));
            let resp = match result {
                Ok(status) => crate::cli::CliResponse::ok_json(
                    serde_json::to_string(&status)
                        .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
                ),
                Err(e) => crate::cli::CliResponse::bad_request(e),
            };
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // POST /cli/tunnel/stop — stop the supervised frpc child.
        "/cli/tunnel/stop" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let resp = match k2so_core::tunnel::stop_tunnel() {
                Ok(()) => crate::cli::CliResponse::ok_json(r#"{"ok":true}"#.to_string()),
                Err(e) => crate::cli::CliResponse::bad_request(e),
            };
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // GET/POST /cli/tunnel/config — read or set the K2 Connect tunnel
        // config. GET returns a REDACTED view (tokenSet bool, never the
        // secret token); POST applies a partial update and persists
        // ~/.k2so/tunnel.json. This is what the desktop K2 Connect page
        // calls to BIND a chosen subdomain (token + subdomain) before
        // `start`. GET must NOT read a body (read_post_body blocks on a
        // bodyless keep-alive GET); only POST drains the body.
        "/cli/tunnel/config" => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                if is_post {
                    let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
                }
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let resp = if is_post {
                let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
                match serde_json::from_slice::<k2so_core::tunnel::TunnelConfigUpdate>(&body_bytes) {
                    Ok(upd) => match k2so_core::tunnel::set_config(upd) {
                        Ok(view) => crate::cli::CliResponse::ok_json(
                            serde_json::to_string(&view)
                                .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
                        ),
                        Err(e) => crate::cli::CliResponse::bad_request(e),
                    },
                    Err(e) => {
                        crate::cli::CliResponse::bad_request(format!("invalid JSON body: {e}"))
                    }
                }
            } else {
                match k2so_core::tunnel::get_config_view() {
                    Ok(view) => crate::cli::CliResponse::ok_json(
                        serde_json::to_string(&view)
                            .unwrap_or_else(|e| format!("{{\"error\":\"{e}\"}}")),
                    ),
                    Err(e) => crate::cli::CliResponse::bad_request(e),
                }
            };
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body)
                .await;
        }
        // POST /cli/claude-auth/refresh-now — Phase 2 Unit 5.
        // No body required (the refresh token comes from the local
        // Keychain / credentials file). POST instead of GET because
        // it mutates token state. Returns the same status payload
        // shape as GET /cli/claude-auth/status.
        //
        // NOTE on method gating: the top-level dispatch only rejects
        // non-GET/non-POST methods; it doesn't reject GET on a
        // POST-allowlisted route (most routes accept both today and
        // gate behavior on body-presence). For Unit 5's mutating
        // routes — which have no body — we must explicitly reject
        // GET in the handler, or a curl GET would silently install /
        // refresh / uninstall the user's launchd scheduler. Caught
        // during smoke testing.
        "/cli/claude-auth/refresh-now" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Drain whatever body the client sent so the socket
            // doesn't get half-read state. We don't use it.
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::claude_auth_host::handle_refresh_now();
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/claude-auth/install-scheduler — Phase 2 Unit 5.
        // Writes ~/.k2so/claude-auth-refresh.sh + loads the
        // launchd plist (macOS) or installs the crontab entry
        // (linux). Idempotent. POST-only (see /refresh-now comment).
        "/cli/claude-auth/install-scheduler" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::claude_auth_host::handle_install_scheduler();
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/claude-auth/uninstall-scheduler — Phase 2 Unit 5.
        // Unloads + removes the plist (macOS) or strips the
        // crontab entry (linux). Idempotent. POST-only.
        "/cli/claude-auth/uninstall-scheduler" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::claude_auth_host::handle_uninstall_scheduler();
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/settings/update — Phase 2 Unit 7a.
        // Body: arbitrary JSON object deep-merged into settings.json.
        // F3 closure runs inside `app_settings::update()` — companion-
        // credential changes invalidate live sessions server-side, in
        // the same process that owns the live companion runtime.
        // Method gate per feedback_post_only_route_guards memory.
        "/cli/settings/update" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::settings_routes::handle_settings_update(&body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // POST /cli/settings/reset — Phase 2 Unit 7a.
        // Restores `AppSettings::default()`, deletes Keychain hash,
        // invalidates every live companion session. POST (not GET)
        // so a browser refresh can't accidentally trigger it.
        // Method gate per feedback_post_only_route_guards memory.
        "/cli/settings/reset" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let _ = stream.read(&mut buf).await;
            let result = crate::settings_routes::handle_settings_reset();
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // Phase 2 Unit 3 — terminal PTY lifecycle (POST routes).
        // Each handler runs through the process-wide
        // `k2so_core::terminal::shared()` TerminalManager so daemon
        // ownership is uniform. The blocking create handler is
        // dispatched via `tokio::task::spawn_blocking` (F5) since
        // posix_spawn + alacritty Term::new can stall briefly under
        // load. The non-blocking handlers (kill/resize/scroll/etc.)
        // are cheap mutex+method calls and run inline.
        //
        // Method gate: see the long-form comment on
        // `/cli/claude-auth/refresh-now`. The top-level dispatch
        // does NOT reject GET on POST-allowlisted routes — without
        // the explicit `if !is_post` guard, a curl GET could
        // silently spawn / kill a PTY.
        "/cli/terminal/create" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // F5: posix_spawn + alacritty Term::new can block; run
            // off the accept-loop thread pool.
            let r = tokio::task::spawn_blocking(move || {
                crate::terminal_lifecycle_routes::handle_create(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/kill" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // Kill can block briefly waiting on child reap; F5.
            let r = tokio::task::spawn_blocking(move || {
                crate::terminal_lifecycle_routes::handle_kill(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/resize" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_resize(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/kill-foreground" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_kill_foreground(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/scroll" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_scroll(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/log" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_log(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // POST /cli/terminal/lifecycle-write — byte-level write for
        // TerminalManager-owned terminals. The existing
        // /cli/terminal/write (GET, in terminal_routes.rs) operates on
        // the session_map's UUID-keyed sessions; the legacy
        // arbitrary-string TerminalManager IDs need a parallel path.
        // Body: `{"id":"...","data":"..."}`.
        "/cli/terminal/lifecycle-write" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_lifecycle_write(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/terminal/set-focus" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = crate::terminal_lifecycle_routes::handle_set_focus(&body_bytes);
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // Phase 2 Unit 7c — heartbeat-launchd installer routes.
        // Daemon owns its own `com.k2so.agent-heartbeat.plist` so
        // K2SO Connect (remote daemon without Tauri) can install +
        // remove the scheduler under its own GUI session. Method
        // gates are inline so a stray GET can't trigger a
        // launchctl bootstrap. See `crates/k2so-core/src/heartbeats/
        // install.rs` for the install/uninstall bodies.
        "/cli/heartbeat/install-launchd" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // launchctl bootstrap can stall briefly under load; F5.
            let r = tokio::task::spawn_blocking(move || {
                crate::heartbeat_routes::handle_install_launchd(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/heartbeat/uninstall-launchd" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = tokio::task::spawn_blocking(move || {
                crate::heartbeat_routes::handle_uninstall_launchd(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        "/cli/heartbeat/apply-wake-scheduler" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let r = tokio::task::spawn_blocking(move || {
                crate::heartbeat_routes::handle_apply_wake_scheduler(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // Phase 2 Unit 7c — orphan-agent sweep, refactored out of
        // src-tauri/src/commands/projects.rs's agent_mode-change
        // path. Body: `{"project_path": "/path"}`.
        "/cli/agents/archive-orphans" => {
            if !super::http::require_post(&mut *stream, &mut buf, is_post).await { return DispatchOutcome::Done; }
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            // fs walk + db lock — F5.
            let r = tokio::task::spawn_blocking(move || {
                handle_archive_orphans(&body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
        }
        // Phase 2 Unit 4 — POST routes for git (libgit2 ops). F5:
        // spawn_blocking because diff/merge/status on large repos
        // can block for 100s of ms.
        p if is_post && post_allowed && p.starts_with("/cli/git/") => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::git_routes::dispatch_unit4_git_post(&p_owned, &body_bytes)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // Phase 2 Unit 4 — POST routes for DB-writing domains. JSON-
        // bodied writes; per-route allowlist + same implicit gate
        // pattern as Unit 6. Dispatch is `dispatch_unit4_post`.
        p if is_post && post_allowed && (
            p.starts_with("/cli/states/")
                || p.starts_with("/cli/workspaces/")
                || p.starts_with("/cli/focus-groups/")
                || p.starts_with("/cli/sections/")
                || p.starts_with("/cli/workspace-layouts/")
                || p.starts_with("/cli/timer/")
                || p.starts_with("/cli/presets/")
                || p.starts_with("/cli/window-state/")
                || p.starts_with("/cli/projects/")
        ) => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = crate::db_routes::dispatch_unit4_post(p, &body_bytes);
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body)
                .await;
        }
        // Phase 2 Unit 6 — POST routes for filesystem / chat /
        // themes / skill-layers / review-checklist. All JSON-bodied;
        // delegate to per-domain modules. The match-arm guard
        // (`is_post && post_allowed && starts_with`) is the implicit
        // method gate — a GET on these paths falls through to the
        // generic `/cli/` catchall below, which returns a 404
        // "unknown route" since dispatch doesn't have GET handlers
        // for these paths. Functionally equivalent to Unit 5/7a's
        // explicit 405s; the response code differs but no silent
        // mutation is possible either way.
        p if is_post && post_allowed && (
            p.starts_with("/cli/fs/")
                || p.starts_with("/cli/chat/")
                || p.starts_with("/cli/themes/")
                || p.starts_with("/cli/skill-layers/")
                || p.starts_with("/cli/review-checklist/")
        ) => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            let body_bytes = super::http::read_post_body(&mut *stream, &mut buf).await;
            let result = dispatch_unit6_post(p, &body_bytes);
            super::http::send_response(&mut *stream, result.status, "application/json", &result.body)
                .await;
        }
        // Phase 2.1 — workspace inbox POST routes. Query-string only
        // (no body). Token gate is explicit per method-gate rule;
        // body is drained to keep the connection clean. Filesystem
        // operations run in spawn_blocking per F5 (atomic-rename of
        // a `.md` file isn't slow, but `safe_delete::trash` calls
        // into macOS Finder via AppleScript and CAN block).
        p if is_post && post_allowed && p.starts_with("/cli/inbox/") => {
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Drain body (we don't use it — params come from query).
            let _ = super::http::read_post_body(&mut *stream, &mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let p_owned = p.to_string();
            let result = tokio::task::spawn_blocking(move || {
                crate::inbox_routes::dispatch_post(&p_owned, &params)
            })
            .await
            .unwrap_or_else(|e| crate::cli_response::CliResponse {
                status: "500 Internal Server Error",
                content_type: "application/json",
                body: serde_json::json!({ "error": format!("worker join: {e}") })
                    .to_string(),
            });
            super::http::send_response(&mut *stream, result.status, result.content_type, &result.body).await;
        }
        // Unified /cli/* dispatch. Auth + param validation +
        // per-route handler all live in `crate::cli::dispatch`; main.rs
        // just translates the CliResponse into bytes.
        p if p.starts_with("/cli/") => {
            let _ = stream.read(&mut buf).await;
            let params = super::http::parse_params(&path, &query);
            let req_token = params.get("token").cloned().unwrap_or_default();
            if req_token != *state.token {
                let r = crate::cli::CliResponse::forbidden();
                super::http::send_response(&mut *stream, r.status, r.content_type, &r.body).await;
                return DispatchOutcome::Done;
            }
            let resp = crate::cli::dispatch(p, &params);
            super::http::send_response(&mut *stream, resp.status, resp.content_type, &resp.body).await;
        }
        "/events" => {
            // Token check BEFORE the upgrade so unauthenticated clients
            // see an HTTP 403 instead of a dangling WS close.
            if !super::http::token_ok(&query, state.token.as_str()) {
                let _ = stream.read(&mut buf).await;
                super::http::send_response(
                    &mut *stream,
                    "403 Forbidden",
                    "application/json",
                    r#"{"error":"invalid or missing token"}"#,
                )
                .await;
                return DispatchOutcome::Done;
            }
            // Hand off to tokio-tungstenite; the handshake is still
            // unread in the stream buffer.
            crate::events::serve_events_connection(stream, state.event_tx.clone()).await;
            return DispatchOutcome::Done;
        }
        _ => {
            let _ = stream.read(&mut buf).await;
            super::http::send_response(&mut *stream, "404 Not Found", "text/plain", "not found\n").await;
        }
    }

    // 0.39.7: keep-alive default. Every non-WS arm above ends by
    // calling `send_response`; if the request didn't request close,
    // loop and serve another request on the same socket. WS arms,
    // auth failures, and other error paths short-circuit with explicit
    // `return DispatchOutcome::Done` above so they never reach here.
    if client_wants_close {
        DispatchOutcome::Done
    } else {
        DispatchOutcome::KeepAlive
    }
}

// ─────────────────────────────────────────────────────────────────────
// Dispatch sub-helpers
// ─────────────────────────────────────────────────────────────────────

/// Phase 2 Unit 7c — orphan top-tier agent sweep. Inlined handler
/// (instead of a routes module) because the body is two lines of
/// JSON parse + a direct call into `k2so_core::workspace::migrations`
/// (canonical post-Phase-2.5d path; was `agents::workspace`).
/// Returns `{"success":true,"archived":["<name>", ...]}`.
fn handle_archive_orphans(body: &[u8]) -> crate::cli::CliResponse {
    #[derive(serde::Deserialize)]
    struct Req {
        project_path: String,
    }
    let req: Req = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return crate::cli::CliResponse::bad_request(format!("invalid body: {e}")),
    };
    let archived = k2so_core::workspace::migrations::archive_orphan_top_tier_agents(
        &req.project_path,
    );
    crate::cli::CliResponse::ok_json(
        serde_json::json!({ "success": true, "archived": archived }).to_string(),
    )
}

/// Dispatch a Phase 2 Unit 6 POST request body to the right
/// per-domain handler. Path matching is exact — unknown paths fall
/// through to a 404 so the renderer surfaces "route not found"
/// instead of a silent success.
fn dispatch_unit6_post(path: &str, body: &[u8]) -> crate::cli::CliResponse {
    match path {
        // Filesystem
        "/cli/fs/search-tree" => crate::fs_routes::handle_search_tree(body),
        "/cli/fs/write-file" => crate::fs_routes::handle_write_file(body),
        "/cli/fs/move" => crate::fs_routes::handle_move(body),
        "/cli/fs/copy" => crate::fs_routes::handle_copy(body),
        "/cli/fs/delete" => crate::fs_routes::handle_delete(body),
        "/cli/fs/rename" => crate::fs_routes::handle_rename(body),
        "/cli/fs/create" => crate::fs_routes::handle_create(body),
        "/cli/fs/duplicate" => crate::fs_routes::handle_duplicate(body),
        "/cli/fs/open-finder" => crate::fs_routes::handle_open_finder(body),
        "/cli/fs/open-external" => crate::fs_routes::handle_open_external(body),
        // Chat history (state-mutating)
        "/cli/chat/rename" => crate::chat_routes::handle_rename(body),
        "/cli/chat/toggle-pin" => crate::chat_routes::handle_toggle_pin(body),
        "/cli/chat/migrate-ide" => crate::chat_routes::handle_migrate_ide(body),
        // Themes
        "/cli/themes/create-template" => crate::themes_routes::handle_create_template(body),
        "/cli/themes/delete" => crate::themes_routes::handle_delete(body),
        // Skill layers
        "/cli/skill-layers/create" => crate::skill_layers_routes::handle_create(body),
        "/cli/skill-layers/delete" => crate::skill_layers_routes::handle_delete(body),
        // Review checklist
        "/cli/review-checklist/write" => crate::review_checklist_routes::handle_write(body),
        "/cli/review-checklist/toggle" => crate::review_checklist_routes::handle_toggle(body),
        "/cli/review-checklist/init" => crate::review_checklist_routes::handle_init(body),
        _ => crate::cli::CliResponse::not_found(),
    }
}

// ─────────────────────────────────────────────────────────────────────
// Inline unit tests — dispatch sub-helpers
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dispatch_unit6_post_unknown_path_returns_404() {
        let resp = dispatch_unit6_post("/cli/does-not-exist", b"{}");
        assert_eq!(resp.status, "404 Not Found");
        assert!(
            resp.body.contains("route not found"),
            "404 body should mention 'route not found': {}",
            resp.body,
        );
    }

    #[test]
    fn dispatch_unit6_post_empty_path_returns_404() {
        // A blank path should never match a real route.
        let resp = dispatch_unit6_post("", b"{}");
        assert_eq!(resp.status, "404 Not Found");
    }

    #[test]
    fn dispatch_unit6_post_path_is_case_sensitive() {
        // Exact match required — upper/lower-case variants must NOT
        // route to the lowercase handler. Closing this avoids subtle
        // routing collisions if a future handler uses mixed case.
        let resp = dispatch_unit6_post("/CLI/FS/CREATE", b"{}");
        assert_eq!(resp.status, "404 Not Found");
    }
}
