//! Connect-user accounts + sessions for the PUBLIC K2 Connect tunnel
//! surface (K2SO #617).
//!
//! **Two auth levels.** The daemon owner holds the local daemon token
//! (`~/.k2so/daemon.token`); that token is the OWNER credential — full
//! access including user management + tunnel control. A *connect-user*
//! is a username+password account the owner provisions; the remote
//! person logs in over `https://<sub>.k2.dev` and receives a session
//! token they then pass on every request. A connect-user gets general
//! daemon access but is NOT the owner (no user management, no tunnel
//! control). Granular per-user scopes are a FUTURE pass.
//!
//! **Storage.** Accounts live in `~/.k2so/connect-users.json`, written
//! 0600 (owner-only) via tmp+rename — mirroring `tunnel/config.rs`. The
//! file holds argon2id password *hashes*, never plaintext. Sessions are
//! in-memory only (an `OnceLock<Mutex<HashMap>>` singleton like the
//! tunnel connector): a daemon restart simply forces a re-login, which
//! is fine for a session token.
//!
//! **Security posture.** This is the auth boundary for the public
//! tunnel. `verify` always runs an argon2 hash even for an unknown user
//! (constant-ish work) to blunt timing-based user enumeration. The
//! daemon route layer adds a fixed failure delay on top to slow
//! brute-force; richer rate-limiting is deferred.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use argon2::password_hash::rand_core::{OsRng, RngCore};
use argon2::password_hash::SaltString;
use argon2::{Argon2, PasswordHash, PasswordHasher, PasswordVerifier};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};

/// Default session lifetime. A connect-user's session token is valid
/// for 30 days from issue; after that they re-login. In-memory store
/// means a daemon restart also ends the session early.
const SESSION_TTL_DAYS: i64 = 30;

/// A provisioned connect-user account. Persisted to
/// `~/.k2so/connect-users.json`. The `password_hash` is an argon2id
/// PHC string and is NEVER exposed off-disk (see [`ConnectUserView`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectUser {
    /// Unique, lowercased, `^[a-z0-9_-]{2,}$`.
    pub username: String,
    /// argon2id PHC hash string. Secret — only ever lives on the 0600
    /// file and is the input to [`verify`].
    pub password_hash: String,
    /// RFC3339 creation timestamp.
    pub created_at: DateTime<Utc>,
    /// When true the account cannot authenticate (login fails) but the
    /// row is retained so the owner can re-enable it.
    #[serde(default)]
    pub disabled: bool,
}

/// Redacted projection of a [`ConnectUser`] safe to return over the
/// wire — username + created_at + disabled, NEVER the hash.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ConnectUserView {
    pub username: String,
    pub created_at: DateTime<Utc>,
    pub disabled: bool,
}

impl From<&ConnectUser> for ConnectUserView {
    fn from(u: &ConnectUser) -> Self {
        Self {
            username: u.username.clone(),
            created_at: u.created_at,
            disabled: u.disabled,
        }
    }
}

// ─────────────────────────────────────────────────────────────────────
// Validation
// ─────────────────────────────────────────────────────────────────────

/// Normalize + validate a requested username. Lowercases, then enforces
/// `^[a-z0-9_-]{2,}$`. Returns the canonical (lowercased) form on
/// success, or an error string describing the rejection.
pub fn normalize_username(raw: &str) -> Result<String, String> {
    let lowered = raw.trim().to_ascii_lowercase();
    if lowered.len() < 2 {
        return Err("username must be at least 2 characters".to_string());
    }
    if !lowered
        .chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_' || c == '-')
    {
        return Err(
            "username may contain only lowercase letters, digits, '_' and '-'".to_string(),
        );
    }
    Ok(lowered)
}

// ─────────────────────────────────────────────────────────────────────
// Hashing
// ─────────────────────────────────────────────────────────────────────

/// Hash a password with argon2id + a fresh per-user random salt.
fn hash_password(password: &str) -> Result<String, String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| format!("password hashing failed: {e}"))
}

/// Verify a plaintext password against a stored argon2 PHC hash.
fn verify_hash(password: &str, hash: &str) -> bool {
    let parsed = match PasswordHash::new(hash) {
        Ok(h) => h,
        Err(_) => return false,
    };
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

// ─────────────────────────────────────────────────────────────────────
// Persistence (mirrors tunnel/config.rs: $HOME-relative, 0600, tmp+rename)
// ─────────────────────────────────────────────────────────────────────

fn config_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2so")
}

/// Path to `~/.k2so/connect-users.json`.
pub fn store_path() -> PathBuf {
    config_dir().join("connect-users.json")
}

