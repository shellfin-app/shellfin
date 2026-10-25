//! Transfer commands (§4.2, §6.6). Local paths come only from the native dialogs (opened here)
//! or the drop registry — never from the WebView (§9.4). Dialog titles come from the frontend
//! (§8.2.2) and are sanitized like other frontend texts shown natively.

use std::path::PathBuf;
use std::sync::Arc;

use tauri::ipc::Channel;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::app_state::AppState;
use crate::commands::guarded;
use crate::error::{AppError, ErrorCode};
use crate::external_link::sanitize_dialog_text;
use crate::ipc::dto::{ConflictPolicy, TransferEvent};
use crate::sftp::client::{Sftp, ensure_regular};
use crate::sftp::names::sanitize_local_name;
use crate::sftp::path;
use crate::sftp::transfer::{Conflict, TransferSink, TransferSpec};
use crate::window::MAIN_WINDOW_LABEL;

/// `Channel<TransferEvent>` as a transfer's event sink.
struct ChannelSink(Channel<TransferEvent>);

impl TransferSink for ChannelSink {
    fn send(&self, event: TransferEvent) {
        if let Err(e) = self.0.send(event) {
            tracing::debug!(error = %e, "transfer event channel closed");
        }
    }
}

fn into_paths(picked: Vec<tauri_plugin_dialog::FilePath>) -> Vec<PathBuf> {
    picked
        .into_iter()
        .filter_map(|p| match p.into_path() {
            Ok(p) => Some(p),
            Err(e) => {
                tracing::warn!(error = %e, "ignoring a non-file dialog result");
                None
            }
        })
        .collect()
}

/// A download whose target the user picked (and, if it existed, confirmed) in
/// the native Save dialog always writes it; `transfers.onConflict` applies only to downloads
/// without a dialog (none today) and to uploads.
pub fn download_conflict(chosen_in_dialog: bool, policy: ConflictPolicy) -> Conflict {
    if chosen_in_dialog {
        Conflict::Overwrite
    } else {
        Conflict::resolve(false, policy)
    }
}

/// Native save dialog (default name = sanitized remote basename), then download to
/// `<chosen>.part` → rename → Mark-of-the-Web (Windows). Returns the transferId, or `null` when
/// the dialog was cancelled. `isADirectory` for a remote directory, `notARegularFile` for a
/// FIFO, socket or device (also behind a symlink).
#[tauri::command]
pub async fn transfer_download(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    remote_path: String,
    dialog_title: String,
    progress: Channel<TransferEvent>,
) -> Result<Option<String>, AppError> {
    guarded("transfer_download", async move {
        path::validate_exact(&remote_path, "remotePath")?;
        let session = state.sessions.session(&session_id)?;
        let sftp = Sftp::for_session(&session).await?;
        // A directory ⇒ `isADirectory`; a FIFO, socket or device ⇒ `notARegularFile`
        // (before the Save dialog opens).
        ensure_regular(&sftp.stat(&remote_path).await?, &remote_path)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut dialog = app
            .dialog()
            .file()
            .set_title(sanitize_dialog_text(&dialog_title))
            .set_file_name(sanitize_local_name(&remote_path));
        if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            dialog = dialog.set_parent(&window);
        }
        dialog.save_file(move |picked| {
            let _ = tx.send(picked);
        });
        let Some(picked) = rx.await.ok().flatten() else {
            return Ok(None);
        };
        let Some(local) = into_paths(vec![picked]).pop() else {
            return Ok(None);
        };
        let conflict = download_conflict(true, state.sessions.settings().transfers.on_conflict);
        let id = state.sessions.transfers().start_with(
            session,
            TransferSpec::Download {
                remote: remote_path,
                local,
            },
            conflict,
            Arc::new(ChannelSink(progress)),
        );
        Ok(Some(id))
    })
    .await
}

fn start_uploads(
    state: &AppState,
    session_id: &str,
    paths: Vec<PathBuf>,
    remote_dir: &str,
    overwrite: bool,
    progress: Channel<TransferEvent>,
) -> Result<Vec<String>, AppError> {
    let session = state.sessions.session(session_id)?;
    let sink: Arc<dyn TransferSink> = Arc::new(ChannelSink(progress));
    let conflict = Conflict::resolve(overwrite, state.sessions.settings().transfers.on_conflict);
    Ok(paths
        .into_iter()
        .map(|local| {
            state.sessions.transfers().start_with(
                session.clone(),
                TransferSpec::Upload {
                    local,
                    remote_dir: remote_dir.to_owned(),
                },
                conflict,
                sink.clone(),
            )
        })
        .collect())
}

/// Native multi-file picker, then one upload per file. Returns the transferIds (empty when the
/// dialog was cancelled); per-file failures arrive as `failed` events.
#[tauri::command]
pub async fn transfer_upload_pick(
    app: AppHandle,
    state: State<'_, AppState>,
    session_id: String,
    remote_dir: String,
    dialog_title: String,
    overwrite: bool,
    progress: Channel<TransferEvent>,
) -> Result<Vec<String>, AppError> {
    guarded("transfer_upload_pick", async move {
        path::validate_exact(&remote_dir, "remoteDir")?;
        state.sessions.session(&session_id)?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut dialog = app
            .dialog()
            .file()
            .set_title(sanitize_dialog_text(&dialog_title));
        if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            dialog = dialog.set_parent(&window);
        }
        dialog.pick_files(move |picked| {
            let _ = tx.send(picked);
        });
        let paths = into_paths(rx.await.ok().flatten().unwrap_or_default());
        start_uploads(&state, &session_id, paths, &remote_dir, overwrite, progress)
    })
    .await
}

