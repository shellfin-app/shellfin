//! Hosts & storage commands (§4.2): ssh config snapshot, bookmarks, groups, recents.

use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::app_state::AppState;
use crate::commands::{blocking, guarded};
use crate::error::{AppError, EntityKind, ErrorCode, ImportInvalidReason, IoOp};
use crate::external_link::sanitize_dialog_text;
use crate::ipc::dto::{
    AuthConfig, Bookmark, BookmarkExportFormat, BookmarkInput, BookmarksDto, BookmarksExportResult,
    BookmarksImportPreview, BookmarksImportResult, Group, GroupInput, ImportStrategy, RecentEntry,
    SshConfigSnapshot, events,
};
use crate::ipc::secret::Secret;
use crate::platform::file_trust::FileKind;
use crate::platform::local_path::{LocalPathError, read_local};
use crate::secrets;
use crate::sshconfig::ConfigEnv;
use crate::sshconfig::ParsedConfig;
use crate::storage::bookmarks::{self, PasswordBinding};
use crate::storage::file::now_ms;
use crate::window::MAIN_WINDOW_LABEL;

#[tauri::command]
pub async fn ssh_config_reload(state: State<'_, AppState>) -> Result<SshConfigSnapshot, AppError> {
    guarded("ssh_config_reload", async move {
        let service = state.ssh_config.clone();
        blocking(move || {
            service.reload();
            Ok(service.snapshot())
        })
        .await
    })
    .await
}

/// Every store lock is taken inside `blocking()`, never on a runtime worker: the std
/// mutex can be held for a long store operation (a large import, a slow disk write).
#[tauri::command]
pub async fn bookmarks_get(state: State<'_, AppState>) -> Result<BookmarksDto, AppError> {
    guarded("bookmarks_get", async move {
        let stores = state.stores.clone();
        blocking(move || Ok(stores.lock_bookmarks().dto())).await
    })
    .await
}

/// Delete keychain entries after the store forgot them. If a delete fails, the entry stays
/// orphaned: it is NOT auto-used or offered, because a bookmark account without a stored
/// binding is a mismatch (`KeychainBinding::Missing`), and it is deleted again at the
/// next startup if the bookmark is gone. The failure is logged.
pub(crate) fn forget_keychain(ids: impl IntoIterator<Item = String>) {
    for id in ids {
        if let Err(e) = secrets::delete_password(&secrets::bookmark_account(&id)) {
            tracing::warn!(error = ?e.kind, "could not delete a forgotten keychain entry");
        }
    }
}

#[tauri::command]
pub async fn bookmark_save(
    state: State<'_, AppState>,
    input: BookmarkInput,
) -> Result<Bookmark, AppError> {
    guarded("bookmark_save", async move {
        let stores = state.stores.clone();
        let bookmark = blocking(move || {
            let outcome = stores.lock_bookmarks().save(input, now_ms())?;
            if outcome.forget_password {
                forget_keychain([outcome.bookmark.id.clone()]);
            }
            Ok(outcome.bookmark)
        })
        .await?;
        // Live edits of connected sessions (limits now; label/color via `bookmarkUpdated`).
        state.sessions.apply_bookmark(&bookmark);
        Ok(bookmark)
    })
    .await
}

/// Also deletes the bookmark's keychain entry (and those of bookmarks whose saved password was
/// bound to it as a jump host), and prunes its recents entry (`recents:changed`).
#[tauri::command]
pub async fn bookmark_delete(
    app: AppHandle,
    state: State<'_, AppState>,
    id: String,
) -> Result<(), AppError> {
    guarded("bookmark_delete", async move {
        let stores = state.stores.clone();
        let recents = blocking(move || {
            let (outcome, recents) = stores.delete_bookmark(&id, now_ms())?;
            let mut forget = outcome.forgotten_dependents;
            if outcome.had_password {
                forget.push(id);
            }
            forget_keychain(forget);
            Ok(recents)
        })
        .await?;
        if let Some(list) = recents
            && let Err(e) = app.emit(events::RECENTS_CHANGED, list)
        {
            tracing::warn!(error = %e, "failed to emit recents:changed");
        }
        Ok(())
    })
    .await
}

