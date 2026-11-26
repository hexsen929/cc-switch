use crate::{
    proxy::diagnostics::{self, DiagnosticsConfig, RequestDiagnostics},
    store::AppState,
};
use tauri_plugin_dialog::DialogExt;

#[tauri::command]
pub fn get_request_diagnostics(request_id: String) -> Option<RequestDiagnostics> {
    diagnostics::get(&request_id)
}

#[tauri::command]
pub fn get_request_diagnostics_body(
    request_id: String,
    side: diagnostics::BodySide,
    attempt_index: usize,
) -> diagnostics::BodyResult {
    diagnostics::get_body(&request_id, side, attempt_index)
}

#[tauri::command]
pub async fn export_request_diagnostics_body(
    app: tauri::AppHandle,
    request_id: String,
    side: diagnostics::BodySide,
    attempt_index: usize,
) -> Result<bool, String> {
    let side_name = match side {
        diagnostics::BodySide::Request => "request",
        diagnostics::BodySide::Response => "response",
    };
    let name = format!(
        "upstream-{side_name}-attempt-{}.txt",
        attempt_index.saturating_add(1)
    );
    let selected = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .add_filter("Text", &["txt"])
            .set_file_name(&name)
            .blocking_save_file()
    })
    .await
    .map_err(|error| error.to_string())?;
    let Some(selected) = selected else {
        return Ok(false);
    };
    // Fetch again after the dialog so revoking capture also revokes pending exports.
    let data = diagnostics::get_body(&request_id, side, attempt_index);
    let text = data
        .body
        .ok_or_else(|| "完整正文已过期、未采集或不可读".to_string())?;
    let path = selected.into_path().map_err(|error| error.to_string())?;
    tauri::async_runtime::spawn_blocking(move || {
        crate::config::atomic_write_private(&path, text.as_bytes())
    })
    .await
    .map_err(|error| error.to_string())?
    .map_err(|error| error.to_string())?;
    Ok(true)
}

#[tauri::command]
pub fn get_request_diagnostics_config(
    state: tauri::State<'_, AppState>,
) -> Result<DiagnosticsConfig, String> {
    let value = state
        .db
        .get_setting(diagnostics::CAPTURE_SETTING)
        .map_err(|e| e.to_string())?;
    Ok(DiagnosticsConfig {
        capture_bodies: value.as_deref() == Some("true"),
    })
}

#[tauri::command]
pub fn set_request_diagnostics_config(
    state: tauri::State<'_, AppState>,
    config: DiagnosticsConfig,
) -> Result<(), String> {
    state
        .db
        .set_setting(
            diagnostics::CAPTURE_SETTING,
            if config.capture_bodies {
                "true"
            } else {
                "false"
            },
        )
        .map_err(|e| e.to_string())?;
    if !config.capture_bodies {
        diagnostics::clear();
    }
    Ok(())
}
