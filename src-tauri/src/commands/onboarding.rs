use serde::Serialize;
use sqlx::Row;
use tauri::State;

use crate::error::AppResult;
use crate::state::AppState;

#[derive(Debug, Serialize)]
pub struct OnboardingState {
    pub completed: bool,
}

#[tauri::command]
pub async fn get_onboarding_state(state: State<'_, AppState>) -> AppResult<OnboardingState> {
    let row = sqlx::query("SELECT completed FROM onboarding WHERE id = 1")
        .fetch_optional(&state.db)
        .await?;
    let completed = row
        .and_then(|r| r.try_get::<i64, _>("completed").ok())
        .map(|v| v != 0)
        .unwrap_or(false);
    Ok(OnboardingState { completed })
}

#[tauri::command]
pub async fn complete_onboarding(state: State<'_, AppState>) -> AppResult<()> {
    sqlx::query("UPDATE onboarding SET completed = 1 WHERE id = 1")
        .execute(&state.db)
        .await?;
    Ok(())
}
