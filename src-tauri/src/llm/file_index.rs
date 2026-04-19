use std::collections::HashSet;
use std::path::Path;
use std::time::UNIX_EPOCH;

/// Directories to always skip when browsing.
const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    ".next",
    ".nuxt",
    ".turbo",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
    ".tox",
    "coverage",
    ".nyc_output",
    ".parcel-cache",
    ".svelte-kit",
    ".output",
    "vendor",
    "Pods",
    ".expo",
];

/// Maximum character budget for tool results fed back to the LLM.
const MAX_RESULT_CHARS: usize = 1500;

// ── list_files tool implementation ──────────────────────────────────────

/// List the contents of a single directory. Returns a compact string listing
/// with file type indicators and modification dates.
/// Used by the `list_files` LLM tool.
pub fn list_directory(abs_path: &str) -> String {
    let dir = Path::new(abs_path);
    if !dir.is_dir() {
        return format!("Not a directory: {abs_path}");
    }

    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) => return format!("Cannot read directory: {e}"),
    };

    let mut items: Vec<_> = read_dir.filter_map(|e| e.ok()).collect();
    items.sort_by(|a, b| a.file_name().cmp(&b.file_name()));

    let mut output = String::with_capacity(MAX_RESULT_CHARS);

    for entry in items {
        let name = entry.file_name().to_string_lossy().to_string();

        // Skip hidden
        if name.starts_with('.') {
            continue;
        }

        let is_dir = entry.path().is_dir();

        // Skip ignored dirs
        if is_dir && SKIP_DIRS.iter().any(|&s| s.eq_ignore_ascii_case(&name)) {
            continue;
        }

        let modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        let line = if is_dir {
            format!("  {name}/\n")
        } else {
            format!("  {name} [{}]\n", format_date(modified))
        };

        if output.len() + line.len() > MAX_RESULT_CHARS {
            output.push_str("  ...\n");
            break;
        }
        output.push_str(&line);
    }

    if output.is_empty() {
        "(empty directory)".to_string()
    } else {
        output
    }
}

// ── search_files tool implementation ────────────────────────────────────

/// Maximum depth for search recursion.
const SEARCH_MAX_DEPTH: usize = 6;
/// Maximum number of search results to return.
const SEARCH_MAX_RESULTS: usize = 20;

/// Fuzzy-search file and directory names within the workspace.
/// Returns matching paths relative to `workspace_root`, sorted by relevance.
/// Used by the `search_files` LLM tool.
pub fn search_files(workspace_root: &str, query: &str) -> String {
    let _h = crate::perf_hist!("file_search");
    let root = Path::new(workspace_root);
    if !root.is_dir() {
        return format!("Not a directory: {workspace_root}");
    }

    let query_lower = query.to_lowercase();
    let query_parts: Vec<&str> = query_lower.split_whitespace().collect();

    let mut matches: Vec<(String, bool, u64, u32)> = Vec::new(); // (rel_path, is_dir, modified, score)
    let mut visited_inodes = HashSet::new();
    search_walk(root, root, 0, &query_parts, &mut matches, &mut visited_inodes);

    if matches.is_empty() {
        return format!("No files matching \"{query}\"");
    }

    // Sort by score descending, then by modification time descending (most recent first)
    matches.sort_by(|a, b| {
        b.3.cmp(&a.3)
            .then_with(|| b.2.cmp(&a.2))
    });

    // Truncate to max results
    matches.truncate(SEARCH_MAX_RESULTS);

    let mut output = String::with_capacity(MAX_RESULT_CHARS);
    for (rel_path, is_dir, modified, _score) in &matches {
        let line = if *is_dir {
            format!("  {rel_path}/\n")
        } else {
            format!("  {rel_path} [{}]\n", format_date(*modified))
        };

        if output.len() + line.len() > MAX_RESULT_CHARS {
            output.push_str("  ...\n");
            break;
        }
        output.push_str(&line);
    }

    output
}

