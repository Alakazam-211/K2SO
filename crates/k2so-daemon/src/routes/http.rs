//! HTTP framing helpers shared by the daemon's route dispatcher.
//!
//! Extracted from `main.rs` during the 0.39.0 route refactor.
//! Everything here is plumbing — token parsing, query parsing, response
//! writing, the POST-only method gate, the OPTIONS preflight responder,
//! body reading — that the dispatcher in `routes::dispatcher` invokes on
//! every connection.
//!
//! These helpers were previously `fn` in `main.rs` (binary-private);
//! moving them under `routes/` keeps `main.rs` focused on boot +
//! migrations + watchdog.

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

/// Parse `token=<value>` out of a URL-encoded query string and compare
/// against the expected value. No full urlencoded decoding — the token
/// is always 32 hex chars so there's nothing to decode.
pub(crate) fn token_ok(query: &str, expected: &str) -> bool {
    for pair in query.split('&') {
        if let Some(v) = pair.strip_prefix("token=") {
            return v == expected;
        }
    }
    false
}

/// Reassemble a full `path?query` URL and hand off to k2so_core's
/// URL-decoding query parser. The core helper knows how to unescape
/// `%20`/`+` and multi-byte UTF-8 — we just combine the pieces.
pub(crate) fn parse_params(
    path: &str,
    query: &str,
) -> std::collections::HashMap<String, String> {
    let path_and_query = if query.is_empty() {
        path.to_string()
    } else {
        format!("{}?{}", path, query)
    };
    k2so_core::agent_hooks::parse_query_params(&path_and_query)
}

/// Extract the project directory from query params. Accepts BOTH
/// `project=<path>` (the short form src-tauri's agent_hooks server
/// uses and the k2so CLI sends) and `project_path=<path>` (the long
/// form earlier daemon routes adopted). Empty values are treated the
/// same as missing.
#[allow(dead_code)] // covered by inline tests; reserved for future
                    // handlers that prefer the fallback over inline
                    // `need_project` lookups in `cli.rs`.
pub(crate) fn project_param(
    params: &std::collections::HashMap<String, String>,
) -> Option<String> {
    for key in &["project_path", "project"] {
        if let Some(v) = params.get(*key) {
            if !v.is_empty() {
                return Some(v.clone());
            }
        }
    }
    None
}

/// Write a single HTTP response with the canonical header set the
/// daemon emits on every route — `Connection: close`, permissive CORS
/// for the Tauri WebView, content-length, the supplied status and
/// content-type.
pub(crate) async fn send_response(stream: &mut TcpStream, status: &str, ct: &str, body: &str) {
    // CORS headers on every response so the Tauri WebView (cross-
    // origin from tauri://localhost or http://localhost:5173 to
    // http://127.0.0.1:<port>) can read the body. Token auth
    // gates every real request so permissive origin adds no risk.
    let resp = format!(
        "HTTP/1.1 {status}\r\n\
         Content-Type: {ct}\r\n\
         Content-Length: {}\r\n\
         Access-Control-Allow-Origin: *\r\n\
         Access-Control-Expose-Headers: *\r\n\
         Connection: close\r\n\r\n{}",
        body.len(),
        body,
    );
    let _ = stream.write_all(resp.as_bytes()).await;
}

/// Respond to a CORS preflight (OPTIONS) with permissive headers so
/// the WebView accepts the subsequent GET/POST. 204 No Content is
/// the conventional preflight response status.
pub(crate) async fn send_cors_preflight(stream: &mut TcpStream) {
    let resp = "HTTP/1.1 204 No Content\r\n\
        Access-Control-Allow-Origin: *\r\n\
        Access-Control-Allow-Methods: GET, POST, OPTIONS\r\n\
        Access-Control-Allow-Headers: Content-Type, Authorization\r\n\
        Access-Control-Max-Age: 600\r\n\
        Content-Length: 0\r\n\
        Connection: close\r\n\r\n";
    let _ = stream.write_all(resp.as_bytes()).await;
}

