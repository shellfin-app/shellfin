//! OS notifications: decided and shown by Rust only, never by the webview
//! (no notification permission in the capability).
//!
//! - `general.notifyOnDisconnect`: an established connection was lost unexpectedly.
//! - `transfers.notifyOnComplete`: the last running transfer finished and at least one transfer
//!   completed successfully since the previous notification.
//!
//! Both only while the main window is not focused. Texts are localized by the frontend
//! (`notifications_set_texts`); until then English fallbacks are used (like the macOS menu).
//! Names inserted into the texts are sanitized: no control, bidi or invisible characters, at
//! most [`MAX_NAME_CHARS`] characters.

use std::sync::{Arc, Mutex, PoisonError};

use crate::error::AppError;
use crate::ipc::dto::NotificationTexts;
use crate::ssh::shellquote::is_deceptive_char;
use crate::storage::Stores;

/// Longest name (`{{name}}`) put into a notification.
pub const MAX_NAME_CHARS: usize = 80;
/// Longest text accepted from `notifications_set_texts`.
pub const MAX_TEXT_CHARS: usize = 200;

/// Receives the events that may produce a notification (sessions and transfers call it; the
/// implementation decides).
pub trait Notifier: Send + Sync {
    /// An established connection (tab `label`) was lost unexpectedly.
    fn connection_lost(&self, label: &str);
    /// The transfer queue drained after `count` successful transfers; `name` is the file name
    /// when `count == 1`.
    fn transfers_done(&self, count: u32, name: Option<&str>);
}

/// Shows nothing (tests, and before the app is wired).
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopNotifier;

impl Notifier for NoopNotifier {
    fn connection_lost(&self, _label: &str) {}
    fn transfers_done(&self, _count: u32, _name: Option<&str>) {}
}

/// English fallbacks until the frontend sends its texts.
pub fn default_texts() -> NotificationTexts {
    NotificationTexts {
        connection_lost_title: "Connection lost".to_owned(),
        connection_lost_body: "The connection to {{name}} was lost.".to_owned(),
        transfers_done_title: "Transfers finished".to_owned(),
        transfer_done_body: "Finished: {{name}}".to_owned(),
        transfers_done_body: "Transfers finished: {{count}}".to_owned(),
    }
}

/// `notifications_set_texts` rules: each text 1–200 chars, not blank, no control characters;
/// `invalidArgument{field:"texts.<key>"}` for the first bad one.
pub fn validate_texts(t: &NotificationTexts) -> Result<(), AppError> {
    for (value, key) in [
        (&t.connection_lost_title, "connectionLostTitle"),
        (&t.connection_lost_body, "connectionLostBody"),
        (&t.transfers_done_title, "transfersDoneTitle"),
        (&t.transfer_done_body, "transferDoneBody"),
        (&t.transfers_done_body, "transfersDoneBody"),
    ] {
        let ok = (1..=MAX_TEXT_CHARS).contains(&value.chars().count())
            && !value.trim().is_empty()
            && !value.chars().any(char::is_control);
        if !ok {
            return Err(AppError::invalid_argument(format!("texts.{key}")));
        }
    }
    Ok(())
}

/// A name for a notification: control, bidi and invisible characters removed, trimmed, at most
/// [`MAX_NAME_CHARS`] characters (`…` marks a cut).
pub fn sanitize_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control() && !is_deceptive_char(*c))
        .collect();
    let clean = clean.trim();
    if clean.chars().count() <= MAX_NAME_CHARS {
        clean.to_owned()
    } else {
        let mut cut: String = clean.chars().take(MAX_NAME_CHARS - 1).collect();
        cut.push('…');
        cut
    }
}

/// Replace `token` (e.g. `{{name}}`) in `template` once per occurrence, in one pass (a value that
/// itself contains a token is not expanded again).
pub fn render(template: &str, token: &str, value: &str) -> String {
    template.replace(token, value)
}

/// Escapes `&`, `<` and `>` for the freedesktop notification body, which servers such as KDE
/// and dunst interpret as markup (a hostile file name must not become a link or bold text).
pub fn escape_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            _ => out.push(c),
        }
    }
    out
}

