use serde::Serialize;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

// ── Constants ───────────────────────────────────────────────────────────

const KEYCHAIN_SERVICE: &str = "Claude Code-credentials";
const KEYCHAIN_ACCOUNT: &str = "Claude Code";
const TOKEN_ENDPOINT: &str = "https://platform.claude.com/v1/oauth/token";
const CLIENT_ID: &str = "https://claude.ai/oauth/claude-code-client-metadata";
const EXPIRY_BUFFER_SECS: i64 = 300; // 5 minutes

#[cfg(target_os = "macos")]
const PLIST_LABEL: &str = "com.k2so.claude-auth-refresh";

// ── Types ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClaudeAuthStatus {
    /// "valid" | "expiring" | "expired" | "missing"
    pub state: String,
    /// Unix timestamp in seconds, if token exists
    pub expires_at: Option<i64>,
    /// Seconds until expiry (negative if expired)
    pub seconds_remaining: Option<i64>,
    /// Whether the background scheduler is installed
    pub scheduler_installed: bool,
}

// ── Credential helpers ──────────────────────────────────────────────────

/// Raw credential data from keychain or file.
struct RawCredentials {
    json: serde_json::Value,
    access_token: String,
    refresh_token: String,
    expires_at: i64,
}

fn credentials_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".claude/.credentials.json")
}

/// Read credentials from macOS Keychain, falling back to file.
fn read_credentials() -> Option<RawCredentials> {
    // Try macOS Keychain first
    #[cfg(target_os = "macos")]
    {
        if let Some(creds) = read_keychain_credentials() {
            return Some(creds);
        }
    }

    // Fallback to file
    read_file_credentials()
}

#[cfg(target_os = "macos")]
fn read_keychain_credentials() -> Option<RawCredentials> {
    let output = Command::new("security")
        .args(["find-generic-password", "-s", KEYCHAIN_SERVICE, "-w"])
        .output()
        .ok()?;

    if !output.status.success() {
        return None;
    }

    let raw = String::from_utf8(output.stdout).ok()?;
    parse_credential_json(raw.trim())
}

fn read_file_credentials() -> Option<RawCredentials> {
    let path = credentials_file();
    let raw = fs::read_to_string(&path).ok()?;
    parse_credential_json(&raw)
}

fn parse_credential_json(raw: &str) -> Option<RawCredentials> {
    let json: serde_json::Value = serde_json::from_str(raw).ok()?;
    let oauth = json.get("claudeAiOauth")?;

    let access_token = oauth.get("accessToken")?.as_str()?.to_string();
    let refresh_token = oauth.get("refreshToken")?.as_str()?.to_string();
    let expires_at = oauth.get("expiresAt")?.as_i64()?;

    if access_token.is_empty() || refresh_token.is_empty() {
        return None;
    }

    Some(RawCredentials {
        json,
        access_token,
        refresh_token,
        expires_at,
    })
}

fn compute_auth_state(expires_at: i64) -> (String, i64) {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let remaining = expires_at - now;

    let state = if remaining > EXPIRY_BUFFER_SECS {
        "valid"
    } else if remaining > 0 {
        "expiring"
    } else {
        "expired"
    };

    (state.to_string(), remaining)
}

/// Write updated credentials back to keychain and/or file.
fn write_credentials(updated_json: &serde_json::Value) -> Result<(), String> {
    let json_str = serde_json::to_string(updated_json).map_err(|e| e.to_string())?;

    // Write to macOS Keychain
    #[cfg(target_os = "macos")]
    {
        // Use Command::new directly to avoid shell interpretation of token values
        let status = Command::new("security")
            .args([
                "add-generic-password",
                "-U",
                "-s", KEYCHAIN_SERVICE,
                "-a", KEYCHAIN_ACCOUNT,
                "-w", &json_str,
            ])
            .output()
            .map_err(|e| format!("Failed to run security command: {}", e))?;

        if !status.status.success() {
            let stderr = String::from_utf8_lossy(&status.stderr);
            log_debug!("[claude-auth] Keychain write failed: {}", stderr);
        }
    }

    // Also write to file as fallback
    let path = credentials_file();
    if path.exists() {
        fs::write(&path, &json_str)
            .map_err(|e| format!("Failed to write credentials file: {}", e))?;
    }

    Ok(())
}

// ── Scheduler helpers ───────────────────────────────────────────────────

fn k2so_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".k2so")
}

fn refresh_script_path() -> PathBuf {
    k2so_dir().join("claude-auth-refresh.sh")
}

