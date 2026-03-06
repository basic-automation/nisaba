use tauri::State;

use crate::log_capture::LogLine;
use crate::state::AppState;

#[tauri::command]
pub async fn get_logs(
    state: State<'_, AppState>,
    cursor: Option<usize>,
) -> Result<(Vec<LogLine>, usize), String> {
    Ok(state.log_buffer.read_from(cursor.unwrap_or(0)))
}
