//! `#[tauri::command]` wrappers (§3): parse args, call a service, map errors — nothing else.
//!
//! [`COMMAND_NAMES`] (shared with `build.rs`) is the single list of app commands. The invoke
//! handler is generated from the same macro invocation as [`HANDLER_COMMANDS`], and the ACL test
//! below checks handler ⇔ names ⇔ generated ACL manifest ⇔ `capabilities/main.json`.

pub mod app;
pub mod hosts;
pub mod session;
pub mod settings;
pub mod sftp;
pub mod terminal;
pub mod transfer;
pub mod update;
pub mod window;

include!("names.rs");

macro_rules! app_commands {
    ($($module:ident :: $name:ident),* $(,)?) => {
        /// Commands registered in the invoke handler (must equal [`COMMAND_NAMES`]).
        pub const HANDLER_COMMANDS: &[&str] = &[$(stringify!($name)),*];

        /// The invoke handler for every app command. A panic during synchronous dispatch
        /// (argument parsing, the synchronous commands `term_write` / `term_resize`) is caught
        /// here and answered with `bug{command}`; async command bodies catch their own panics
        /// with [`guarded`] (Tauri runs them in a task spawned inside the generated code).
        pub fn handler() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
            let inner = typed_handler(tauri::generate_handler![$($module::$name),*]);
            move |invoke: tauri::ipc::Invoke| {
                let resolver = invoke.resolver.clone();
                let command = invoke.message.command().to_owned();
                dispatch_catching(invoke, &inner, |error| {
                    // Answer the invoke once. If the panic happened inside Tauri's responder the
                    // response may already be consumed (or its lock poisoned): then there is
                    // nothing left to answer and the second panic is contained here.
                    let answered = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        resolver.reject(error_for(&command, error));
                    }));
                    if answered.is_err() {
                        tracing::warn!(command = %command, "could not answer a panicked command");
                    }
                })
            }
        }
    };
}

/// Fix the closure type of `generate_handler!` (its argument type is inferred from this bound).
fn typed_handler<F>(f: F) -> F
where
    F: Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static,
{
    f
}

/// Run `inner(invoke)`; on a panic call `on_panic` with the `bug` error and report the invoke as
/// handled. Generic so the unit tests can drive it without a Tauri runtime.
pub(crate) fn dispatch_catching<I>(
    invoke: I,
    inner: impl Fn(I) -> bool,
    on_panic: impl FnOnce(crate::error::AppError),
) -> bool {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| inner(invoke))) {
        Ok(handled) => handled,
        Err(_) => {
            on_panic(caught_panic_error());
            true
        }
    }
}

/// The `bug` error for a panic just caught on this thread (location from the panic hook); the
/// command name is filled in by [`error_for`].
fn caught_panic_error() -> crate::error::AppError {
    crate::error::AppError::panicked("", crate::logging::take_panic_location().as_ref())
}

/// Put the command name into a `bug` error, and log the panic once per command (the panic
/// hook has already logged its location; the payload is never logged or returned).
fn error_for(command: &str, mut error: crate::error::AppError) -> crate::error::AppError {
    let name = crate::logging::sanitize_for_log(command, 64);
    tracing::error!(command = %name, "a command panicked; answered with `bug`");
    error.kind = crate::error::ErrorCode::Bug { command: name };
    error
}

/// Run an async command body and turn a panic into `bug{command}` (§4.3). Every async command
/// wraps its body in this (`guarded_commands_are_complete` checks it). The state stays usable:
/// locks are taken with `unwrap_or_else(PoisonError::into_inner)` throughout.
pub(crate) async fn guarded<T>(
    command: &'static str,
    body: impl std::future::Future<Output = Result<T, crate::error::AppError>>,
) -> Result<T, crate::error::AppError> {
    use futures_util::FutureExt;
    match std::panic::AssertUnwindSafe(body).catch_unwind().await {
        // A panic caught in [`blocking`] comes back as `bug` without a command name.
        Ok(Err(mut e)) => {
            if let crate::error::ErrorCode::Bug { command: name } = &mut e.kind
                && name.is_empty()
            {
                command.clone_into(name);
            }
            Err(e)
        }
        Ok(result) => result,
        Err(_) => Err(error_for(command, caught_panic_error())),
    }
}

