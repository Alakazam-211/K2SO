use std::path::Path;
use std::process::Command;

/// Detect the formatter command for a file extension.
fn formatter_for_ext(ext: &str) -> Option<(&'static str, Vec<&'static str>)> {
    match ext {
        // JavaScript / TypeScript / CSS / HTML / JSON / Markdown
        "js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx"
        | "css" | "scss" | "less" | "html" | "htm" | "json" | "jsonc"
        | "md" | "mdx" | "yaml" | "yml" | "vue" | "svelte" | "astro"
        | "graphql" | "gql" => Some(("npx", vec!["prettier", "--write"])),

        // Rust
        "rs" => Some(("rustfmt", vec![])),

        // Python
        "py" | "pyw" | "pyi" => Some(("black", vec![])),

        // Go
        "go" => Some(("gofmt", vec!["-w"])),

        // Swift
        "swift" => Some(("swift-format", vec!["format", "--in-place"])),

        // C / C++
        "c" | "cpp" | "h" | "hpp" | "cc" | "cxx" => Some(("clang-format", vec!["-i"])),

        _ => None,
    }
}

/// Check if a formatter is available on PATH.
fn is_available(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[tauri::command]
pub fn format_file(file_path: String) -> Result<String, String> {
    let path = Path::new(&file_path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    let (cmd, base_args) = formatter_for_ext(ext)
        .ok_or_else(|| format!("No formatter configured for .{} files", ext))?;

    // For npx we don't need to check PATH (it bootstraps itself)
    if cmd != "npx" && !is_available(cmd) {
        return Err(format!("Formatter '{}' not found on PATH", cmd));
    }

    let mut args = base_args.iter().map(|s| s.to_string()).collect::<Vec<_>>();
    args.push(file_path.clone());

    let output = Command::new(cmd)
        .args(&args)
        .current_dir(path.parent().unwrap_or(Path::new(".")))
        .output()
        .map_err(|e| format!("Failed to run {}: {}", cmd, e))?;

    if output.status.success() {
        Ok(format!("Formatted with {}", cmd))
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(format!("{} failed: {}", cmd, stderr.trim()))
    }
}

#[tauri::command]
pub fn format_file_check(file_path: String) -> Result<serde_json::Value, String> {
    let path = Path::new(&file_path);
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");

    match formatter_for_ext(ext) {
        Some((cmd, _)) => {
            let available = cmd == "npx" || is_available(cmd);
            Ok(serde_json::json!({
                "formatter": cmd,
                "available": available,
                "extension": ext,
            }))
        }
        None => Ok(serde_json::json!({
            "formatter": null,
            "available": false,
            "extension": ext,
        })),
    }
}
