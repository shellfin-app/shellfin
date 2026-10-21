//! Settings commands (§4.2). Values outside the §4.4 ranges are rejected with
//! `invalidArgument{field}`; the file is written atomically (§7.2).

use tauri::{AppHandle, Emitter, State};

use crate::app_state::AppState;
use crate::commands::blocking;
use crate::error::AppError;
use crate::ipc::dto::{NotificationTexts, Settings, events};

/// A clone under a lock that is never held across file I/O (`settings_update` writes
/// under a separate writer lock), so this stays on the async runtime.
#[tauri::command]
pub async fn settings_get(state: State<'_, AppState>) -> Result<Settings, AppError> {
    Ok(state.stores.settings())
}

/// Write the defaults atomically, apply them like `settings_update`, return them.
#[tauri::command]
pub async fn settings_reset(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Settings, AppError> {
    let stores = state.stores.clone();
    let saved = blocking(move || stores.reset_settings()).await?;
    apply(&app, &state).await;
    Ok(saved)
}

#[tauri::command]
pub async fn settings_update(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, AppError> {
    let stores = state.stores.clone();
    let saved = blocking(move || stores.update_settings(settings)).await?;
    apply(&app, &state).await;
    Ok(saved)
}

/// Settings that take effect immediately: transfer limits, gate and timestamps, log level, and
/// `connection.readSshConfig`: a change re-parses (or empties) the config and
/// emits `ssh-config:changed`.
///
/// Applies the settings as stored *now*, not the ones this call saved: two overlapping updates
/// write the file in one order (writer lock) but may reach this point in the other, and the
/// last apply must match the file.
async fn apply(app: &AppHandle, state: &AppState) {
    let saved = &state.stores.settings();
    state.sessions.apply_settings(saved);
    crate::logging::set_level(saved.advanced.log_level);
    let service = state.ssh_config.clone();
    if service.set_enabled(saved.connection.read_ssh_config) {
        let snapshot = blocking(move || {
            service.reload();
            Ok(service.snapshot())
        })
        .await;
        match snapshot {
            Ok(s) => {
                if let Err(e) = app.emit(events::SSH_CONFIG_CHANGED, s) {
                    tracing::warn!(error = %e, "failed to emit ssh-config:changed");
                }
            }
            Err(e) => tracing::warn!(code = ?e.kind, "ssh config reload failed"),
        }
    }
}

/// Localized texts of the OS notifications. Each 1–200 chars, not blank,
/// no control characters (`invalidArgument{field:"texts.<key>"}`); replaces the whole set.
#[tauri::command]
pub async fn notifications_set_texts(
    state: State<'_, AppState>,
    texts: NotificationTexts,
) -> Result<(), AppError> {
    state.notifications.set_texts(texts)
}
