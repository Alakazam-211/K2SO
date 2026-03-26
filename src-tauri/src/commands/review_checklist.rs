use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const CHECKLIST_FILENAME: &str = ".k2so/review-checklist.md";

fn checklist_path(workspace_path: &str) -> PathBuf {
    Path::new(workspace_path).join(CHECKLIST_FILENAME)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChecklistItem {
    pub text: String,
    pub checked: bool,
    pub section: String, // "verify" or "criteria"
}

/// Parse a markdown checklist file into structured items.
fn parse_checklist(content: &str) -> Vec<ChecklistItem> {
    let mut items = Vec::new();
    let mut current_section = String::new();

    for line in content.lines() {
        let trimmed = line.trim();

        // Detect section headers
        if trimmed.starts_with("## ") {
            let header = trimmed[3..].trim().to_lowercase();
            if header.contains("verify") || header.contains("feature") {
                current_section = "verify".to_string();
            } else if header.contains("test") || header.contains("criteria") {
                current_section = "criteria".to_string();
            }
            continue;
        }

        // Parse checkbox lines
        if trimmed.starts_with("- [x] ") || trimmed.starts_with("- [X] ") {
            items.push(ChecklistItem {
                text: trimmed[6..].to_string(),
                checked: true,
                section: current_section.clone(),
            });
        } else if trimmed.starts_with("- [ ] ") {
            items.push(ChecklistItem {
                text: trimmed[6..].to_string(),
                checked: false,
                section: current_section.clone(),
            });
        }
    }

    items
}

/// Serialize checklist items back to markdown.
fn serialize_checklist(items: &[ChecklistItem], agent_name: &str, branch: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Review: {}\n", agent_name));
    out.push_str(&format!("Branch: {}\n\n", branch));

    // Verify Features section
    let verify_items: Vec<&ChecklistItem> = items.iter().filter(|i| i.section == "verify").collect();
    if !verify_items.is_empty() {
        out.push_str("## Verify Features\n");
        for item in &verify_items {
            let mark = if item.checked { "x" } else { " " };
            out.push_str(&format!("- [{}] {}\n", mark, item.text));
        }
        out.push('\n');
    }

    // Test Criteria section
    let criteria_items: Vec<&ChecklistItem> = items.iter().filter(|i| i.section == "criteria").collect();
    if !criteria_items.is_empty() {
        out.push_str("## Test Criteria\n");
        for item in &criteria_items {
            let mark = if item.checked { "x" } else { " " };
            out.push_str(&format!("- [{}] {}\n", mark, item.text));
        }
        out.push('\n');
    }

    out
}

#[tauri::command]
pub fn review_checklist_read(workspace_path: String) -> Result<Vec<ChecklistItem>, String> {
    let path = checklist_path(&workspace_path);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read checklist: {}", e))?;
    Ok(parse_checklist(&content))
}

#[tauri::command]
pub fn review_checklist_write(
    workspace_path: String,
    items: Vec<ChecklistItem>,
    agent_name: String,
    branch: String,
) -> Result<(), String> {
    let path = checklist_path(&workspace_path);

    // Ensure .k2so directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .k2so directory: {}", e))?;
    }

    let content = serialize_checklist(&items, &agent_name, &branch);
    fs::write(&path, content)
        .map_err(|e| format!("Failed to write checklist: {}", e))?;

    Ok(())
}

#[tauri::command]
pub fn review_checklist_toggle(
    workspace_path: String,
    index: usize,
    agent_name: String,
    branch: String,
) -> Result<Vec<ChecklistItem>, String> {
    let path = checklist_path(&workspace_path);
    let content = if path.exists() {
        fs::read_to_string(&path)
            .map_err(|e| format!("Failed to read checklist: {}", e))?
    } else {
        String::new()
    };

    let mut items = parse_checklist(&content);
    if index < items.len() {
        items[index].checked = !items[index].checked;
    }

    // Ensure .k2so directory exists
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .k2so directory: {}", e))?;
    }

    let out = serialize_checklist(&items, &agent_name, &branch);
    fs::write(&path, out)
        .map_err(|e| format!("Failed to write checklist: {}", e))?;

    Ok(items)
}

#[tauri::command]
pub fn review_checklist_init(
    workspace_path: String,
    items: Vec<ChecklistItem>,
    agent_name: String,
    branch: String,
) -> Result<(), String> {
    let path = checklist_path(&workspace_path);

    // Only write if the file doesn't exist yet (don't overwrite user edits)
    if path.exists() {
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .k2so directory: {}", e))?;
    }

    let content = serialize_checklist(&items, &agent_name, &branch);
    fs::write(&path, content)
        .map_err(|e| format!("Failed to write checklist: {}", e))?;

    Ok(())
}
