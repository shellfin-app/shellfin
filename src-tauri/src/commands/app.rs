//! App commands (§4.2 "App"). Thin wrappers: parse args, call a service, map errors.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use tauri::{AppHandle, Manager, State};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_opener::OpenerExt;

use crate::app_state::AppState;
use crate::commands::{blocking, guarded};
use crate::error::{AppError, IoOp, catch_panic, join_failed, tauri_task_panicked};
use crate::external_link;
use crate::ipc::dto::{
    AgentStatus, AppBootstrap, EnvInfo, EnvKnownHostsInfo, EnvSshConfigInfo, ErrorReport,
    ErrorReportInput, LocalKeyInfo, Platform, RevealKind, SaveTextOutcome, StartupAction,
    WindowState,
};
use crate::known_hosts;
use crate::platform::file_trust::FileKind;
use crate::platform::local_path::read_local;
use crate::quit::{self, QuitMachine};
use crate::ssh::{agent, keys};
use crate::window::MAIN_WINDOW_LABEL;
use crate::{logging, report};

#[tauri::command]
pub async fn app_frontend_ready(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<AppBootstrap, AppError> {
    guarded("app_frontend_ready", async move {
        let app_version = app.package_info().version.to_string();
        // A reloaded page (or a restarted renderer) starts with no sessions; the old page's
        // sessions, terminals (up to 12 MiB backlog each) and reconnect loops would run on with no
        // one listening. Close them (recents recorded, transfers cancelled, agent keys removed) in
        // the background; the new page gets `recents:changed` when that is done.
        if state.frontend_loads.record() {
            // The ids are taken now, before the new page can open (or restore) sessions.
            let orphaned = state.sessions.session_ids();
            if !orphaned.is_empty() {
                tracing::info!(
                    count = orphaned.len(),
                    "page reloaded: closing the sessions of the old page"
                );
                let sessions = state.sessions.clone();
                tauri::async_runtime::spawn(
                    async move { sessions.close_orphaned(&orphaned).await },
                );
            }
        }
        let stores = state.stores.clone();
        let service = state.ssh_config.clone();
        let warnings = state.startup_warnings.clone();
        let window = app
            .get_webview_window(MAIN_WINDOW_LABEL)
            .map(|w| crate::window::state_of(&w.as_ref().window()))
            .unwrap_or(WindowState {
                maximized: false,
                fullscreen: false,
                focused: true,
            });
        blocking(move || {
            let settings = stores.settings();
            let bookmarks = stores.lock_bookmarks().dto();
            let restore_bookmark_ids = restore_ids(
                settings.general.on_startup,
                stores.lock_recents().open_at_quit(),
                &bookmarks,
            );
            Ok(AppBootstrap {
                app_version,
                platform: Platform::current(),
                settings,
                ssh_config: service.snapshot(),
                bookmarks,
                recents: stores.lock_recents().list(),
                warnings,
                window,
                restore_bookmark_ids,
            })
        })
        .await
    })
    .await
}

/// With `restore`, the bookmarks open at the last quit that
/// still exist, in order; otherwise none.
pub fn restore_ids(
    on_startup: StartupAction,
    open_at_quit: &[String],
    bookmarks: &crate::ipc::dto::BookmarksDto,
) -> Vec<String> {
    if on_startup != StartupAction::Restore {
        return Vec::new();
    }
    open_at_quit
        .iter()
        .filter(|id| bookmarks.bookmarks.iter().any(|b| &b.id == *id))
        .cloned()
        .collect()
}

/// The frontend received `app:quit-requested` and handles it.
#[tauri::command]
pub async fn app_quit_ack(state: State<'_, AppState>) -> Result<(), AppError> {
    guarded("app_quit_ack", async move {
        state.quit.with(QuitMachine::on_ack);
        Ok(())
    })
    .await
}

/// The user cancelled quitting; the quit state is reset. An update staged by "Update and
/// restart" is dropped.
#[tauri::command]
pub async fn app_quit_cancel(state: State<'_, AppState>) -> Result<(), AppError> {
    guarded("app_quit_cancel", async move {
        state.quit.with(QuitMachine::on_cancel);
        state.updates.discard_staged();
        Ok(())
    })
    .await
}

/// `lastDirs`: sessionId → tree root. The process exits.
#[tauri::command]
pub async fn app_quit(
    app: AppHandle,
    state: State<'_, AppState>,
    last_dirs: HashMap<String, Option<String>>,
) -> Result<(), AppError> {
    guarded("app_quit", async move {
        if state.quit.with(QuitMachine::on_quit) {
            quit::perform_quit(&app, last_dirs).await;
        }
        Ok(())
    })
    .await
}

async fn reveal(app: AppHandle, path: PathBuf) -> Result<(), AppError> {
    let shown = path.display().to_string();
    tauri::async_runtime::spawn_blocking(move || {
        catch_panic("", || app.opener().reveal_item_in_dir(&path))
    })
    .await
    .map_err(|e| join_failed(tauri_task_panicked(&e), ""))??
    .map_err(|err| {
        AppError::new(crate::error::ErrorCode::Io {
            op: IoOp::RevealPath,
            path: Some(shown),
        })
        .with_detail(err.to_string())
    })
}

#[tauri::command]
pub async fn app_open_logs_dir(app: AppHandle, state: State<'_, AppState>) -> Result<(), AppError> {
    guarded("app_open_logs_dir", async move {
        reveal(app, state.paths.app_log_dir.clone()).await
    })
    .await
}

#[tauri::command]
pub async fn app_reveal_path(
    app: AppHandle,
    state: State<'_, AppState>,
    kind: RevealKind,
) -> Result<(), AppError> {
    guarded("app_reveal_path", async move {
        let path = match kind {
            RevealKind::SshConfig => state.paths.user_ssh_config.clone(),
            RevealKind::KnownHosts => state.paths.user_known_hosts.clone(),
        };
        reveal(app, path).await
    })
    .await
}

/// Native confirm + open of an http(s) URL (§9.4). Returns `false` if the user declined or
/// another confirm is already open.
#[tauri::command]
pub async fn app_open_external(
    app: AppHandle,
    url: String,
    confirm_title: String,
    confirm_body: String,
) -> Result<bool, AppError> {
    guarded("app_open_external", async move {
        let url = external_link::normalize_external_url(&url)?;
        let Some(_slot) = external_link::ConfirmSlot::try_acquire() else {
            return Ok(false);
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut dialog = app
            .dialog()
            .message(external_link::confirm_message(&url, &confirm_body))
            .title(external_link::sanitize_dialog_text(&confirm_title))
            .kind(MessageDialogKind::Warning)
            .buttons(MessageDialogButtons::OkCancel);
        if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            dialog = dialog.parent(&window);
        }
        dialog.show(move |ok| {
            let _ = tx.send(ok);
        });
        if !rx.await.unwrap_or(false) {
            return Ok(false);
        }
        tracing::info!(
            scheme = url.scheme(),
            host = url.host_str().unwrap_or(""),
            "opening external link"
        );
        let target = url.to_string();
        tauri::async_runtime::spawn_blocking(move || {
            catch_panic("", || app.opener().open_url(target, None::<&str>))
        })
        .await
        .map_err(|e| join_failed(tauri_task_panicked(&e), ""))??
        .map_err(|err| {
            AppError::internal("could not open the link").with_detail(err.to_string())
        })?;
        Ok(true)
    })
    .await
}

#[tauri::command]
pub async fn env_info(state: State<'_, AppState>) -> Result<EnvInfo, AppError> {
    guarded("env_info", async move {
        let service = state.ssh_config.clone();
        let known_hosts_path = state.paths.user_known_hosts.clone();
        let ssh_config_path = state.paths.user_ssh_config.clone();
        let log_dir = state.paths.app_log_dir.display().to_string();
        let agent = agent::status(&state.ssh_config.env().env).await;
        blocking(move || {
            let parsed = service.current();
            let snapshot = service.snapshot();
            // Untrusted/missing/oversized known_hosts ⇒ 0 entries (the session ignores it as well).
            let entry_count = read_local(&known_hosts_path, FileKind::UserKnownHosts)
                .map(|bytes| {
                    let display = known_hosts_path.display().to_string();
                    let (file, _) = known_hosts::parse(
                        &known_hosts_path,
                        &display,
                        known_hosts::Origin::User,
                        &bytes,
                    );
                    u32::try_from(file.entries.len()).unwrap_or(u32::MAX)
                })
                .unwrap_or(0);
            Ok(EnvInfo {
                ssh_config: EnvSshConfigInfo {
                    path: snapshot.main_file,
                    exists: ssh_config_path.is_file(),
                    host_count: parsed.host_count(),
                    include_count: parsed.include_count(),
                    loaded_at_ms: snapshot.loaded_at_ms,
                },
                known_hosts: EnvKnownHostsInfo {
                    path: known_hosts_path.display().to_string(),
                    entry_count,
                },
                agent,
                log_dir,
            })
        })
        .await
    })
    .await
}

/// Owner-verified default agent (§5.5): identities, or a warning such as `agentUntrusted`.
#[tauri::command]
pub async fn agent_status(state: State<'_, AppState>) -> Result<AgentStatus, AppError> {
    guarded("agent_status", async move {
        Ok(agent::status(&state.ssh_config.env().env).await)
    })
    .await
}

/// Private keys in `~/.ssh` (never decrypted), marked when the agent holds them.
#[tauri::command]
pub async fn keys_list(state: State<'_, AppState>) -> Result<Vec<LocalKeyInfo>, AppError> {
    guarded("keys_list", async move {
        let status = agent::status(&state.ssh_config.env().env).await;
        let in_agent: HashSet<String> = status
            .identities
            .into_iter()
            .map(|k| k.fingerprint)
            .collect();
        let env = state.ssh_config.env().clone();
        blocking(move || Ok(keys::local_keys(&env.user_ssh_dir, &env.home, &in_agent))).await
    })
    .await
}

/// Public key text (one OpenSSH line) of a key shown by `keys_list`; `path` must be a listed
/// path (for "Copy public key"). Never decrypts.
#[tauri::command]
pub async fn keys_read_public(
    state: State<'_, AppState>,
    path: String,
) -> Result<String, AppError> {
    guarded("keys_read_public", async move {
        let env = state.ssh_config.env().clone();
        blocking(move || keys::read_public(&env.user_ssh_dir, &env.home, &path)).await
    })
    .await
}

/// Native file picker for an identity file (title and filter name come from the frontend, §8.2.2).
/// The returned path is stored in a bookmark and validated as a `Config`-source path whenever it
/// is used (§9.4). `null` when cancelled.
#[tauri::command]
pub async fn dialog_pick_identity_file(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
    filter_name: String,
) -> Result<Option<String>, AppError> {
    guarded("dialog_pick_identity_file", async move {
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut picker = app
            .dialog()
            .file()
            .set_title(external_link::sanitize_dialog_text(&title));
        // Key files usually have no extension. Windows "*.*" matches them; GTK/macOS patterns would
        // hide extension-less files, so the filter is only added on Windows.
        if cfg!(windows) {
            picker = picker.add_filter(external_link::sanitize_dialog_text(&filter_name), &["*"]);
        }
        if state.paths.user_ssh_dir.is_dir() {
            picker = picker.set_directory(&state.paths.user_ssh_dir);
        }
        if let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) {
            picker = picker.set_parent(&window);
        }
        picker.pick_file(move |picked| {
            let _ = tx.send(picked);
        });
        let Some(picked) = rx.await.ok().flatten() else {
            return Ok(None);
        };
        let path = picked
            .into_path()
            .map_err(|e| AppError::invalid_argument("path").with_detail(e.to_string()))?;
        path.to_str().map(|s| Some(s.to_owned())).ok_or_else(|| {
            AppError::invalid_argument("path").with_detail("path is not valid UTF-8")
        })
    })
    .await
}