app_commands![
    app::app_frontend_ready,
    app::app_quit_ack,
    app::app_quit_cancel,
    app::app_quit,
    app::app_open_logs_dir,
    app::app_reveal_path,
    app::app_open_external,
    app::env_info,
    app::agent_status,
    app::keys_list,
    app::keys_read_public,
    app::dialog_pick_identity_file,
    app::app_error_report,
    hosts::ssh_config_reload,
    hosts::bookmarks_get,
    hosts::bookmark_save,
    hosts::bookmark_delete,
    hosts::bookmark_copy_from_ssh_config,
    hosts::bookmark_set_password,
    hosts::group_save,
    hosts::group_delete,
    hosts::bookmark_move,
    hosts::group_move,
    hosts::recents_get,
    hosts::recents_clear,
    hosts::bookmarks_export,
    hosts::bookmarks_import_open,
    hosts::bookmarks_import_apply,
    hosts::bookmarks_import_cancel,
    settings::settings_get,
    settings::settings_update,
    settings::settings_reset,
    settings::notifications_set_texts,
    session::session_open,
    session::session_answer_prompt,
    session::session_reconnect_now,
    session::session_disconnect,
    session::session_close,
    session::session_probe_all,
    terminal::term_open,
    terminal::term_write,
    terminal::term_resize,
    terminal::term_ack,
    terminal::term_restart,
    terminal::term_close,
    sftp::sftp_realpath,
    sftp::sftp_list,
    sftp::sftp_stat,
    sftp::sftp_read_text,
    sftp::sftp_write_text,
    sftp::sftp_create_file,
    sftp::sftp_mkdir,
    sftp::sftp_rename,
    sftp::sftp_delete,
    transfer::transfer_download,
    transfer::transfer_upload_pick,
    transfer::transfer_upload_dropped,
    transfer::transfer_retry,
    transfer::transfer_cancel,
    window::window_minimize,
    window::window_toggle_maximize,
    window::window_toggle_fullscreen,
    window::window_close,
    window::window_start_dragging,
    app::app_save_text_file,
    window::menu_set_labels,
    update::update_check,
    update::update_install,
    update::update_cancel,
    update::update_skip_version,
];

/// Run blocking work (local file I/O, keychain, config parsing) off the async runtime (§1.3).
/// A panic in `f` is caught on the blocking thread and becomes `bug` ([`guarded`] adds the
/// command name); the panic payload never reaches an error or a log line.
pub(crate) async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, crate::error::AppError> + Send + 'static,
) -> Result<T, crate::error::AppError> {
    use crate::error::{catch_panic, join_failed, tauri_task_panicked};
    tauri::async_runtime::spawn_blocking(move || catch_panic("", f))
        .await
        .map_err(|e| join_failed(tauri_task_panicked(&e), ""))??
}

/// `allow-<command-with-dashes>` (autogenerated app permission name).
pub fn allow_permission(command: &str) -> String {
    format!("allow-{}", command.replace('_', "-"))
}

#[cfg(test)]
mod panic_tests {
    //! Panics in commands become `bug{command}` (§4.3) and leave the state usable.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::{Mutex, PoisonError};

    use super::*;
    use crate::error::{AppError, ErrorCode};

    fn install_hook() {
        crate::logging::install_test_panic_hook();
    }