#[tauri::command]
pub async fn bookmark_copy_from_ssh_config(
    state: State<'_, AppState>,
    alias: String,
) -> Result<Bookmark, AppError> {
    guarded("bookmark_copy_from_ssh_config", async move {
        let stores = state.stores.clone();
        let service = state.ssh_config.clone();
        blocking(move || {
            if !service.lists_host(&alias) {
                return Err(AppError::new(ErrorCode::NotFound {
                    what: EntityKind::SshConfigHost,
                    id: alias,
                }));
            }
            stores
                .lock_bookmarks()
                .copy_from_ssh_config(&alias, now_ms())
        })
        .await
    })
    .await
}

/// The keychain binding of a bookmark's destination (§5.6, §7.2): bookmark fields win; with
/// `useSshConfig` the config fills empty fields; defaults hostname = host, port 22, local user.
fn destination_binding(b: &Bookmark, cfg: &ParsedConfig, env: &ConfigEnv) -> PasswordBinding {
    let from_cfg = b
        .use_ssh_config
        .then(|| cfg.query(&b.host, env).ok())
        .flatten();
    let hostname = from_cfg
        .as_ref()
        .and_then(|c| c.hostname.clone())
        .unwrap_or_else(|| b.host.clone());
    let port = b
        .port
        .or_else(|| from_cfg.as_ref().and_then(|c| c.port))
        .unwrap_or(22);
    let user = b
        .user
        .clone()
        .or_else(|| from_cfg.as_ref().and_then(|c| c.user.clone()))
        .unwrap_or_else(|| env.local_user.clone());
    PasswordBinding {
        hostname,
        port,
        user,
    }
}

/// No new secret is stored while `connection.rememberPasswords` is off
/// (`secretStorageDisabled`); forgetting a password is always allowed.
pub fn secret_storage_allowed(settings: &crate::ipc::dto::Settings) -> Result<(), AppError> {
    if settings.connection.remember_passwords {
        Ok(())
    } else {
        Err(AppError::new(ErrorCode::SecretStorageDisabled))
    }
}

/// Save (`Some`) or forget (`null`) the bookmark's password in the OS keychain, bound to the
/// destination `{hostname, port, user}`. Only `password` auth can have a saved password.
#[tauri::command]
pub async fn bookmark_set_password(
    state: State<'_, AppState>,
    id: String,
    password: Option<Secret<String>>,
) -> Result<Bookmark, AppError> {
    guarded("bookmark_set_password", async move {
        let stores = state.stores.clone();
        let service = state.ssh_config.clone();
        blocking(move || {
            let bookmark = stores.lock_bookmarks().get(&id).ok_or_else(|| {
                AppError::new(ErrorCode::NotFound {
                    what: EntityKind::Bookmark,
                    id: id.clone(),
                })
            })?;
            if !matches!(bookmark.auth, AuthConfig::Password { .. }) {
                return Err(AppError::invalid_argument("auth"));
            }
            let account = secrets::bookmark_account(&id);
            match password {
                None => {
                    secrets::delete_password(&account)?;
                    stores
                        .lock_bookmarks()
                        .set_password_saved(&id, None, now_ms())
                }
                Some(pw) => {
                    secret_storage_allowed(&stores.settings())?;
                    secrets::availability()?;
                    let binding = destination_binding(&bookmark, &service.current(), service.env());
                    secrets::set_password(&account, pw.expose())?;
                    let saved =
                        stores
                            .lock_bookmarks()
                            .set_password_saved(&id, Some(binding), now_ms());
                    if saved.is_err() {
                        // Keep keychain and store consistent: no entry without a binding.
                        let _ = secrets::delete_password(&account);
                    }
                    saved
                }
            }
        })
        .await
    })
    .await
}

#[tauri::command]
pub async fn group_save(state: State<'_, AppState>, input: GroupInput) -> Result<Group, AppError> {
    guarded("group_save", async move {
        let stores = state.stores.clone();
        blocking(move || stores.lock_bookmarks().group_save(input)).await
    })
    .await
}

/// Move a bookmark to `group_id` (`null` = ungrouped) at `index` (clamped); one atomic write.
#[tauri::command]
pub async fn bookmark_move(
    state: State<'_, AppState>,
    id: String,
    group_id: Option<String>,
    index: u32,
) -> Result<BookmarksDto, AppError> {
    guarded("bookmark_move", async move {
        let stores = state.stores.clone();
        blocking(move || {
            stores
                .lock_bookmarks()
                .bookmark_move(&id, group_id, index, now_ms())
        })
        .await
    })
    .await
}