/// The body as handed to the OS: markup-escaped on Linux (freedesktop body markup), verbatim
/// elsewhere (Windows and macOS show plain text).
pub fn body_for_platform(body: &str) -> String {
    if cfg!(target_os = "linux") {
        escape_markup(body)
    } else {
        body.to_owned()
    }
}

/// Show only when enabled and the main window is known to be unfocused.
pub fn should_show(enabled: bool, focused: Option<bool>) -> bool {
    enabled && focused == Some(false)
}

/// Shows one notification (`title`, `body`); the app uses `tauri-plugin-notification`.
pub type ShowFn = Arc<dyn Fn(String, String) + Send + Sync>;
/// Whether the main window is focused (`None` = not known yet).
pub type FocusFn = Arc<dyn Fn() -> Option<bool> + Send + Sync>;

/// The app's [`Notifier`]: settings + focus decide, localized texts render.
pub struct NotificationHub {
    stores: Arc<Stores>,
    texts: Mutex<NotificationTexts>,
    show: ShowFn,
    focused: FocusFn,
}

impl std::fmt::Debug for NotificationHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NotificationHub").finish_non_exhaustive()
    }
}

impl NotificationHub {
    pub fn new(stores: Arc<Stores>, show: ShowFn, focused: FocusFn) -> Self {
        Self {
            stores,
            texts: Mutex::new(default_texts()),
            show,
            focused,
        }
    }

    /// `notifications_set_texts`: validate, then replace the whole set.
    pub fn set_texts(&self, texts: NotificationTexts) -> Result<(), AppError> {
        validate_texts(&texts)?;
        *self.texts.lock().unwrap_or_else(PoisonError::into_inner) = texts;
        Ok(())
    }

