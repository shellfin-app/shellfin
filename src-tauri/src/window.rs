//! Main window creation with the navigation lock (§9.1). The window is declared in
//! `tauri.conf.json` with `"create": false` and built here so the navigation handlers can be
//! attached: navigation is allowed only to the app origin (plus `devUrl` under `cfg(dev)`),
//! and every new-window request is denied. External links go through `app_open_external`.

use std::sync::{Mutex, PoisonError};

use tauri::webview::NewWindowResponse;
use tauri::{AppHandle, Emitter, Manager, Runtime, Url, WebviewWindow, WebviewWindowBuilder};

use crate::ipc::dto::{WindowState, events};

pub const MAIN_WINDOW_LABEL: &str = "main";

/// `tauri://localhost` (macOS/Linux) and `http(s)://tauri.localhost` (Windows), or the dev
/// server origin when `dev_url` is given (only under `cfg(dev)`).
pub fn is_app_origin(url: &Url, dev_url: Option<&Url>) -> bool {
    let app_origin = matches!(
        (url.scheme(), url.host_str(), url.port()),
        ("tauri", Some("localhost"), None) | ("http" | "https", Some("tauri.localhost"), None)
    );
    app_origin || dev_url.is_some_and(|dev| url.origin() == dev.origin())
}

pub fn create_main_window<R: Runtime>(
    app: &AppHandle<R>,
) -> Result<WebviewWindow<R>, tauri::Error> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN_WINDOW_LABEL)
        .cloned()
        .ok_or(tauri::Error::WindowNotFound)?;
    let dev_url = if cfg!(dev) {
        app.config().build.dev_url.clone()
    } else {
        None
    };

    let mut builder = WebviewWindowBuilder::from_config(app, &config)?
        // (all platforms): no native context menu, no drop navigation (see GUARD_JS).
        .initialization_script(GUARD_JS)
        // Ctrl+=/−/0, Ctrl+wheel and pinch zoom stay off (WebView2 IsZoomControlEnabled /
        // IsPinchZoomEnabled; Tauri's default, made explicit).
        .zoom_hotkeys_enabled(false);
    builder = apply_chrome(builder);
    // `e2e` test builds: a WebView profile below the e2e root (src/e2e.rs).
    if let Some(l) = crate::e2e::active() {
        builder = builder.data_directory(l.webview_data_dir.clone());
    }
    let permission_dev_url = dev_url.clone();
    let window = builder
        .on_navigation(move |url| {
            let allowed = is_app_origin(url, dev_url.as_ref());
            if !allowed {
                tracing::warn!(
                    scheme = url.scheme(),
                    host = url.host_str().unwrap_or(""),
                    "blocked navigation"
                );
            }
            allowed
        })
        .on_new_window(|url, _features| {
            tracing::warn!(
                scheme = url.scheme(),
                host = url.host_str().unwrap_or(""),
                "blocked new window"
            );
            NewWindowResponse::Deny
        })
        // The webview never writes local files (hard rule 3), so WebView downloads
        // (`<a download href="blob:…">`, which WebView2/WebKit allow by default) are refused.
        .on_download(|_webview, event| download_allowed(&event))
        .build()?;
    harden_webview(&window, permission_dev_url);
    #[cfg(debug_assertions)]
    if std::env::var_os("SHELLFIN_DEVTOOLS").is_some_and(|v| v == "1") {
        // Debug builds only: browser accelerator keys (F12, Ctrl+Shift+I) are off, so devtools
        // are opened on request instead. Release builds have no devtools at all.
        window.open_devtools();
    }
    Ok(window)
}

/// WebView download policy: every download is refused and logged (scheme only).
fn download_allowed(event: &tauri::webview::DownloadEvent<'_>) -> bool {
    if let tauri::webview::DownloadEvent::Requested { url, .. } = event {
        tracing::warn!(scheme = url.scheme(), "blocked webview download");
    }
    false
}

