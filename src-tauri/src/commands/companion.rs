use tauri::AppHandle;

/// Start the companion API proxy. Returns the ngrok tunnel URL.
#[tauri::command]
pub fn companion_start(app: AppHandle) -> Result<String, String> {
    crate::companion::start_companion(app)
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

/// Set the companion password. Hashes with argon2 and stores in settings.
#[tauri::command]
pub fn companion_set_password(app: AppHandle, password: String) -> Result<(), String> {
    let hash = crate::companion::auth::hash_password(&password)?;

    // Update settings with the hashed password
    let updates = serde_json::json!({
        "companion": {
            "passwordHash": hash,
        }
    });
    crate::commands::settings::settings_update(app, updates)?;
    Ok(())
}

/// Disconnect a specific companion session.
#[tauri::command]
pub fn companion_disconnect_session(session_token: String) -> Result<(), String> {
    let guard = crate::companion::STATE.lock().unwrap();
    let state = guard.as_ref().ok_or("Companion is not running")?;

    // Remove session
    state.sessions.lock().unwrap().remove(&session_token);

    // Remove associated WebSocket clients
    state.ws_clients.lock().unwrap().retain(|c| c.session_token != session_token);

    Ok(())
}
