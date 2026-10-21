//! Live edits of a connected bookmark.
//!
//! A session opened from a bookmark keeps a [`LiveBookmark`]: what the UI shows (label, color) and
//! the bandwidth overrides can follow later `bookmark_save` calls at once. Everything that shapes
//! the connection itself (host, port, user, auth, `useSshConfig`, `proxyJump`) is frozen in the
//! session's `ResolvedTarget`; the edit only sets `needs_reconnect` by comparing the saved bookmark
//! with the values the connection was made with.

use crate::ipc::dto::{AuthConfig, Bookmark, BookmarkColor, JumpRef};
use crate::sftp::throttle::LimitOverrides;
use crate::ssh::target::ResolvedTarget;

/// The bookmark fields that need a new connection to take effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectKey {
    host: String,
    port: Option<u16>,
    user: Option<String>,
    use_ssh_config: bool,
    proxy_jump: Option<JumpRef>,
    /// Method plus identity file; `passwordSaved` is a storage flag, not part of the identity.
    auth: String,
}

impl ConnectKey {
    pub fn of(b: &Bookmark) -> Self {
        Self {
            host: b.host.clone(),
            port: b.port,
            user: b.user.clone(),
            use_ssh_config: b.use_ssh_config,
            proxy_jump: b.proxy_jump.clone(),
            auth: match &b.auth {
                AuthConfig::Auto => "auto".to_owned(),
                AuthConfig::Key { identity_file } => format!("key:{identity_file}"),
                AuthConfig::Password { .. } => "password".to_owned(),
                AuthConfig::Agent => "agent".to_owned(),
            },
        }
    }
}

/// Result of applying a saved bookmark.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub label: String,
    pub color: Option<BookmarkColor>,
    pub overrides: LimitOverrides,
    pub needs_reconnect: bool,
}

#[derive(Debug, Clone)]
pub struct LiveBookmark {
    pub label: String,
    pub color: Option<BookmarkColor>,
    pub overrides: LimitOverrides,
    baseline: Option<ConnectKey>,
}

impl LiveBookmark {
    pub fn new(target: &ResolvedTarget) -> Self {
        Self {
            label: target.label.clone(),
            color: target.bookmark.as_ref().and_then(|b| b.color),
            overrides: target.limits,
            baseline: target.bookmark.as_ref().map(ConnectKey::of),
        }
    }

    /// Take over the saved bookmark's live-editable fields.
    pub fn apply(&mut self, b: &Bookmark) -> Applied {
        self.label.clone_from(&b.name);
        self.color = b.color;
        self.overrides = LimitOverrides {
            upload_kibps: b.upload_limit_kibps,
            download_kibps: b.download_limit_kibps,
        };
        let needs_reconnect = self
            .baseline
            .as_ref()
            .is_some_and(|base| *base != ConnectKey::of(b));
        Applied {
            label: self.label.clone(),
            color: self.color,
            overrides: self.overrides,
            needs_reconnect,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn bm() -> Bookmark {
        Bookmark {
            id: "b".into(),
            name: "nas".into(),
            host: "nas.local".into(),
            port: Some(22),
            user: Some("alice".into()),
            auth: AuthConfig::Password {
                password_saved: false,
            },
            proxy_jump: None,
            group_id: None,
            tags: vec![],
            start_dir: None,
            use_ssh_config: false,
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

    fn live(b: &Bookmark) -> LiveBookmark {
        LiveBookmark {
            label: b.name.clone(),
            color: b.color,
            overrides: LimitOverrides::default(),
            baseline: Some(ConnectKey::of(b)),
        }
    }

    #[test]
    fn display_and_limit_edits_do_not_need_a_reconnect() {
        let base = bm();
        let mut l = live(&base);
        let mut edited = base.clone();
        edited.name = "NAS".into();
        edited.color = Some(BookmarkColor::Teal);
        edited.upload_limit_kibps = Some(64);
        edited.download_limit_kibps = Some(0);
        edited.tags = vec!["x".into()];
        edited.start_dir = Some("/srv".into());
        edited.group_id = Some("g".into());
        edited.auth = AuthConfig::Password {
            password_saved: true,
        };
        let a = l.apply(&edited);
        assert_eq!(a.label, "NAS");
        assert_eq!(a.color, Some(BookmarkColor::Teal));
        assert_eq!(
            a.overrides,
            LimitOverrides {
                upload_kibps: Some(64),
                download_kibps: Some(0)
            }
        );
        assert!(!a.needs_reconnect);
    }

    #[test]
    fn connection_fields_set_the_flag_and_undoing_clears_it() {
        let base = bm();
        let mut l = live(&base);
        let changes: [fn(&mut Bookmark); 6] = [
            |b| b.host = "other".into(),
            |b| b.port = Some(2222),
            |b| b.user = None,
            |b| b.use_ssh_config = true,
            |b| b.proxy_jump = Some(JumpRef::Bookmark { id: "j".into() }),
            |b| {
                b.auth = AuthConfig::Key {
                    identity_file: "~/.ssh/id".into(),
                }
            },
        ];
        for change in changes {
            let mut e = base.clone();
            change(&mut e);
            assert!(l.apply(&e).needs_reconnect);
            assert!(!l.apply(&base).needs_reconnect);
        }
    }
}
