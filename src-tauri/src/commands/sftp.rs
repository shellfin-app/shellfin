//! SFTP commands (§4.2, §6). Thin wrappers over `sftp::*`.
//!
//! Common errors: `sessionNotFound`, `notConnected` (no live connection, or it was lost while the
//! request ran), `sftpUnavailable`, `invalidArgument{field}` (path not absolute, NUL, or — for
//! operations on a file — a lossy name containing U+FFFD), `timeout{sftpRequest}`.

use tauri::State;

use crate::app_state::AppState;
use crate::error::AppError;
use crate::ipc::dto::{DirListing, FileEntry, SaveRequest, SaveResult, TextDocument};
use crate::sftp::client::Sftp;
use crate::sftp::{path, save};

const MIB: u64 = 1024 * 1024;

async fn sftp(state: &AppState, session_id: &str) -> Result<Sftp, AppError> {
    let session = state.sessions.session(session_id)?;
    Sftp::for_session(&session).await
}

fn max_text_bytes(state: &AppState) -> u64 {
    u64::from(state.sessions.settings().editor.max_file_size_mib) * MIB
}

#[tauri::command]
pub async fn sftp_realpath(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<String, AppError> {
    if path.is_empty() || path.contains('\0') || path.len() > 4096 {
        return Err(AppError::invalid_argument("path"));
    }
    sftp(&state, &session_id).await?.realpath(&path).await
}

/// Bounded listing (§6.2): `truncated` after 100 000 entries or 48 MiB of listing budget
/// (`LIST_MAX_BYTES`), or after 60 s with the entries read so far; `timeout{listing}`
/// when nothing arrived within 60 s.
#[tauri::command]
pub async fn sftp_list(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<DirListing, AppError> {
    path::validate(&path, "path")?;
    sftp(&state, &session_id).await?.list(&path).await
}

#[tauri::command]
pub async fn sftp_stat(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<FileEntry, AppError> {
    path::validate(&path, "path")?;
    sftp(&state, &session_id).await?.entry(&path).await
}

#[tauri::command]
pub async fn sftp_read_text(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
    encoding: Option<String>,
) -> Result<TextDocument, AppError> {
    path::validate_exact(&path, "path")?;
    let limit = max_text_bytes(&state);
    sftp(&state, &session_id)
        .await?
        .read_text(&path, encoding.as_deref(), limit)
        .await
}

#[tauri::command]
pub async fn sftp_write_text(
    state: State<'_, AppState>,
    session_id: String,
    req: SaveRequest,
) -> Result<SaveResult, AppError> {
    path::validate_exact(&req.path, "req.path")?;
    let limit = max_text_bytes(&state);
    let sftp = sftp(&state, &session_id).await?;
    save::save_text(&sftp, &req, limit).await
}

#[tauri::command]
pub async fn sftp_create_file(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<FileEntry, AppError> {
    path::validate_exact(&path, "path")?;
    sftp(&state, &session_id).await?.create_file(&path).await
}

#[tauri::command]
pub async fn sftp_mkdir(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<FileEntry, AppError> {
    path::validate_exact(&path, "path")?;
    sftp(&state, &session_id).await?.mkdir(&path).await
}

/// Never overwrites (`remoteExists`).
#[tauri::command]
pub async fn sftp_rename(
    state: State<'_, AppState>,
    session_id: String,
    from: String,
    to: String,
) -> Result<FileEntry, AppError> {
    path::validate_exact(&from, "from")?;
    path::validate_exact(&to, "to")?;
    sftp(&state, &session_id).await?.rename(&from, &to).await
}

/// A file, symlink or empty directory (`directoryNotEmpty` otherwise; never recursive).
#[tauri::command]
pub async fn sftp_delete(
    state: State<'_, AppState>,
    session_id: String,
    path: String,
) -> Result<(), AppError> {
    path::validate_exact(&path, "path")?;
    if path::basename(&path).is_empty() {
        return Err(AppError::invalid_argument("path"));
    }
    sftp(&state, &session_id).await?.delete(&path).await
}
