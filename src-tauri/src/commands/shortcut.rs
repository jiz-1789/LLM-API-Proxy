use tauri::State;

use llm_api_proxy_lib::AppState;

// ============================================================================
// Commands
// ============================================================================

/// Check if this is the first run (data directory does not exist).
#[tauri::command]
pub fn check_first_run() -> Result<bool, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("获取当前程序路径失败: {}", e))?;
    let exe_dir = exe.parent()
        .ok_or("无法获取程序目录")?;
    let data_dir = exe_dir.join("data");

    Ok(!data_dir.exists())
}

/// Check if a desktop shortcut already exists.
#[tauri::command]
pub fn check_desktop_shortcut() -> Result<bool, String> {
    let desktop = std::env::var("USERPROFILE")
        .map(|p| std::path::PathBuf::from(p).join("Desktop"))
        .or_else(|_| std::env::var("PUBLIC").map(|p| std::path::PathBuf::from(p).join("Desktop")))
        .map_err(|_| "无法获取桌面路径".to_string())?;

    let shortcut = desktop.join("LLM-API-Proxy.lnk");
    Ok(shortcut.exists())
}

/// Create a desktop shortcut for the current executable.
#[tauri::command]
pub fn create_desktop_shortcut() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("获取当前程序路径失败: {}", e))?;
    let exe_dir = exe.parent()
        .ok_or("无法获取程序目录")?;
    let exe_path = exe.to_string_lossy().to_string();
    let exe_dir_str = exe_dir.to_string_lossy().to_string();

    let desktop = std::env::var("USERPROFILE")
        .map(|p| std::path::PathBuf::from(p).join("Desktop"))
        .or_else(|_| std::env::var("PUBLIC").map(|p| std::path::PathBuf::from(p).join("Desktop")))
        .map_err(|_| "无法获取桌面路径".to_string())?;

    let shortcut_path = desktop.join("LLM-API-Proxy.lnk");

    let ps_script = format!(
        "$ws = New-Object -ComObject WScript.Shell; \
         $s = $ws.CreateShortcut('{}'); \
         $s.TargetPath = '{}'; \
         $s.WorkingDirectory = '{}'; \
         $s.IconLocation = '{}'; \
         $s.Description = 'LLM-API-Proxy'; \
         $s.Save()",
        shortcut_path.to_string_lossy().replace('\\', "\\\\"),
        exe_path.replace('\\', "\\\\"),
        exe_dir_str.replace('\\', "\\\\"),
        exe_path.replace('\\', "\\\\") + ",0",
    );

    let mut cmd = std::process::Command::new("powershell");
    cmd.args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-Command", &ps_script]);

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }

    let output = cmd
        .output()
        .map_err(|e| format!("执行 PowerShell 命令失败: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("创建快捷方式失败: {}", stderr));
    }

    tracing::info!("Desktop shortcut created at {:?}", shortcut_path);
    Ok(())
}


