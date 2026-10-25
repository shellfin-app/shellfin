//! Shellfin backend (architecture: `docs/ARCHITECTURE.md`).

// `AppError { kind: ErrorCode, detail }` is the IPC error contract (§4.3); every command returns
// `Result<T, AppError>`. Its size (~136 bytes) is irrelevant next to an IPC round trip, and
// boxing it would change the documented shape, so the size lint is allowed crate-wide.
#![allow(clippy::result_large_err)]

// `cfg(dev)` is set whenever tauri's `custom-protocol` feature is off, i.e. for any build
// not produced by the Tauri CLI (`tauri build`, also in CI and release). Such a binary loads `devUrl`
// (http://localhost:5173) with full IPC, no CSP, and the dev origin allowed by the navigation lock
// (§9.1). Optimised builds must therefore never be dev builds.
#[cfg(all(dev, not(debug_assertions)))]
compile_error!(
    "release builds must be produced by `tauri build` (custom-protocol); a dev-mode release \
     binary would load devUrl with full IPC and no CSP"
);

pub mod app_menu;
pub mod app_state;
pub mod caption_overlay;
pub mod commands;
pub mod e2e;
pub mod error;
pub mod external_link;
pub mod ipc;
pub mod known_hosts;
pub mod logging;
pub mod notify;
pub mod platform;
pub mod quit;
pub mod report;
pub mod secrets;
pub mod session;
pub mod sftp;
pub mod ssh;
pub mod sshconfig;
pub mod storage;
pub mod terminal;
pub mod updater;
pub mod watcher;
pub mod window;

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, RunEvent, WindowEvent};

use crate::app_state::AppState;
use crate::ipc::dto::events;
use crate::platform::Paths;
use crate::sshconfig::service::{SshConfigService, production_env};

/// Startup (§3): resolve paths → `harden_process()` (private dirs, no core dumps) → logging →
/// managed state → main window with the navigation lock (§9.1).
fn setup(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    // `e2e` test builds only (activated in `run`; never otherwise): every piece of user state
    // lives below the e2e root (src/e2e.rs).
    let paths = match e2e::active() {
        Some(l) => Paths::new(
            l.home.clone(),
            l.app_config_dir.clone(),
            l.app_log_dir.clone(),
        ),
        None => {
            let resolver = app.path();
            Paths::new(
                resolver.home_dir()?,
                resolver.app_config_dir()?,
                resolver.app_log_dir()?,
            )
        }
    };

    let hardening_issues = platform::harden_process(&paths);
    if let Err(err) = logging::init(&paths.app_log_dir) {
        logging::init_stderr_only();
        tracing::warn!(error = %err, "file logging unavailable");
    }
    for issue in hardening_issues {
        tracing::warn!("{issue}");
    }
    if let Some(l) = e2e::active() {
        tracing::warn!(root = %l.root.display(), "E2E MODE: all user state is redirected");
    }
    tracing::info!(version = %app.package_info().version, "starting Shellfin");

    let ssh_config = Arc::new(SshConfigService::new(production_env(paths.home.clone())));
    let recents_emitter = app.clone();
    let recents_changed = Arc::new(move |list: Vec<ipc::dto::RecentEntry>| {
        if let Err(e) = recents_emitter.emit(events::RECENTS_CHANGED, list) {
            tracing::warn!(error = %e, "failed to emit recents:changed");
        }
    });
    // OS notifications are shown from Rust only (no webview permission).
    let notifier_app = app.clone();
    let show_notification: notify::ShowFn = Arc::new(move |title: String, body: String| {
        let app = notifier_app.clone();
        tauri::async_runtime::spawn_blocking(move || {
            use tauri_plugin_notification::NotificationExt;
            let body = notify::body_for_platform(&body);
            if let Err(e) = app.notification().builder().title(title).body(body).show() {
                tracing::warn!(error = %e, "could not show a notification");
            }
        });
    });
    let state = AppState::open(
        paths,
        ssh_config.clone(),
        recents_changed,
        show_notification,
        Arc::new(window::main_window_focused),
    );
    logging::set_level(state.stores.settings().advanced.log_level);
    app.manage(state);

    // §7.5: re-parse on changes; emit `ssh-config:changed` when the snapshot differs.
    let emitter = app.clone();
    if let Err(err) = watcher::start(ssh_config, move |snapshot| {
        if let Err(e) = emitter.emit(events::SSH_CONFIG_CHANGED, snapshot) {
            tracing::warn!(error = %e, "failed to emit ssh-config:changed");
        }
    }) {
        tracing::warn!(error = %err, "ssh config watcher unavailable; manual reload only");
    }

    let main = window::create_main_window(app)?;
    // Seed the cached window state (notifications need the focus before the first event).
    window::emit_state_if_changed(&main.as_ref().window());
    // The native menu bar exists on macOS only (Windows/Linux draw their own title bar).
    // English until the frontend sends translated labels (`menu_set_labels`).
    #[cfg(target_os = "macos")]
    {
        let labels = app_menu::Labels::new();
        app.set_menu(app_menu::build(app, &labels)?)?;
        app.manage(app_menu::MenuLabels(std::sync::Mutex::new(labels)));
    }
    // Updater (§9.8): automatic checks, 15 s after startup then daily; not in dev/e2e.
    updater::spawn_scheduler(app);
    Ok(())
}

