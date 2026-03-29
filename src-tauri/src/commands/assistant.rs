use serde::Serialize;
use std::sync::atomic::{AtomicU32, Ordering};
use tauri::{AppHandle, Manager, State};

use crate::llm::download;
use crate::llm::file_index;
use crate::llm::tools::{self, AssistantResponse};
use crate::state::AppState;

/// Maximum concurrent LLM worker subprocesses (prevents GPU/CPU exhaustion).
/// Zed pattern: semaphore-style concurrency control for resource-intensive operations.
static LLM_ACTIVE_WORKERS: AtomicU32 = AtomicU32::new(0);
const MAX_LLM_WORKERS: u32 = 2;

/// Maximum size of LLM subprocess stdout (10MB) to prevent OOM from runaway output.
const MAX_LLM_OUTPUT_BYTES: usize = 10 * 1024 * 1024;

/// RAII guard that cleans up the temp file when dropped (even on panic).
/// Zed pattern: `defer()` / `Deferred<F>` for guaranteed cleanup.
struct TempFileGuard {
    path: std::path::PathBuf,
}
impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

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
/// Run LLM inference in a child process to isolate Metal/ggml crashes.
/// The ggml backend calls C abort() on certain Metal failures, which kills
/// the entire process. No signal handler or catch_unwind can prevent this.
/// By forking into a subprocess, crashes only kill the child — K2SO survives.
fn safe_generate(
    _manager: &crate::llm::LlmManager,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    // Concurrency guard: limit concurrent LLM workers to prevent GPU/CPU exhaustion
    let current = LLM_ACTIVE_WORKERS.load(Ordering::Relaxed);
    if current >= MAX_LLM_WORKERS {
        return Err(format!("Too many concurrent LLM workers ({}/{}). Try again shortly.", current, MAX_LLM_WORKERS));
    }
    LLM_ACTIVE_WORKERS.fetch_add(1, Ordering::Relaxed);

    // Get the model path so the child process can load it independently
    let model_path = _manager.get_model_path()
        .ok_or_else(|| {
            LLM_ACTIVE_WORKERS.fetch_sub(1, Ordering::Relaxed);
            "No model loaded".to_string()
        })?;

    let exe = std::env::current_exe().map_err(|e| {
        LLM_ACTIVE_WORKERS.fetch_sub(1, Ordering::Relaxed);
        format!("Cannot find exe: {e}")
    })?;

    // Pass system prompt and user message via a temp file to avoid arg length limits
    // RAII guard ensures cleanup even on panic (Zed's defer pattern)
    let tmp = std::env::temp_dir().join(format!("k2so-llm-{}-{}.json", std::process::id(),
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()));
    let _tmp_guard = TempFileGuard { path: tmp.clone() };
    let payload = serde_json::json!({
        "model": model_path,
        "system": system_prompt,
        "message": user_message,
    });
    std::fs::write(&tmp, payload.to_string())
        .map_err(|e| {
            LLM_ACTIVE_WORKERS.fetch_sub(1, Ordering::Relaxed);
            format!("Failed to write LLM payload: {e}")
        })?;

    let mut child = std::process::Command::new(&exe)
        .args(["--llm-worker", tmp.to_string_lossy().as_ref()])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            LLM_ACTIVE_WORKERS.fetch_sub(1, Ordering::Relaxed);
            format!("Failed to spawn LLM worker: {e}")
        })?;

    // Timeout: kill subprocess if it takes longer than 45 seconds
    let timeout = std::time::Duration::from_secs(45);
    let start = std::time::Instant::now();
    let output = loop {
        match child.try_wait() {
            Ok(Some(_status)) => break child.wait_with_output().map_err(|e| {
                LLM_ACTIVE_WORKERS.fetch_sub(1, Ordering::Relaxed);
                format!("LLM worker error: {e}")
            })?,
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    LLM_ACTIVE_WORKERS.fetch_sub(1, Ordering::Relaxed);
                    return Err("LLM inference timed out (45s limit)".to_string());
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(e) => {
                LLM_ACTIVE_WORKERS.fetch_sub(1, Ordering::Relaxed);
                return Err(format!("LLM worker error: {e}"));
            }
        }
    };

    LLM_ACTIVE_WORKERS.fetch_sub(1, Ordering::Relaxed);
    // Note: _tmp_guard handles cleanup via Drop

    let exit_code = output.status.code();
    let stderr = String::from_utf8_lossy(&output.stderr);
    // Log token count from worker stderr if present
    for line in stderr.lines() {
        if line.contains("Prompt:") || line.contains("tokens") {
            log_debug!("[llm-worker] {}", line.trim());
        }
    }

    if output.status.success() {
        // Guard against runaway LLM output (prevents OOM)
        if output.stdout.len() > MAX_LLM_OUTPUT_BYTES {
            return Err(format!("LLM output too large ({} bytes, max {})", output.stdout.len(), MAX_LLM_OUTPUT_BYTES));
        }
        let result = String::from_utf8_lossy(&output.stdout).trim().to_string();
        log_debug!("[llm-worker] Output: {} bytes", result.len());
        if result.is_empty() {
            Err("LLM produced empty output".to_string())
        } else {
            Ok(result)
        }
    } else {
        let stderr_trimmed = stderr.trim().to_string();
        log_debug!("[llm-worker] Failed: exit={:?} stderr_len={}", exit_code, stderr_trimmed.len());
        if exit_code.is_none() || stderr_trimmed.contains("abort") {
            // Signal kill (no exit code) = Metal crash
            Err(format!("LLM crashed (signal). Last stderr: {}",
                stderr_trimmed.lines().rev().take(3).collect::<Vec<_>>().join(" | ")))
        } else {
            Err(format!("LLM error: {}", if stderr_trimmed.is_empty() { "unknown" } else { &stderr_trimmed }))
        }
    }
}

