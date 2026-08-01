use tauri::State;

use crate::claude_append_instructions::{
    self, ClaudeAppendInstructionsConfig, ClaudeAppendInstructionsFileStatus,
};
use crate::store::AppState;

#[tauri::command]
pub async fn get_claude_append_instructions_config(
    state: State<'_, AppState>,
) -> Result<ClaudeAppendInstructionsConfig, String> {
    claude_append_instructions::get_config(state.db.as_ref()).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn set_claude_append_instructions_config(
    config: ClaudeAppendInstructionsConfig,
    state: State<'_, AppState>,
) -> Result<ClaudeAppendInstructionsConfig, String> {
    claude_append_instructions::update_config(state.db.as_ref(), config)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn inspect_claude_append_instructions_file(
    configuredPath: String,
) -> Result<ClaudeAppendInstructionsFileStatus, String> {
    tauri::async_runtime::spawn_blocking(move || {
        claude_append_instructions::inspect_file(&configuredPath)
    })
    .await
    .map_err(|error| format!("Failed to inspect Claude append instructions file: {error}"))
}

#[tauri::command]
pub async fn read_claude_append_instructions_file(
    configuredPath: String,
) -> Result<Option<String>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        claude_append_instructions::read_file(&configuredPath)
    })
    .await
    .map_err(|error| format!("Failed to read Claude append instructions file: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn write_claude_append_instructions_file(
    configuredPath: String,
    content: String,
    state: State<'_, AppState>,
) -> Result<ClaudeAppendInstructionsFileStatus, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        claude_append_instructions::write_file(db.as_ref(), &configuredPath, &content)
    })
    .await
    .map_err(|error| format!("Failed to write Claude append instructions file: {error}"))?
    .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn delete_claude_append_instructions_file(
    configuredPath: String,
    state: State<'_, AppState>,
) -> Result<bool, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || {
        claude_append_instructions::delete_file(db.as_ref(), &configuredPath)
    })
    .await
    .map_err(|error| format!("Failed to delete Claude append instructions file: {error}"))?
    .map_err(|error| error.to_string())
}
