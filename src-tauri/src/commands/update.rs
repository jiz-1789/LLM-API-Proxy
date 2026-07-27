use serde::{Deserialize, Serialize};
use tauri::{Emitter, Manager};

use llm_api_proxy_lib::AppState;

// ============================================================================
// DTO Types
// ============================================================================

/// Release info returned to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub has_update: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_notes: String,
    pub published_at: String,
    pub source: String,
    pub github_release_url: String,
    pub github_download_url: String,
    pub gitee_release_url: String,
    pub gitee_download_url: String,
}

/// Download progress payload sent to the frontend via Tauri events.
#[derive(Clone, Serialize)]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
    pub percentage: f64,
}

/// Parsed release info from a single source (GitHub or Gitee).
struct ParsedRelease {
    latest_version: String,
    release_url: String,
    download_url: String,
    release_notes: String,
    published_at: String,
}

// ============================================================================
// Helpers
// ============================================================================

fn github_download_url_for_version(version: &str) -> String {
    format!(
        "https://github.com/jiz-1789/LLM-API-Proxy/releases/download/v{}/LLM-API-Proxy_v{}_x64_portable.exe",
        version, version
    )
}

fn gitee_download_url_for_version(version: &str) -> String {
    format!(
        "https://gitee.com/yilichenaiosi/LLM-API-Proxy/releases/download/v{}/LLM-API-Proxy_v{}_x64_portable.exe",
        version, version
    )
}

fn extract_portable_download_url(json: &serde_json::Value) -> String {
    json.get("assets")
        .and_then(|a| a.as_array())
        .and_then(|assets| {
            assets.iter().find_map(|asset| {
                let name = asset.get("name")?.as_str()?;
                if name.contains("portable") && name.ends_with(".exe") {
                    asset.get("browser_download_url")?.as_str()
                } else {
                    None
                }
            })
        })
        .unwrap_or("")
        .to_string()
}