    fn texts(&self) -> NotificationTexts {
        self.texts
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

impl Notifier for NotificationHub {
    fn connection_lost(&self, label: &str) {
        let enabled = self.stores.settings().general.notify_on_disconnect;
        if !should_show(enabled, (self.focused)()) {
            return;
        }
        let t = self.texts();
        let body = render(&t.connection_lost_body, "{{name}}", &sanitize_name(label));
        (self.show)(t.connection_lost_title, body);
    }

    fn transfers_done(&self, count: u32, name: Option<&str>) {
        if count == 0 {
            return;
        }
        let enabled = self.stores.settings().transfers.notify_on_complete;
        if !should_show(enabled, (self.focused)()) {
            return;
        }
        let t = self.texts();
        let body = match (count, name) {
            (1, Some(n)) => render(&t.transfer_done_body, "{{name}}", &sanitize_name(n)),
            _ => render(&t.transfers_done_body, "{{count}}", &count.to_string()),
        };
        (self.show)(t.transfers_done_title, body);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    type Shown = Arc<Mutex<Vec<(String, String)>>>;

    fn hub(focused: Option<bool>) -> (tempfile::TempDir, Arc<Stores>, NotificationHub, Shown) {
        let dir = tempfile::tempdir().unwrap();
        let (stores, _) = Stores::open(dir.path());
        let stores = Arc::new(stores);
        let shown: Shown = Arc::default();
        let s2 = shown.clone();
        let hub = NotificationHub::new(
            stores.clone(),
            Arc::new(move |t, b| s2.lock().unwrap().push((t, b))),
            Arc::new(move || focused),
        );
        (dir, stores, hub, shown)
    }

    fn enable(stores: &Stores) {
        let mut s = stores.settings();
        s.general.notify_on_disconnect = true;
        s.transfers.notify_on_complete = true;
        stores.update_settings(s).unwrap();
    }

    #[test]
    fn decisions_follow_settings_and_focus() {
        assert!(should_show(true, Some(false)));
        assert!(!should_show(true, Some(true)));
        assert!(!should_show(true, None), "unknown focus never notifies");
        assert!(!should_show(false, Some(false)));

        // Off by default.
        let (_d, stores, h, shown) = hub(Some(false));
        h.connection_lost("web");
        h.transfers_done(2, None);
        assert!(shown.lock().unwrap().is_empty());
        // Enabled + unfocused: shown with the English fallbacks.
        enable(&stores);
        h.connection_lost("web");
        h.transfers_done(1, Some("report.pdf"));
        h.transfers_done(3, Some("ignored.txt"));
        h.transfers_done(0, None);
        assert_eq!(
            *shown.lock().unwrap(),
            vec![
                (
                    "Connection lost".to_owned(),
                    "The connection to web was lost.".to_owned()
                ),
                (
                    "Transfers finished".to_owned(),
                    "Finished: report.pdf".to_owned()
                ),
                (
                    "Transfers finished".to_owned(),
                    "Transfers finished: 3".to_owned()
                ),
            ]
        );
        // Focused: nothing.
        let (_d, stores, h, shown) = hub(Some(true));
        enable(&stores);
        h.connection_lost("web");
        h.transfers_done(1, Some("x"));
        assert!(shown.lock().unwrap().is_empty());
    }

    #[test]
    fn texts_are_validated_and_localized() {
        let (_d, stores, h, shown) = hub(Some(false));
        enable(&stores);
        let uk = NotificationTexts {
            connection_lost_title: "З'єднання втрачено".into(),
            connection_lost_body: "З'єднання з {{name}} втрачено.".into(),
            transfers_done_title: "Передавання завершено".into(),
            transfer_done_body: "Завершено: {{name}}".into(),
            transfers_done_body: "Завершено передавань: {{count}}".into(),
        };
        h.set_texts(uk.clone()).unwrap();
        h.connection_lost("nas");
        assert_eq!(
            shown.lock().unwrap()[0].1,
            "З'єднання з nas втрачено.".to_owned()
        );
        for (set, field) in [
            (
                (|t: &mut NotificationTexts| t.connection_lost_title = String::new())
                    as fn(&mut NotificationTexts),
                "texts.connectionLostTitle",
            ),
            (
                |t| t.connection_lost_body = "  ".into(),
                "texts.connectionLostBody",
            ),
            (
                |t| t.transfers_done_title = "a\nb".into(),
                "texts.transfersDoneTitle",
            ),
            (
                |t| t.transfer_done_body = "x".repeat(201),
                "texts.transferDoneBody",
            ),
            (
                |t| t.transfers_done_body = "\u{1b}[31m".into(),
                "texts.transfersDoneBody",
            ),
        ] {
            let mut bad = uk.clone();
            set(&mut bad);
            assert_eq!(
                h.set_texts(bad).unwrap_err().kind,
                crate::error::ErrorCode::InvalidArgument {
                    field: field.into()
                }
            );
        }
        // A rejected set changes nothing.
        assert_eq!(h.texts(), uk);
        validate_texts(&default_texts()).unwrap();
    }

    /// Server/user-controlled names cannot inject controls, bidi tricks or huge text.
    #[test]
    fn markup_in_names_is_escaped_for_freedesktop() {
        let name = sanitize_name("<a href=http:evil.example>x</a> & <b>y</b>");
        let body = render("Transfer finished: {{name}}", "{{name}}", &name);
        assert_eq!(
            escape_markup(&body),
            "Transfer finished: &lt;a href=http:evil.example&gt;x&lt;/a&gt; &amp; &lt;b&gt;y&lt;/b&gt;"
        );
        assert_eq!(
            escape_markup("&amp;"),
            "&amp;amp;",
            "already escaped text is escaped again"
        );
        let shown = body_for_platform(&body);
        if cfg!(target_os = "linux") {
            assert!(!shown.contains('<'));
        } else {
            assert_eq!(shown, body);
        }
    }

    #[test]
    fn hostile_names_are_sanitized() {
        assert_eq!(sanitize_name("report.pdf"), "report.pdf");
        assert_eq!(sanitize_name("a\nb\r\tc\u{1b}[31m"), "abc[31m");
        assert_eq!(sanitize_name("evil\u{202e}fdp.exe"), "evilfdp.exe");
        assert_eq!(sanitize_name("zero\u{200b}width\u{feff}"), "zerowidth");
        assert_eq!(sanitize_name("  \u{85} "), "");
        let long = sanitize_name(&"й".repeat(500));
        assert_eq!(long.chars().count(), MAX_NAME_CHARS);
        assert!(long.ends_with('…'));
        // A name holding a token is not expanded again.
        let (_d, stores, h, shown) = hub(Some(false));
        enable(&stores);
        h.transfers_done(1, Some("{{count}}"));
        assert_eq!(shown.lock().unwrap()[0].1, "Finished: {{count}}");
    }
}
