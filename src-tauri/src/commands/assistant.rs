use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::llm::download;
use crate::llm::file_index;
use crate::llm::tools::{self, AssistantResponse};
use crate::state::AppState;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantStatus {
    pub loaded: bool,
    pub model_path: Option<String>,
    pub downloading: bool,
}

/// A single LLM inference pass for debug logging.
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct DebugPass {
    /// What was sent as the user message for this pass.
    pub prompt: String,
    /// The raw text the LLM produced.
    pub raw_output: String,
}

/// Serializable response sent to the frontend.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    pub raw: String,
    pub parsed: AssistantResponse,
    /// Debug trace of each inference pass (for tuning/debugging).
    pub debug_passes: Vec<DebugPass>,
}

/// Execute any file-browsing tool calls (`list_files`, `search_files`)
/// server-side and return the combined results as a string for the next
/// LLM inference pass.
fn execute_file_tools(
    tool_calls: &[tools::ToolCall],
    workspace_path: &str,
) -> Option<String> {
    let mut results = Vec::new();

    for call in tool_calls {
        match call.tool.as_str() {
            "list_files" => {
                let rel_path = call
                    .args
                    .get("path")
                    .and_then(|v| v.as_str())
                    .unwrap_or(".");

                let abs_path = if rel_path == "." || rel_path.is_empty() {
                    std::path::PathBuf::from(workspace_path)
                } else {
                    std::path::Path::new(workspace_path).join(rel_path)
                };

                let listing = file_index::list_directory(abs_path.to_string_lossy().as_ref());
                results.push(format!("[list_files: {rel_path}]\n{listing}"));
            }
            "search_files" => {
                let query = call
                    .args
                    .get("query")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                if !query.is_empty() {
                    let search_results = file_index::search_files(workspace_path, query);
                    results.push(format!("[search_files: \"{query}\"]\n{search_results}"));
                }
            }
            _ => {}
        }
    }

    if results.is_empty() {
        None
    } else {
        Some(results.join("\n"))
    }
}

/// Send a message to the local LLM and get a response.
/// Accepts `workspace_path` so the LLM can browse files via `list_files`.
///
/// If the LLM's first response contains `list_files` tool calls, they are
/// executed server-side and the results are fed back for a second inference
/// pass (max 2 passes to keep latency bounded).
#[tauri::command]
pub fn assistant_chat(
    state: State<'_, AppState>,
    message: String,
    workspace_path: Option<String>,
) -> Result<ChatResponse, String> {
    let manager = state
        .llm_manager
        .lock()
        .map_err(|e| format!("Failed to lock LLM manager: {e}"))?;

    let system_prompt = tools::WORKSPACE_SYSTEM_PROMPT;
    let mut debug_passes: Vec<DebugPass> = Vec::new();

    eprintln!("[assistant] User message: {message}");

    // First pass
    let raw = manager.generate(system_prompt, &message)?;
    eprintln!("[assistant] Raw LLM response (pass 1): {raw}");
    debug_passes.push(DebugPass {
        prompt: message.clone(),
        raw_output: raw.clone(),
    });
    let parsed = tools::parse_llm_response(&raw);

    // Check if the LLM wants to browse/search files
    if let AssistantResponse::ToolCalls { ref tool_calls } = parsed {
        let has_file_tools = tool_calls
            .iter()
            .any(|c| c.tool == "list_files" || c.tool == "search_files");

        if has_file_tools {
            if let Some(ref ws_path) = workspace_path {
                let listing = execute_file_tools(tool_calls, ws_path);

                if let Some(listing_text) = listing {
                    let action_calls: Vec<_> = tool_calls
                        .iter()
                        .filter(|c| c.tool != "list_files" && c.tool != "search_files")
                        .cloned()
                        .collect();

                    let follow_up = format!(
                        "File listing results:\n{listing_text}\n\nOriginal request: {message}\n\nNow output the tool_calls to fulfill the request using the file paths above."
                    );
                    eprintln!("[assistant] Follow-up prompt (pass 2): {follow_up}");

                    let raw2 = manager.generate(system_prompt, &follow_up)?;
                    eprintln!("[assistant] Raw LLM response (pass 2): {raw2}");
                    debug_passes.push(DebugPass {
                        prompt: follow_up,
                        raw_output: raw2.clone(),
                    });
                    let parsed2 = tools::parse_llm_response(&raw2);

                    let final_parsed = if action_calls.is_empty() {
                        parsed2
                    } else if let AssistantResponse::ToolCalls {
                        tool_calls: mut pass2_calls,
                    } = parsed2
                    {
                        let mut merged = action_calls;
                        merged.append(&mut pass2_calls);
                        AssistantResponse::ToolCalls {
                            tool_calls: merged,
                        }
                    } else if action_calls.is_empty() {
                        parsed2
                    } else {
                        AssistantResponse::ToolCalls {
                            tool_calls: action_calls,
                        }
                    };

                    return Ok(ChatResponse {
                        raw: raw2,
                        parsed: final_parsed,
                        debug_passes,
                    });
                }
            }
        }
    }

    Ok(ChatResponse {
        raw,
        parsed,
        debug_passes,
    })
}

/// Get the current status of the assistant.
/// Uses try_lock to avoid blocking when the model is loading on a
/// background thread (Metal shader init can take ~9 seconds).
#[tauri::command]
pub fn assistant_status(state: State<'_, AppState>) -> Result<AssistantStatus, String> {
    match state.llm_manager.try_lock() {
        Ok(manager) => Ok(AssistantStatus {
            loaded: manager.is_loaded(),
            model_path: manager.get_model_path(),
            downloading: manager.is_downloading(),
        }),
        Err(_) => {
            // Lock held by model loading thread — report as loading
            Ok(AssistantStatus {
                loaded: false,
                model_path: None,
                downloading: false,
            })
        }
    }
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