    /// A panic inside `blocking` (a `spawn_blocking` thread) becomes `bug{<command>}` without
    /// its payload; tokio's `JoinError` text (which carries it) is never used.
    #[tokio::test]
    async fn blocking_panic_becomes_bug_without_payload() {
        install_hook();
        let result: Result<(), AppError> = guarded("keys_list", async {
            blocking(|| panic!("test panic, payload marker PAYLOAD-91c2")).await
        })
        .await;
        let err = result.unwrap_err();
        assert_eq!(
            err.kind,
            ErrorCode::Bug {
                command: "keys_list".into()
            }
        );
        let detail = err.detail.clone().unwrap();
        assert!(detail.starts_with("panicked at "), "{detail}");
        assert!(detail.contains("commands"), "{detail}");
        let json = serde_json::to_string(&err).unwrap();
        assert!(!json.contains("PAYLOAD-91c2"), "{json}");
        // Without `guarded` the name stays empty, never the payload.
        let bare = blocking::<()>(|| panic!("PAYLOAD-91c2")).await.unwrap_err();
        assert_eq!(
            bare.kind,
            ErrorCode::Bug {
                command: String::new()
            }
        );
        assert!(
            !serde_json::to_string(&bare)
                .unwrap()
                .contains("PAYLOAD-91c2")
        );
        // Results pass through.
        assert_eq!(blocking(|| Ok(4)).await.unwrap(), 4);
    }

    #[tokio::test]
    async fn async_panic_becomes_bug_and_state_stays_usable() {
        install_hook();
        let state = Mutex::new(1_u32);
        let result: Result<u32, AppError> = guarded("sftp_list", async {
            let mut guard = state.lock().unwrap();
            *guard += 1;
            // Poisons the lock while holding it.
            if *guard == 2 {
                panic!("test panic, payload marker PAYLOAD-7f3a");
            }
            Ok(*guard)
        })
        .await;
        let err = result.unwrap_err();
        assert_eq!(
            err.kind,
            ErrorCode::Bug {
                command: "sftp_list".into()
            }
        );
        let detail = err.detail.clone().unwrap();
        assert!(detail.starts_with("panicked at "), "{detail}");
        assert!(detail.contains("commands"), "{detail}");
        assert!(
            !detail.contains("PAYLOAD-7f3a"),
            "the payload never leaves: {detail}"
        );
        let json = serde_json::to_string(&err).unwrap();
        assert!(!json.contains("PAYLOAD-7f3a"), "{json}");

        // The next call works: the state is read through the poison, as the app does.
        let next = guarded("sftp_list", async {
            let mut guard = state.lock().unwrap_or_else(PoisonError::into_inner);
            *guard += 1;
            Ok(*guard)
        })
        .await;
        assert_eq!(next.unwrap(), 3);
    }

    #[tokio::test]
    async fn guarded_passes_results_and_errors_through() {
        assert_eq!(guarded("x", async { Ok(5) }).await.unwrap(), 5);
        let err = guarded::<()>("x", async { Err(AppError::invalid_argument("f")) })
            .await
            .unwrap_err();
        assert_eq!(err.kind, ErrorCode::InvalidArgument { field: "f".into() });
    }

    #[test]
    fn sync_dispatch_panic_is_answered_once_with_bug() {
        install_hook();
        let mut answered = Vec::new();
        let handled = dispatch_catching(
            7_u32,
            |n| {
                assert_eq!(n, 7);
                panic!("sync boom")
            },
            |e| answered.push(error_for("term_write", e)),
        );
        assert!(handled);
        assert_eq!(answered.len(), 1);
        assert_eq!(
            answered[0].kind,
            ErrorCode::Bug {
                command: "term_write".into()
            }
        );
        assert!(
            answered[0]
                .detail
                .as_deref()
                .unwrap()
                .starts_with("panicked at ")
        );

        // No panic: the inner result is returned and nothing is answered here.
        let mut called = false;
        assert!(!dispatch_catching(1_u32, |_| false, |_| called = true));
        assert!(!called);
    }

