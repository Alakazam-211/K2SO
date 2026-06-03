//! K2SO #617 — daemon route handlers for connect-user accounts + login.
//!
//! Two route families, two auth levels (enforced by the *dispatcher*, not
//! here — these handlers assume the gate already ran):
//!
//! - **`/cli/users/*`** (OWNER-ONLY at the gate): the owner provisions and
//!   manages username+password accounts. Bodies carry credentials so they
//!   stay out of URL logs.
//! - **`/cli/auth/login`** (PUBLIC — no token at the gate): how a remote
//!   connect-user trades username+password for a session token. On failure
//!   it returns a GENERIC 401 (never reveals whether the user exists) and
//!   the dispatcher adds a small fixed delay to blunt brute force.
//!
//! The actual account/session logic lives in
//! `k2so_core::connect_users`; these are thin body-parse + shape wrappers
//! per the daemon-first contract.

use crate::cli_response::CliResponse;
use k2so_core::connect_users;

/// `GET /cli/users` → `{"users":[{username,createdAt,disabled}, ...]}`.
/// Redacted views only — never the password hash.
pub fn handle_list() -> CliResponse {
    match connect_users::list_users() {
        Ok(views) => {
            // Re-key to camelCase wire shape (createdAt) for the client.
            let users: Vec<serde_json::Value> = views
                .iter()
                .map(|v| {
                    serde_json::json!({
                        "username": v.username,
                        "createdAt": v.created_at.to_rfc3339(),
                        "disabled": v.disabled,
                    })
                })
                .collect();
            CliResponse::ok_json(serde_json::json!({ "users": users }).to_string())
        }
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct AddReq {
    username: String,
    password: String,
}

/// `POST /cli/users/add` `{username,password}` → `{"success":true}`.
pub fn handle_add(body: &[u8]) -> CliResponse {
    let req: AddReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match connect_users::add_user(&req.username, &req.password) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct RemoveReq {
    username: String,
}

/// `POST /cli/users/remove` `{username}` → `{"success":true}`.
pub fn handle_remove(body: &[u8]) -> CliResponse {
    let req: RemoveReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match connect_users::remove_user(&req.username) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct SetPasswordReq {
    username: String,
    password: String,
}

/// `POST /cli/users/set-password` `{username,password}` →
/// `{"success":true}`. Revokes the user's live sessions server-side.
pub fn handle_set_password(body: &[u8]) -> CliResponse {
    let req: SetPasswordReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match connect_users::set_password(&req.username, &req.password) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct SetDisabledReq {
    username: String,
    disabled: bool,
}

/// `POST /cli/users/set-disabled` `{username,disabled}` →
/// `{"success":true}`. Disabling revokes live sessions.
pub fn handle_set_disabled(body: &[u8]) -> CliResponse {
    let req: SetDisabledReq = match serde_json::from_slice(body) {
        Ok(r) => r,
        Err(e) => return CliResponse::bad_request(format!("invalid JSON body: {e}")),
    };
    match connect_users::set_disabled(&req.username, req.disabled) {
        Ok(()) => CliResponse::ok_json(r#"{"success":true}"#.to_string()),
        Err(e) => CliResponse::bad_request(e),
    }
}

#[derive(serde::Deserialize)]
struct LoginReq {
    username: String,
    password: String,
}

/// Generic 401 for a failed login. Deliberately does NOT distinguish
/// "no such user" from "wrong password" — no user enumeration.
fn login_failed() -> CliResponse {
    CliResponse {
        status: "401 Unauthorized",
        content_type: "application/json",
        body: r#"{"error":"invalid username or password"}"#.to_string(),
    }
}

/// `POST /cli/auth/login` `{username,password}` (PUBLIC — no token gate).
///
/// On success: `200 {token, username, expiresAt}`. On any failure (bad
/// body, unknown user, wrong password, disabled account): a generic
/// `401 {"error":"invalid username or password"}`.
///
/// The dispatcher adds a small fixed delay before returning THIS response
/// when it's a 401, to blunt online brute force (richer rate-limiting is
/// deferred). `connect_users::verify` already runs constant-ish argon2
/// work to blunt user-enumeration timing.
pub fn handle_login(body: &[u8]) -> CliResponse {
    let req: LoginReq = match serde_json::from_slice(body) {
        // A malformed body is an auth failure, not a 400 — don't help an
        // attacker probe the parser separately from the credential check.
        Ok(r) => r,
        Err(_) => return login_failed(),
    };
    if !connect_users::verify(&req.username, &req.password) {
        return login_failed();
    }
    // verify() lowercases internally; issue the session under the
    // canonical username so whoami/echo is stable.
    let username = connect_users::normalize_username(&req.username)
        .unwrap_or_else(|_| req.username.to_ascii_lowercase());
    let token = connect_users::create_session(&username);
    // Mirror the 30-day TTL the core applies (kept in sync by contract;
    // the authoritative expiry lives in the in-memory session).
    let expires_at = (chrono::Utc::now() + chrono::Duration::days(30)).to_rfc3339();
    CliResponse::ok_json(
        serde_json::json!({
            "token": token,
            "username": username,
            "expiresAt": expires_at,
        })
        .to_string(),
    )
}

/// `GET /cli/auth/whoami` (authorized — owner OR connect-user) →
/// `{username, owner}`.
///
/// - Owner token → `{"username":null,"owner":true}`.
/// - Connect-user session → `{"username":"<name>","owner":false}`.
///
/// The dispatcher resolves which by checking the owner token first, then
/// the session; it passes the result in.
pub fn handle_whoami(username: Option<String>, owner: bool) -> CliResponse {
    CliResponse::ok_json(
        serde_json::json!({
            "username": username,
            "owner": owner,
        })
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_rejects_malformed_body() {
        let r = handle_add(b"not json");
        assert_eq!(r.status, "400 Bad Request");
    }

    #[test]
    fn login_malformed_body_is_generic_401() {
        let r = handle_login(b"not json");
        assert_eq!(r.status, "401 Unauthorized");
        assert!(r.body.contains("invalid username or password"));
    }

    #[test]
    fn whoami_owner_shape() {
        let r = handle_whoami(None, true);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["owner"], serde_json::json!(true));
        assert_eq!(v["username"], serde_json::Value::Null);
    }

    #[test]
    fn whoami_connect_user_shape() {
        let r = handle_whoami(Some("alice".to_string()), false);
        let v: serde_json::Value = serde_json::from_str(&r.body).unwrap();
        assert_eq!(v["owner"], serde_json::json!(false));
        assert_eq!(v["username"], serde_json::json!("alice"));
    }
}