/// The error details dialog's report (§9.6): redacted Markdown plus the pre-filled GitHub bug
/// form URL. Reads the app's own state and, with `includeLog`, the tail of the current log file;
/// sends nothing anywhere.
#[tauri::command]
pub async fn app_error_report(
    app: AppHandle,
    state: State<'_, AppState>,
    input: ErrorReportInput,
) -> Result<ErrorReport, AppError> {
    guarded("app_error_report", async move {
        let app_version = app.package_info().version.to_string();
        let sessions: Vec<report::OpenSession> = state
            .sessions
            .ids()
            .iter()
            .filter_map(|id| state.sessions.get(id))
            .map(|s| {
                // The live connection's resolved hops and directories; skipped while the
                // supervisor holds the lock (a connect or reconnect in progress).
                let (hops, dirs) = s
                    .connection
                    .try_read()
                    .ok()
                    .and_then(|c| {
                        c.as_ref().map(|c| {
                            (
                                c.hops.iter().map(|h| h.info.hop.clone()).collect(),
                                c.home_dir.iter().chain(&c.start_dir).cloned().collect(),
                            )
                        })
                    })
                    .unwrap_or_default();
                report::OpenSession {
                    label: s.label.clone(),
                    target: s.recent.clone(),
                    hops,
                    dirs,
                }
            })
            .collect();
        let stores = state.stores.clone();
        let ssh_config = state.ssh_config.clone();
        let log_dir = state.paths.app_log_dir.clone();
        let home = state.paths.home.display().to_string();
        blocking(move || {
            let known = report::collect_known(
                &stores.lock_bookmarks().dto(),
                &ssh_config.snapshot().hosts,
                &stores.lock_recents().list(),
                &sessions,
                &stores.settings().connection.default_user,
                &report::LocalIdentity {
                    user: crate::platform::local_user_name(),
                    computer: crate::platform::os_info::computer_name(),
                    home,
                },
            );
            let log = input.include_log.then(|| {
                logging::current_log_file(&log_dir)
                    .and_then(|path| {
                        logging::read_log_tail(
                            &path,
                            logging::LOG_TAIL_MAX_LINES,
                            logging::LOG_TAIL_MAX_BYTES,
                        )
                        .map_err(|e| tracing::debug!(error = %e, "log tail unavailable"))
                        .ok()
                    })
                    .unwrap_or_default()
            });
            let env = report::current_environment(&app_version, input.ui_language.as_deref());
            let built = report::build(
                &report::ReportInput {
                    code: input.code,
                    params: input.params,
                    detail: input.detail,
                    message: input.message,
                    context: input.context,
                    stack: input.stack,
                    occurred_at_ms: input.occurred_at_ms,
                    log,
                },
                &env,
                &known,
            );
            tracing::info!(
                log_lines = built.log_lines,
                url_chars = built.issue_url.len(),
                "error report prepared"
            );
            Ok(built)
        })
        .await
    })
    .await
}