async fn fetch_github_release(client: &reqwest::Client) -> Result<ParsedRelease, String> {
    let url = "https://api.github.com/repos/jiz-1789/LLM-API-Proxy/releases/latest";
    let resp = client
        .get(url)
        .header("User-Agent", "LLM-API-Proxy")
        .header("Accept", "application/vnd.github+json")
        .send()
        .await
        .map_err(|e| format!("请求 GitHub API 失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("GitHub API 返回错误: HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 GitHub API 响应失败: {}", e))?;

    let tag = json.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
    let latest_version = tag.trim_start_matches('v').to_string();
    let release_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://github.com/jiz-1789/LLM-API-Proxy/releases")
        .to_string();
    let download_url = extract_portable_download_url(&json);
    let download_url = if download_url.is_empty() {
        github_download_url_for_version(&latest_version)
    } else {
        download_url
    };
    let release_notes = json.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let published_at = json.get("published_at").and_then(|v| v.as_str()).unwrap_or("").to_string();

    Ok(ParsedRelease {
        latest_version,
        release_url,
        download_url,
        release_notes,
        published_at,
    })
}

async fn fetch_gitee_release(client: &reqwest::Client) -> Result<ParsedRelease, String> {
    let url = "https://gitee.com/api/v5/repos/yilichenaiosi/LLM-API-Proxy/releases/latest";
    let resp = client
        .get(url)
        .header("User-Agent", "LLM-API-Proxy")
        .send()
        .await
        .map_err(|e| format!("请求 Gitee API 失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("Gitee API 返回错误: HTTP {}", resp.status()));
    }

    let json: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("解析 Gitee API 响应失败: {}", e))?;

    let tag = json.get("tag_name").and_then(|v| v.as_str()).unwrap_or("");
    let latest_version = tag.trim_start_matches('v').to_string();
    let release_url = json
        .get("html_url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://gitee.com/yilichenaiosi/LLM-API-Proxy/releases")
        .to_string();
    let download_url = extract_portable_download_url(&json);
    let download_url = if download_url.is_empty() {
        gitee_download_url_for_version(&latest_version)
    } else {
        download_url
    };
    let release_notes = json.get("body").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let published_at = json
        .get("created_at")
        .or_else(|| json.get("published_at"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    Ok(ParsedRelease {
        latest_version,
        release_url,
        download_url,
        release_notes,
        published_at,
    })
}

fn compare_versions(latest: &str, current: &str) -> bool {
    let parse = |s: &str| -> Vec<u32> {
        s.split('.')
            .filter_map(|p| p.parse::<u32>().ok())
            .collect()
    };
    let l = parse(latest);
    let c = parse(current);
    for i in 0..l.len().max(c.len()) {
        let lv = l.get(i).copied().unwrap_or(0);
        let cv = c.get(i).copied().unwrap_or(0);
        if lv > cv {
            return true;
        }
        if lv < cv {
            return false;
        }
    }
    false
}

// ============================================================================
// Commands
// ============================================================================

/// Check for updates from GitHub (primary) and Gitee (fallback).
#[tauri::command]
pub async fn check_for_updates() -> Result<UpdateCheckResult, String> {
    let current_version = env!("CARGO_PKG_VERSION");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let github_result = fetch_github_release(&client).await;
    let gitee_result = fetch_gitee_release(&client).await;

    let (primary, source) = match (&github_result, &gitee_result) {
        (Ok(gh), _) => (gh, "github"),
        (Err(_), Ok(gitee)) => (gitee, "gitee"),
        (Err(gh_err), Err(_)) => {
            return Err(format!("GitHub 和 Gitee 均无法访问: {}", gh_err));
        }
    };

    let has_update = compare_versions(&primary.latest_version, current_version);

    let github_release_url = github_result.as_ref().map(|r| r.release_url.clone()).unwrap_or_default();
    let github_download_url = github_result.as_ref().map(|r| r.download_url.clone()).unwrap_or_default();
    let gitee_release_url = gitee_result.as_ref().map(|r| r.release_url.clone()).unwrap_or_default();
    let gitee_download_url = gitee_result.as_ref().map(|r| r.download_url.clone()).unwrap_or_default();

    Ok(UpdateCheckResult {
        has_update,
        current_version: current_version.to_string(),
        latest_version: primary.latest_version.clone(),
        release_notes: primary.release_notes.clone(),
        published_at: primary.published_at.clone(),
        source: source.to_string(),
        github_release_url,
        github_download_url,
        gitee_release_url,
        gitee_download_url,
    })
}

/// Check if a pending update exists.
#[tauri::command]
pub fn check_pending_update() -> Result<bool, String> {
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("获取当前程序路径失败: {}", e))?;
    let exe_dir = current_exe.parent()
        .ok_or("无法获取程序目录")?;

    let downloading = exe_dir.join("_update_downloading.exe");
    if downloading.exists() {
        let _ = std::fs::remove_file(&downloading);
    }

    let pending = exe_dir.join("_update_pending.exe");
    Ok(pending.exists())
}

/// Download the new portable exe.
#[tauri::command]
pub async fn download_update(
    download_url: String,
    latest_version: String,
    app_handle: tauri::AppHandle,
) -> Result<(), String> {
    if !download_url.starts_with("https://") {
        return Err("下载地址无效，仅支持 HTTPS".to_string());
    }

    let current_exe = std::env::current_exe()
        .map_err(|e| format!("获取当前程序路径失败: {}", e))?;
    let exe_dir = current_exe.parent()
        .ok_or("无法获取程序目录")?;

    let downloading_path = exe_dir.join("_update_downloading.exe");
    let pending_path = exe_dir.join("_update_pending.exe");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败: {}", e))?;

    let resp = client
        .get(&download_url)
        .header("User-Agent", "LLM-API-Proxy")
        .send()
        .await
        .map_err(|e| format!("下载失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("下载失败: HTTP {}", resp.status()));
    }

    let total_size = resp.content_length().unwrap_or(0);

    let _ = app_handle.emit("update-progress", DownloadProgress {
        downloaded: 0,
        total: total_size,
        percentage: 0.0,
    });

    let mut file = std::fs::File::create(&downloading_path)
        .map_err(|e| format!("创建临时文件失败: {}", e))?;

    use std::io::Write;
    let mut resp = resp;
    let mut downloaded: u64 = 0;
    let mut last_report: u64 = 0;

    loop {
        let chunk = resp.chunk()
            .await
            .map_err(|e| {
                let _ = std::fs::remove_file(&downloading_path);
                format!("读取下载数据失败: {}", e)
            })?
            .unwrap_or_default();

        if chunk.is_empty() {
            break;
        }

        file.write_all(&chunk)
            .map_err(|e| {
                let _ = std::fs::remove_file(&downloading_path);
                format!("写入临时文件失败: {}", e)
            })?;

        downloaded += chunk.len() as u64;

        if downloaded - last_report >= 102_400 || (total_size > 0 && downloaded == total_size) {
            last_report = downloaded;
            let percentage = if total_size > 0 {
                (downloaded as f64 / total_size as f64) * 100.0
            } else {
                0.0
            };
            let _ = app_handle.emit("update-progress", DownloadProgress {
                downloaded,
                total: total_size,
                percentage,
            });
        }
    }

    drop(file);

    tracing::info!("Download complete: {} bytes", downloaded);

    let _ = app_handle.emit("update-progress", DownloadProgress {
        downloaded,
        total: total_size,
        percentage: 100.0,
    });

    std::fs::rename(&downloading_path, &pending_path)
        .map_err(|e| {
            let _ = std::fs::remove_file(&downloading_path);
            format!("重命名下载文件失败: {}", e)
        })?;

    let version_path = exe_dir.join("_update_version.txt");
    let _ = std::fs::write(&version_path, &latest_version);

    Ok(())
}

/// Apply a pending update: create a batch updater script, shut down the gateway, and exit.
#[tauri::command]
pub fn apply_update(app_handle: tauri::AppHandle) -> Result<(), String> {
    let current_exe = std::env::current_exe()
        .map_err(|e| format!("获取当前程序路径失败: {}", e))?;
    let exe_dir = current_exe.parent()
        .ok_or("无法获取程序目录")?;

    let pending_path = exe_dir.join("_update_pending.exe");
    if !pending_path.exists() {
        return Err("未找到待安装的更新文件，请重新下载".to_string());
    }

    let exe_name = current_exe.file_name()
        .and_then(|n| n.to_str())
        .ok_or("无法获取程序文件名")?;

    let exe_path = current_exe.to_string_lossy().to_string();

    let version_path = exe_dir.join("_update_version.txt");
    let latest_version = std::fs::read_to_string(&version_path).unwrap_or_default().trim().to_string();
    let new_exe_name = if latest_version.is_empty() {
        exe_name.to_string()
    } else {
        format!("LLM-API-Proxy_v{}_x64_portable.exe", latest_version)
    };
    let new_exe_path = exe_dir.join(&new_exe_name).to_string_lossy().to_string();

    let ps_content = format!(
        "Set-Location -LiteralPath '{exe_dir}'; \
         Start-Sleep -Seconds 2; \
         $maxRetries = 10; \
         $retryCount = 0; \
         while ($retryCount -lt $maxRetries) {{ \
             try {{ Rename-Item -LiteralPath '{exe_path}' -NewName '{exe_name}.bak' -ErrorAction Stop; break; }} \
             catch {{ $retryCount++; Start-Sleep -Seconds 1; }} \
         }}; \
         Move-Item -LiteralPath '{exe_dir}\\_update_pending.exe' -Destination '{new_exe_path}' -Force; \
         Remove-Item -LiteralPath '{exe_dir}\\{exe_name}.bak' -ErrorAction SilentlyContinue; \
         Remove-Item -LiteralPath '{exe_dir}\\_update_version.txt' -ErrorAction SilentlyContinue; \
         $desktop = [Environment]::GetFolderPath('Desktop'); \
         $lnk = Join-Path $desktop 'LLM-API-Proxy.lnk'; \
         if (Test-Path $lnk) {{ \
             $ws = New-Object -ComObject WScript.Shell; \
             $s = $ws.CreateShortcut($lnk); \
             $s.TargetPath = '{new_exe_path}'; \
             $s.WorkingDirectory = '{exe_dir}'; \
             $s.IconLocation = '{new_exe_path},0'; \
             $s.Description = 'LLM-API-Proxy'; \
             $s.Save(); \
         }}; \
         ie4uinit.exe -show 2>$null; \
         Start-Process -FilePath '{new_exe_path}'",
        exe_name = exe_name,
        exe_path = exe_path,
        new_exe_path = new_exe_path,
        exe_dir = exe_dir.to_string_lossy(),
    );

    tracing::info!("Starting update process for {}", exe_path);

    let state = app_handle.state::<AppState>();
    state.shutdown();

    use base64::Engine;
    let ps_bytes: Vec<u8> = ps_content.encode_utf16()
        .flat_map(|c| c.to_le_bytes())
        .collect();
    let encoded_command = base64::engine::general_purpose::STANDARD.encode(&ps_bytes);

    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded_command])
        .current_dir(exe_dir);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    cmd.spawn()
        .map_err(|e| format!("启动更新脚本失败: {}", e))?;

    tracing::info!("Updater script spawned. Exiting app in 500ms to allow handoff.");

    std::thread::sleep(std::time::Duration::from_millis(500));

    app_handle.exit(0);

    Ok(())
}