/// Window chrome. Windows/Linux: no native frame; the frontend draws the title bar
/// (drag via `window_start_dragging`, buttons via `window_*` commands). The native drop shadow
/// and the resize borders are kept (tauri-runtime-wry's undecorated-resizing handles the hit
/// test in Rust on Windows and Linux; no `core:window` permission is involved). macOS: native
/// traffic lights over the content (`Overlay` + hidden title) at a fixed position.
fn apply_chrome<'a, R: Runtime, M: tauri::Manager<R>>(
    builder: WebviewWindowBuilder<'a, R, M>,
) -> WebviewWindowBuilder<'a, R, M> {
    #[cfg(any(windows, target_os = "linux"))]
    let builder = builder.decorations(false).shadow(true);
    #[cfg(target_os = "macos")]
    let builder = builder
        .title_bar_style(tauri::TitleBarStyle::Overlay)
        .hidden_title(true)
        .traffic_light_position(tauri::LogicalPosition::new(
            TRAFFIC_LIGHT_X,
            TRAFFIC_LIGHT_Y,
        ));
    builder
}

/// macOS traffic-light origin (logical px) for the 44 px title bar; the frontend leaves
/// [`MAC_TRAFFIC_LIGHT_INSET`] free on the left.
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_X: f64 = 16.0;
#[cfg(target_os = "macos")]
const TRAFFIC_LIGHT_Y: f64 = 15.0;

/// Width (logical px) the frontend keeps free at the left of the title bar on macOS.
pub const MAC_TRAFFIC_LIGHT_INSET: u32 = 78;

/// Current state of `window` for `AppBootstrap.window` and `window:state`.
pub fn state_of<R: Runtime>(window: &tauri::Window<R>) -> WindowState {
    WindowState {
        maximized: window.is_maximized().unwrap_or(false),
        fullscreen: window.is_fullscreen().unwrap_or(false),
        focused: window.is_focused().unwrap_or(false),
    }
}

/// A second launch (single-instance plugin): bring the running main window to
/// the front: show it, restore it from the taskbar/dock and focus it. Each step is best effort
/// (logged); no UI is opened and nothing from the second launch's command line is used.
pub fn focus_main_window<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW_LABEL) else {
        tracing::info!("second launch before the main window exists; ignored");
        return;
    };
    tracing::info!("second launch: focusing the running window");
    if let Err(e) = window.show() {
        tracing::debug!(error = %e, "show failed");
    }
    if window.is_minimized().unwrap_or(false)
        && let Err(e) = window.unminimize()
    {
        tracing::debug!(error = %e, "unminimize failed");
    }
    if let Err(e) = window.set_focus() {
        tracing::debug!(error = %e, "set_focus failed");
    }
}

static LAST_STATE: Mutex<Option<WindowState>> = Mutex::new(None);

/// The main window's focus as last reported by a window event
/// (`None` before the first one). Never calls into the window, so it is safe on any thread.
pub fn main_window_focused() -> Option<bool> {
    LAST_STATE
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .map(|s| s.focused)
}

/// Emit `window:state` when maximize/fullscreen/focus changed since the last emit (resize
/// events fire continuously while dragging an edge; those do not change the state).
pub fn emit_state_if_changed<R: Runtime>(window: &tauri::Window<R>) {
    if window.label() != MAIN_WINDOW_LABEL {
        return;
    }
    let now = state_of(window);
    {
        let mut last = LAST_STATE.lock().unwrap_or_else(PoisonError::into_inner);
        if *last == Some(now) {
            return;
        }
        *last = Some(now);
    }
    if let Err(e) = window.app_handle().emit(events::WINDOW_STATE, now) {
        tracing::warn!(error = %e, "failed to emit window:state");
    }
}

/// Injected into the main frame before any page script (not subject to the page CSP):
/// - `contextmenu` (capture): `preventDefault` suppresses the native menu on every platform
///   (WebView2, WKWebView, WebKitGTK); the event still propagates, so the frontend's own
///   `contextmenu` handlers run and can show app menus;
/// - `dragover`/`drop` (capture): an unhandled drop never navigates (a dragged link or file);
///   `dropEffect` defaults to `none`, so a frontend drop target must set its own `dropEffect` and
///   call `preventDefault` in its (bubble-phase) handler. OS file drops are intercepted
///   natively by Tauri's drag-drop handler before they reach the page.
pub const GUARD_JS: &str = r#"(() => {
  const capture = { capture: true };
  window.addEventListener("contextmenu", (e) => e.preventDefault(), capture);
  window.addEventListener("dragover", (e) => {
    e.preventDefault();
    if (e.dataTransfer) e.dataTransfer.dropEffect = "none";
  }, capture);
  window.addEventListener("drop", (e) => e.preventDefault(), capture);
})();"#;

