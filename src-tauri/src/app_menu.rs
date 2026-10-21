//! Native application menu. Installed on **macOS only** (Windows and Linux draw their
//! own title bar; there is no native menu bar there). Every custom item emits the global event
//! `menu:action` with `{ id }`; the frontend maps the id to the same action as the keyboard
//! shortcut or palette command. The list of ids is the contract (ARCHITECTURE §4.2) and is checked by
//! the tests below (every id must be mapped in `src/features/shell/shellEvents.ts`). Labels are
//! English at startup; the frontend sends translated ones with `menu_set_labels`
//! (`docs/ARCHITECTURE.md` §4.2) and the menu is rebuilt. Standard roles (undo, copy, hide, quit,
//! …) use the system's own handling and wording and are not forwarded.
//!
//! Accelerators are shown by AppKit and also performed by it. WKWebView offers a key to the page
//! first, so a frontend keydown handler that calls `preventDefault` wins and the item does not
//! fire; otherwise AppKit fires the item. Either way the action runs once.

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

use tauri::menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Emitter, Runtime};

use crate::error::AppError;
use crate::ipc::dto::{MenuActionPayload, events};

/// A custom menu item.
#[derive(Debug, Clone, Copy)]
pub struct Item {
    pub id: &'static str,
    pub label: &'static str,
    /// Muda accelerator syntax; `CmdOrCtrl` is Cmd on macOS.
    pub accelerator: Option<&'static str>,
}

/// Standard system items (handled by AppKit, not forwarded).
#[derive(Debug, Clone, Copy)]
pub enum Role {
    About,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    Fullscreen,
    Minimize,
    Maximize,
    BringAllToFront,
}

#[derive(Debug, Clone, Copy)]
pub enum Entry {
    Item(Item),
    Role(Role),
    Separator,
}

#[derive(Debug, Clone, Copy)]
pub struct MenuSpec {
    /// Label key for the top-level title (`menu_set_labels`), e.g. `menu.file`.
    pub key: &'static str,
    /// English fallback title.
    pub title: &'static str,
    pub entries: &'static [Entry],
}

const fn item(id: &'static str, label: &'static str, accelerator: Option<&'static str>) -> Entry {
    Entry::Item(Item {
        id,
        label,
        accelerator,
    })
}

use Entry::Separator as Sep;

