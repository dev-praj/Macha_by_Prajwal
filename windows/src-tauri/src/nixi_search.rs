// Nixi File Search — Phase 5D
// Searches user files across standard library folders (Desktop, Documents, Downloads, etc.)
// and reveals them in File Explorer using `explorer.exe /select,"path"`.
// HARD CONSTRAINT: Never execute the file. Only reveal it.

use std::path::{Path, PathBuf};
use std::process::Command;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NixiFileMatch {
    pub path: String,
    pub name: String,
    pub folder: String,
    pub size_bytes: u64,
}

/// Search user folders for files matching `query`. Returns up to `limit` matches.
pub fn search_files(query: &str, limit: usize) -> Vec<NixiFileMatch> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }

    let search_dirs = get_user_search_roots();
    let mut matches = Vec::new();

    for dir in search_dirs {
        if matches.len() >= limit {
            break;
        }
        collect_matches(&dir, &q, 0, 4, limit, &mut matches);
    }

    // Sort by name match closeness (exact stem match first)
    matches.sort_by(|a, b| {
        let a_exact = a.name.to_lowercase() == q || a.name.to_lowercase().starts_with(&q);
        let b_exact = b.name.to_lowercase() == q || b.name.to_lowercase().starts_with(&q);
        b_exact.cmp(&a_exact).then_with(|| a.name.cmp(&b.name))
    });

    matches
}

fn get_user_search_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(userprofile) = std::env::var_os("USERPROFILE") {
        let base = PathBuf::from(userprofile);
        for folder in &["Desktop", "Documents", "Downloads", "Pictures", "Music", "Videos"] {
            let p = base.join(folder);
            if p.is_dir() {
                roots.push(p);
            }
        }
    }
    roots
}

fn collect_matches(
    dir: &Path,
    query: &str,
    depth: usize,
    max_depth: usize,
    limit: usize,
    matches: &mut Vec<NixiFileMatch>,
) {
    if depth > max_depth || matches.len() >= limit {
        return;
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return,
    };

    let mut subdirs = Vec::new();

    for entry in entries.flatten() {
        if matches.len() >= limit {
            return;
        }
        let path = entry.path();
        let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();

        // Skip hidden files and special directories
        if name.starts_with('.') || name.starts_with('$') || name == "node_modules" || name == "target" || name == "AppData" {
            continue;
        }

        if path.is_file() {
            let lower = name.to_lowercase();
            if lower.contains(query) {
                let size_bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
                let folder = dir.to_string_lossy().to_string();
                matches.push(NixiFileMatch {
                    path: path.to_string_lossy().to_string(),
                    name,
                    folder,
                    size_bytes,
                });
            }
        } else if path.is_dir() {
            subdirs.push(path);
        }
    }

    for sub in subdirs {
        collect_matches(&sub, query, depth + 1, max_depth, limit, matches);
    }
}

/// Reveals a file in File Explorer with the file highlighted.
/// NEVER executes the file.
pub fn reveal_in_explorer(file_path: &str) -> Result<(), String> {
    let path = Path::new(file_path);
    if !path.exists() {
        return Err(format!("File does not exist: {file_path}"));
    }

    #[cfg(target_os = "windows")]
    {
        // explorer.exe /select,"<path>"
        let arg = format!("/select,{}", path.display());
        Command::new("explorer.exe")
            .arg(arg)
            .spawn()
            .map_err(|e| format!("Failed to open File Explorer: {e}"))?;
        crate::log::line(format!("nixi_search: revealed file {file_path} in Explorer"));
        Ok(())
    }

    #[cfg(not(target_os = "windows"))]
    {
        crate::platform::reveal_folder(file_path);
        Ok(())
    }
}

// ── Tauri Commands ──────────────────────────────────────────────────────────

#[tauri::command]
pub fn nixi_search_files(query: String, limit: Option<usize>) -> Vec<NixiFileMatch> {
    search_files(&query, limit.unwrap_or(10))
}

#[tauri::command]
pub fn nixi_reveal_file(path: String) -> Result<(), String> {
    reveal_in_explorer(&path)
}

// ── Unit Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_search_file_matches_query() {
        let temp = std::env::temp_dir();
        let test_file = temp.join("nixi_test_search_sample.txt");
        let _ = std::fs::write(&test_file, "sample content");

        let mut matches = Vec::new();
        collect_matches(&temp, "nixi_test_search_sample", 0, 1, 10, &mut matches);

        assert!(!matches.is_empty());
        assert_eq!(matches[0].name, "nixi_test_search_sample.txt");

        let _ = std::fs::remove_file(test_file);
    }

    #[test]
    fn test_empty_query_returns_no_results() {
        assert!(search_files("", 10).is_empty());
        assert!(search_files("   ", 10).is_empty());
    }
}