#[cfg(unix)]
fn restrict_mode(file: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Err(e) = fs::set_permissions(file, fs::Permissions::from_mode(0o600)) {
        crate::log_debug!("[connect_users] WARN chmod 0600 {}: {e}", file.display());
    }
}

#[cfg(not(unix))]
fn restrict_mode(_file: &Path) {}

/// On-disk shape: a flat list of accounts.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Store {
    #[serde(default)]
    users: Vec<ConnectUser>,
}

/// Load the store. A missing/empty file yields an empty store; a
/// malformed file is an error (fail loud — never silently drop accounts
/// over a corrupt credential store).
fn load_store() -> Result<Store, String> {
    let file = store_path();
    if !file.exists() {
        return Ok(Store::default());
    }
    let raw = fs::read_to_string(&file).map_err(|e| format!("read {}: {e}", file.display()))?;
    if raw.trim().is_empty() {
        return Ok(Store::default());
    }
    serde_json::from_str(&raw).map_err(|e| format!("parse {}: {e}", file.display()))
}

/// Persist the store via tmp+rename then chmod 0600.
fn save_store(store: &Store) -> Result<(), String> {
    let dir = config_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("mkdir {}: {e}", dir.display()))?;
    let file = store_path();
    let tmp = dir.join(format!("connect-users.json.tmp.{}", std::process::id()));
    let body =
        serde_json::to_string_pretty(store).map_err(|e| format!("serialize store: {e}"))?;
    fs::write(&tmp, body.as_bytes()).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    restrict_mode(&tmp);
    fs::rename(&tmp, &file).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("rename into place {}: {e}", file.display())
    })?;
    restrict_mode(&file);
    Ok(())
}

/// Load-modify-save under a process lock so concurrent mutations don't
/// clobber each other.
fn update_store<R>(f: impl FnOnce(&mut Store) -> Result<R, String>) -> Result<R, String> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    let _g = LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    let mut store = load_store()?;
    let r = f(&mut store)?;
    save_store(&store)?;
    Ok(r)
}

// ─────────────────────────────────────────────────────────────────────
// Account management (owner-only at the route layer)
// ─────────────────────────────────────────────────────────────────────

/// List all accounts as redacted views (no hashes). Sorted by username.
pub fn list_users() -> Result<Vec<ConnectUserView>, String> {
    let store = load_store()?;
    let mut views: Vec<ConnectUserView> = store.users.iter().map(ConnectUserView::from).collect();
    views.sort_by(|a, b| a.username.cmp(&b.username));
    Ok(views)
}

/// Provision a new account. Validates + lowercases the username, rejects
/// duplicates, hashes the password with a fresh salt.
pub fn add_user(username: &str, password: &str) -> Result<(), String> {
    let username = normalize_username(username)?;
    if password.is_empty() {
        return Err("password must not be empty".to_string());
    }
    let hash = hash_password(password)?;
    update_store(|store| {
        if store.users.iter().any(|u| u.username == username) {
            return Err(format!("user '{username}' already exists"));
        }
        store.users.push(ConnectUser {
            username,
            password_hash: hash,
            created_at: Utc::now(),
            disabled: false,
        });
        Ok(())
    })
}

/// Remove an account and revoke any live sessions for it.
pub fn remove_user(username: &str) -> Result<(), String> {
    let username = normalize_username(username)?;
    update_store(|store| {
        let before = store.users.len();
        store.users.retain(|u| u.username != username);
        if store.users.len() == before {
            return Err(format!("user '{username}' not found"));
        }
        Ok(())
    })?;
    revoke_user_sessions(&username);
    Ok(())
}

/// Reset an account's password (re-hashes) and revoke its live sessions
/// so the old session token can't be replayed after a rotation.
pub fn set_password(username: &str, password: &str) -> Result<(), String> {
    let username = normalize_username(username)?;
    if password.is_empty() {
        return Err("password must not be empty".to_string());
    }
    let hash = hash_password(password)?;
    update_store(|store| {
        let user = store
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or_else(|| format!("user '{username}' not found"))?;
        user.password_hash = hash;
        Ok(())
    })?;
    revoke_user_sessions(&username);
    Ok(())
}

/// Enable/disable an account. Disabling revokes live sessions so a
/// disabled user is immediately locked out, not just blocked at next
/// login.
pub fn set_disabled(username: &str, disabled: bool) -> Result<(), String> {
    let username = normalize_username(username)?;
    update_store(|store| {
        let user = store
            .users
            .iter_mut()
            .find(|u| u.username == username)
            .ok_or_else(|| format!("user '{username}' not found"))?;
        user.disabled = disabled;
        Ok(())
    })?;
    if disabled {
        revoke_user_sessions(&username);
    }
    Ok(())
}