/// The menu bar, in display order. The first menu is the application menu.
pub const MENUS: &[MenuSpec] = &[
    MenuSpec {
        key: "menu.app",
        title: "Shellfin",
        entries: &[
            Entry::Role(Role::About),
            // Updater: macOS convention puts it in the app menu (Help menu elsewhere).
            item("help.checkForUpdates", "Check for Updates\u{2026}", None),
            Sep,
            item("app.settings", "Settings\u{2026}", Some("CmdOrCtrl+,")),
            Sep,
            Entry::Role(Role::Services),
            Sep,
            Entry::Role(Role::Hide),
            Entry::Role(Role::HideOthers),
            Entry::Role(Role::ShowAll),
            Sep,
            Entry::Role(Role::Quit),
        ],
    },
    MenuSpec {
        key: "menu.file",
        title: "File",
        entries: &[
            item(
                "connection.new",
                "New Connection\u{2026}",
                Some("CmdOrCtrl+N"),
            ),
            item("file.upload", "Upload Files\u{2026}", None),
            Sep,
            item("file.importBookmarks", "Import Bookmarks\u{2026}", None),
            item("file.exportBookmarks", "Export Bookmarks\u{2026}", None),
            Sep,
            item("file.save", "Save", Some("CmdOrCtrl+S")),
            item("file.saveAll", "Save All", Some("CmdOrCtrl+Alt+S")),
            Sep,
            item("file.closeTab", "Close Tab", Some("CmdOrCtrl+W")),
            item(
                "connection.close",
                "Close Connection",
                Some("CmdOrCtrl+Shift+W"),
            ),
        ],
    },
    MenuSpec {
        key: "menu.edit",
        title: "Edit",
        entries: &[
            Entry::Role(Role::Undo),
            Entry::Role(Role::Redo),
            Sep,
            Entry::Role(Role::Cut),
            Entry::Role(Role::Copy),
            Entry::Role(Role::Paste),
            Entry::Role(Role::SelectAll),
            Sep,
            item("edit.find", "Find\u{2026}", Some("CmdOrCtrl+F")),
        ],
    },
    MenuSpec {
        key: "menu.view",
        title: "View",
        entries: &[
            item(
                "view.commandPalette",
                "Command Palette\u{2026}",
                Some("CmdOrCtrl+K"),
            ),
            Sep,
            item("view.toggleSidebar", "Toggle Sidebar", Some("CmdOrCtrl+B")),
            item(
                "view.toggleTerminal",
                "Toggle Terminal Panel",
                Some("Ctrl+`"),
            ),
            item("view.maximizePanel", "Maximize Terminal Panel", None),
            item(
                "view.focusFiles",
                "Focus File Tree",
                Some("CmdOrCtrl+Shift+E"),
            ),
            Sep,
            item("view.zoomIn", "Increase Font Size", Some("CmdOrCtrl+=")),
            item("view.zoomOut", "Decrease Font Size", Some("CmdOrCtrl+-")),
            item("view.zoomReset", "Reset Font Size", Some("CmdOrCtrl+0")),
            Sep,
            Entry::Role(Role::Fullscreen),
        ],
    },
    MenuSpec {
        key: "menu.connection",
        title: "Connection",
        entries: &[
            item("connection.reconnect", "Reconnect", None),
            item("connection.disconnect", "Disconnect", None),
            Sep,
            item("connection.edit", "Edit Bookmark\u{2026}", None),
            item(
                "connection.saveAsBookmark",
                "Save as Bookmark\u{2026}",
                None,
            ),
            item("connection.copyTarget", "Copy user@host", None),
            Sep,
            item("connection.keys", "SSH Keys\u{2026}", None),
        ],
    },
    MenuSpec {
        key: "menu.terminal",
        title: "Terminal",
        entries: &[
            item("terminal.new", "New Terminal", Some("Ctrl+Shift+`")),
            item("terminal.restart", "Restart Terminal", None),
            item("terminal.clear", "Clear Terminal", None),
            Sep,
            item("terminal.saveOutput", "Save Output\u{2026}", None),
        ],
    },
    MenuSpec {
        key: "menu.window",
        title: "Window",
        entries: &[
            Entry::Role(Role::Minimize),
            Entry::Role(Role::Maximize),
            Sep,
            item("window.nextTab", "Next Connection", None),
            item("window.prevTab", "Previous Connection", None),
            Sep,
            Entry::Role(Role::BringAllToFront),
        ],
    },
    MenuSpec {
        key: "menu.help",
        title: "Help",
        entries: &[
            item("help.shortcuts", "Keyboard Shortcuts", None),
            item("help.openLogs", "Open Log Folder", None),
            item("help.website", "Shellfin Website", None),
        ],
    },
];

/// Every custom item.
pub fn items() -> impl Iterator<Item = &'static Item> {
    MENUS
        .iter()
        .flat_map(|m| m.entries.iter())
        .filter_map(|e| match e {
            Entry::Item(i) => Some(i),
            _ => None,
        })
}

/// `true` if `id` is one of our custom items (only those are forwarded to the frontend).
pub fn is_action(id: &str) -> bool {
    items().any(|i| i.id == id)
}

fn role_item<R: Runtime>(app: &AppHandle<R>, role: Role) -> tauri::Result<PredefinedMenuItem<R>> {
    match role {
        Role::About => PredefinedMenuItem::about(app, None, None),
        Role::Services => PredefinedMenuItem::services(app, None),
        Role::Hide => PredefinedMenuItem::hide(app, None),
        Role::HideOthers => PredefinedMenuItem::hide_others(app, None),
        Role::ShowAll => PredefinedMenuItem::show_all(app, None),
        Role::Quit => PredefinedMenuItem::quit(app, None),
        Role::Undo => PredefinedMenuItem::undo(app, None),
        Role::Redo => PredefinedMenuItem::redo(app, None),
        Role::Cut => PredefinedMenuItem::cut(app, None),
        Role::Copy => PredefinedMenuItem::copy(app, None),
        Role::Paste => PredefinedMenuItem::paste(app, None),
        Role::SelectAll => PredefinedMenuItem::select_all(app, None),
        Role::Fullscreen => PredefinedMenuItem::fullscreen(app, None),
        Role::Minimize => PredefinedMenuItem::minimize(app, None),
        Role::Maximize => PredefinedMenuItem::maximize(app, None),
        Role::BringAllToFront => PredefinedMenuItem::bring_all_to_front(app, None),
    }
}

/// Longest accepted label, in characters.
pub const MAX_LABEL_CHARS: usize = 80;
/// Most entries accepted in one `menu_set_labels` call (known keys are far fewer; this only
/// bounds hostile input before the per-key checks).
pub const MAX_LABEL_ENTRIES: usize = 256;

/// Validated translated labels: label key (item id or `menu.*` title key) -> text.
pub type Labels = BTreeMap<&'static str, String>;

