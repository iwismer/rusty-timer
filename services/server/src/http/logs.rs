use crate::state::AppState;
use axum::{Json, extract::State};
use serde::Serialize;

#[derive(Serialize)]
pub struct LogsResponse {
    pub entries: Vec<String>,
}

pub async fn get_logs(State(state): State<AppState>) -> Json<LogsResponse> {
    Json(LogsResponse {
        entries: state.logger.entries(),
    })
}