/// Whether the single-instance plugin is registered: in release builds only, and never in an
/// `e2e` run (parallel test instances with their own roots must not hand over to each other).
/// Debug builds rely on the storage instance lock alone, so `tauri dev` can run next to an
/// installed Shellfin without being redirected to it.
pub fn single_instance_enabled(debug_build: bool, e2e_active: bool) -> bool {
    !debug_build && !e2e_active
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::install_panic_hook();

    // `e2e` test builds refuse to start without a valid SHELLFIN_E2E_ROOT (fail closed; never
    // the real user state). Without the feature this is a no-op that reads nothing (src/e2e.rs).
    let e2e_ready = e2e::layout_from(|name| std::env::var_os(name)).and_then(|layout| {
        layout
            .map(e2e::activate)
            .transpose()
            .map_err(|e| e.to_string())
    });
    if let Err(err) = e2e_ready {
        eprintln!("Shellfin failed to start: {err}");
        std::process::exit(1);
    }

    let mut builder = tauri::Builder::default();
    // One running instance. Registered first, as the plugin requires: a second
    // launch hands over to the running app (focus, unminimize) and exits before any setup runs.
    // The storage instance lock (`storage::INSTANCE_LOCK_FILE`) stays as defence in depth.
    // Rust-side only: the plugin has no WebView permission. Off in debug and e2e builds
    // (`single_instance_enabled`), so a dev build can run next to the installed app.
    if single_instance_enabled(cfg!(debug_assertions), e2e::active().is_some()) {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            window::focus_main_window(app);
        }));
    }
    let built = builder
        // Rust-side use only: no plugin permissions are granted to the WebView (§9.1).
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        // Signed updates (§9.8); used from Rust only, no `updater:` permission in the capability.
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(commands::handler())
        .setup(|app| setup(app.handle()))
        .on_menu_event(|app, event| app_menu::on_menu_event(app, &event.id().0))
        .on_window_event(|window, event| match event {
            // Windows: keep the Snap Layouts overlay on the maximize button (§8.4), then
            // `window:state` for the custom title bar (deduplicated).
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => {
                window::sync_caption_overlay(window);
                window::emit_state_if_changed(window);
            }
            WindowEvent::Focused(_) => window::emit_state_if_changed(window),
            // Quit flow (§8.9): the close button never closes the window directly.
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                quit::handle_close_request(window.app_handle());
            }
            // OS drag & drop (§6.6): paths stay in Rust (drop registry).
            WindowEvent::DragDrop(drag) => commands::transfer::on_drag_drop(window, drag),
            _ => {}
        })
        .build(tauri::generate_context!());

    let app = match built {
        Ok(app) => app,
        Err(err) => {
            tracing::error!(error = %err, "failed to start");
            eprintln!("Shellfin failed to start: {err}");
            std::process::exit(1);
        }
    };

    app.run(|app, event| {
        // User/OS-initiated exit (Cmd+Q, app menu Quit, shutdown) → quit flow. `AppHandle::exit`
        // (code = Some) is the end of the quit flow and is not prevented.
        if let RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
            quit::handle_close_request(app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_instance_only_in_release_builds_outside_e2e() {
        assert!(single_instance_enabled(false, false));
        assert!(!single_instance_enabled(true, false));
        assert!(!single_instance_enabled(false, true));
        assert!(!single_instance_enabled(true, true));
    }
}