#[cfg(target_os = "macos")]
fn plist_path() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Library/LaunchAgents/com.k2so.claude-auth-refresh.plist")
}

fn generate_refresh_script() -> String {
    let home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .to_string_lossy()
        .to_string();

    format!(
        r##"#!/bin/bash
# K2SO Claude Auth Refresh — DO NOT EDIT (managed by K2SO)
# Runs via launchd/cron every 20 minutes.

CRED_SERVICE="Claude Code-credentials"
CRED_ACCOUNT="Claude Code"
TOKEN_URL="{token_url}"
CLIENT_ID="{client_id}"
LOG_FILE="{home}/.k2so/claude-auth-refresh.log"
BUFFER_SECONDS=300

ts() {{ date '+%Y-%m-%d %H:%M:%S'; }}

# Read credentials from keychain (macOS) or file
CREDS=""
if command -v security &>/dev/null; then
    CREDS=$(security find-generic-password -s "$CRED_SERVICE" -w 2>/dev/null)
fi
if [ -z "$CREDS" ] && [ -f "{home}/.claude/.credentials.json" ]; then
    CREDS=$(cat "{home}/.claude/.credentials.json" 2>/dev/null)
fi
if [ -z "$CREDS" ]; then
    echo "$(ts) No credentials found — skipping" >> "$LOG_FILE"
    exit 0
fi

# Extract fields using python3
if ! command -v python3 &>/dev/null; then
    echo "$(ts) python3 not found — cannot parse credentials" >> "$LOG_FILE"
    exit 1
fi

EXPIRES_AT=$(echo "$CREDS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('claudeAiOauth',{{}}).get('expiresAt',''))" 2>/dev/null)
REFRESH_TOKEN=$(echo "$CREDS" | python3 -c "import sys,json; d=json.load(sys.stdin); print(d.get('claudeAiOauth',{{}}).get('refreshToken',''))" 2>/dev/null)

if [ -z "$REFRESH_TOKEN" ]; then
    echo "$(ts) No refresh token found — skipping" >> "$LOG_FILE"
    exit 0
fi

# Check if token needs refresh
NOW=$(date +%s)
if echo "$EXPIRES_AT" | grep -qE '^[0-9]+$'; then
    EXPIRY=$EXPIRES_AT
else
    EXPIRY=0
fi

REMAINING=$((EXPIRY - NOW))
if [ "$REMAINING" -gt "$BUFFER_SECONDS" ]; then
    exit 0
fi

echo "$(ts) Token expiring in ${{REMAINING}}s — refreshing..." >> "$LOG_FILE"

# Perform OAuth refresh
RESPONSE=$(curl -s -X POST "$TOKEN_URL" \
    -H "Content-Type: application/x-www-form-urlencoded" \
    --connect-timeout 10 --max-time 30 \
    -d "grant_type=refresh_token&client_id=$CLIENT_ID&refresh_token=$REFRESH_TOKEN")

if echo "$RESPONSE" | python3 -c "import sys,json; d=json.load(sys.stdin); sys.exit(0 if 'access_token' in d else 1)" 2>/dev/null; then
    : # success
else
    echo "$(ts) Refresh failed: $RESPONSE" >> "$LOG_FILE"
    exit 1
fi

# Update credentials
NEW_CREDS=$(python3 -c "
import json, time, sys
creds = json.loads(sys.argv[1])
resp = json.loads(sys.argv[2])
oauth = creds.get('claudeAiOauth', {{}})
oauth['accessToken'] = resp['access_token']
if 'refresh_token' in resp:
    oauth['refreshToken'] = resp['refresh_token']
expires_in = resp.get('expires_in', 3600)
oauth['expiresAt'] = int(time.time()) + expires_in
creds['claudeAiOauth'] = oauth
print(json.dumps(creds))
" "$CREDS" "$RESPONSE" 2>/dev/null)

if [ -z "$NEW_CREDS" ]; then
    echo "$(ts) Failed to construct updated credentials" >> "$LOG_FILE"
    exit 1
fi

# Write back
if command -v security &>/dev/null; then
    security add-generic-password -U -s "$CRED_SERVICE" -a "$CRED_ACCOUNT" -w "$NEW_CREDS" 2>/dev/null
fi
if [ -f "{home}/.claude/.credentials.json" ]; then
    echo "$NEW_CREDS" > "{home}/.claude/.credentials.json"
fi

echo "$(ts) Token refreshed successfully" >> "$LOG_FILE"

# Trim log
tail -100 "$LOG_FILE" > "$LOG_FILE.tmp" && mv "$LOG_FILE.tmp" "$LOG_FILE"
"##,
        token_url = TOKEN_ENDPOINT,
        client_id = CLIENT_ID,
        home = home,
    )
}

#[cfg(target_os = "macos")]
fn generate_plist() -> String {
    let script = refresh_script_path();
    let home = dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("~"))
        .to_string_lossy()
        .to_string();

    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>{label}</string>
    <key>ProgramArguments</key>
    <array>
        <string>/bin/bash</string>
        <string>{script}</string>
    </array>
    <key>StartInterval</key>
    <integer>1200</integer>
    <key>RunAtLoad</key>
    <false/>
    <key>StandardErrorPath</key>
    <string>{home}/.k2so/claude-auth-refresh-stderr.log</string>
</dict>
</plist>"#,
        label = PLIST_LABEL,
        script = script.to_string_lossy(),
        home = home,
    )
}