/// Upload the files of an OS drop (`drop:files`); the drop is consumed. `dropExpired` if the
/// dropId is unknown, already used or older than 60 s.
#[tauri::command]
pub async fn transfer_upload_dropped(
    state: State<'_, AppState>,
    session_id: String,
    drop_id: String,
    remote_dir: String,
    overwrite: bool,
    progress: Channel<TransferEvent>,
) -> Result<Vec<String>, AppError> {
    guarded("transfer_upload_dropped", async move {
        path::validate_exact(&remote_dir, "remoteDir")?;
        state.sessions.session(&session_id)?;
        let paths = state.drops.take(&drop_id)?;
        start_uploads(&state, &session_id, paths, &remote_dir, overwrite, progress)
    })
    .await
}

/// Re-run a failed retryable transfer (e.g. `remoteExists` with `overwrite: true`). Returns the
/// new transferId; `notFound{what:"transfer"}` if unknown, expired (10 min) or its session closed.
#[tauri::command]
pub async fn transfer_retry(
    state: State<'_, AppState>,
    transfer_id: String,
    overwrite: bool,
    progress: Channel<TransferEvent>,
) -> Result<String, AppError> {
    guarded("transfer_retry", async move {
        let transfers = state.sessions.transfers();
        let (session_id, spec) = transfers.take_failed(&transfer_id)?;
        let Some(session) = state.sessions.get(&session_id) else {
            return Err(AppError::new(ErrorCode::NotFound {
                what: crate::error::EntityKind::Transfer,
                id: transfer_id,
            }));
        };
        let conflict = match &spec {
            TransferSpec::Upload { .. } => {
                Conflict::resolve(overwrite, state.sessions.settings().transfers.on_conflict)
            }
            // The target was chosen in the Save dialog of the original download.
            TransferSpec::Download { .. } => {
                download_conflict(true, state.sessions.settings().transfers.on_conflict)
            }
        };
        Ok(transfers.start_with(session, spec, conflict, Arc::new(ChannelSink(progress))))
    })
    .await
}

/// `WindowEvent::DragDrop` → `drop:hover` / `drop:leave` / `drop:files` (§4.2, §6.6). Positions
/// are converted to logical px. On drop the paths go into the drop registry (off the UI thread:
/// `isDir` needs a stat, which can be slow on network paths) and only names + `isDir` + the
/// `dropId` are emitted.
pub fn on_drag_drop<R: tauri::Runtime>(window: &tauri::Window<R>, event: &tauri::DragDropEvent) {
    use tauri::Emitter;

    use crate::ipc::dto::{DropFilesPayload, DropHoverPayload, EmptyPayload, events};

    let scale = window.scale_factor().unwrap_or(1.0);
    let logical = |p: &tauri::PhysicalPosition<f64>| {
        let l = p.to_logical::<f64>(scale);
        (l.x, l.y)
    };
    let emit = |name: &str, result: Result<(), tauri::Error>| {
        if let Err(e) = result {
            tracing::warn!(error = %e, event = name, "could not emit a drop event");
        }
    };
    match event {
        tauri::DragDropEvent::Enter { position, .. } | tauri::DragDropEvent::Over { position } => {
            let (position_x, position_y) = logical(position);
            emit(
                events::DROP_HOVER,
                window.emit(
                    events::DROP_HOVER,
                    DropHoverPayload {
                        position_x,
                        position_y,
                    },
                ),
            );
        }
        tauri::DragDropEvent::Leave => {
            emit(
                events::DROP_LEAVE,
                window.emit(events::DROP_LEAVE, EmptyPayload {}),
            );
        }
        tauri::DragDropEvent::Drop { paths, position } => {
            let (position_x, position_y) = logical(position);
            let paths = paths.clone();
            let app = window.app_handle().clone();
            tauri::async_runtime::spawn_blocking(move || {
                let entries = paths
                    .iter()
                    .map(|p| crate::sftp::drops::entry_of(p))
                    .collect();
                let Some(state) = app.try_state::<AppState>() else {
                    return;
                };
                let drop_id = state.drops.register(paths);
                let payload = DropFilesPayload {
                    drop_id,
                    entries,
                    position_x,
                    position_y,
                };
                if let Err(e) = app.emit(events::DROP_FILES, payload) {
                    tracing::warn!(error = %e, "could not emit drop:files");
                }
            });
        }
        _ => {}
    }
}

/// Idempotent: a running transfer stops (`cancelled`, temp/`.part` files removed); a failed one is
/// forgotten.
#[tauri::command]
pub async fn transfer_cancel(
    state: State<'_, AppState>,
    transfer_id: String,
) -> Result<(), AppError> {
    guarded("transfer_cancel", async move {
        state.sessions.transfers().cancel(&transfer_id);
        Ok(())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `onConflict = skip` never skips a download chosen in the Save
    /// dialog; it still applies without a dialog.
    #[test]
    fn dialog_downloads_are_never_skipped() {
        for p in [
            ConflictPolicy::Ask,
            ConflictPolicy::Overwrite,
            ConflictPolicy::Skip,
        ] {
            assert_eq!(download_conflict(true, p), Conflict::Overwrite, "{p:?}");
        }
        assert_eq!(
            download_conflict(false, ConflictPolicy::Skip),
            Conflict::Skip
        );
        assert_eq!(
            download_conflict(false, ConflictPolicy::Ask),
            Conflict::Fail
        );
    }
}