/// Current labels, managed by Tauri on macOS so the menu can be rebuilt.
#[derive(Debug, Default)]
pub struct MenuLabels(pub Mutex<Labels>);

/// `true` for a key `menu_set_labels` understands: a custom item id or a top-level title key.
fn known_key(key: &str) -> Option<&'static str> {
    items()
        .map(|i| i.id)
        .chain(MENUS.iter().map(|m| m.key))
        .find(|k| *k == key)
}

/// Validate a `menu_set_labels` map. Unknown keys are **ignored** (a newer frontend may know
/// more ids than this backend); a known key with a blank, overlong (> 80 chars) or
/// control-character label rejects the whole call with `invalidArgument{field:"labels.<key>"}`,
/// as does a map with more than 256 entries (`field:"labels"`). Keys that are absent keep their
/// English fallback.
pub fn validate_labels(raw: HashMap<String, String>) -> Result<Labels, AppError> {
    if raw.len() > MAX_LABEL_ENTRIES {
        return Err(AppError::invalid_argument("labels"));
    }
    let mut out = Labels::new();
    for (key, text) in raw {
        let Some(known) = known_key(&key) else {
            continue;
        };
        let chars = text.chars().count();
        if chars == 0
            || chars > MAX_LABEL_CHARS
            || text.trim().is_empty()
            || text.chars().any(char::is_control)
        {
            return Err(AppError::invalid_argument(format!("labels.{known}")));
        }
        out.insert(known, text);
    }
    Ok(out)
}

/// Build the menu bar from [`MENUS`], using `labels` where present and the English text otherwise.
pub fn build<R: Runtime>(app: &AppHandle<R>, labels: &Labels) -> tauri::Result<Menu<R>> {
    let text = |key: &str, fallback: &'static str| -> String {
        labels
            .get(key)
            .map_or_else(|| fallback.to_owned(), Clone::clone)
    };
    let mut submenus = Vec::with_capacity(MENUS.len());
    for spec in MENUS {
        let mut built: Vec<Box<dyn IsMenuItem<R>>> = Vec::with_capacity(spec.entries.len());
        for entry in spec.entries {
            match entry {
                Entry::Item(i) => built.push(Box::new(MenuItem::with_id(
                    app,
                    i.id,
                    text(i.id, i.label),
                    true,
                    i.accelerator,
                )?)),
                Entry::Role(r) => built.push(Box::new(role_item(app, *r)?)),
                Entry::Separator => built.push(Box::new(PredefinedMenuItem::separator(app)?)),
            }
        }
        let refs: Vec<&dyn IsMenuItem<R>> = built.iter().map(|b| b.as_ref()).collect();
        submenus.push(Submenu::with_items(
            app,
            text(spec.key, spec.title),
            true,
            &refs,
        )?);
    }
    let refs: Vec<&dyn IsMenuItem<R>> = submenus.iter().map(|s| s as &dyn IsMenuItem<R>).collect();
    Menu::with_items(app, &refs)
}

