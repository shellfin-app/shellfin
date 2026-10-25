//! Windows 11 Snap Layouts on the custom maximize button (#14, `docs/ARCHITECTURE.md` §8.4, §9.1).
//!
//! The title bar is HTML (frameless window), and wry hosts WebView2 in child windows that cover the whole
//! client area, so the top-level window never sees a `WM_NCHITTEST` over the maximize button and Windows cannot
//! show the snap flyout there. An invisible native child window sits on top of the HTML maximize button and
//! answers `WM_NCHITTEST` with `HTMAXBUTTON` (the technique Tauri itself uses for the resize border,
//! `TAURI_DRAG_RESIZE_BORDERS`). It never paints and never takes focus. A left click on it maximizes or restores
//! the main window natively (`WM_SYSCOMMAND`); hover changes go to the page (`window:max-hover`) so the HTML
//! button can show its hover state. The keyboard path (Tab + Enter/Space on the HTML button) is unchanged.
//!
//! The geometry and the pointer state machine are pure functions (tested on every platform); the Win32 part is
//! Windows-only. If the overlay cannot be created, the app logs it and keeps the plain HTML button.

/// Height of the title bar in CSS px (`--h-titlebar` in `src/styles/tokens.css`).
pub const TITLEBAR_HEIGHT_DIP: i64 = 44;
/// Width of one window-control button in CSS px (`.window-controls__button` in `shell.css`). The close button
/// is flush with the right edge, the maximize button is the one left of it.
pub const CAPTION_BUTTON_WIDTH_DIP: i64 = 46;

/// A rectangle in physical pixels, relative to the parent's client area.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OverlayRect {
    pub x: i32,
    pub y: i32,
    pub width: i32,
    pub height: i32,
}

/// `dip` CSS px at `dpi`, rounded half up.
fn scale_round(dip: i64, dpi: u32) -> i64 {
    (dip * i64::from(dpi) + 48) / 96
}

/// `dip` CSS px at `dpi`, rounded up.
fn scale_ceil(dip: i64, dpi: u32) -> i64 {
    (dip * i64::from(dpi) + 95) / 96
}

/// Where the overlay goes for a client area of `client_width` × `client_height` physical px at `dpi`
/// (1 CSS px = 1 DIP: page zoom is off). `frame_y` is the top resize strip of a restored window
/// (`SM_CYFRAME` at `dpi`, the strip Tauri's resize window keeps); a maximized window has none.
///
/// `None` (overlay hidden): fullscreen (no window controls), a zero-size or minimized window, an unknown DPI, a
/// window too narrow for both buttons, or a strip that leaves no height. The overlay never reaches into the close
/// button: its right edge is rounded towards the left.
pub fn max_button_rect(
    client_width: i32,
    client_height: i32,
    dpi: u32,
    maximized: bool,
    fullscreen: bool,
    frame_y: i32,
) -> Option<OverlayRect> {
    if fullscreen || client_width <= 0 || client_height <= 0 || dpi == 0 {
        return None;
    }
    let width = i64::from(client_width);
    let left = width - scale_round(2 * CAPTION_BUTTON_WIDTH_DIP, dpi);
    let right = width - scale_ceil(CAPTION_BUTTON_WIDTH_DIP, dpi);
    if left < 0 || right <= left {
        return None;
    }
    let top = if maximized {
        0
    } else {
        i64::from(frame_y.max(0))
    };
    let bottom = scale_round(TITLEBAR_HEIGHT_DIP, dpi).min(i64::from(client_height));
    if bottom <= top {
        return None;
    }
    Some(OverlayRect {
        x: i32::try_from(left).ok()?,
        y: i32::try_from(top).ok()?,
        width: i32::try_from(right - left).ok()?,
        height: i32::try_from(bottom - top).ok()?,
    })
}

/// Pointer input on the overlay (non-client mouse messages).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerInput {
    Move,
    Leave,
    LeftDown,
    /// The second press of a double click; counts as a press.
    LeftDoubleClick,
    LeftUp,
}

/// What the overlay does after one input.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct TrackerOutput {
    /// The hover state changed to this value.
    pub hover: Option<bool>,
    /// A full click (press and release on the button): maximize or restore.
    pub click: bool,
}

/// Hover and press state of the overlay. A click needs a press and a release without leaving in between;
/// a release without a press (the press started elsewhere) is not a click.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ButtonTracker {
    hovered: bool,
    pressed: bool,
}

impl ButtonTracker {
    pub fn hovered(&self) -> bool {
        self.hovered
    }

