//! Process-wide state managed by Tauri (§3). Transfers live in the session manager (they are
//! cancelled with their session).

use std::sync::Arc;

use crate::error::AppError;
use crate::ipc::dto::RecentEntry;
use crate::known_hosts::TrustOnceApprovals;
use crate::notify::{FocusFn, NotificationHub, ShowFn};
use crate::platform::Paths;
use crate::quit::QuitState;
use crate::session::manager::SessionManager;
use crate::session::session::{ConnectCtxTemplate, SessionDeps};
use crate::sftp::drops::DropRegistry;
use crate::sshconfig::service::SshConfigService;
use crate::storage::Stores;
use crate::storage::bookmarks::PendingImports;
use crate::updater::UpdateState;

pub struct AppState {
    pub paths: Paths,
    pub quit: QuitState,
    pub stores: Arc<Stores>,
    pub ssh_config: Arc<SshConfigService>,
    pub approvals: Arc<TrustOnceApprovals>,
    pub sessions: Arc<SessionManager>,
    /// OS drops waiting to be redeemed by `transfer_upload_dropped` (§6.6).
    pub drops: DropRegistry,
    /// Problems found while opening the stores (corrupt/quarantined files, read-only, …),
    /// returned in `AppBootstrap.warnings`.
    pub startup_warnings: Vec<AppError>,
    /// OS notifications (texts from `notifications_set_texts`).
    pub notifications: Arc<NotificationHub>,
    /// Updater: the current update, a running download, a staged install (§9.8).
    pub updates: Arc<UpdateState>,
    /// How many times the page called `app_frontend_ready` (a later call is a reload).
    pub frontend_loads: FrontendLoads,
    /// The parsed bookmark import waiting for apply or cancel (§7.7).
    pub pending_import: PendingImports,
}

/// Counts page loads (`app_frontend_ready`). A second load means the WebView was reloaded or
/// its renderer restarted: the new page knows nothing of the sessions and terminals the old
/// one opened, whose channels now lead nowhere.
#[derive(Debug, Default)]
pub struct FrontendLoads(std::sync::atomic::AtomicU64);

impl FrontendLoads {
    /// Record a load; `true` if an earlier page was loaded before (a reload).
    pub fn record(&self) -> bool {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0
    }
}

impl std::fmt::Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState")
            .field("paths", &self.paths)
            .finish_non_exhaustive()
    }
}

impl AppState {
    /// Open the stores and wire the session manager (blocking; called from `setup`).
    /// `show_notification` / `window_focused` back the OS notifications.
    pub fn open(
        paths: Paths,
        ssh_config: Arc<SshConfigService>,
        recents_changed: Arc<dyn Fn(Vec<RecentEntry>) + Send + Sync>,
        show_notification: ShowFn,
        window_focused: FocusFn,
    ) -> Self {
        let (stores, startup_warnings) = Stores::open(&paths.app_config_dir);
        for w in &startup_warnings {
            tracing::warn!(warning = ?w.kind, "storage warning at startup");
        }
        // Keychain entries of bookmarks dropped on load (or saved without a binding)
        // are deleted once, off the startup path.
        let orphans = stores.lock_bookmarks().take_orphaned_keychain_ids();
        if !orphans.is_empty() {
            let spawned = std::thread::Builder::new()
                .name("keychain-cleanup".into())
                .spawn(move || crate::commands::hosts::forget_keychain(orphans));
            if let Err(e) = spawned {
                tracing::warn!(error = %e, "could not start the keychain cleanup");
            }
        }
        let stores = Arc::new(stores);
        // `connection.readSshConfig = false` must hold before the first parse.
        ssh_config.set_enabled(stores.settings().connection.read_ssh_config);
        let notifications = Arc::new(NotificationHub::new(
            stores.clone(),
            show_notification,
            window_focused,
        ));
        let approvals = Arc::new(TrustOnceApprovals::default());
        let env = ssh_config.env().clone();
        let deps = Arc::new(SessionDeps {
            stores: stores.clone(),
            approvals: approvals.clone(),
            ctx_template: ConnectCtxTemplate {
                profile_root: cfg!(windows).then(|| paths.home.clone()),
                home: env.home.clone(),
                user_ssh_dir: env.user_ssh_dir.clone(),
                env: env.env.clone(),
                keychain_available: crate::secrets::availability().is_ok(),
            },
            recents_changed,
            notifier: notifications.clone(),
        });
        let sessions = Arc::new(SessionManager::new(deps, ssh_config.clone()));
        Self {
            paths,
            quit: QuitState::default(),
            stores,
            ssh_config,
            approvals,
            sessions,
            drops: DropRegistry::default(),
            startup_warnings,
            notifications,
            updates: Arc::new(UpdateState::default()),
            frontend_loads: FrontendLoads::default(),
            pending_import: PendingImports::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_later_page_loads_count_as_reloads() {
        let loads = FrontendLoads::default();
        assert!(!loads.record(), "first load");
        assert!(loads.record(), "reload");
        assert!(loads.record(), "another reload");
    }
}
