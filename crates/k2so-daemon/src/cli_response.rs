//! Shared response shape used by every `/cli/*` route handler.
//!
//! Extracted into its own module so lib-side handler modules
//! (`terminal_routes`, future `companion_routes`, etc.) can share
//! the type with `main.rs`'s dispatch without needing `cli.rs` —
//! which contains `crate::handle_*` references scoped to main.rs
//! and therefore can't move into the lib.

/// Final HTTP response body + status line the dispatch caller
/// emits. Owned struct so the caller attaches the
/// `Content-Length` / `Connection: close` boilerplate once at the
/// top of the dispatch.
pub struct CliResponse {
    pub status: &'static str,
    pub content_type: &'static str,
    pub body: String,
}

impl CliResponse {
    pub fn ok_json(body: String) -> Self {
        Self {
            status: "200 OK",
            content_type: "application/json",
            body,
        }
    }
    pub fn ok_text(body: String) -> Self {
        Self {
            status: "200 OK",
            content_type: "text/plain; charset=utf-8",
            body,
        }
    }
    pub fn bad_request(err: impl std::fmt::Display) -> Self {
        Self {
            status: "400 Bad Request",
            content_type: "application/json",
            body: serde_json::json!({ "error": err.to_string() }).to_string(),
        }
    }
    pub fn internal_error(err: impl std::fmt::Display) -> Self {
        Self {
            status: "500 Internal Server Error",
            content_type: "application/json",
            body: serde_json::json!({ "error": err.to_string() }).to_string(),
        }
    }
    pub fn not_found() -> Self {
        Self {
            status: "404 Not Found",
            content_type: "application/json",
            body: r#"{"error":"route not found"}"#.to_string(),
        }
    }
    pub fn forbidden() -> Self {
        Self {
            status: "403 Forbidden",
            content_type: "application/json",
            body: r#"{"error":"Invalid or missing auth token"}"#.to_string(),
        }
    }
    /// 0.39.0f Phase 2.1b: `/cli/work/*` routes are retired but the
    /// route entries remain so external callers get a clear HTTP-410
    /// signal (rather than a silent 404 on the catch-all). The body
    /// must point them at the replacement and `help-deprecated`.
    pub fn gone(err: impl std::fmt::Display) -> Self {
        Self {
            status: "410 Gone",
            content_type: "application/json",
            body: serde_json::json!({ "error": err.to_string() }).to_string(),
        }
    }
}