    #[test]
    fn command_names_in_bug_are_bounded_and_escaped() {
        let e = error_for("x\u{1b}[2J\u{202E}y", AppError::bug(""));
        let ErrorCode::Bug { command } = e.kind else {
            panic!("not bug")
        };
        assert!(
            !command.contains('\u{1b}') && !command.contains('\u{202E}'),
            "{command}"
        );
        let e = error_for(&"a".repeat(500), AppError::bug(""));
        let ErrorCode::Bug { command } = e.kind else {
            panic!("not bug")
        };
        assert!(command.chars().count() <= 65);
    }

    /// Every async command wraps its body in `guarded("<its own name>", …)`; the synchronous
    /// ones are covered by the handler's catch. Keeps future commands from losing panics.
    #[test]
    fn guarded_commands_are_complete() {
        const SYNC_COMMANDS: &[&str] = &["term_write", "term_resize"];
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/commands");
        let mut seen = std::collections::BTreeSet::new();
        for entry in std::fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path)
                .unwrap()
                .replace("\r\n", "\n");
            // Split into the literal here would match this very line; build the marker instead.
            let marker = concat!("\n#[tauri::", "command]\n");
            for chunk in text.split(marker).skip(1) {
                let header = chunk.split('(').next().unwrap();
                let name = header.rsplit("fn ").next().unwrap().trim().to_owned();
                if header.contains("async fn") {
                    // The body up to the next command (or helper) is enough to find the call.
                    let call = format!("guarded(\"{name}\",");
                    assert!(
                        chunk.contains(&call)
                            || chunk.contains(&format!("guarded(\n        \"{name}\",")),
                        "{}: async command {name} must wrap its body in {call} …)",
                        path.display()
                    );
                } else {
                    assert!(
                        SYNC_COMMANDS.contains(&name.as_str()),
                        "{name} is synchronous"
                    );
                }
                assert!(seen.insert(name.clone()), "duplicate {name}");
            }
        }
        let names: std::collections::BTreeSet<String> =
            COMMAND_NAMES.iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(seen, names);
    }
}

#[cfg(test)]
mod acl_tests {
    //! §9.1 ACL test: least privilege, and every command is registered, in the manifest and allowed.
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::collections::BTreeSet;
    use std::path::{Path, PathBuf};

    use pretty_assertions::assert_eq;
    use serde_json::Value;

    use super::*;

    // `notification:`: OS notifications are decided and shown by Rust only.
    // `updater:`: checks, downloads and installs run in Rust only (§9.8).
    const FORBIDDEN_PREFIXES: &[&str] = &[
        "opener:",
        "dialog:",
        "fs:",
        "shell:",
        "core:window:",
        "notification:",
        "updater:",
        // The single-instance plugin is driven from Rust only.
        "single-instance:",
    ];
    const CORE_PERMISSIONS: &[&str] = &["core:event:allow-listen", "core:event:allow-unlisten"];

