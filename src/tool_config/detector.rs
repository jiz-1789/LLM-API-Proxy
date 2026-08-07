//! Tool installation detection helpers.
//!
//! Detection is primarily based on **config file existence** (like cc-switch),
//! with PATH executable lookup as a secondary signal. GUI tools (Claude
//! Desktop) and tools installed via npm/pnpm/bun are often NOT on PATH, so
//! checking config dirs is the reliable approach.

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

/// Windows `%APPDATA%` directory (falls back to `~/AppData/Roaming`).
pub fn appdata_dir() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(PathBuf::from).or_else(|| {
        home_dir().map(|h| h.join("AppData").join("Roaming"))
    })
}

/// Windows `%LOCALAPPDATA%` directory (falls back to `~/AppData/Local`).
pub fn local_appdata_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(PathBuf::from).or_else(|| {
        home_dir().map(|h| h.join("AppData").join("Local"))
    })
}

// ============================================================================
// Per-tool config path resolution (cc-switch compatible)
// ============================================================================

/// Claude Code settings path: `~/.claude/settings.json`.
pub fn claude_settings_path() -> Option<PathBuf> {
    home_path(".claude/settings.json")
}

/// Claude Desktop config path.
///
/// Windows (cc-switch compatible): `%LOCALAPPDATA%\Claude\claude_desktop_config.json`
/// (also checks the 3p dir `%LOCALAPPDATA%\Claude-3p\`), falling back to
/// `%APPDATA%\Claude\...` for older installs.
/// macOS: `~/Library/Application Support/Claude/claude_desktop_config.json`.
/// Linux: `~/.config/Claude/claude_desktop_config.json`.
pub fn claude_desktop_config_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    #[cfg(windows)]
    {
        if let Some(la) = local_appdata_dir() {
            out.push(la.join("Claude").join("claude_desktop_config.json"));
            out.push(la.join("Claude-3p").join("claude_desktop_config.json"));
        }
        if let Some(ap) = appdata_dir() {
            out.push(ap.join("Claude").join("claude_desktop_config.json"));
        }
    }
    #[cfg(target_os = "macos")]
    {
        if let Some(h) = home_dir() {
            out.push(
                h.join("Library")
                    .join("Application Support")
                    .join("Claude")
                    .join("claude_desktop_config.json"),
            );
        }
    }
    #[cfg(all(not(windows), not(target_os = "macos")))]
    {
        if let Some(h) = home_dir() {
            out.push(h.join(".config").join("Claude").join("claude_desktop_config.json"));
        }
    }
    out
}

/// Codex CLI config: `~/.codex/auth.json` + `~/.codex/config.toml`.
pub fn codex_auth_path() -> Option<PathBuf> {
    home_path(".codex/auth.json")
}
pub fn codex_config_path() -> Option<PathBuf> {
    home_path(".codex/config.toml")
}

/// Gemini CLI: `~/.gemini/settings.json` (+ `.env`).
pub fn gemini_settings_path() -> Option<PathBuf> {
    home_path(".gemini/settings.json")
}

/// Grok CLI: `~/.grok/config.toml`.
pub fn grok_config_path() -> Option<PathBuf> {
    home_path(".grok/config.toml")
}

/// OpenCode: XDG-based `~/.config/opencode/opencode.json` (or `.jsonc`),
/// plus legacy `~/.opencode/config.json` and Windows desktop app data
/// (`%APPDATA%\ai.opencode.desktop\opencode\`).
pub fn opencode_config_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
        && !xdg.is_empty()
    {
        let xdg_base = PathBuf::from(&xdg).join("opencode");
        out.push(xdg_base.join("opencode.json"));
        out.push(xdg_base.join("opencode.jsonc"));
    }
    if let Some(p) = home_path(".config/opencode/opencode.json") {
        out.push(p);
    }
    if let Some(p) = home_path(".config/opencode/opencode.jsonc") {
        out.push(p);
    }
    if let Some(p) = home_path(".opencode/config.json") {
        out.push(p);
    }
    #[cfg(windows)]
    if let Some(ap) = appdata_dir() {
        out.push(ap.join("ai.opencode.desktop").join("opencode").join("opencode.json"));
        out.push(ap.join("ai.opencode.desktop").join("opencode").join("opencode.jsonc"));
    }
    out
}