/// Verify a username+password against the store. Returns `true` only for
/// a known, NON-disabled account whose password matches.
///
/// **Anti-enumeration:** always runs an argon2 verify even when the user
/// is unknown (against a throwaway hash) so the response time doesn't
/// leak whether the username exists. No early return on miss.
pub fn verify(username: &str, password: &str) -> bool {
    // Normalize without leaking via early-return: an invalid username can
    // never match a stored (already-normalized) one, but we still do the
    // dummy hash work below before returning.
    let normalized = normalize_username(username).ok();

    let store = match load_store() {
        Ok(s) => s,
        Err(_) => None.unwrap_or_default(),
    };

    let found = normalized
        .as_ref()
        .and_then(|n| store.users.iter().find(|u| &u.username == n));

    // A process-wide dummy hash so the unknown/disabled branches incur the
    // same argon2 cost as a real verify — blunting user-enumeration timing.
    static DUMMY_HASH: OnceLock<String> = OnceLock::new();
    let dummy = DUMMY_HASH.get_or_init(build_dummy_hash);

    match found {
        Some(user) if !user.disabled => verify_hash(password, &user.password_hash),
        Some(_) => {
            // Disabled: still burn a hash cycle, then fail.
            let _ = verify_hash(password, dummy);
            false
        }
        None => {
            // Unknown user: run a verify against a fixed dummy hash so the
            // timing matches the found-user path. Result is discarded.
            let _ = verify_hash(password, dummy);
            false
        }
    }
}

fn build_dummy_hash() -> String {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(b"k2so-dummy-verify-target", &salt)
        .map(|h| h.to_string())
        // Fall back to a baked PHC-shaped string only if hashing fails
        // (effectively never); verify against it will just return false.
        .unwrap_or_else(|_| String::from("$argon2id$v=19$m=19456,t=2,p=1$AAAAAAAAAAAAAAAAAAAAAA$AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"))
}

// ─────────────────────────────────────────────────────────────────────
// Sessions (in-memory singleton)
// ─────────────────────────────────────────────────────────────────────

/// A live login session for a connect-user.
#[derive(Debug, Clone)]
pub struct Session {
    pub username: String,
    pub expires_at: DateTime<Utc>,
}

fn sessions() -> &'static Mutex<HashMap<String, Session>> {
    static SESSIONS: OnceLock<Mutex<HashMap<String, Session>>> = OnceLock::new();
    SESSIONS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn lock_sessions() -> std::sync::MutexGuard<'static, HashMap<String, Session>> {
    sessions().lock().unwrap_or_else(|p| p.into_inner())
}