/// Method-gate guard for POST-only `/cli/*` routes.
///
/// Per the [`feedback_post_only_route_guards`] memory: every mutating
/// `/cli/*` route must reject non-POST methods at the handler top, NOT
/// rely on the top-level dispatch's GET/POST allowlist — that allowlist
/// only blocks methods like PUT/DELETE/HEAD, it does NOT block GET on
/// POST-allowlisted routes. Without an explicit per-handler gate, a
/// `curl http://127.0.0.1:<port>/cli/dangerous-route?token=X` GET would
/// silently trigger the mutation.
///
/// Returns `true` when the request is a POST and the caller should
/// continue; returns `false` after sending a `405 Method Not Allowed`
/// response, in which case the caller MUST early-return without touching
/// the stream further. The peeked request bytes are drained on rejection
/// so the response actually goes out.
///
/// Usage:
///
/// ```ignore
/// if !require_post(&mut stream, &mut buf, is_post).await { return; }
/// ```
pub(crate) async fn require_post(stream: &mut TcpStream, buf: &mut [u8], is_post: bool) -> bool {
    if is_post {
        return true;
    }
    let _ = stream.read(buf).await;
    send_response(
        stream,
        "405 Method Not Allowed",
        "application/json",
        r#"{"error":"POST required"}"#,
    )
    .await;
    false
}

/// Read the body of a POST request. Consumes the request line and
/// headers from the peeked stream, then returns whatever bytes
/// follow the `\r\n\r\n` separator up to the Content-Length header.
///
/// MVP implementation — assumes the full request arrived in the
/// 4KB peek buffer (fine for the single JSON AgentSignal payloads
/// E7 + E8 handle). Production-grade Content-Length-driven
/// streaming is deferred; the largest signal we expect is ~1KB, so
/// 4KB is 4× the headroom.
pub(crate) async fn read_post_body(stream: &mut TcpStream, buf: &mut [u8]) -> Vec<u8> {
    // Phase 4.5: the old single-read version worked with curl (which
    // batches headers + body into one TCP send) but broke with
    // browser fetch, which sends headers in one packet and body in
    // a separate packet. A single `stream.read()` would only return
    // the headers, leaving the body unread — and the JSON parser
    // got "EOF at column 0".
    //
    // Loop until we have the full body: read headers (first chunk),
    // parse Content-Length, then keep reading until we've got that
    // many body bytes or EOF.
    let mut accumulated: Vec<u8> = Vec::new();
    let mut header_end: Option<usize> = None;
    let mut content_length: Option<usize> = None;

    loop {
        // Read into `buf` and append to `accumulated` until headers
        // end is found.
        let n = match stream.read(buf).await {
            Ok(0) => break, // EOF
            Ok(n) => n,
            Err(_) => return Vec::new(),
        };
        accumulated.extend_from_slice(&buf[..n]);

        if header_end.is_none() {
            if let Some(pos) = accumulated
                .windows(4)
                .position(|w| w == b"\r\n\r\n")
            {
                header_end = Some(pos + 4);
                let headers_str =
                    std::str::from_utf8(&accumulated[..pos]).unwrap_or("");
                content_length = headers_str.lines().find_map(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower
                        .strip_prefix("content-length:")
                        .and_then(|v| v.trim().parse::<usize>().ok())
                });
            }
        }

        // Once headers end is known, check if we have the whole body.
        if let (Some(body_start), Some(clen)) = (header_end, content_length) {
            if accumulated.len() >= body_start + clen {
                return accumulated[body_start..body_start + clen].to_vec();
            }
        }
        // Without Content-Length, fall back to "one read gave us
        // everything" heuristic once we've seen the headers.
        if let (Some(body_start), None) = (header_end, content_length) {
            return accumulated[body_start..].to_vec();
        }
    }

    // EOF before we got the full body (or before headers ended).
    // Return whatever we have between header end and EOF; caller's
    // parser will surface a helpful error if it's incomplete.
    if let Some(body_start) = header_end {
        if accumulated.len() > body_start {
            return accumulated[body_start..].to_vec();
        }
    }
    Vec::new()
}

