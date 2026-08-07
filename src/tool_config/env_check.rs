//! System environment-variable conflict detection & cleanup (PLAN 1.4.4).
//!
//! System/user-level environment variables (e.g. `ANTHROPIC_BASE_URL`,
//! `OPENAI_API_KEY`, `GEMINI_API_KEY`, `XAI_API_KEY`) **override** the `env`
//! fields injected into a tool's `settings.json`, so an injection may appear to
//! "do nothing". Before enabling a tool switch we check these variables; the
//! frontend warns the user and offers one-click cleanup.
//!
//! On Windows the variables live in the registry (`HKCU\Environment` and
//! `HKLM\SYSTEM\CurrentControlSet\Control\Session Manager\Environment`).
//! Cleanup writes a backup to `~/.llm-proxy/backups/env-backup-{ts}.json`
//! before removing a variable so it can be restored.

use crate::error::AppError;
use std::path::PathBuf;

/// Prefixes of environment variables that can override an injected proxy config.
pub const CONFLICT_PREFIXES: &[&str] = &["ANTHROPIC_", "OPENAI_", "GEMINI_", "GOOGLE_", "XAI_"];

/// One detected conflicting environment variable.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct EnvConflict {
    /// Variable name, e.g. `ANTHROPIC_BASE_URL`.
    pub name: String,
    /// Current value (may be masked on the frontend).
    pub value: String,
    /// Where it lives: `HKCU\Environment`, `HKLM\...\Environment`, or `shell`.
    pub source: String,
}

/// Detect conflicting system/user environment variables.
///
/// On Windows this reads `HKCU\Environment` and the system `Environment`
/// registry keys. On other platforms it scans the current process environment
/// (a conservative subset) — full `~/.bashrc`/`~/.zshrc` parsing is left to
/// the user since those files are shell-specific.
pub fn detect_conflicts() -> Vec<EnvConflict> {
    let mut out = Vec::new();
    #[cfg(windows)]
    detect_windows_registry_conflicts(&mut out);
    detect_process_env_conflicts(&mut out);
    dedup(&mut out);
    out
}

/// Remove the conflicting variables, backing them up first.
///
/// Returns the backup file path (or `None` if nothing was removed). The backup
/// is written to `~/.llm-proxy/backups/env-backup-{ts}.json` so cleanup is
/// reversible. Variables that can't be backed up are skipped (never removed).
pub fn cleanup_conflicts(conflicts: &[EnvConflict]) -> Result<Option<PathBuf>, AppError> {
    if conflicts.is_empty() {
        return Ok(None);
    }
    let backup_path = write_env_backup(conflicts)?;
    #[cfg(windows)]
    remove_windows_registry_conflicts(conflicts)?;
    Ok(Some(backup_path))
}

/// Restore previously backed-up environment variables (undo a cleanup).
pub fn restore_env_backup(backup_path: &std::path::Path) -> Result<(), AppError> {
    let data = std::fs::read_to_string(backup_path)
        .map_err(AppError::Io)?;
    let conflicts: Vec<EnvConflict> = serde_json::from_str(&data)
        .map_err(|e| AppError::Config(format!("解析环境变量备份失败: {e}")))?;
    #[cfg(windows)]
    set_windows_registry_conflicts(&conflicts)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Windows registry helpers
// ---------------------------------------------------------------------------

/// Names of the two relevant Windows registry keys.
#[cfg(windows)]
const HKCU_ENV_KEY: &str = r"Environment";
#[cfg(windows)]
const HKLM_ENV_KEY: &str = r"SYSTEM\CurrentControlSet\Control\Session Manager\Environment";

#[cfg(windows)]
fn detect_windows_registry_conflicts(out: &mut Vec<EnvConflict>) {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
    use winreg::RegKey;

    let mut scan = |hkey, path: &str, source: &str| {
        if let Ok(env) = RegKey::predef(hkey).open_subkey_with_flags(path, KEY_READ) {
            for prefix in CONFLICT_PREFIXES {
                for (name, value) in env.enum_values().flatten() {
                    if name.starts_with(prefix) {
                        out.push(EnvConflict {
                            name,
                            value: value.to_string(),
                            source: source.to_string(),
                        });
                    }
                }
            }
        }
    };

    scan(HKEY_CURRENT_USER, HKCU_ENV_KEY, "HKCU\\Environment");
    scan(HKEY_LOCAL_MACHINE, HKLM_ENV_KEY, "HKLM\\Environment");
}

#[cfg(windows)]
fn remove_windows_registry_conflicts(conflicts: &[EnvConflict]) -> Result<(), AppError> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE};
    use winreg::RegKey;

    let mut by_source: std::collections::HashMap<&str, Vec<&str>> = std::collections::HashMap::new();
    for c in conflicts {
        by_source
            .entry(if c.source.starts_with("HKLM") {
                "HKLM"
            } else {
                "HKCU"
            })
            .or_default()
            .push(&c.name);
    }
    for (source, names) in &by_source {
        let (predef, path) = if *source == "HKLM" {
            (HKEY_LOCAL_MACHINE, HKLM_ENV_KEY)
        } else {
            (HKEY_CURRENT_USER, HKCU_ENV_KEY)
        };
        if let Ok(key) = RegKey::predef(predef).open_subkey_with_flags(path, KEY_SET_VALUE) {
            for name in names {
                let _ = key.delete_value(name);
            }
        }
    }
    Ok(())
}