/// Public wrapper for safe_generate — used by the agent triage system.
/// Runs LLM inference in a subprocess to isolate Metal/ggml crashes.
pub fn safe_generate_for_triage(
    manager: &crate::llm::LlmManager,
    system_prompt: &str,
    user_message: &str,
) -> Result<String, String> {
    safe_generate(manager, system_prompt, user_message)
}

#[tauri::command]
pub fn assistant_chat(
    state: State<'_, AppState>,
    message: String,
    workspace_path: Option<String>,
    is_git_repo: Option<bool>,
) -> Result<ChatResponse, String> {
    let manager = state.llm_manager.lock();

    // Guard: ensure model is loaded before attempting inference
    if !manager.is_loaded() {
        return Err("Model is still loading. Please wait a moment and try again.".to_string());
    }

    let system_prompt = tools::build_system_prompt(is_git_repo.unwrap_or(false));
    let mut debug_passes: Vec<DebugPass> = Vec::new();

    log_debug!("[assistant] User message: {message}");
    log_debug!("[assistant] System prompt length: {} chars", system_prompt.len());

    // First pass (wrapped in catch_unwind to survive Metal crashes)
    let raw = safe_generate(&manager, &system_prompt, &message)?;
    log_debug!("[assistant] Raw LLM response (pass 1): {raw}");
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
                    log_debug!("[assistant] Follow-up prompt (pass 2): {follow_up}");

                    let raw2 = safe_generate(&manager, &system_prompt, &follow_up)?;
                    log_debug!("[assistant] Raw LLM response (pass 2): {raw2}");
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
        Some(manager) => Ok(AssistantStatus {
            loaded: manager.is_loaded(),
            model_path: manager.get_model_path(),
            downloading: manager.is_downloading(),
        }),
        None => {
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

        log_debug!("[llm] Copying model to {:?}", dest);
        std::fs::copy(&src, &dest)
            .map_err(|e| format!("Failed to copy model: {e}"))?;

        dest.to_string_lossy().to_string()
    };

    let mut manager = state.llm_manager.lock();

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
    let manager = state.llm_manager.lock();

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
            app_state.llm_manager.lock()
                .downloading
                .store(false, std::sync::atomic::Ordering::Relaxed);

            // If download succeeded, auto-load the model
            if result.is_ok() {
                let mut mgr = app_state.llm_manager.lock();
                let _ = mgr.load_model(&dest_str);
            }
        }

        if let Err(e) = result {
            log_debug!("Model download failed: {e}");
        }
    });

    Ok(())
}

/// Check if the default model file exists at ~/.k2so/models/.
#[tauri::command]
pub fn assistant_check_model(_state: State<'_, AppState>) -> Result<bool, String> {
    download::default_model_exists()
}
