//! Tool installation detection helpers.

use std::path::{Path, PathBuf};

/// Get the user's home directory.
pub fn home_dir() -> Option<PathBuf> {
    dirs::home_dir()
}

/// Check if a file/dir exists at the given path.
pub fn exists(path: &Path) -> bool {
    path.exists()
}

/// Resolve a path under the user's home directory (e.g. `~/.claude/settings.json`).
pub fn home_path(relative: &str) -> Option<PathBuf> {
    home_dir().map(|h| h.join(relative))
}

/// Find an executable in PATH.
#[cfg(windows)]
pub fn which_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        for ext in ["", ".exe", ".cmd", ".bat"] {
            let candidate = dir.join(format!("{}{}", name, ext));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Find an executable in PATH.
#[cfg(not(windows))]
pub fn which_in_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_home_dir_returns_some() {
        assert!(home_dir().is_some());
    }

    #[test]
    fn test_home_path_resolves() {
        let p = home_path(".claude/settings.json").unwrap();
        assert!(p.to_string_lossy().contains(".claude"));
        assert!(p.ends_with("settings.json"));
    }

    #[test]
    fn test_exists_positive_and_negative() {
        assert!(exists(Path::new(".")));
        assert!(!exists(Path::new("definitely_not_a_real_path_xyz")));
    }
}