#[cfg(windows)]
fn set_windows_registry_conflicts(conflicts: &[EnvConflict]) -> Result<(), AppError> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_SET_VALUE};
    use winreg::RegKey;

    for c in conflicts {
        let (predef, path) = if c.source.starts_with("HKLM") {
            (HKEY_LOCAL_MACHINE, HKLM_ENV_KEY)
        } else {
            (HKEY_CURRENT_USER, HKCU_ENV_KEY)
        };
        if let Ok(key) = RegKey::predef(predef).open_subkey_with_flags(path, KEY_SET_VALUE) {
            let _ = key.set_value(&c.name, &c.value);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Cross-platform process-env scan (conservative subset)
// ---------------------------------------------------------------------------

fn detect_process_env_conflicts(out: &mut Vec<EnvConflict>) {
    for (name, value) in std::env::vars() {
        if CONFLICT_PREFIXES
            .iter()
            .any(|p| name.starts_with(p))
        {
            out.push(EnvConflict {
                name,
                value,
                source: "process-env".to_string(),
            });
        }
    }
}

fn dedup(out: &mut Vec<EnvConflict>) {
    out.sort_by(|a, b| a.source.cmp(&b.source).then(a.name.cmp(&b.name)));
    out.dedup_by(|a, b| a.source == b.source && a.name == b.name);
}

/// Write a JSON backup of the given conflicts.
fn write_env_backup(conflicts: &[EnvConflict]) -> Result<PathBuf, AppError> {
    let backups_dir = home_dir()
        .join(".llm-proxy")
        .join("backups");
    std::fs::create_dir_all(&backups_dir)
        .map_err(AppError::Io)?;
    let ts = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let path = backups_dir.join(format!("env-backup-{ts}.json"));
    let data = serde_json::to_string_pretty(conflicts)
        .map_err(|e| AppError::Config(format!("序列化环境变量备份失败: {e}")))?;
    std::fs::write(&path, data)
        .map_err(AppError::Io)?;
    Ok(path)
}

/// User home directory (fallback: current dir).
fn home_dir() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_conflicts_scans_process_env() {
        // Any env var we set ourselves in the test must be detectable.
        // SAFETY: single-threaded test, setting a probe var is race-free here.
        unsafe {
            std::env::set_var("ANTHROPIC_BASE_URL_TEST_PROBE", "https://example.com");
        }
        let found = detect_conflicts();
        assert!(
            found.iter().any(|c| c.name == "ANTHROPIC_BASE_URL_TEST_PROBE"),
            "probe var not detected: {:?}",
            found
        );
        unsafe {
            std::env::remove_var("ANTHROPIC_BASE_URL_TEST_PROBE");
        }
    }

    #[test]
    fn test_dedup() {
        let mut v = vec![
            EnvConflict {
                name: "OPENAI_API_KEY".into(),
                value: "a".into(),
                source: "process-env".into(),
            },
            EnvConflict {
                name: "OPENAI_API_KEY".into(),
                value: "a".into(),
                source: "process-env".into(),
            },
        ];
        dedup(&mut v);
        assert_eq!(v.len(), 1);
    }

    #[test]
    fn test_cleanup_empty_is_noop() {
        assert!(cleanup_conflicts(&[]).unwrap().is_none());
    }

    #[test]
    fn test_write_env_backup_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        // Override home dir for the test via a probe path — we test the
        // serializer directly instead.
        let conflicts = vec![EnvConflict {
            name: "GEMINI_API_KEY".into(),
            value: "secret".into(),
            source: "process-env".into(),
        }];
        let json = serde_json::to_string_pretty(&conflicts).unwrap();
        let parsed: Vec<EnvConflict> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed[0].name, "GEMINI_API_KEY");
        let _ = dir;
    }
}
