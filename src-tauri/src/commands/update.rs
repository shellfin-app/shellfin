//! Updater commands (§4.2 "Updates", §9.8). Thin wrappers around `crate::updater`.
//! The WebView has no `updater:` plugin permission; these four commands are its only access.

use tauri::ipc::Channel;
use tauri::{AppHandle, State};

use crate::app_state::AppState;
use crate::commands::blocking;
use crate::error::AppError;
use crate::ipc::dto::{UpdateCheckOutcome, UpdateProgress};
use crate::updater;

/// Manual "Check for updates…": checks the selected channel now, also when automatic checks
/// are off, and reports a skipped version too. `updatesDisabled` in dev/debug/e2e builds.
#[tauri::command]
pub async fn update_check(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<UpdateCheckOutcome, AppError> {
    let updates = state.updates.clone();
    updater::manual_check(&app, &updates).await
}

/// "Update and restart": download `version` (it must be the update last reported), verify its
/// signature, then start the quit flow; the install happens at its end.
#[tauri::command]
pub async fn update_install(
    app: AppHandle,
    state: State<'_, AppState>,
    version: String,
    progress: Channel<UpdateProgress>,
) -> Result<(), AppError> {
    let updates = state.updates.clone();
    updater::install(&app, &updates, &version, move |event| {
        if let Err(e) = progress.send(event) {
            tracing::debug!(error = %e, "update progress channel closed");
        }
    })
    .await
}

/// Stop a running download (it then fails with `cancelled`) and drop a staged update.
#[tauri::command]
pub async fn update_cancel(state: State<'_, AppState>) -> Result<(), AppError> {
    state.updates.cancel();
    Ok(())
}

/// "Skip this version": automatic checks stop announcing exactly this version (stored in
/// `recents.json`; a manual check still shows it).
#[tauri::command]
pub async fn update_skip_version(
    state: State<'_, AppState>,
    version: String,
) -> Result<(), AppError> {
    let stores = state.stores.clone();
    blocking(move || stores.lock_recents().set_skipped_update_version(&version)).await
}
