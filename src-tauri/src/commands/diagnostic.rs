use tauri::State;

use llm_api_proxy_lib::AppState;

// ============================================================================
// Diagnostic Package Commands
// ============================================================================

/// Export a one-click diagnostic ZIP package.
/// Shows a native save dialog and writes the archive to the chosen path.
/// All sensitive data (API keys, tokens) is masked before export.
#[tauri::command]
pub async fn export_diagnostic(state: State<'_, AppState>) -> Result<String, String> {
    let file_handle = rfd::AsyncFileDialog::new()
        .set_file_name("llm-api-proxy-diagnostic.zip")
        .add_filter("ZIP 压缩包", &["zip"])
        .add_filter("所有文件", &["*"])
        .save_file()
        .await
        .ok_or_else(|| "用户取消了导出".to_string())?;

    let path = file_handle.path().to_path_buf();

    llm_api_proxy_lib::diagnostic::export_diagnostic_zip(&state.db, &path)
        .map_err(|e| format!("导出诊断包失败: {}", e))?;

    Ok(path.to_string_lossy().to_string())
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    // The export_diagnostic command requires a Tauri runtime and cannot be
    // unit-tested directly. The underlying logic is tested in the
    // diagnostic module (src/diagnostic.rs).
}
