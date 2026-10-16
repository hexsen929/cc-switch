use crate::{
    proxy::diagnostics::{self, DiagnosticsConfig, RequestDiagnostics},
    store::AppState,
};

#[tauri::command]
pub fn get_request_diagnostics(request_id: String) -> Option<RequestDiagnostics> {
    diagnostics::get(&request_id)
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