/// Move a group to `index` (clamped) among the groups; one atomic write.
#[tauri::command]
pub async fn group_move(
    state: State<'_, AppState>,
    id: String,
    index: u32,
) -> Result<BookmarksDto, AppError> {
    guarded("group_move", async move {
        let stores = state.stores.clone();
        blocking(move || stores.lock_bookmarks().group_move(&id, index)).await
    })
    .await
}

#[tauri::command]
pub async fn group_delete(state: State<'_, AppState>, id: String) -> Result<(), AppError> {
    guarded("group_delete", async move {
        let stores = state.stores.clone();
        blocking(move || stores.lock_bookmarks().group_delete(&id, now_ms())).await
    })
    .await
}

#[tauri::command]
pub async fn recents_get(state: State<'_, AppState>) -> Result<Vec<RecentEntry>, AppError> {
    guarded("recents_get", async move {
        let stores = state.stores.clone();
        blocking(move || Ok(stores.lock_recents().list())).await
    })
    .await
}

/// Default file names offered by the export Save dialog.
pub const EXPORT_JSON_NAME: &str = "shellfin-bookmarks.json";
pub const EXPORT_SSH_CONFIG_NAME: &str = "shellfin-ssh-config";

/// Map a failed read of the import file (chosen in the native dialog) to an app error.
fn import_read_error(e: LocalPathError, path: &std::path::Path) -> AppError {
    let shown = path.display().to_string();
    match e {
        LocalPathError::NotFound => AppError::new(ErrorCode::LocalNotFound { path: shown }),
        LocalPathError::PermissionDenied => {
            AppError::new(ErrorCode::LocalPermissionDenied { path: shown })
        }
        LocalPathError::TooLarge(_) => AppError::new(ErrorCode::ImportInvalidFile {
            reason: ImportInvalidReason::TooLarge,
        }),
        LocalPathError::NotAFile => AppError::new(ErrorCode::LocalIsADirectory { path: shown }),
        other => AppError::new(ErrorCode::Io {
            op: IoOp::ReadLocalFile,
            path: Some(shown),
        })
        .with_detail(other.to_string()),
    }
}

/// Export the bookmarks through the native Save dialog (the path never comes from the WebView).
/// JSON never contains secrets; `sshConfig` writes OpenSSH `Host` blocks. Atomic write, 0600 on
/// Unix. A cancelled dialog is `saved: false`.
#[tauri::command]
pub async fn bookmarks_export(
    app: AppHandle,
    state: State<'_, AppState>,
    format: BookmarkExportFormat,
    dialog_title: String,
) -> Result<BookmarksExportResult, AppError> {
    guarded("bookmarks_export", async move {
        let stores = state.stores.clone();
        let dto = blocking(move || Ok(stores.lock_bookmarks().dto())).await?;
        let now = now_ms();
        let (bytes, count, skipped, default_name) = match format {
            BookmarkExportFormat::Json => {
                let (bytes, count) = bookmarks::export_json(&dto, now);
                (bytes, count, Vec::new(), EXPORT_JSON_NAME)
            }
            BookmarkExportFormat::SshConfig => {
                let (text, count, skipped) = bookmarks::export_ssh_config(&dto, now);
                (text.into_bytes(), count, skipped, EXPORT_SSH_CONFIG_NAME)
            }
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut dialog = app
            .dialog()
            .file()
            .set_title(sanitize_dialog_text(&dialog_title))
            .set_file_name(default_name);
        if let Ok(downloads) = app.path().download_dir()
            && downloads.is_dir()
        {
            dialog = dialog.set_directory(downloads);
        }
        if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            dialog = dialog.set_parent(&window);
        }
        dialog.save_file(move |picked| {
            let _ = tx.send(picked);
        });
        let Some(picked) = rx.await.ok().flatten() else {
            return Ok(BookmarksExportResult {
                saved: false,
                file_name: None,
                count: 0,
                skipped: Vec::new(),
            });
        };
        let path = picked
            .into_path()
            .map_err(|e| AppError::invalid_argument("path").with_detail(e.to_string()))?;
        let target = path.clone();
        blocking(move || crate::storage::file::write_chosen_file(&target, &bytes)).await?;
        tracing::info!(count, skipped = skipped.len(), "exported bookmarks");
        Ok(BookmarksExportResult {
            saved: true,
            file_name: path.file_name().map(|n| n.to_string_lossy().into_owned()),
            count,
            skipped,
        })
    })
    .await
}

