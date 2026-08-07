//! Atomic file writing engine for tool configuration files.
//!
//! Tool config files (e.g. `~/.claude/settings.json`) are user-owned and must
//! never be corrupted by a partial write. This module provides:
//! - `atomic_write`: write a single file via a unique temp file + atomic replace
//! - `atomic_write_multi`: transactional multi-file write with rollback

use crate::error::AppError;
use std::path::{Path, PathBuf};

/// Atomically write a single file.
///
/// Strategy: write to a unique temp file in the same directory, then replace
/// the target. On Windows, `std::fs::rename` fails when the target exists, so
/// we remove the target first only as a fallback (the temp+rename approach
/// keeps the window minimal and the parent dir is preserved).
pub fn atomic_write(path: &Path, content: &[u8]) -> Result<(), AppError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|e| AppError::Config(format!("创建目录失败 {}: {e}", parent.display())))?;

    let tmp = unique_tmp_path(path)?;
    std::fs::write(&tmp, content)
        .map_err(|e| AppError::Config(format!("写入临时文件失败 {}: {e}", tmp.display())))?;

    replace_atomically(&tmp, path)
}

/// Transactionally write multiple files; rollback all on any failure.
pub fn atomic_write_multi(files: &[(PathBuf, String)]) -> Result<(), AppError> {
    let old: Vec<(PathBuf, Option<Vec<u8>>)> = files
        .iter()
        .map(|(p, _)| (p.clone(), std::fs::read(p).ok()))
        .collect();

    for (i, (path, content)) in files.iter().enumerate() {
        if let Err(e) = atomic_write(path, content.as_bytes()) {
            // Roll back the first `i` files already written.
            for (old_path, old_bytes) in old.iter().take(i) {
                match old_bytes {
                    Some(b) => {
                        let _ = atomic_write(old_path, b);
                    }
                    None => {
                        let _ = std::fs::remove_file(old_path);
                    }
                }
            }
            return Err(e);
        }
    }
    Ok(())
}

fn unique_tmp_path(path: &Path) -> Result<PathBuf, AppError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("config");
    for _ in 0..16 {
        let candidate = parent.join(format!(
            ".{}.tmp.{}.{}",
            file_name,
            std::process::id(),
            uuid::Uuid::new_v4().simple()
        ));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(AppError::Config("无法生成唯一临时文件名".to_string()))
}

#[cfg(windows)]
fn replace_atomically(tmp: &Path, path: &Path) -> Result<(), AppError> {
    // Windows: remove target if exists then rename (best-effort atomic).
    let _ = std::fs::remove_file(path);
    match std::fs::rename(tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(tmp);
            Err(AppError::Config(format!("原子替换失败 {}: {e}", path.display())))
        }
    }
}

#[cfg(not(windows))]
fn replace_atomically(tmp: &Path, path: &Path) -> Result<(), AppError> {
    match std::fs::rename(tmp, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(tmp);
            Err(AppError::Config(format!("原子替换失败 {}: {e}", path.display())))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_atomic_write_creates_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("settings.json");

        atomic_write(&target, b"{\"a\":1}").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"a\":1}");

        // Overwrite (Windows rename-with-existing must work)
        atomic_write(&target, b"{\"b\":2}").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "{\"b\":2}");
    }

    #[test]
    fn test_atomic_write_nested_dir() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("a/b/c/settings.json");
        atomic_write(&target, b"data").unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "data");
    }

    #[test]
    fn test_atomic_write_multi_rolls_back_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        let f1 = dir.path().join("f1.json");
        atomic_write(&f1, b"old1").unwrap();

        // Second target's parent is a FILE, so write fails -> f1 must roll back.
        let blocker = dir.path().join("blocker");
        atomic_write(&blocker, b"i am a file not a dir").unwrap();
        let bad_target = blocker.join("nested.json");

        let result = atomic_write_multi(&[
            (f1.clone(), "new1".to_string()),
            (bad_target.clone(), "new2".to_string()),
        ]);
        assert!(result.is_err());
        assert_eq!(std::fs::read_to_string(&f1).unwrap(), "old1");
    }
}
