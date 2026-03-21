use serde::Serialize;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

/// Default model: Qwen2.5-1.5B-Instruct Q4_K_M (~1.1 GB)
pub const DEFAULT_MODEL_URL: &str =
    "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf";

pub const DEFAULT_MODEL_FILENAME: &str = "qwen2.5-1.5b-instruct-q4_k_m.gguf";

/// Progress event payload emitted during model download.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub percent: f64,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
}

/// Returns the default models directory: ~/.k2so/models/
pub fn models_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or_else(|| "Cannot determine home directory".to_string())?;
    Ok(home.join(".k2so").join("models"))
}

/// Returns the full path to the default model file.
pub fn default_model_path() -> Result<PathBuf, String> {
    Ok(models_dir()?.join(DEFAULT_MODEL_FILENAME))
}

/// Checks if the default model file exists.
pub fn default_model_exists() -> Result<bool, String> {
    let path = default_model_path()?;
    Ok(path.exists() && path.metadata().map(|m| m.len() > 0).unwrap_or(false))
}

/// Downloads a GGUF model file from a URL with progress events.
///
/// Creates the destination directory if needed.
/// Emits `assistant:download-progress` events to the frontend.
pub fn download_model(url: &str, dest_path: &str, app_handle: AppHandle) -> Result<(), String> {
    // Ensure parent directory exists
    let dest = PathBuf::from(dest_path);
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create models directory: {e}"))?;
    }

    // Use a temp file to avoid partial downloads being mistaken for complete ones
    let tmp_path = format!("{dest_path}.tmp");

    // Perform the download using a blocking HTTP client
    let response = reqwest::blocking::Client::new()
        .get(url)
        .header("User-Agent", "K2SO/0.1")
        .send()
        .map_err(|e| format!("Failed to start download: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "Download failed with HTTP status: {}",
            response.status()
        ));
    }

    let total_bytes = response.content_length().unwrap_or(0);
    let mut bytes_downloaded: u64 = 0;

    let mut file =
        fs::File::create(&tmp_path).map_err(|e| format!("Failed to create temp file: {e}"))?;

    // Read in chunks and emit progress
    let mut stream = response;
    let mut buffer = vec![0u8; 256 * 1024]; // 256KB chunks

    loop {
        let bytes_read = std::io::Read::read(&mut stream, &mut buffer)
            .map_err(|e| format!("Download read error: {e}"))?;

        if bytes_read == 0 {
            break;
        }

        file.write_all(&buffer[..bytes_read])
            .map_err(|e| format!("Failed to write to file: {e}"))?;

        bytes_downloaded += bytes_read as u64;

        let percent = if total_bytes > 0 {
            (bytes_downloaded as f64 / total_bytes as f64) * 100.0
        } else {
            0.0
        };

        // Emit progress event (best-effort, don't fail on emit errors)
        let _ = app_handle.emit(
            "assistant:download-progress",
            DownloadProgress {
                percent,
                bytes_downloaded,
                total_bytes,
            },
        );
    }

    file.flush()
        .map_err(|e| format!("Failed to flush file: {e}"))?;
    drop(file);

    // Rename tmp file to final destination
    fs::rename(&tmp_path, dest_path)
        .map_err(|e| format!("Failed to rename downloaded file: {e}"))?;

    Ok(())
}