/// Web permission policy (all platforms that ask the app): only `clipboard-read`, only for the
/// app's own origin (plus `devUrl` in dev builds), only from a user gesture (the app menu's
/// Paste). Everything else (camera, microphone, geolocation, notifications, …) is denied.
pub fn clipboard_read_allowed(uri: &str, user_initiated: bool, dev_url: Option<&Url>) -> bool {
    user_initiated && Url::parse(uri).is_ok_and(|u| is_app_origin(&u, dev_url))
}

/// (Windows): WebView2 settings Tauri does not expose. Default context menus off and
/// browser accelerator keys off (F5/Ctrl+R reload, Ctrl+P print, Ctrl+U source, Ctrl+S save,
/// Ctrl+F find, Alt+←/→, F12/Ctrl+Shift+I devtools, zoom keys). DOM keyboard events are still
/// delivered to the page, and editing keys (Ctrl+C/V/X/A/Z) keep working. Both settings take
/// effect immediately. Plus the permission policy ([`clipboard_read_allowed`]): WebView2 would
/// otherwise prompt for `clipboard-read`. Other platforms: the init script covers the context
/// menu; WKWebView and WebKitGTK have no built-in reload/print/zoom shortcuts, and their
/// clipboard reads keep WebKit's own one-click Paste confirmation (accepted for v1, `docs/ARCHITECTURE.md` §9.1).
fn harden_webview<R: Runtime>(window: &WebviewWindow<R>, dev_url: Option<Url>) {
    #[cfg(windows)]
    {
        let result = window.with_webview(move |webview| {
            // SAFETY: COM calls on the WebView2 controller, on the thread that owns it (Tauri runs
            // this closure on the main/UI thread).
            let applied = unsafe { apply_webview2_settings(&webview.controller(), dev_url) };
            match applied {
                Ok((menus, accelerators)) => tracing::info!(
                    default_context_menus = menus,
                    browser_accelerator_keys = accelerators,
                    "webview2 hardened"
                ),
                Err(e) => tracing::warn!(error = %e, "could not apply webview2 settings"),
            }
        });
        if let Err(e) = result {
            tracing::warn!(error = %e, "webview2 settings not scheduled");
        }
    }
    #[cfg(not(windows))]
    let _ = (window, dev_url);
}