/// Step 1 of a bookmark import: pick a Shellfin JSON file in the native Open dialog, read it
/// through the `OsChosen` local-path rules with a 5 MiB cap and validate it field by field.
/// Nothing is written: the parsed file waits in `AppState.pending_import` under the returned
/// token (a new open drops any previous pending import, also when its dialog is cancelled).
#[tauri::command]
pub async fn bookmarks_import_open(
    app: AppHandle,
    state: State<'_, AppState>,
    dialog_title: String,
) -> Result<BookmarksImportPreview, AppError> {
    guarded("bookmarks_import_open", async move {
        state.pending_import.clear();
        let stores = state.stores.clone();
        if blocking(move || Ok(stores.lock_bookmarks().read_only())).await? {
            return Err(AppError::new(ErrorCode::StorageReadOnly));
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut dialog = app
            .dialog()
            .file()
            .set_title(sanitize_dialog_text(&dialog_title))
            .add_filter("JSON", &["json"]);
        if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            dialog = dialog.set_parent(&window);
        }
        dialog.pick_file(move |picked| {
            let _ = tx.send(picked);
        });
        let Some(picked) = rx.await.ok().flatten() else {
            return Ok(BookmarksImportPreview {
                picked: false,
                token: None,
                bookmarks: 0,
                conflicts: 0,
                invalid: Vec::new(),
                startup_commands: Vec::new(),
            });
        };
        let path = picked
            .into_path()
            .map_err(|e| AppError::invalid_argument("path").with_detail(e.to_string()))?;
        let stores = state.stores.clone();
        let (parsed, mut preview) = blocking(move || {
            let bytes = read_local(&path, FileKind::ImportSource)
                .map_err(|e| import_read_error(e, &path))?;
            let parsed = bookmarks::parse_import(&bytes)?;
            let preview = stores.lock_bookmarks().import_preview(&parsed);
            Ok((parsed, preview))
        })
        .await?;
        preview.token = Some(state.pending_import.put(parsed));
        tracing::info!(
            bookmarks = preview.bookmarks,
            conflicts = preview.conflicts,
            invalid = preview.invalid.len(),
            startup_commands = preview.startup_commands.len(),
            "opened bookmark import"
        );
        Ok(preview)
    })
    .await
}

/// Step 2: apply the pending import of `token` in one atomic write with the chosen conflict
/// `strategy`. Its startup commands are imported only with `keep_startup_commands` (the user
/// opted in after seeing them). The pending import is consumed by every attempt. No secret is
/// ever imported.
#[tauri::command]
pub async fn bookmarks_import_apply(
    state: State<'_, AppState>,
    token: String,
    strategy: ImportStrategy,
    keep_startup_commands: bool,
) -> Result<BookmarksImportResult, AppError> {
    guarded("bookmarks_import_apply", async move {
        let parsed = state.pending_import.take(&token)?;
        let stores = state.stores.clone();
        let (outcome, dto) = blocking(move || {
            let mut store = stores.lock_bookmarks();
            // `import` refuses a read-only store (`storageReadOnly`) before changing anything.
            let outcome = store.import(parsed, strategy, keep_startup_commands, now_ms())?;
            let dto = outcome.changed.then(|| store.dto());
            drop(store);
            forget_keychain(outcome.forget_password.clone());
            Ok((outcome, dto))
        })
        .await?;
        if let Some(dto) = &dto {
            for id in &outcome.replaced_ids {
                if let Some(b) = dto.bookmarks.iter().find(|b| &b.id == id) {
                    state.sessions.apply_bookmark(b);
                }
            }
        }
        tracing::info!(
            added = outcome.added,
            replaced = outcome.replaced,
            skipped = outcome.skipped,
            invalid = outcome.invalid.len(),
            startup_commands_stripped = outcome.startup_commands_stripped.len(),
            "imported bookmarks"
        );
        Ok(BookmarksImportResult {
            picked: true,
            added: outcome.added,
            replaced: outcome.replaced,
            skipped: outcome.skipped,
            invalid: outcome.invalid,
            bookmarks: dto,
            startup_commands_stripped: outcome.startup_commands_stripped,
        })
    })
    .await
}

/// Drop the pending import of `token` (the user closed the preview). Another or an unknown
/// token is a no-op.
#[tauri::command]
pub async fn bookmarks_import_cancel(
    state: State<'_, AppState>,
    token: String,
) -> Result<(), AppError> {
    guarded("bookmarks_import_cancel", async move {
        state.pending_import.cancel(&token);
        Ok(())
    })
    .await
}