/// OpenClaw: `~/.openclaw/openclaw.json`.
pub fn openclaw_config_path() -> Option<PathBuf> {
    home_path(".openclaw/openclaw.json")
}

/// Hermes: Windows `%LOCALAPPDATA%\hermes\config.yaml`, else `~/.hermes/config.yaml`.
pub fn hermes_config_path() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        local_appdata_dir().map(|d| d.join("hermes").join("config.yaml"))
    }
    #[cfg(not(windows))]
    {
        home_path(".hermes/config.yaml")
    }
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

// ============================================================================
// Executable-first installation detection (cc-switch compatible dirs)
// ============================================================================

/// npm/pnpm/yarn/bun global bin directories commonly added to PATH.
fn global_bin_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    // Windows npm: %APPDATA%\npm
    if let Some(ap) = appdata_dir() {
        out.push(ap.join("npm"));
    }
    // ~/.local/bin, ~/.bin (Unix)
    if let Some(h) = home_dir() {
        out.push(h.join(".local").join("bin"));
        out.push(h.join(".bin"));
        out.push(h.join("AppData").join("Roaming").join("npm"));
    }
    out
}

/// Locate a CLI tool's executable, checking PATH plus common global bin dirs
/// and known per-tool install locations.
pub fn find_cli(name: &str) -> Option<PathBuf> {
    if let Some(p) = which_in_path(name) {
        return Some(p);
    }
    #[cfg(windows)]
    let exts = ["", ".exe", ".cmd", ".bat"];
    #[cfg(not(windows))]
    let exts = [""];
    for dir in global_bin_dirs() {
        for ext in exts {
            let candidate = dir.join(format!("{}{}", name, ext));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    // Known per-tool install locations (tools often not on PATH).
    let home = home_dir();
    #[cfg(windows)]
    {
        if name == "opencode"
            && let Some(la) = local_appdata_dir()
        {
            // OpenCode desktop app
            for ext in exts {
                let c = la
                    .join("Programs")
                    .join("@opencode-aidesktop")
                    .join(format!("OpenCode{}", ext));
                if c.is_file() {
                    return Some(c);
                }
            }
        }
        if name == "codex"
            && let Some(la) = local_appdata_dir()
        {
            // OpenAI Codex (MSIX/Programs)
            let codex_dir = la.join("OpenAI").join("Codex");
            if codex_dir.is_dir() {
                for ext in exts {
                    let c = codex_dir.join(format!("codex{}", ext));
                    if c.is_file() {
                        return Some(c);
                    }
                }
            }
        }
    }
    if let Some(h) = home {
        // ~/.opencode/bin/opencode
        if name == "opencode" {
            let opencode_bin = h.join(".opencode").join("bin").join("opencode");
            if opencode_bin.is_file() {
                return Some(opencode_bin);
            }
        }
        // ~/.claude/local/claude (Claude Code local install)
        if name == "claude" {
            let claude_local = h.join(".claude").join("local").join("claude");
            if claude_local.is_file() {
                return Some(claude_local);
            }
        }
    }
    None
}

/// Check whether a CLI tool is installed (executable-first).
pub fn cli_installed(name: &str) -> bool {
    find_cli(name).is_some()
}

/// A config directory that counts as "installed" when it contains at least
/// `min_files` entries (a lone orphan file like a stray config.yaml is not a
/// reliable install signal).
pub fn config_dir_installed(dir: Option<PathBuf>, min_files: usize) -> bool {
    let Some(dir) = dir else {
        return false;
    };
    if !dir.is_dir() {
        return false;
    }
    let count = std::fs::read_dir(&dir)
        .map(|entries| entries.filter_map(Result::ok).count())
        .unwrap_or(0);
    count >= min_files
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

    #[test]
    fn test_claude_settings_path() {
        let p = claude_settings_path().unwrap();
        assert!(p.ends_with("settings.json"));
    }

    #[test]
    fn test_opencode_config_path_uses_xdg_or_config() {
        let paths = opencode_config_paths();
        assert!(!paths.is_empty());
        let joined = paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(joined.contains("opencode"), "got {joined}");
        assert!(
            joined.contains("opencode.json") || joined.contains("opencode.jsonc"),
            "got {joined}"
        );
    }

    #[test]
    fn test_codex_paths() {
        assert!(codex_auth_path().unwrap().ends_with("auth.json"));
        assert!(codex_config_path().unwrap().ends_with("config.toml"));
    }

    #[test]
    fn test_gemini_and_grok_paths() {
        assert!(gemini_settings_path().unwrap().ends_with("settings.json"));
        assert!(grok_config_path().unwrap().ends_with("config.toml"));
        assert!(openclaw_config_path().unwrap().ends_with("openclaw.json"));
    }

    #[test]
    fn test_hermes_path() {
        let p = hermes_config_path().unwrap();
        assert!(p.ends_with("config.yaml"), "got {}", p.display());
    }

    #[test]
    fn test_claude_desktop_windows_uses_localappdata() {
        let paths = claude_desktop_config_paths();
        assert!(!paths.is_empty(), "should resolve at least one candidate");
        #[cfg(windows)]
        {
            let s = paths[0].to_string_lossy();
            assert!(s.contains("Claude"), "first candidate should mention Claude, got {s}");
            assert!(s.ends_with("claude_desktop_config.json"), "got {s}");
        }
    }

    #[test]
    fn test_config_dir_installed_requires_min_files() {
        let dir = tempfile::tempdir().unwrap();
        // Single orphan file → NOT installed
        std::fs::write(dir.path().join("config.yaml"), "stray").unwrap();
        assert!(!config_dir_installed(Some(dir.path().to_path_buf()), 2));

        // Add a second file → installed
        std::fs::write(dir.path().join("another.txt"), "x").unwrap();
        assert!(config_dir_installed(Some(dir.path().to_path_buf()), 2));
    }

    #[test]
    fn test_config_dir_installed_none_path() {
        assert!(!config_dir_installed(None, 2));
        assert!(!config_dir_installed(Some(PathBuf::from("definitely_missing_xyz")), 2));
    }

    #[test]
    fn test_find_cli_scopes_per_tool_locations() {
        // The per-tool location checks must not leak across tool names:
        // looking for "hermes" must never return OpenCode.exe even if it exists.
        #[cfg(windows)]
        {
            let la = local_appdata_dir();
            let opencode_exe = la
                .map(|d| d.join("Programs").join("@opencode-aidesktop").join("OpenCode.exe"))
                .filter(|p| p.exists());
            if let Some(_exe) = opencode_exe {
                // This is the regression: "hermes" must NOT resolve to OpenCode.exe.
                let found = find_cli("hermes");
                assert!(
                    found
                        .as_ref()
                        .map(|p| !p.to_string_lossy().contains("opencode"))
                        .unwrap_or(true),
                    "find_cli(\"hermes\") wrongly returned OpenCode path: {found:?}"
                );
            }
        }
    }

    #[test]
    fn test_opencode_includes_desktop_and_config_dirs() {
        let paths = opencode_config_paths();
        let joined = paths
            .iter()
            .map(|p| p.to_string_lossy().to_string())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(joined.contains("opencode.json"), "got {joined}");
        #[cfg(windows)]
        assert!(joined.contains("opencode.desktop"), "desktop path missing, got {joined}");
    }
}