/// Forward a custom item's click to the frontend.
pub fn on_menu_event<R: Runtime>(app: &AppHandle<R>, id: &str) {
    if !is_action(id) {
        return;
    }
    tracing::debug!(id, "menu action");
    if let Err(e) = app.emit(events::MENU_ACTION, MenuActionPayload { id: id.to_owned() }) {
        tracing::warn!(error = %e, "failed to emit menu:action");
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn ids_are_unique_dotted_camel_case() {
        let ids: Vec<&str> = items().map(|i| i.id).collect();
        let set: BTreeSet<&str> = ids.iter().copied().collect();
        assert_eq!(set.len(), ids.len(), "duplicate menu item id");
        for id in ids {
            let (group, name) = id.split_once('.').expect(id);
            assert!(!group.is_empty() && !name.is_empty(), "{id}");
            assert!(
                id.chars().all(|c| c.is_ascii_alphanumeric() || c == '.'),
                "{id}"
            );
        }
        assert!(is_action("file.save"));
        assert!(!is_action("quit"));
        assert!(!is_action(""));
    }

    /// Accelerators use muda's grammar: known modifiers joined by `+`, then one key.
    #[test]
    fn accelerators_are_well_formed_and_unique() {
        const MODS: &[&str] = &["CmdOrCtrl", "Ctrl", "Alt", "Shift"];
        const PUNCT: &[&str] = &[",", "=", "-", "`"];
        let mut seen = BTreeSet::new();
        for i in items() {
            let Some(acc) = i.accelerator else { continue };
            let parts: Vec<&str> = acc.split('+').collect();
            let (key, mods) = parts.split_last().unwrap();
            assert!(mods.iter().all(|m| MODS.contains(m)), "{acc}");
            assert!(!mods.is_empty(), "{acc}: needs a modifier");
            let letter_or_digit = key.len() == 1
                && key
                    .chars()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
            assert!(letter_or_digit || PUNCT.contains(key), "{acc}");
            assert!(seen.insert(acc), "duplicate accelerator {acc}");
        }
    }

    /// Every menu id is mapped by the frontend (`shellEvents.ts`, a tracked file).
    #[test]
    fn every_menu_id_is_mapped_in_the_frontend() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../src/features/shell/shellEvents.ts"),
        )
        .unwrap();
        for i in items() {
            assert!(
                src.contains(&format!("\"{}\"", i.id)),
                "{} is missing from src/features/shell/shellEvents.ts",
                i.id
            );
        }
    }

    #[test]
    fn title_keys_are_unique_and_do_not_clash_with_ids() {
        let keys: BTreeSet<&str> = MENUS.iter().map(|m| m.key).collect();
        assert_eq!(keys.len(), MENUS.len());
        assert!(keys.iter().all(|k| k.starts_with("menu.") && !is_action(k)));
    }

    fn map(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[test]
    fn labels_accept_known_keys_and_ignore_unknown() {
        let ok = validate_labels(map(&[
            ("file.save", "Speichern"),
            ("menu.file", "Datei"),
            ("nope.unknown", "x"),
            ("", "x"),
        ]))
        .unwrap();
        assert_eq!(ok.len(), 2);
        assert_eq!(ok["file.save"], "Speichern");
        assert_eq!(ok["menu.file"], "Datei");
        assert!(validate_labels(HashMap::new()).unwrap().is_empty());
    }

    #[test]
    fn bookmark_import_export_items_are_in_the_file_menu_and_localizable() {
        let file = MENUS.iter().find(|m| m.key == "menu.file").unwrap();
        for id in ["file.importBookmarks", "file.exportBookmarks"] {
            assert!(
                file.entries
                    .iter()
                    .any(|e| matches!(e, Entry::Item(i) if i.id == id)),
                "{id}"
            );
            assert!(is_action(id));
            let ok = validate_labels(map(&[(id, "Importieren")])).unwrap();
            assert_eq!(ok[id], "Importieren");
        }
    }

    #[test]
    fn maximize_panel_item_is_in_the_view_menu_and_localizable() {
        let view = MENUS.iter().find(|m| m.key == "menu.view").unwrap();
        let id = "view.maximizePanel";
        assert!(
            view.entries
                .iter()
                .any(|e| matches!(e, Entry::Item(i) if i.id == id))
        );
        assert!(is_action(id));
        let ok = validate_labels(map(&[(id, "Panel maximieren")])).unwrap();
        assert_eq!(ok[id], "Panel maximieren");
    }

    /// Updater: "Check for Updates…" sits in the app menu (macOS convention), right after
    /// About, has no accelerator and takes a translated label.
    #[test]
    fn check_for_updates_item_is_in_the_app_menu_and_localizable() {
        let app = MENUS.first().unwrap();
        assert_eq!(app.key, "menu.app");
        let id = "help.checkForUpdates";
        let pos = app
            .entries
            .iter()
            .position(|e| matches!(e, Entry::Item(i) if i.id == id))
            .unwrap();
        assert!(matches!(app.entries[pos - 1], Entry::Role(Role::About)));
        assert!(items().any(|i| i.id == id && i.accelerator.is_none()));
        assert!(is_action(id));
        let ok = validate_labels(map(&[(id, "Перевірити оновлення…")])).unwrap();
        assert_eq!(ok[id], "Перевірити оновлення…");
    }

    #[test]
    fn labels_reject_bad_text() {
        let long = "a".repeat(MAX_LABEL_CHARS + 1);
        for bad in [
            "",
            "   ",
            "a\nb",
            "a\u{7}b",
            "\u{202e}x\u{0}",
            long.as_str(),
        ] {
            let err = validate_labels(map(&[("file.save", bad)])).unwrap_err();
            assert!(
                matches!(err.code(), crate::error::ErrorCode::InvalidArgument { field } if field == "labels.file.save"),
                "{bad:?}"
            );
        }
        let exact = "ä".repeat(MAX_LABEL_CHARS);
        assert!(validate_labels(map(&[("file.save", &exact)])).is_ok());
    }

    #[test]
    fn labels_reject_oversized_map() {
        let many: HashMap<String, String> = (0..=MAX_LABEL_ENTRIES)
            .map(|n| (format!("k{n}"), "x".to_owned()))
            .collect();
        assert!(validate_labels(many).is_err());
    }
}