    pub fn handle(&mut self, input: PointerInput) -> TrackerOutput {
        let mut out = TrackerOutput::default();
        match input {
            PointerInput::Move => out.hover = self.set_hovered(true),
            PointerInput::Leave => {
                self.pressed = false;
                out.hover = self.set_hovered(false);
            }
            PointerInput::LeftDown | PointerInput::LeftDoubleClick => {
                self.pressed = true;
                out.hover = self.set_hovered(true);
            }
            PointerInput::LeftUp => {
                out.click = self.pressed;
                self.pressed = false;
                out.hover = self.set_hovered(true);
            }
        }
        out
    }

    /// Back to idle (the overlay moved or was hidden). Returns whether it was hovered.
    pub fn reset(&mut self) -> bool {
        let was = self.hovered;
        *self = Self::default();
        was
    }

    fn set_hovered(&mut self, hovered: bool) -> Option<bool> {
        if self.hovered == hovered {
            None
        } else {
            self.hovered = hovered;
            Some(hovered)
        }
    }
}

#[cfg(windows)]
pub use native::{HoverSink, install, sync};

#[cfg(windows)]
mod native {
    use std::io;
    use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

    use windows_sys::Win32::Foundation::{
        ERROR_CLASS_ALREADY_EXISTS, GetLastError, HWND, LPARAM, LRESULT, RECT, WPARAM,
    };
    use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
        TME_LEAVE, TME_NONCLIENT, TRACKMOUSEEVENT, TrackMouseEvent,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, GetClientRect, GetSystemMetrics, HTMAXBUTTON, HWND_TOP,
        IsZoomed, PostMessageW, RegisterClassExW, SC_MAXIMIZE, SC_RESTORE, SM_CYFRAME,
        SWP_HIDEWINDOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOOWNERZORDER, SWP_NOSIZE, SWP_NOZORDER,
        SWP_SHOWWINDOW, SetWindowPos, WM_CONTEXTMENU, WM_ERASEBKGND, WM_NCDESTROY, WM_NCHITTEST,
        WM_NCLBUTTONDBLCLK, WM_NCLBUTTONDOWN, WM_NCLBUTTONUP, WM_NCMBUTTONDBLCLK, WM_NCMBUTTONDOWN,
        WM_NCMBUTTONUP, WM_NCMOUSELEAVE, WM_NCMOUSEMOVE, WM_NCRBUTTONDBLCLK, WM_NCRBUTTONDOWN,
        WM_NCRBUTTONUP, WM_SYSCOMMAND, WNDCLASSEXW, WS_CHILD, WS_CLIPSIBLINGS,
    };

    use super::{ButtonTracker, OverlayRect, PointerInput, max_button_rect};

    /// Receives hover changes (`true` = the pointer is over the maximize button).
    pub type HoverSink = Arc<dyn Fn(bool) + Send + Sync>;

    const CLASS_NAME: *const u16 = windows_sys::w!("ShellfinMaxButtonOverlay");

    /// The one overlay (the app has one window). Window handles are stored as integers (`HWND` is a raw pointer,
    /// not `Send`); they are only used on the UI thread that owns the windows.
    struct Installed {
        overlay: usize,
        parent: usize,
        tracker: ButtonTracker,
        /// `TrackMouseEvent` is armed (a `WM_NCMOUSELEAVE` will follow).
        tracking: bool,
        rect: Option<OverlayRect>,
        on_hover: HoverSink,
    }

    static OVERLAY: Mutex<Option<Installed>> = Mutex::new(None);

    /// Never held across a Win32 call that can send a message to the overlay (the window procedure locks it too).
    fn state() -> MutexGuard<'static, Option<Installed>> {
        OVERLAY.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Creates the overlay as a child of `parent` (hidden until the first [`sync`]). Main/UI thread only: the
    /// overlay's messages are handled on the thread that creates it.
    // An `HWND` is an opaque handle that the system validates (a stale one fails the call); it is never
    // dereferenced here.
    #[allow(clippy::not_unsafe_ptr_arg_deref)]
    pub fn install(parent: HWND, on_hover: HoverSink) -> io::Result<HWND> {
        if parent.is_null() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "no parent window",
            ));
        }
        if state().is_some() {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "caption overlay already installed",
            ));
        }
        // SAFETY: plain Win32 calls with valid arguments: a null module name returns this executable's module;
        // the class structure lives for the duration of the call and names a `'static` UTF-16 string and a
        // window procedure with the `WNDPROC` signature; `parent` is a live window owned by this thread.
        let overlay = unsafe {
            let instance = GetModuleHandleW(std::ptr::null());
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                lpfnWndProc: Some(overlay_proc),
                hInstance: instance,
                lpszClassName: CLASS_NAME,
                // No cursor, icon or background brush: the window never paints (`WM_ERASEBKGND` is answered).
                ..Default::default()
            };
            if RegisterClassExW(&class) == 0 && GetLastError() != ERROR_CLASS_ALREADY_EXISTS {
                return Err(io::Error::last_os_error());
            }
            CreateWindowExW(
                0,
                CLASS_NAME,
                std::ptr::null(),
                WS_CHILD | WS_CLIPSIBLINGS,
                0,
                0,
                0,
                0,
                parent,
                std::ptr::null_mut(),
                instance,
                std::ptr::null(),
            )
        };
        if overlay.is_null() {
            return Err(io::Error::last_os_error());
        }
        *state() = Some(Installed {
            overlay: overlay as usize,
            parent: parent as usize,
            tracker: ButtonTracker::default(),
            tracking: false,
            rect: None,
            on_hover,
        });
        Ok(overlay)
    }

    /// Moves the overlay over the maximize button of `parent` (or hides it), on top of the WebView. Call it after
    /// every resize, maximize/restore, fullscreen and DPI change. The DPI is read from `parent` itself
    /// (`GetDpiForWindow`): a guessed scale would put the overlay over the close button at 150 %, so a DPI that
    /// cannot be read hides the overlay instead. Does nothing when no overlay is installed on `parent`.
    #[allow(clippy::not_unsafe_ptr_arg_deref)] // opaque handle, see `install`
    pub fn sync(parent: HWND, maximized: bool, fullscreen: bool) {
        // 0 (unknown) makes `max_button_rect` return `None`: hidden.
        place(parent, window_dpi(parent), maximized, fullscreen);
    }

    type GetDpiForWindowFn = unsafe extern "system" fn(HWND) -> u32;

    /// `GetDpiForWindow(hwnd)`, or 0 when it is unavailable (before Windows 10 1607) or fails (0 = invalid
    /// window). Looked up at run time like `GetSystemMetricsForDpi`.
    fn window_dpi(hwnd: HWND) -> u32 {
        static FOR_WINDOW: OnceLock<Option<GetDpiForWindowFn>> = OnceLock::new();
        let for_window = FOR_WINDOW.get_or_init(|| {
            // SAFETY: as in `frame_height`; the transmute gives the function its documented signature
            // `UINT (HWND hwnd)`.
            unsafe {
                let user32 = GetModuleHandleW(windows_sys::w!("user32.dll"));
                if user32.is_null() {
                    return None;
                }
                GetProcAddress(user32, windows_sys::s!("GetDpiForWindow")).map(|f| {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, GetDpiForWindowFn>(
                        f,
                    )
                })
            }
        });
        match for_window {
            // SAFETY: an opaque handle the system validates; a stale one returns 0.
            Some(f) => unsafe { f(hwnd) },
            None => 0,
        }
    }

    /// [`sync`] at a given `dpi` (0 hides the overlay).
    fn place(parent: HWND, dpi: u32, maximized: bool, fullscreen: bool) {
        let overlay = match state().as_ref() {
            Some(s) if s.parent == parent as usize => s.overlay as HWND,
            _ => return,
        };
        let mut client = RECT::default();
        // SAFETY: `parent` is a live window owned by this thread; `client` is a valid out pointer.
        let have_client = unsafe { GetClientRect(parent, &mut client) } != 0;
        let rect = if have_client {
            max_button_rect(
                client.right.saturating_sub(client.left),
                client.bottom.saturating_sub(client.top),
                dpi,
                maximized,
                fullscreen,
                frame_height(dpi),
            )
        } else {
            None
        };
        // SAFETY: `overlay` is the live child window created by `install` (cleared on `WM_NCDESTROY`).
        // `SWP_NOACTIVATE` keeps the focus where it is; `HWND_TOP` keeps the overlay above the WebView's windows.
        let moved = unsafe {
            match rect {
                Some(r) => SetWindowPos(
                    overlay,
                    HWND_TOP,
                    r.x,
                    r.y,
                    r.width,
                    r.height,
                    SWP_NOACTIVATE | SWP_NOOWNERZORDER | SWP_SHOWWINDOW,
                ),
                None => SetWindowPos(
                    overlay,
                    std::ptr::null_mut(),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOACTIVATE
                        | SWP_NOOWNERZORDER
                        | SWP_NOMOVE
                        | SWP_NOSIZE
                        | SWP_NOZORDER
                        | SWP_HIDEWINDOW,
                ),
            }
        } != 0;
        if !moved {
            tracing::debug!(
                error = %io::Error::last_os_error(),
                "caption overlay not repositioned"
            );
        }
        // A moved or hidden overlay is no longer under the pointer as far as the page knows.
        let unhover = {
            let mut guard = state();
            match guard.as_mut() {
                Some(s) if s.overlay == overlay as usize && s.rect != rect => {
                    s.rect = rect;
                    s.tracking = false;
                    s.tracker.reset().then(|| s.on_hover.clone())
                }
                _ => None,
            }
        };
        if let Some(sink) = unhover {
            sink(false);
        }
    }

    type GetSystemMetricsForDpiFn = unsafe extern "system" fn(i32, u32) -> i32;

    /// `SM_CYFRAME` at `dpi`: the top resize strip Tauri keeps on a restored frameless window. Looked up at run
    /// time like Tauri does (Windows 10 1607+); older systems get the system-DPI value.
    fn frame_height(dpi: u32) -> i32 {
        static FOR_DPI: OnceLock<Option<GetSystemMetricsForDpiFn>> = OnceLock::new();
        let for_dpi = FOR_DPI.get_or_init(|| {
            // SAFETY: `user32.dll` is loaded in every GUI process; the names are NUL-terminated literals. The
            // transmute gives the function its documented signature `int (int nIndex, UINT dpi)`.
            unsafe {
                let user32 = GetModuleHandleW(windows_sys::w!("user32.dll"));
                if user32.is_null() {
                    return None;
                }
                GetProcAddress(user32, windows_sys::s!("GetSystemMetricsForDpi")).map(|f| {
                    std::mem::transmute::<
                        unsafe extern "system" fn() -> isize,
                        GetSystemMetricsForDpiFn,
                    >(f)
                })
            }
        });
        // SAFETY: both functions take plain integers and have no preconditions.
        unsafe {
            match for_dpi {
                Some(f) => f(SM_CYFRAME, dpi),
                None => GetSystemMetrics(SM_CYFRAME),
            }
        }
    }

    /// The window procedure. A panic must never unwind out of an `extern "system"` function (the process would
    /// abort), so the work runs inside `catch_unwind`; on a panic the message is swallowed.
    unsafe extern "system" fn overlay_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        let handled = std::panic::catch_unwind(|| dispatch(hwnd, msg));
        match handled {
            Ok(Some(result)) => result,
            // SAFETY: forwarding the unchanged message of a live window to the default procedure.
            Ok(None) => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
            Err(_) => 0,
        }
    }

    /// `Some(result)` for a handled message, `None` for the default procedure. Left-button messages never reach
    /// `DefWindowProcW`: with `HTMAXBUTTON` it would run its own caption-button logic on this child window.
    fn dispatch(hwnd: HWND, msg: u32) -> Option<LRESULT> {
        match msg {
            WM_NCHITTEST => Some(HTMAXBUTTON as LRESULT),
            WM_ERASEBKGND => Some(1),
            // The move also goes to `DefWindowProcW`: with the `HTMAXBUTTON` hit test, that is what tells Windows 11
            // the pointer rests on a maximize button, so it shows (and later hides) the Snap Layouts flyout. It
            // draws nothing here (the window has no caption); only the button-press messages above and below
            // must not reach it.
            WM_NCMOUSEMOVE => {
                pointer(hwnd, PointerInput::Move);
                None
            }
            WM_NCMOUSELEAVE => {
                pointer(hwnd, PointerInput::Leave);
                Some(0)
            }
            WM_NCLBUTTONDOWN => {
                pointer(hwnd, PointerInput::LeftDown);
                Some(0)
            }
            WM_NCLBUTTONDBLCLK => {
                pointer(hwnd, PointerInput::LeftDoubleClick);
                Some(0)
            }
            WM_NCLBUTTONUP => {
                pointer(hwnd, PointerInput::LeftUp);
                Some(0)
            }
            // Right and middle buttons do nothing on the maximize button (no system menu from a child window).
            WM_NCRBUTTONDOWN | WM_NCRBUTTONUP | WM_NCRBUTTONDBLCLK | WM_NCMBUTTONDOWN
            | WM_NCMBUTTONUP | WM_NCMBUTTONDBLCLK | WM_CONTEXTMENU => Some(0),
            WM_NCDESTROY => {
                let mut guard = state();
                if guard.as_ref().is_some_and(|s| s.overlay == hwnd as usize) {
                    *guard = None;
                }
                None
            }
            _ => None,
        }
    }

    fn pointer(hwnd: HWND, input: PointerInput) {
        let (out, sink, parent, arm) = {
            let mut guard = state();
            let Some(s) = guard.as_mut().filter(|s| s.overlay == hwnd as usize) else {
                return;
            };
            let out = s.tracker.handle(input);
            let arm = input != PointerInput::Leave && !s.tracking;
            s.tracking = input != PointerInput::Leave;
            (out, s.on_hover.clone(), s.parent as HWND, arm)
        };
        if arm {
            let mut track = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE | TME_NONCLIENT,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            // SAFETY: `track` is a valid, initialised structure for the live overlay window.
            if unsafe { TrackMouseEvent(&mut track) } == 0 {
                tracing::debug!("caption overlay: TrackMouseEvent failed");
            }
        }
        if let Some(hovered) = out.hover {
            sink(hovered);
        }
        if out.click {
            toggle_maximize(parent);
        }
    }

    /// Maximize or restore like the native caption button (posted: the overlay may move while it runs).
    fn toggle_maximize(parent: HWND) {
        // SAFETY: `parent` is the live top-level window the overlay belongs to; posting a system command
        // carries no pointers.
        let posted = unsafe {
            let command = if IsZoomed(parent) != 0 {
                SC_RESTORE
            } else {
                SC_MAXIMIZE
            };
            PostMessageW(parent, WM_SYSCOMMAND, command as WPARAM, 0)
        };
        if posted == 0 {
            tracing::warn!(
                error = %io::Error::last_os_error(),
                "caption overlay: maximize/restore not posted"
            );
        }
    }

    #[cfg(test)]
    mod tests {
        #![allow(clippy::unwrap_used, clippy::expect_used)]

        use std::sync::atomic::{AtomicU8, Ordering};

        use windows_sys::Win32::Foundation::POINT;
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CWP_SKIPINVISIBLE, ChildWindowFromPointEx, DestroyWindow, GWL_STYLE, GetWindowLongPtrW,
            MSG, PM_REMOVE, PeekMessageW, SendMessageW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
        };

        use super::*;

        /// The overlay is process-global: tests that create windows run one at a time.
        static SERIAL: Mutex<()> = Mutex::new(());

        unsafe extern "system" fn plain_proc(
            hwnd: HWND,
            msg: u32,
            wparam: WPARAM,
            lparam: LPARAM,
        ) -> LRESULT {
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }

        /// A hidden top-level window (the app window) with a full-size visible child (the WebView host).
        fn parent_with_webview_stand_in() -> (HWND, HWND) {
            unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                let class_name = windows_sys::w!("ShellfinTestParent");
                let class = WNDCLASSEXW {
                    cbSize: size_of::<WNDCLASSEXW>() as u32,
                    lpfnWndProc: Some(plain_proc),
                    hInstance: instance,
                    lpszClassName: class_name,
                    ..Default::default()
                };
                RegisterClassExW(&class);
                let parent = CreateWindowExW(
                    0,
                    class_name,
                    std::ptr::null(),
                    WS_OVERLAPPEDWINDOW,
                    0,
                    0,
                    900,
                    600,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                );
                assert!(!parent.is_null());
                let (w, h) = client_size(parent);
                let child = CreateWindowExW(
                    0,
                    windows_sys::w!("STATIC"),
                    std::ptr::null(),
                    WS_CHILD | WS_VISIBLE,
                    0,
                    0,
                    w,
                    h,
                    parent,
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                );
                assert!(!child.is_null());
                (parent, child)
            }
        }

        fn client_size(hwnd: HWND) -> (i32, i32) {
            let mut r = RECT::default();
            assert_ne!(unsafe { GetClientRect(hwnd, &mut r) }, 0);
            (r.right - r.left, r.bottom - r.top)
        }

        fn child_at(parent: HWND, x: i32, y: i32) -> HWND {
            unsafe { ChildWindowFromPointEx(parent, POINT { x, y }, CWP_SKIPINVISIBLE) }
        }

        fn has_visible_style(hwnd: HWND) -> bool {
            let style = unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32;
            style & WS_VISIBLE != 0
        }

        fn take_syscommand(parent: HWND) -> Option<usize> {
            let mut msg = MSG::default();
            let found =
                unsafe { PeekMessageW(&mut msg, parent, WM_SYSCOMMAND, WM_SYSCOMMAND, PM_REMOVE) };
            (found != 0).then_some(msg.wParam)
        }

        fn send(hwnd: HWND, msg: u32) -> LRESULT {
            unsafe { SendMessageW(hwnd, msg, HTMAXBUTTON as WPARAM, 0) }
        }

        // Hover events as a small log: 0 = none, 1 = last was `true`, 2 = last was `false`.
        static HOVER: AtomicU8 = AtomicU8::new(0);

        #[test]
        fn overlay_hit_tests_follows_the_window_and_clicks() {
            let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
            let (parent, webview) = parent_with_webview_stand_in();
            HOVER.store(0, Ordering::SeqCst);
            let sink: HoverSink =
                Arc::new(|h| HOVER.store(if h { 1 } else { 2 }, Ordering::SeqCst));
            let overlay = install(parent, sink.clone()).unwrap();
            assert_eq!(
                install(parent, sink).unwrap_err().kind(),
                io::ErrorKind::AlreadyExists
            );
            // Hidden until the first sync.
            assert!(!has_visible_style(overlay));

            place(parent, 96, false, false);
            let (w, _) = client_size(parent);
            let frame = frame_height(96);
            let centre = (w - 69, frame + (44 - frame) / 2);
            assert!(has_visible_style(overlay));
            assert_eq!(send(overlay, WM_NCHITTEST), HTMAXBUTTON as LRESULT);
            // On top of the full-size WebView stand-in at the button, the WebView everywhere else.
            assert_eq!(child_at(parent, centre.0, centre.1), overlay);
            assert_eq!(child_at(parent, w - 23, centre.1), webview, "close button");
            assert_eq!(
                child_at(parent, w - 93, centre.1),
                webview,
                "left of the button"
            );
            assert_eq!(
                child_at(parent, centre.0, 45),
                webview,
                "below the title bar"
            );
            if frame > 0 {
                assert_eq!(child_at(parent, centre.0, 0), webview, "top resize strip");
            }

            // Hover goes to the sink; press + release posts maximize; press, leave, release does not.
            send(overlay, WM_NCMOUSEMOVE);
            assert_eq!(HOVER.load(Ordering::SeqCst), 1);
            send(overlay, WM_NCLBUTTONDOWN);
            send(overlay, WM_NCLBUTTONUP);
            assert_eq!(take_syscommand(parent), Some(SC_MAXIMIZE as usize));
            send(overlay, WM_NCLBUTTONDOWN);
            send(overlay, WM_NCMOUSELEAVE);
            assert_eq!(HOVER.load(Ordering::SeqCst), 2);
            send(overlay, WM_NCLBUTTONUP);
            assert_eq!(take_syscommand(parent), None);
            // A release whose press started elsewhere is not a click.
            send(overlay, WM_NCLBUTTONUP);
            assert_eq!(take_syscommand(parent), None);
            // Right button: swallowed, no command.
            assert_eq!(send(overlay, WM_NCRBUTTONUP), 0);
            assert_eq!(take_syscommand(parent), None);

            // Follows a resize (the old spot is the WebView again); a move while hovered clears the hover.
            send(overlay, WM_NCMOUSEMOVE);
            assert_eq!(HOVER.load(Ordering::SeqCst), 1);
            unsafe {
                SetWindowPos(
                    parent,
                    std::ptr::null_mut(),
                    0,
                    0,
                    1200,
                    700,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            let (w2, h2) = client_size(parent);
            unsafe {
                // The WebView host follows the window (wry resizes it without changing the z-order).
                SetWindowPos(
                    webview,
                    std::ptr::null_mut(),
                    0,
                    0,
                    w2,
                    h2,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            place(parent, 96, false, false);
            assert_eq!(HOVER.load(Ordering::SeqCst), 2);
            assert!(w2 > w);
            assert_eq!(child_at(parent, w2 - 69, centre.1), overlay);
            assert_eq!(child_at(parent, centre.0, centre.1), webview);

            // Maximized: the overlay reaches the top edge. Higher DPI: larger.
            place(parent, 96, true, false);
            assert_eq!(child_at(parent, w2 - 69, 0), overlay);
            place(parent, 192, false, false);
            assert_eq!(child_at(parent, w2 - 138, 80), overlay);

            // An unknown DPI hides the overlay instead of guessing a scale.
            place(parent, 0, false, false);
            assert!(!has_visible_style(overlay));
            assert_eq!(child_at(parent, w2 - 69, centre.1), webview);

            // `sync` reads the window's own DPI: the overlay lands exactly where the geometry says for it.
            let dpi = window_dpi(parent);
            assert!(dpi >= 96, "GetDpiForWindow: {dpi}");
            sync(parent, false, false);
            let expected = max_button_rect(w2, h2, dpi, false, false, frame_height(dpi)).unwrap();
            assert_eq!(state().as_ref().unwrap().rect, Some(expected));
            assert!(has_visible_style(overlay));
            assert_eq!(window_dpi(std::ptr::null_mut()), 0, "no window, no DPI");

            // Fullscreen: hidden, no hot spot.
            place(parent, 96, false, true);
            assert!(!has_visible_style(overlay));
            assert_eq!(child_at(parent, w2 - 69, centre.1), webview);
            place(parent, 96, false, false);
            assert!(has_visible_style(overlay));

            // Destroying the window clears the state; a new window can get an overlay again.
            unsafe { DestroyWindow(parent) };
            assert!(state().is_none());
            let (parent2, _) = parent_with_webview_stand_in();
            let overlay2 = install(parent2, Arc::new(|_| {})).unwrap();
            // A sync for another window is ignored.
            place(parent, 96, false, false);
            assert!(!has_visible_style(overlay2));
            unsafe { DestroyWindow(parent2) };
            assert!(state().is_none());
        }

        #[test]
        fn install_rejects_a_null_parent() {
            let _serial = SERIAL.lock().unwrap_or_else(PoisonError::into_inner);
            assert_eq!(
                install(std::ptr::null_mut(), Arc::new(|_| {}))
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Option<OverlayRect> {
        Some(OverlayRect {
            x,
            y,
            width,
            height,
        })
    }

    #[test]
    fn rect_at_common_dpis() {
        // (dpi, left offset from the right edge, close-button width rounded up, title-bar height)
        for (dpi, left, close, bar) in [
            (96, 92, 46, 44),
            (120, 115, 58, 55),
            (144, 138, 69, 66),
            (168, 161, 81, 77),
            (192, 184, 92, 88),
        ] {
            assert_eq!(
                max_button_rect(1440, 900, dpi, false, false, 4),
                rect(1440 - left, 4, left - close, bar - 4),
                "restored at {dpi}"
            );
            assert_eq!(
                max_button_rect(1440, 900, dpi, true, false, 4),
                rect(1440 - left, 0, left - close, bar),
                "maximized at {dpi}"
            );
        }
    }

    #[test]
    fn never_reaches_into_the_close_button() {
        for dpi in 1..=480u32 {
            let Some(r) = max_button_rect(2000, 900, dpi, true, false, 0) else {
                continue;
            };
            let close_left = 2000.0 - 46.0 * f64::from(dpi) / 96.0;
            assert!(f64::from(r.x + r.width) <= close_left, "{dpi}");
            assert!(r.x >= 0 && r.width > 0 && r.height > 0, "{dpi}");
        }
    }

    #[test]
    fn hidden_when_there_is_no_button() {
        assert_eq!(max_button_rect(1440, 900, 96, false, true, 4), None);
        assert_eq!(max_button_rect(1440, 900, 96, true, true, 4), None);
        assert_eq!(max_button_rect(0, 900, 96, false, false, 4), None);
        assert_eq!(max_button_rect(1440, 0, 96, false, false, 4), None);
        assert_eq!(max_button_rect(1440, 900, 0, false, false, 4), None);
    }

    #[test]
    fn hostile_sizes() {
        for w in [i32::MIN, -1, 0, 1, 91] {
            assert_eq!(max_button_rect(w, 900, 96, false, false, 4), None, "{w}");
        }
        assert_eq!(
            max_button_rect(92, 900, 96, false, false, 4),
            rect(0, 4, 46, 40)
        );
        assert_eq!(
            max_button_rect(i32::MAX, i32::MAX, 96, false, false, 4),
            rect(i32::MAX - 92, 4, 46, 40)
        );
        // A client area lower than the title bar is clipped; a strip that eats all of it hides the overlay.
        assert_eq!(
            max_button_rect(1440, 20, 96, false, false, 4),
            rect(1348, 4, 46, 16)
        );
        assert_eq!(max_button_rect(1440, 900, 96, false, false, 44), None);
        assert_eq!(max_button_rect(1440, 900, 96, false, false, i32::MAX), None);
        assert_eq!(
            max_button_rect(1440, 900, 96, false, false, i32::MIN),
            rect(1348, 0, 46, 44)
        );
        assert_eq!(max_button_rect(1440, 900, u32::MAX, false, false, 4), None);
    }

    /// Why `sync` reads the DPI from the window and hides the overlay when it cannot: a 96-dpi guess on a
    /// 150 % window reaches into the close button, an unknown DPI (0) gives no rect at all.
    #[test]
    fn a_wrong_dpi_would_cover_the_close_button_so_unknown_hides() {
        // The minimum window (580 CSS px) at 150 %.
        let width = 870;
        let close_left = width - 69;
        let right = |r: OverlayRect| r.x + r.width;
        let actual = max_button_rect(width, 750, 144, false, false, 6).unwrap();
        assert_eq!(
            actual,
            OverlayRect {
                x: width - 138,
                y: 6,
                width: 69,
                height: 60
            }
        );
        assert!(right(actual) <= close_left);
        let guessed = max_button_rect(width, 750, 96, false, false, 6).unwrap();
        assert!(
            right(guessed) > close_left,
            "a 96-dpi guess overlaps the close button"
        );
        assert_eq!(max_button_rect(width, 750, 0, false, false, 6), None);
        assert_eq!(max_button_rect(width, 750, 0, true, false, 0), None);
    }

    #[test]
    fn tracker_click_needs_press_and_release_inside() {
        use PointerInput::*;
        let mut t = ButtonTracker::default();
        assert_eq!(
            t.handle(Move),
            TrackerOutput {
                hover: Some(true),
                click: false
            }
        );
        assert_eq!(t.handle(Move), TrackerOutput::default());
        assert_eq!(t.handle(LeftDown), TrackerOutput::default());
        assert_eq!(
            t.handle(LeftUp),
            TrackerOutput {
                hover: None,
                click: true
            }
        );
        // One press, one click: a second release is not another click.
        assert_eq!(t.handle(LeftUp), TrackerOutput::default());

        // Press, leave, release: no click.
        t.handle(LeftDown);
        assert_eq!(
            t.handle(Leave),
            TrackerOutput {
                hover: Some(false),
                click: false
            }
        );
        assert_eq!(
            t.handle(LeftUp),
            TrackerOutput {
                hover: Some(true),
                click: false
            }
        );

        // Release without a press (the press started elsewhere): no click.
        let mut t = ButtonTracker::default();
        assert!(!t.handle(LeftUp).click);

        // The second press of a double click counts as a press.
        let mut t = ButtonTracker::default();
        t.handle(LeftDown);
        assert!(t.handle(LeftUp).click);
        assert_eq!(
            t.handle(LeftDoubleClick),
            TrackerOutput {
                hover: None,
                click: false
            }
        );
        assert!(t.handle(LeftUp).click);

        // A press without a prior move hovers.
        let mut t = ButtonTracker::default();
        assert_eq!(t.handle(LeftDown).hover, Some(true));
    }

    #[test]
    fn tracker_leave_and_reset_clear_everything() {
        use PointerInput::*;
        let mut t = ButtonTracker::default();
        assert_eq!(t.handle(Leave), TrackerOutput::default());
        t.handle(LeftDown);
        assert!(t.hovered());
        assert!(t.reset());
        assert!(!t.hovered());
        assert!(!t.reset());
        assert!(!t.handle(LeftUp).click, "reset forgets the press");
    }

    /// The overlay geometry mirrors the HTML title bar; this keeps the two in step.
    #[test]
    fn geometry_matches_the_title_bar_css() {
        const TOKENS: &str = include_str!("../../src/styles/tokens.css");
        const SHELL: &str = include_str!("../../src/features/shell/shell.css");
        const TITLE_BAR: &str = include_str!("../../src/features/shell/TitleBar.tsx");

        fn rule<'a>(css: &'a str, selector: &str) -> &'a str {
            let start = css
                .find(&format!("\n{selector} {{"))
                .unwrap_or_else(|| panic!("{selector} not found"));
            let body = &css[start..];
            &body[..body.find('}').unwrap()]
        }

        assert!(
            TOKENS.contains(&format!("--h-titlebar: {TITLEBAR_HEIGHT_DIP}px;")),
            "--h-titlebar"
        );
        let bar = rule(SHELL, ".title-bar");
        assert!(bar.contains("height: var(--h-titlebar);"));
        assert!(!bar.contains("padding-right"));
        assert!(!bar.contains("direction"), "the controls stay on the right");
        let padding = bar
            .lines()
            .find_map(|l| l.trim().strip_prefix("padding:"))
            .unwrap();
        let parts: Vec<&str> = padding
            .trim()
            .trim_end_matches(';')
            .split_whitespace()
            .collect();
        assert_eq!(parts.len(), 4, "padding: top right bottom left");
        assert_eq!(
            parts[1], "0",
            "no right padding: the close button is flush right"
        );

        let controls = rule(SHELL, ".window-controls");
        assert!(controls.contains("align-self: stretch;"), "full bar height");
        assert!(!controls.contains("margin-right") && !controls.contains("padding"));
        let button = rule(SHELL, ".window-controls__button");
        assert!(button.contains(&format!("width: {CAPTION_BUTTON_WIDTH_DIP}px;")));
        assert!(button.contains("height: 100%;"));
        assert!(!button.contains("margin") && !button.contains("padding"));

        // Minimize, maximize, close in this order, and the controls are the bar's last item.
        let min = TITLE_BAR.find("windowMinimize()").unwrap();
        let max = TITLE_BAR.find("windowToggleMaximize()").unwrap();
        let close = TITLE_BAR.find("windowClose()").unwrap();
        assert!(min < max && max < close);
        let last = TITLE_BAR.rfind("<WindowControls />").unwrap();
        let tail = &TITLE_BAR[last + "<WindowControls />".len()..];
        let rest = &tail[..tail.find("</header>").unwrap()];
        assert!(
            rest.chars().all(|c| c == '}' || c.is_whitespace()),
            "nothing after the window controls: {rest:?}"
        );
    }
}