/// Returns the settings read back after applying them (`(context menus, accelerator keys)`).
#[cfg(windows)]
unsafe fn apply_webview2_settings(
    controller: &webview2_com::Microsoft::Web::WebView2::Win32::ICoreWebView2Controller,
    dev_url: Option<Url>,
) -> windows_core::Result<(bool, bool)> {
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_PERMISSION_KIND, COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ,
        COREWEBVIEW2_PERMISSION_STATE_ALLOW, COREWEBVIEW2_PERMISSION_STATE_DENY,
        ICoreWebView2PermissionRequestedEventArgs3, ICoreWebView2Settings3,
    };
    use webview2_com::PermissionRequestedEventHandler;
    use windows_core::{BOOL, Interface, PWSTR};

    // SAFETY: plain COM getters/setters on live interfaces owned by the controller; the event
    // handler runs on the UI thread with the args WebView2 passes in.
    unsafe {
        let core = controller.CoreWebView2()?;
        let handler = PermissionRequestedEventHandler::create(Box::new(move |_, args| {
            let Some(args) = args else { return Ok(()) };
            let mut kind = COREWEBVIEW2_PERMISSION_KIND::default();
            args.PermissionKind(&mut kind)?;
            let mut uri = PWSTR::null();
            args.Uri(&mut uri)?;
            let uri = webview2_com::take_pwstr(uri);
            let mut user = BOOL::default();
            args.IsUserInitiated(&mut user)?;
            let allow = kind == COREWEBVIEW2_PERMISSION_KIND_CLIPBOARD_READ
                && clipboard_read_allowed(&uri, user.as_bool(), dev_url.as_ref());
            // Never remember a decision in the WebView profile.
            if let Ok(args3) = args.cast::<ICoreWebView2PermissionRequestedEventArgs3>() {
                let _ = args3.SetSavesInProfile(false);
            }
            args.SetState(if allow {
                COREWEBVIEW2_PERMISSION_STATE_ALLOW
            } else {
                COREWEBVIEW2_PERMISSION_STATE_DENY
            })?;
            tracing::debug!(
                kind = kind.0,
                user_initiated = user.as_bool(),
                allow,
                "webview permission request"
            );
            Ok(())
        }));
        let mut token = 0i64;
        core.add_PermissionRequested(&handler, &mut token)?;

        let settings = core.Settings()?;
        settings.SetAreDefaultContextMenusEnabled(false)?;
        settings.SetIsZoomControlEnabled(false)?;
        let settings3: ICoreWebView2Settings3 = settings.cast()?;
        settings3.SetAreBrowserAcceleratorKeysEnabled(false)?;
        let mut menus = BOOL::default();
        settings.AreDefaultContextMenusEnabled(&mut menus)?;
        let mut accelerators = BOOL::default();
        settings3.AreBrowserAcceleratorKeysEnabled(&mut accelerators)?;
        Ok((menus.as_bool(), accelerators.as_bool()))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Clipboard read: app origin + user gesture only.
    #[test]
    fn clipboard_read_policy() {
        assert!(clipboard_read_allowed(
            "http://tauri.localhost/",
            true,
            None
        ));
        assert!(clipboard_read_allowed(
            "https://tauri.localhost/x",
            true,
            None
        ));
        assert!(clipboard_read_allowed("tauri://localhost/", true, None));
        assert!(!clipboard_read_allowed(
            "http://tauri.localhost/",
            false,
            None
        ));
        for uri in [
            "https://example.com/",
            "http://tauri.localhost.example.com/",
            "http://tauri.localhost:8080/",
            "http://localhost:5173/",
            "file:///C:/x",
            "",
            "not a url",
        ] {
            assert!(!clipboard_read_allowed(uri, true, None), "{uri}");
        }
        let dev = u("http://localhost:5173");
        assert!(clipboard_read_allowed(
            "http://localhost:5173/",
            true,
            Some(&dev)
        ));
    }

    /// The guard cancels the native menu and unhandled drops in the capture phase
    /// without stopping propagation (the frontend's own handlers must still run).
    #[test]
    fn guard_script_shape() {
        assert!(GUARD_JS.is_ascii());
        for ev in ["contextmenu", "dragover", "drop"] {
            assert!(
                GUARD_JS.contains(&format!("addEventListener(\"{ev}\"")),
                "{ev}"
            );
        }
        assert!(GUARD_JS.contains("capture: true"));
        assert!(!GUARD_JS.contains("stopPropagation"));
        assert!(!GUARD_JS.contains("stopImmediatePropagation"));
    }

    fn u(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn app_origins_are_allowed() {
        for ok in [
            "tauri://localhost",
            "tauri://localhost/index.html",
            "http://tauri.localhost/",
            "https://tauri.localhost/a?b#c",
        ] {
            assert!(is_app_origin(&u(ok), None), "{ok}");
        }
    }

    #[test]
    fn foreign_origins_are_blocked() {
        for bad in [
            "https://example.com/",
            "http://localhost:1420/",
            "http://tauri.localhost.evil.com/",
            "http://tauri.localhost:8080/",
            "tauri://evil",
            "file:///C:/Windows/win.ini",
            "javascript:alert(1)",
            "data:text/html,hi",
            "about:blank",
            "https://user@tauri.localhost.example/",
        ] {
            assert!(!is_app_origin(&u(bad), None), "{bad}");
        }
    }

    #[test]
    fn dev_url_only_when_given() {
        let dev = u("http://localhost:1420");
        assert!(is_app_origin(
            &u("http://localhost:1420/src/main.tsx"),
            Some(&dev)
        ));
        assert!(!is_app_origin(&u("http://localhost:1421/"), Some(&dev)));
        assert!(!is_app_origin(&u("https://localhost:1420/"), Some(&dev)));
        assert!(!is_app_origin(&u("http://localhost:1420/"), None));
    }

    /// Every webview download is refused, from the app origin too (blob: URLs).
    #[test]
    fn webview_downloads_are_refused() {
        use tauri::webview::DownloadEvent;
        for url in [
            "blob:http://tauri.localhost/0b7e-4c1a",
            "http://tauri.localhost/x.bin",
            "https://example.com/payload.exe",
            "data:application/octet-stream;base64,AAAA",
        ] {
            let mut destination = std::path::PathBuf::from("/tmp/x");
            assert!(!download_allowed(&DownloadEvent::Requested {
                url: u(url),
                destination: &mut destination,
            }));
        }
        assert!(!download_allowed(&DownloadEvent::Finished {
            url: u("http://tauri.localhost/x"),
            path: None,
            success: false,
        }));
    }
}