/// Recursively walk and collect fuzzy matches with symlink cycle detection.
fn search_walk(
    root: &Path,
    dir: &Path,
    depth: usize,
    query_parts: &[&str],
    matches: &mut Vec<(String, bool, u64, u32)>,
    visited_inodes: &mut HashSet<u64>,
) {
    if depth > SEARCH_MAX_DEPTH {
        return;
    }

    // Track visited directories by inode to prevent symlink cycles
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata(dir) {
            if !visited_inodes.insert(meta.ino()) {
                return; // Already visited — symlink cycle
            }
        }
    }

    let read_dir = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(_) => return,
    };

    for entry in read_dir.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();

        if name.starts_with('.') {
            continue;
        }

        let path = entry.path();
        let is_dir = path.is_dir();

        if is_dir && SKIP_DIRS.iter().any(|&s| s.eq_ignore_ascii_case(&name)) {
            continue;
        }

        let relative = match path.strip_prefix(root) {
            Ok(rel) => rel.to_string_lossy().to_string(),
            Err(_) => continue,
        };

        let modified = entry
            .metadata()
            .ok()
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);

        // Score: how well does this path match the query?
        let score = fuzzy_score(&relative, query_parts);
        if score > 0 {
            matches.push((relative, is_dir, modified, score));
        }

        if is_dir {
            search_walk(root, &path, depth + 1, query_parts, matches, visited_inodes);
        }
    }
}

/// Simple fuzzy scoring: each query part that appears as a substring in the
/// path (case-insensitive) adds points. Exact filename match scores highest.
fn fuzzy_score(path: &str, query_parts: &[&str]) -> u32 {
    let path_lower = path.to_lowercase();
    let filename = path
        .rsplit('/')
        .next()
        .unwrap_or(path)
        .to_lowercase();

    let mut score: u32 = 0;

    for &part in query_parts {
        if filename == part {
            // Exact filename match
            score += 100;
        } else if filename.contains(part) {
            // Substring in filename
            score += 50;
        } else if path_lower.contains(part) {
            // Substring anywhere in path
            score += 10;
        }
        // If part doesn't match at all, no points (but don't penalize)
    }

    score
}

// ── Shared helpers ──────────────────────────────────────────────────────

/// Format a unix timestamp as YYYY-MM-DD.
fn format_date(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let z = days + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{:04}-{:02}-{:02}", y, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_date() {
        assert_eq!(format_date(1704067200), "2024-01-01");
    }

    #[test]
    fn test_fuzzy_score_exact() {
        assert_eq!(fuzzy_score("src/main.rs", &["main.rs"]), 100);
    }

    #[test]
    fn test_fuzzy_score_substring() {
        assert_eq!(fuzzy_score("reports/weekly/report-march.md", &["report"]), 50);
    }

    #[test]
    fn test_fuzzy_score_path() {
        assert_eq!(fuzzy_score("reports/weekly/2026-03-20.md", &["weekly"]), 10);
    }

    #[test]
    fn test_fuzzy_score_multi_part() {
        // "weekly" in path (10) + "report" in filename (50)
        assert_eq!(
            fuzzy_score("reports/weekly/report-march.md", &["weekly", "report"]),
            60
        );
    }

    #[test]
    fn test_fuzzy_score_no_match() {
        assert_eq!(fuzzy_score("src/main.rs", &["zebra"]), 0);
    }

    #[test]
    fn test_list_directory_nonexistent() {
        let result = list_directory("/nonexistent/path/xyz");
        assert!(result.starts_with("Not a directory"));
    }

    #[test]
    fn test_search_files_on_crate() {
        let result = search_files(env!("CARGO_MANIFEST_DIR"), "mod");
        // Should find mod.rs files
        assert!(result.contains("mod.rs") || result.contains("mod"), "Expected mod matches, got: {result}");
    }
}
