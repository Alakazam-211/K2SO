use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::llm::download;
use crate::llm::tools::{self, AssistantResponse};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantStatus {
    pub loaded: bool,
    pub model_path: Option<String>,
    pub downloading: bool,
}

/// Serializable response sent to the frontend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    pub raw: String,
    pub parsed: AssistantResponse,
}

/// Send a message to the local LLM and get a response.
#[tauri::command]
pub fn assistant_chat(
    state: State<'_, AppState>,
    message: String,
) -> Result<ChatResponse, String> {
    let manager = state
        .llm_manager
        .lock()
        .map_err(|e| format!("Failed to lock LLM manager: {e}"))?;

    eprintln!("[assistant] User message: {message}");
    let raw = manager.generate(tools::WORKSPACE_SYSTEM_PROMPT, &message)?;
    eprintln!("[assistant] Raw LLM response: {raw}");
    let parsed = tools::parse_llm_response(&raw);
    let serialized = serde_json::to_string(&parsed).unwrap_or_default();
    eprintln!("[assistant] Parsed (serialized to frontend): {serialized}");

    Ok(ChatResponse { raw, parsed })
}

/// Get the current status of the assistant.
#[tauri::command]
pub fn assistant_status(state: State<'_, AppState>) -> Result<AssistantStatus, String> {
    let manager = state
        .llm_manager
        .lock()
        .map_err(|e| format!("Failed to lock LLM manager: {e}"))?;

    Ok(AssistantStatus {
        loaded: manager.is_loaded(),
        model_path: manager.get_model_path(),
        downloading: manager.is_downloading(),
    })
}

/// Load a model from a specific file path.
/// If the file is outside ~/.k2so/models/, copies it there first.
#[tauri::command]
pub fn assistant_load_model(
    state: State<'_, AppState>,
    path: String,
) -> Result<String, String> {
    let src = std::path::PathBuf::from(&path);
    if !src.exists() {
        return Err(format!("File not found: {path}"));
    }

    let models_dir = download::models_dir()?;
    let final_path = if src.starts_with(&models_dir) {
        // Already in our models directory
        path.clone()
    } else {
        // Copy to ~/.k2so/models/
        std::fs::create_dir_all(&models_dir)
            .map_err(|e| format!("Failed to create models directory: {e}"))?;

        let filename = src.file_name()
            .ok_or_else(|| "Invalid file path".to_string())?;
        let dest = models_dir.join(filename);

        eprintln!("[llm] Copying model to {:?}", dest);
        std::fs::copy(&src, &dest)
            .map_err(|e| format!("Failed to copy model: {e}"))?;

        dest.to_string_lossy().to_string()
    };

    let mut manager = state
        .llm_manager
        .lock()
        .map_err(|e| format!("Failed to lock LLM manager: {e}"))?;

    manager.load_model(&final_path)?;
    Ok(final_path)
}

/// Download the default Qwen2.5-1.5B model from HuggingFace.
/// Runs the download on a background thread to avoid blocking the UI.
#[tauri::command]
pub fn assistant_download_default_model(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<(), String> {
    // Check if already downloading
    let manager = state
        .llm_manager
        .lock()
        .map_err(|e| format!("Failed to lock LLM manager: {e}"))?;

    if manager.is_downloading() {
        return Err("A download is already in progress".to_string());
    }

    // Set downloading flag
    manager
        .downloading
        .store(true, std::sync::atomic::Ordering::Relaxed);

    drop(manager); // Release the lock before spawning

    let dest = download::default_model_path()?;
    let dest_str = dest
        .to_str()
        .ok_or_else(|| "Invalid model path".to_string())?
        .to_string();
    let url = download::DEFAULT_MODEL_URL.to_string();

    // Clone the state handle for the background thread
    let state_handle = app.clone();

    std::thread::spawn(move || {
        let result = download::download_model(&url, &dest_str, app);

        // Clear downloading flag
        if let Some(app_state) = state_handle.try_state::<AppState>() {
            match app_state.llm_manager.lock() {
                Ok(mgr) => {
                    mgr.downloading
                        .store(false, std::sync::atomic::Ordering::Relaxed);
                }
                Err(e) => {
                    eprintln!("Failed to lock LLM manager to clear downloading flag: {e}");
                }
            }

            // If download succeeded, auto-load the model
            if result.is_ok() {
                match app_state.llm_manager.lock() {
                    Ok(mut mgr) => {
                        let _ = mgr.load_model(&dest_str);
                    }
                    Err(e) => {
                        eprintln!("Failed to lock LLM manager to load model: {e}");
                    }
                }
            }
        }

        if let Err(e) = result {
            eprintln!("Model download failed: {e}");
        }
    });

    Ok(())
}

/// Check if the default model file exists at ~/.k2so/models/.
#[tauri::command]
pub fn assistant_check_model(_state: State<'_, AppState>) -> Result<bool, String> {
    download::default_model_exists()
}