fn is_scheduler_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        return is_scheduler_installed_macos();
    }

    #[cfg(target_os = "linux")]
    {
        return is_scheduler_installed_linux();
    }

    #[allow(unreachable_code)]
    false
}

#[cfg(target_os = "macos")]
fn is_scheduler_installed_macos() -> bool {
    let plist = plist_path();
    if !plist.exists() {
        return false;
    }
    // Check if it's actually loaded
    Command::new("launchctl")
        .args(["list", PLIST_LABEL])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn is_scheduler_installed_linux() -> bool {
    Command::new("crontab")
        .args(["-l"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.contains("k2so-claude-auth-refresh"))
        .unwrap_or(false)
}

// ── Tauri Commands ──────────────────────────────────────────────────────

#[tauri::command]
pub fn claude_auth_status() -> Result<ClaudeAuthStatus, String> {
    let scheduler_installed = is_scheduler_installed();

    match read_credentials() {
        Some(creds) => {
            let (state, remaining) = compute_auth_state(creds.expires_at);
            Ok(ClaudeAuthStatus {
                state,
                expires_at: Some(creds.expires_at),
                seconds_remaining: Some(remaining),
                scheduler_installed,
            })
        }
        None => Ok(ClaudeAuthStatus {
            state: "missing".to_string(),
            expires_at: None,
            seconds_remaining: None,
            scheduler_installed,
        }),
    }
}

#[tauri::command]
pub fn claude_auth_refresh() -> Result<ClaudeAuthStatus, String> {
    let creds = read_credentials()
        .ok_or_else(|| "No Claude credentials found".to_string())?;

    // Build form body
    let body = format!(
        "grant_type=refresh_token&client_id={}&refresh_token={}",
        CLIENT_ID, creds.refresh_token
    );

    // Perform the refresh
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("Failed to create HTTP client: {}", e))?;

    let response = client
        .post(TOKEN_ENDPOINT)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .map_err(|e| format!("Refresh request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().unwrap_or_default();
        return Err(format!("Refresh failed (HTTP {}): {}", status, body));
    }

    let resp_text = response.text()
        .map_err(|e| format!("Failed to read refresh response: {}", e))?;
    let resp_json: serde_json::Value = serde_json::from_str(&resp_text)
        .map_err(|e| format!("Failed to parse refresh response: {}", e))?;

    let new_access = resp_json.get("access_token")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "No access_token in refresh response".to_string())?;

    let new_refresh = resp_json.get("refresh_token")
        .and_then(serde_json::Value::as_str)
        .unwrap_or(&creds.refresh_token);

    let expires_in = resp_json.get("expires_in")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(3600);

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let new_expires_at = now + expires_in;

    // Update the JSON
    let mut updated = creds.json.clone();
    if let Some(oauth) = updated.get_mut("claudeAiOauth") {
        oauth["accessToken"] = serde_json::Value::String(new_access.to_string());
        oauth["refreshToken"] = serde_json::Value::String(new_refresh.to_string());
        oauth["expiresAt"] = serde_json::Value::Number(serde_json::Number::from(new_expires_at));
    }

    write_credentials(&updated)?;

    let (state, remaining) = compute_auth_state(new_expires_at);
    Ok(ClaudeAuthStatus {
        state,
        expires_at: Some(new_expires_at),
        seconds_remaining: Some(remaining),
        scheduler_installed: is_scheduler_installed(),
    })
}

#[tauri::command]
pub fn claude_auth_install_scheduler() -> Result<(), String> {
    let dir = k2so_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("Failed to create ~/.k2so: {}", e))?;

    // Write refresh script
    let script_path = refresh_script_path();
    let script_content = generate_refresh_script();
    fs::write(&script_path, &script_content)
        .map_err(|e| format!("Failed to write refresh script: {}", e))?;

    // Make executable
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = fs::Permissions::from_mode(0o755);
        fs::set_permissions(&script_path, perms)
            .map_err(|e| format!("Failed to set script permissions: {}", e))?;
    }

    // Platform-specific scheduler installation
    #[cfg(target_os = "macos")]
    {
        install_launchd()?;
    }

    #[cfg(target_os = "linux")]
    {
        install_crontab()?;
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn install_launchd() -> Result<(), String> {
    let plist = plist_path();

    // Ensure LaunchAgents directory exists
    if let Some(parent) = plist.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create LaunchAgents dir: {}", e))?;
    }

    // Unload existing if present (ignore errors)
    if plist.exists() {
        let _ = Command::new("launchctl")
            .args(["unload", &plist.to_string_lossy()])
            .output();
    }

    // Write plist
    let plist_content = generate_plist();
    fs::write(&plist, &plist_content)
        .map_err(|e| format!("Failed to write plist: {}", e))?;

    // Load
    let output = Command::new("launchctl")
        .args(["load", &plist.to_string_lossy()])
        .output()
        .map_err(|e| format!("Failed to run launchctl load: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("launchctl load failed: {}", stderr));
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn install_crontab() -> Result<(), String> {
    let script = refresh_script_path();
    let marker = "# k2so-claude-auth-refresh";
    let entry = format!("*/20 * * * * {} {}", script.to_string_lossy(), marker);

    // Read existing crontab
    let existing = Command::new("crontab")
        .args(["-l"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .unwrap_or_default();

    // Remove old entry if present, add new one
    let mut lines: Vec<&str> = existing
        .lines()
        .filter(|l| !l.contains("k2so-claude-auth-refresh"))
        .collect();
    lines.push(&entry);
    let new_crontab = lines.join("\n") + "\n";

    // Write new crontab
    let mut child = Command::new("crontab")
        .args(["-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn crontab: {}", e))?;

    use std::io::Write;
    child
        .stdin
        .as_mut()
        .ok_or("Failed to open crontab stdin")?
        .write_all(new_crontab.as_bytes())
        .map_err(|e| format!("Failed to write crontab: {}", e))?;

    let status = child.wait().map_err(|e| format!("crontab wait failed: {}", e))?;
    if !status.success() {
        return Err("crontab install failed".to_string());
    }

    Ok(())
}

#[tauri::command]
pub fn claude_auth_uninstall_scheduler() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        uninstall_launchd()?;
    }

    #[cfg(target_os = "linux")]
    {
        uninstall_crontab()?;
    }

    // Remove refresh script
    let script = refresh_script_path();
    if script.exists() {
        let _ = fs::remove_file(&script);
    }

    Ok(())
}

#[cfg(target_os = "macos")]
fn uninstall_launchd() -> Result<(), String> {
    let plist = plist_path();

    if plist.exists() {
        // Unload first
        let _ = Command::new("launchctl")
            .args(["unload", &plist.to_string_lossy()])
            .output();

        fs::remove_file(&plist)
            .map_err(|e| format!("Failed to remove plist: {}", e))?;
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_crontab() -> Result<(), String> {
    let existing = Command::new("crontab")
        .args(["-l"])
        .output()
        .ok()
        .and_then(|o| if o.status.success() { String::from_utf8(o.stdout).ok() } else { None })
        .unwrap_or_default();

    let new_crontab: String = existing
        .lines()
        .filter(|l| !l.contains("k2so-claude-auth-refresh"))
        .collect::<Vec<&str>>()
        .join("\n")
        + "\n";

    let mut child = Command::new("crontab")
        .args(["-"])
        .stdin(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn crontab: {}", e))?;

    use std::io::Write;
    child
        .stdin
        .as_mut()
        .ok_or("Failed to open crontab stdin")?
        .write_all(new_crontab.as_bytes())
        .map_err(|e| format!("Failed to write crontab: {}", e))?;

    child.wait().map_err(|e| format!("crontab wait failed: {}", e))?;
    Ok(())
}

#[tauri::command]
pub fn claude_auth_scheduler_installed() -> Result<bool, String> {
    Ok(is_scheduler_installed())
}