// ─────────────────────────────────────────────────────────────────────
// Inline unit tests — pure-logic helpers that gate every connection
// ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── token_ok: auth gate ─────────────────────────────────────────

    #[test]
    fn token_ok_accepts_matching_token() {
        assert!(token_ok("token=abc123", "abc123"));
    }

    #[test]
    fn token_ok_rejects_mismatched_token() {
        assert!(!token_ok("token=wrong", "abc123"));
    }

    #[test]
    fn token_ok_rejects_missing_token_param() {
        // Other params present but no `token=` key.
        assert!(!token_ok("project_path=/tmp/foo&name=bar", "abc123"));
    }

    #[test]
    fn token_ok_rejects_empty_query() {
        assert!(!token_ok("", "abc123"));
    }

    #[test]
    fn token_ok_rejects_empty_token_value_when_expected_nonempty() {
        // `token=` with no value should not slip through against a
        // non-empty expected token.
        assert!(!token_ok("token=", "abc123"));
    }

    #[test]
    fn token_ok_finds_token_among_multiple_params() {
        // Token may appear anywhere in the query string — the loop
        // must keep walking past earlier params.
        let q = "project_path=/tmp/foo&name=bar&token=abc123&extra=baz";
        assert!(token_ok(q, "abc123"));
    }

    #[test]
    fn token_ok_first_token_value_wins_when_duplicated() {
        // If `token=` appears twice (malformed query), the first
        // value is what gates the request — that's the current
        // behavior and the contract callers depend on.
        assert!(token_ok("token=first&token=second", "first"));
        assert!(!token_ok("token=first&token=second", "second"));
    }

    #[test]
    fn token_ok_is_case_sensitive() {
        // Tokens are random hex, but if the field name's case ever
        // shifts the auth gate must fail closed (no match means no
        // entry). `Token=` is not equivalent to `token=`.
        assert!(!token_ok("Token=abc123", "abc123"));
    }

    // ── project_param: project_path / project fallback ──────────────

    #[test]
    fn project_param_returns_project_path_when_set() {
        let mut params = std::collections::HashMap::new();
        params.insert("project_path".to_string(), "/tmp/work".to_string());
        assert_eq!(project_param(&params), Some("/tmp/work".to_string()));
    }

    #[test]
    fn project_param_falls_back_to_project_alias() {
        let mut params = std::collections::HashMap::new();
        params.insert("project".to_string(), "/tmp/alias".to_string());
        assert_eq!(project_param(&params), Some("/tmp/alias".to_string()));
    }

    #[test]
    fn project_param_prefers_project_path_over_alias() {
        // Both set → primary key wins.
        let mut params = std::collections::HashMap::new();
        params.insert("project_path".to_string(), "/tmp/primary".to_string());
        params.insert("project".to_string(), "/tmp/alias".to_string());
        assert_eq!(project_param(&params), Some("/tmp/primary".to_string()));
    }

    #[test]
    fn project_param_returns_none_when_neither_set() {
        let params = std::collections::HashMap::new();
        assert_eq!(project_param(&params), None);
    }

    #[test]
    fn project_param_skips_empty_string_value() {
        // An empty value must be treated as "unset" so the fallback
        // alias key gets a chance. If `project_path=` is empty but
        // `project=/tmp/x` is set, callers expect /tmp/x.
        let mut params = std::collections::HashMap::new();
        params.insert("project_path".to_string(), "".to_string());
        params.insert("project".to_string(), "/tmp/x".to_string());
        assert_eq!(project_param(&params), Some("/tmp/x".to_string()));
    }

    // ── parse_params: thin wrapper over core's query parser ─────────

    #[test]
    fn parse_params_extracts_keys_from_query_string() {
        let params = parse_params("/cli/foo", "name=bar&token=xyz");
        assert_eq!(params.get("name").map(String::as_str), Some("bar"));
        assert_eq!(params.get("token").map(String::as_str), Some("xyz"));
    }

    #[test]
    fn parse_params_empty_query_returns_empty_map() {
        let params = parse_params("/cli/foo", "");
        assert!(
            params.is_empty(),
            "expected empty params, got: {params:?}",
        );
    }
}
