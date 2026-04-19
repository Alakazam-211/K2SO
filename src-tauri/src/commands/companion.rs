use tauri::AppHandle;

/// Start the companion API proxy. Returns the ngrok tunnel URL.
#[tauri::command]
pub fn companion_start() -> Result<String, String> {
    crate::companion::start_companion()
}

/// Stop the companion API proxy.
#[tauri::command]
pub fn companion_stop() -> Result<(), String> {
    crate::companion::stop_companion()
}

/// Get companion status (running, URL, connected clients, sessions).
#[tauri::command]
pub fn companion_status() -> Result<serde_json::Value, String> {
    Ok(crate::companion::companion_status())
}

/// Set the companion password. Hashes with argon2, stores in the macOS
/// Keychain (preferred), or falls back to settings.json on other platforms
/// or if the Keychain is unavailable. Always invalidates any live sessions
/// so a rotated password can't be used with a stale token.
#[tauri::command]
pub fn companion_set_password(app: AppHandle, password: String) -> Result<(), String> {
    let hash = crate::companion::auth::hash_password(&password)?;

    // Try Keychain first. On success, clear the on-disk copy so only the
    // Keychain entry is authoritative.
    let keychain_ok = crate::companion::keychain::write_password_hash(&hash).is_ok();

    let updates = if keychain_ok {
        serde_json::json!({
            "companion": {
                "passwordHash": "",
                "passwordSet": true,
            }
        })
    } else {
        // Fallback: store the hash on disk under 0o600.
        serde_json::json!({
            "companion": {
                "passwordHash": hash,
                "passwordSet": true,
            }
        })
    };
    crate::commands::settings::settings_update(app, updates)?;

    // settings_update already invalidates on password_hash change, but the
    // Keychain path blanks password_hash first which skips that trigger.
    crate::companion::invalidate_all_sessions("password changed");
    Ok(())
}

/// Disconnect a specific companion session.
#[tauri::command]
pub fn companion_disconnect_session(session_token: String) -> Result<(), String> {
    let guard = crate::companion::STATE.lock();
    let state = guard.as_ref().ok_or("Companion is not running")?;

    // Remove session
    state.sessions.lock().remove(&session_token);

    // Remove associated WebSocket clients
    state.ws_clients.lock().retain(|c| c.session_token != session_token);

    Ok(())
}