/// Generate a 32-byte (256-bit) random token, hex-encoded.
fn new_token() -> String {
    let mut bytes = [0u8; 32];
    OsRng.fill_bytes(&mut bytes);
    let mut s = String::with_capacity(64);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Issue a new session token for an authenticated `username`. Default
/// 30-day expiry. The caller is responsible for having already verified
/// credentials.
pub fn create_session(username: &str) -> String {
    let token = new_token();
    let session = Session {
        username: username.to_string(),
        expires_at: Utc::now() + Duration::days(SESSION_TTL_DAYS),
    };
    lock_sessions().insert(token.clone(), session);
    token
}

/// Validate a session token. Returns the owning username if the session
/// exists and hasn't expired; lazily drops an expired session.
pub fn validate_session(token: &str) -> Option<String> {
    let mut map = lock_sessions();
    let expired = match map.get(token) {
        Some(s) if s.expires_at > Utc::now() => return Some(s.username.clone()),
        Some(_) => true,
        None => false,
    };
    if expired {
        map.remove(token);
    }
    None
}

/// Revoke every live session belonging to `username`. Called on remove,
/// disable, and password rotation so a stale token can't outlive the
/// account change.
pub fn revoke_user_sessions(username: &str) {
    lock_sessions().retain(|_, s| s.username != username);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::test_support::with_temp_home;

    #[test]
    fn hash_then_verify_round_trips() {
        let h = hash_password("correct horse battery staple").expect("hash");
        assert!(verify_hash("correct horse battery staple", &h));
    }

    #[test]
    fn wrong_password_fails_verify() {
        let h = hash_password("hunter2").expect("hash");
        assert!(!verify_hash("hunter3", &h));
    }

    #[test]
    fn normalize_username_lowercases_and_validates() {
        assert_eq!(normalize_username("Rosson").unwrap(), "rosson");
        assert_eq!(normalize_username("  a_b-1 ").unwrap(), "a_b-1");
        assert!(normalize_username("a").is_err(), "too short");
        assert!(normalize_username("has space").is_err());
        assert!(normalize_username("Bang!").is_err());
    }

    #[test]
    fn add_user_then_verify_succeeds() {
        with_temp_home(|| {
            add_user("Alice", "s3cret").expect("add");
            // Stored lowercased; verify is case-insensitive on the name.
            assert!(verify("alice", "s3cret"));
            assert!(verify("ALICE", "s3cret"));
            assert!(!verify("alice", "wrong"));
        });
    }

    #[test]
    fn verify_unknown_user_is_false_and_still_does_work() {
        with_temp_home(|| {
            // No users provisioned at all.
            assert!(!verify("ghost", "whatever"));
            // Add one, then check a different unknown user still false.
            add_user("real", "pw").expect("add");
            assert!(!verify("nobody", "pw"));
        });
    }

    #[test]
    fn add_rejects_duplicate() {
        with_temp_home(|| {
            add_user("bob", "pw1").expect("add");
            let err = add_user("BOB", "pw2").expect_err("dup must reject");
            assert!(err.contains("already exists"), "got: {err}");
        });
    }

    #[test]
    fn add_rejects_invalid_username() {
        with_temp_home(|| {
            assert!(add_user("x", "pw").is_err(), "too short");
            assert!(add_user("bad name", "pw").is_err(), "space");
            assert!(add_user("ok", "").is_err(), "empty password");
        });
    }

    #[test]
    fn list_users_redacts_hash() {
        with_temp_home(|| {
            add_user("carol", "pw").expect("add");
            let views = list_users().expect("list");
            assert_eq!(views.len(), 1);
            assert_eq!(views[0].username, "carol");
            assert!(!views[0].disabled);
            // ConnectUserView simply has no hash field — compile-time
            // guarantee. Serialize and confirm no hash leaks.
            let json = serde_json::to_string(&views[0]).unwrap();
            assert!(!json.contains("password"), "view leaked a hash: {json}");
        });
    }

    #[test]
    fn set_password_changes_credential_and_revokes_sessions() {
        with_temp_home(|| {
            add_user("dave", "old").expect("add");
            let tok = create_session("dave");
            assert_eq!(validate_session(&tok), Some("dave".to_string()));
            set_password("dave", "new").expect("set-password");
            assert!(!verify("dave", "old"));
            assert!(verify("dave", "new"));
            assert_eq!(validate_session(&tok), None, "rotation must revoke session");
        });
    }

    #[test]
    fn set_disabled_blocks_login_and_revokes_sessions() {
        with_temp_home(|| {
            add_user("eve", "pw").expect("add");
            let tok = create_session("eve");
            set_disabled("eve", true).expect("disable");
            assert!(!verify("eve", "pw"), "disabled user must not verify");
            assert_eq!(validate_session(&tok), None, "disable must revoke session");
            set_disabled("eve", false).expect("re-enable");
            assert!(verify("eve", "pw"), "re-enabled user verifies again");
        });
    }

    #[test]
    fn session_create_validate_expire() {
        with_temp_home(|| {
            let tok = create_session("frank");
            assert_eq!(validate_session(&tok), Some("frank".to_string()));

            // Construct an already-expired session directly and confirm
            // validate drops it.
            let expired_tok = new_token();
            lock_sessions().insert(
                expired_tok.clone(),
                Session {
                    username: "frank".to_string(),
                    expires_at: Utc::now() - Duration::seconds(1),
                },
            );
            assert_eq!(validate_session(&expired_tok), None, "expired -> None");
            // Lazily dropped.
            assert!(
                !lock_sessions().contains_key(&expired_tok),
                "expired session must be evicted on validate"
            );
        });
    }

    #[test]
    fn remove_user_revokes_sessions() {
        with_temp_home(|| {
            add_user("grace", "pw").expect("add");
            let tok = create_session("grace");
            assert_eq!(validate_session(&tok), Some("grace".to_string()));
            remove_user("grace").expect("remove");
            assert_eq!(validate_session(&tok), None, "remove must revoke session");
            assert!(!verify("grace", "pw"), "removed user must not verify");
            let err = remove_user("grace").expect_err("removing twice errors");
            assert!(err.contains("not found"), "got: {err}");
        });
    }

    #[test]
    fn unknown_token_validates_to_none() {
        with_temp_home(|| {
            assert_eq!(validate_session("deadbeef"), None);
        });
    }

    #[test]
    fn token_is_64_hex_chars() {
        let t = new_token();
        assert_eq!(t.len(), 64, "32 bytes -> 64 hex chars");
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