/// Forget the recent connections (`recents.json` emptied); emits `recents:changed`.
#[tauri::command]
pub async fn recents_clear(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<Vec<RecentEntry>, AppError> {
    guarded("recents_clear", async move {
        let stores = state.stores.clone();
        let list = blocking(move || stores.lock_recents().clear()).await?;
        if let Err(e) = app.emit(events::RECENTS_CHANGED, list.clone()) {
            tracing::warn!(error = %e, "failed to emit recents:changed");
        }
        Ok(list)
    })
    .await
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::ipc::dto::AuthConfig;
    use crate::sshconfig::{StdFsSource, load};

    /// The export file is written atomically and privately; secrets are not in it.
    #[test]
    fn export_file_is_atomic_and_private() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join(EXPORT_JSON_NAME);
        std::fs::write(&target, b"old").unwrap();
        let (bytes, count) = bookmarks::export_json(&BookmarksDto::default(), 0);
        assert_eq!(count, 0);
        crate::storage::file::atomic_write(&target, &bytes).unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), bytes);
        let leftovers: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "no temp file is left behind");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let fresh = dir.path().join(EXPORT_SSH_CONFIG_NAME);
            crate::storage::file::atomic_write(&fresh, b"x").unwrap();
            assert_eq!(
                std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn remember_passwords_off_refuses_new_secrets() {
        let mut s = crate::storage::settings::default_settings();
        secret_storage_allowed(&s).unwrap();
        s.connection.remember_passwords = false;
        let e = secret_storage_allowed(&s).unwrap_err();
        assert_eq!(e.kind, ErrorCode::SecretStorageDisabled);
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({ "code": "secretStorageDisabled" })
        );
    }

    #[test]
    fn import_read_errors_map_to_codes() {
        let p = std::path::Path::new("/x/y.json");
        assert!(matches!(
            import_read_error(LocalPathError::NotFound, p).kind,
            ErrorCode::LocalNotFound { .. }
        ));
        assert!(matches!(
            import_read_error(LocalPathError::PermissionDenied, p).kind,
            ErrorCode::LocalPermissionDenied { .. }
        ));
        assert_eq!(
            import_read_error(LocalPathError::TooLarge(1), p).kind,
            ErrorCode::ImportInvalidFile {
                reason: ImportInvalidReason::TooLarge
            }
        );
        assert!(matches!(
            import_read_error(LocalPathError::NotAFile, p).kind,
            ErrorCode::LocalIsADirectory { .. }
        ));
    }

    fn bookmark(host: &str, port: Option<u16>, user: Option<&str>, use_cfg: bool) -> Bookmark {
        Bookmark {
            id: "b1".into(),
            name: "n".into(),
            host: host.into(),
            port,
            user: user.map(str::to_owned),
            auth: AuthConfig::Password {
                password_saved: false,
            },
            proxy_jump: None,
            group_id: None,
            tags: vec![],
            start_dir: None,
            use_ssh_config: use_cfg,
            color: None,
            order: 0,
            upload_limit_kibps: None,
            download_limit_kibps: None,
            confirm_connect: false,
            startup_command: None,
            notes: None,
            created_at_ms: 0,
            updated_at_ms: 0,
        }
    }

    #[test]
    fn binding_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().to_path_buf();
        std::fs::create_dir(home.join(".ssh")).unwrap();
        std::fs::write(
            home.join(".ssh/config"),
            "Host nas\n  HostName nas.example.com\n  Port 2222\n  User admin\n",
        )
        .unwrap();
        let env = ConfigEnv::new(
            home.clone(),
            "alice".into(),
            home.join("etc"),
            home.join("etc/ssh_config"),
        );
        let cfg = load(&StdFsSource, &env);
        let b = destination_binding(&bookmark("nas", None, None, true), &cfg, &env);
        assert_eq!(
            (b.hostname.as_str(), b.port, b.user.as_str()),
            ("nas.example.com", 2222, "admin")
        );
        let b = destination_binding(&bookmark("nas", Some(22), Some("bob"), true), &cfg, &env);
        assert_eq!(
            (b.hostname.as_str(), b.port, b.user.as_str()),
            ("nas.example.com", 22, "bob")
        );
        let b = destination_binding(&bookmark("nas", None, None, false), &cfg, &env);
        assert_eq!(
            (b.hostname.as_str(), b.port, b.user.as_str()),
            ("nas", 22, "alice")
        );
    }
}
