//! Per-workspace review checklist (Phase 2 Unit 6).
//!
//! The checklist lives on disk at `<workspace>/.k2so/review-checklist.md`
//! as a markdown file with two H2 sections: "Verify Features" and
//! "Test Criteria". Each section is a list of `- [ ]` / `- [x]`
//! checkbox lines. Parsing and serialization are round-trip safe so
//! the user can hand-edit the file directly.

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
    pub section: String,
}

pub fn parse_checklist(content: &str) -> Vec<ChecklistItem> {
    let mut items = Vec::new();
    let mut current_section = String::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## ") {
            let header = trimmed[3..].trim().to_lowercase();
            if header.contains("verify") || header.contains("feature") {
                current_section = "verify".to_string();
            } else if header.contains("test") || header.contains("criteria") {
                current_section = "criteria".to_string();
            }
            continue;
        }
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

pub fn serialize_checklist(items: &[ChecklistItem], agent_name: &str, branch: &str) -> String {
    let mut out = String::new();
    out.push_str(&format!("# Review: {}\n", agent_name));
    out.push_str(&format!("Branch: {}\n\n", branch));
    let verify_items: Vec<&ChecklistItem> =
        items.iter().filter(|i| i.section == "verify").collect();
    if !verify_items.is_empty() {
        out.push_str("## Verify Features\n");
        for item in &verify_items {
            let mark = if item.checked { "x" } else { " " };
            out.push_str(&format!("- [{}] {}\n", mark, item.text));
        }
        out.push('\n');
    }
    let criteria_items: Vec<&ChecklistItem> =
        items.iter().filter(|i| i.section == "criteria").collect();
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

pub fn read(workspace_path: &str) -> Result<Vec<ChecklistItem>, String> {
    let path = checklist_path(workspace_path);
    if !path.exists() {
        return Ok(Vec::new());
    }
    let content = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read checklist: {}", e))?;
    Ok(parse_checklist(&content))
}

pub fn write(
    workspace_path: &str,
    items: &[ChecklistItem],
    agent_name: &str,
    branch: &str,
) -> Result<(), String> {
    let path = checklist_path(workspace_path);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .k2so directory: {}", e))?;
    }
    let content = serialize_checklist(items, agent_name, branch);
    fs::write(&path, content).map_err(|e| format!("Failed to write checklist: {}", e))?;
    Ok(())
}

pub fn toggle(
    workspace_path: &str,
    index: usize,
    agent_name: &str,
    branch: &str,
) -> Result<Vec<ChecklistItem>, String> {
    let path = checklist_path(workspace_path);
    let content = if path.exists() {
        fs::read_to_string(&path).map_err(|e| format!("Failed to read checklist: {}", e))?
    } else {
        String::new()
    };
    let mut items = parse_checklist(&content);
    if index < items.len() {
        items[index].checked = !items[index].checked;
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .k2so directory: {}", e))?;
    }
    let out = serialize_checklist(&items, agent_name, branch);
    fs::write(&path, out).map_err(|e| format!("Failed to write checklist: {}", e))?;
    Ok(items)
}

pub fn init(
    workspace_path: &str,
    items: &[ChecklistItem],
    agent_name: &str,
    branch: &str,
) -> Result<(), String> {
    let path = checklist_path(workspace_path);
    if path.exists() {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create .k2so directory: {}", e))?;
    }
    let content = serialize_checklist(items, agent_name, branch);
    fs::write(&path, content).map_err(|e| format!("Failed to write checklist: {}", e))?;
    Ok(())
}