    fn manifest_dir() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    }

    fn read_json(path: &Path) -> Value {
        let text =
            std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
    }

    fn names() -> BTreeSet<&'static str> {
        let set: BTreeSet<_> = COMMAND_NAMES.iter().copied().collect();
        assert_eq!(set.len(), COMMAND_NAMES.len(), "duplicate command name");
        set
    }

    fn expected_permissions() -> BTreeSet<String> {
        CORE_PERMISSIONS
            .iter()
            .map(|p| (*p).to_owned())
            .chain(COMMAND_NAMES.iter().map(|c| allow_permission(c)))
            .collect()
    }

    fn assert_no_forbidden(permissions: &[Value], origin: &str) {
        for p in permissions {
            let id = p
                .as_str()
                .or_else(|| p.get("identifier").and_then(Value::as_str))
                .unwrap_or_default();
            assert!(!id.is_empty(), "{origin}: unexpected permission entry {p}");
            assert!(
                !FORBIDDEN_PREFIXES.iter().any(|f| id.starts_with(f)),
                "{origin}: forbidden permission {id}"
            );
            assert_ne!(
                id, "core:default",
                "{origin}: core:default grants window permissions"
            );
        }
    }

    #[test]
    fn handler_registers_exactly_the_contract_commands() {
        let handler: BTreeSet<_> = HANDLER_COMMANDS.iter().copied().collect();
        assert_eq!(
            handler.len(),
            HANDLER_COMMANDS.len(),
            "duplicate handler command"
        );
        assert_eq!(handler, names());
        assert_eq!(COMMAND_NAMES.len(), 70);
    }

    /// Updater: the WebView reaches the updater only through our four commands.
    #[test]
    fn updater_is_reachable_only_through_app_commands() {
        for c in [
            "update_check",
            "update_install",
            "update_cancel",
            "update_skip_version",
        ] {
            assert!(names().contains(c), "{c}");
        }
        let cap = read_json(&manifest_dir().join("capabilities/main.json"));
        let text = cap.to_string();
        assert!(!text.contains("updater:"), "no updater plugin permission");
        for p in [
            "allow-update-check",
            "allow-update-install",
            "allow-update-cancel",
            "allow-update-skip-version",
        ] {
            assert!(
                cap["permissions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v == p),
                "{p}"
            );
        }
    }

    #[test]
    fn only_the_main_capability_exists() {
        let files: BTreeSet<String> = std::fs::read_dir(manifest_dir().join("capabilities"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(files, BTreeSet::from(["main.json".to_owned()]));
    }

    #[test]
    fn main_capability_is_least_privilege_and_complete() {
        let cap = read_json(&manifest_dir().join("capabilities/main.json"));
        assert_eq!(cap["identifier"], "main");
        assert_eq!(cap["windows"], serde_json::json!(["main"]));
        assert!(
            cap.get("remote").is_none(),
            "remote URLs must not get IPC access"
        );
        let permissions = cap["permissions"].as_array().unwrap();
        assert_no_forbidden(permissions, "capabilities/main.json");
        let actual: BTreeSet<String> = permissions
            .iter()
            .map(|p| p.as_str().unwrap().to_owned())
            .collect();
        assert_eq!(actual.len(), permissions.len(), "duplicate permission");
        assert_eq!(actual, expected_permissions());
    }

    #[test]
    fn tauri_conf_enables_only_main_capability() {
        let conf = read_json(&manifest_dir().join("tauri.conf.json"));
        assert_eq!(
            conf["app"]["security"]["capabilities"],
            serde_json::json!(["main"])
        );
        assert_eq!(conf["app"]["withGlobalTauri"], false);
        assert_eq!(conf["app"]["security"]["freezePrototype"], true);
        let windows = conf["app"]["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0]["label"], "main");
        assert_eq!(
            windows[0]["create"], false,
            "the window is created in Rust with the navigation lock"
        );
    }

    #[test]
    fn generated_acl_matches_command_list() {
        let schemas = manifest_dir().join("gen/schemas");
        let manifests = read_json(&schemas.join("acl-manifests.json"));
        let app_acl = &manifests["__app-acl__"];
        let permissions: BTreeSet<String> = app_acl["permissions"]
            .as_object()
            .expect("__app-acl__.permissions")
            .keys()
            .cloned()
            .collect();
        let expected: BTreeSet<String> = COMMAND_NAMES
            .iter()
            .flat_map(|c| [allow_permission(c), format!("deny-{}", c.replace('_', "-"))])
            .collect();
        assert_eq!(permissions, expected);

        // The merged capability set generated from capabilities/*.json.
        let caps = read_json(&schemas.join("capabilities.json"));
        let caps = caps.as_object().unwrap();
        assert_eq!(
            caps.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["main"]
        );
        assert_no_forbidden(
            caps["main"]["permissions"].as_array().unwrap(),
            "gen/schemas/capabilities.json",
        );
    }
}