/// Largest text `app_save_text_file` accepts (UTF-8 bytes).
pub const MAX_SAVE_TEXT_BYTES: usize = 64 * 1024 * 1024;
/// Used when the suggested name sanitizes to nothing.
const SAVE_TEXT_FALLBACK_NAME: &str = "output.txt";

/// The name offered in the Save dialog: sanitized like a download name (no separators, no
/// reserved device names, bounded length).
pub fn save_text_name(suggested: &str) -> String {
    let name = crate::sftp::names::sanitize_local_name(suggested);
    if name == crate::sftp::names::FALLBACK {
        SAVE_TEXT_FALLBACK_NAME.to_owned()
    } else {
        name
    }
}

/// Save text (e.g. terminal output) through the native Save dialog. The default
/// folder is Downloads and the suggested name is sanitized; the destination is chosen by the
/// user in the OS dialog and never comes from the WebView. The text is written as UTF-8
/// atomically (temp file + rename). More than 64 MiB → `invalidArgument{contents}`.
#[tauri::command]
pub async fn app_save_text_file(
    app: AppHandle,
    suggested_name: String,
    contents: String,
    dialog_title: String,
) -> Result<SaveTextOutcome, AppError> {
    guarded("app_save_text_file", async move {
        if contents.len() > MAX_SAVE_TEXT_BYTES {
            return Err(AppError::invalid_argument("contents"));
        }
        let (tx, rx) = tokio::sync::oneshot::channel();
        let mut dialog = app
            .dialog()
            .file()
            .set_title(external_link::sanitize_dialog_text(&dialog_title))
            .set_file_name(save_text_name(&suggested_name));
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
            return Ok(SaveTextOutcome {
                saved: false,
                file_name: None,
            });
        };
        let path = picked
            .into_path()
            .map_err(|e| AppError::invalid_argument("path").with_detail(e.to_string()))?;
        let target = path.clone();
        blocking(move || crate::storage::file::write_chosen_file(&target, contents.as_bytes()))
            .await?;
        let file_name = path.file_name().map(|n| n.to_string_lossy().into_owned());
        tracing::info!(
            bytes = path.metadata().map_or(0, |m| m.len()),
            "saved text file"
        );
        Ok(SaveTextOutcome {
            saved: true,
            file_name,
        })
    })
    .await
}

