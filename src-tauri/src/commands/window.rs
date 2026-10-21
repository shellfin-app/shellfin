//! Window commands for the custom title bar. They are app commands so the capability
//! needs no `core:window:*` permission: each acts on the calling window only and does nothing
//! beyond what a native title bar offers. `window_close` is the close button: it enters the quit
//! flow (§8.9) exactly like the native one did.

use tauri::{AppHandle, WebviewWindow};

use crate::error::AppError;
use crate::quit;

fn failed(what: &str, err: &tauri::Error) -> AppError {
    AppError::internal(format!("window {what} failed")).with_detail(err.to_string())
}

#[tauri::command]
pub async fn window_minimize(window: WebviewWindow) -> Result<(), AppError> {
    window.minimize().map_err(|e| failed("minimize", &e))
}

/// Maximize, or restore when already maximized. The result arrives as `window:state`.
#[tauri::command]
pub async fn window_toggle_maximize(window: WebviewWindow) -> Result<(), AppError> {
    let maximized = window
        .is_maximized()
        .map_err(|e| failed("state query", &e))?;
    let result = if maximized {
        window.unmaximize()
    } else {
        window.maximize()
    };
    result.map_err(|e| failed("maximize", &e))
}

/// Enter or leave full screen (F11). The result arrives as `window:state`.
#[tauri::command]
pub async fn window_toggle_fullscreen(window: WebviewWindow) -> Result<(), AppError> {
    let fullscreen = window
        .is_fullscreen()
        .map_err(|e| failed("state query", &e))?;
    window
        .set_fullscreen(!fullscreen)
        .map_err(|e| failed("fullscreen", &e))
}

/// The close button: starts the quit flow (`app:quit-requested`), never closes directly.
#[tauri::command]
pub async fn window_close(app: AppHandle) -> Result<(), AppError> {
    quit::handle_close_request(&app);
    Ok(())
}

/// Begin an OS window drag. Call it from a primary-button `mousedown` on a drag region (the OS
/// ends the drag on button release). Double-click handling stays in the frontend
/// (`window_toggle_maximize`).
#[tauri::command]
pub async fn window_start_dragging(window: WebviewWindow) -> Result<(), AppError> {
    window.start_dragging().map_err(|e| failed("drag", &e))
}

/// Translated labels for the macOS native menu (see `app_menu::validate_labels`). `labels` maps
/// custom item ids and `menu.*` title keys to display text; unknown keys are ignored and bad
/// text is `invalidArgument{labels.<key>}`. Each call replaces the previous set (absent keys
/// fall back to English). On macOS the menu bar is rebuilt; elsewhere it only validates.
#[tauri::command]
pub async fn menu_set_labels(
    app: AppHandle,
    labels: std::collections::HashMap<String, String>,
) -> Result<(), AppError> {
    let valid = crate::app_menu::validate_labels(labels)?;
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager;
        let menu = crate::app_menu::build(&app, &valid).map_err(|e| failed("menu build", &e))?;
        app.set_menu(menu).map_err(|e| failed("menu install", &e))?;
        let state = app.state::<crate::app_menu::MenuLabels>();
        *state
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = valid;
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (&app, valid);
    }
    Ok(())
}