#[cfg(test)]
mod save_text_tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn suggested_names_are_sanitized() {
        assert_eq!(save_text_name("web-01 terminal.txt"), "web-01 terminal.txt");
        assert_eq!(save_text_name("../../etc/passwd"), "passwd");
        assert_eq!(save_text_name(r"C:\x\a:b.log"), "a_b.log");
        assert_eq!(save_text_name(""), SAVE_TEXT_FALLBACK_NAME);
        assert_eq!(save_text_name(".."), SAVE_TEXT_FALLBACK_NAME);
        assert_eq!(save_text_name("CON.txt"), "CON_.txt");
        assert_eq!(save_text_name("a\u{202E}b.txt"), "ab.txt");
    }

    #[test]
    fn size_cap_is_64_mib() {
        assert_eq!(MAX_SAVE_TEXT_BYTES, 67_108_864);
    }

    /// Restore only with `onStartup = restore`, only existing bookmarks, in order.
    #[test]
    fn restore_ids_follow_the_setting_and_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let (mut store, _) = crate::storage::bookmarks::BookmarkStore::load(dir.path());
        let a = store.copy_from_ssh_config("a", 1).unwrap();
        let b = store.copy_from_ssh_config("b", 1).unwrap();
        let dto = store.dto();
        let open = vec![b.id.clone(), "gone".to_owned(), a.id.clone()];
        assert!(restore_ids(StartupAction::Welcome, &open, &dto).is_empty());
        assert_eq!(
            restore_ids(StartupAction::Restore, &open, &dto),
            vec![b.id, a.id]
        );
        assert!(restore_ids(StartupAction::Restore, &[], &dto).is_empty());
    }
}
