//! Signed self-update (`docs/ARCHITECTURE.md` §9.8, §11.3).
//!
//! Everything runs in Rust; the WebView has no `updater:` permission and talks to the four app
//! commands in `commands/update.rs`. The official `tauri-plugin-updater` fetches the update
//! manifest over HTTPS, downloads the artifact and verifies its minisign signature against the
//! public key compiled into the app (`plugins.updater.pubkey`, `requireSignedVersion`).
//!
//! - **Guard** ([`check_allowed`]): never in dev/debug builds, never in `e2e` builds, never
//!   without an HTTPS `stable.json` endpoint.
//! - **One channel**: the configured endpoint whose file is `stable.json` ([`stable_endpoint`]).
//! - **Schedule** ([`run_schedule`]): first check 15 s after startup, then every 24 h of wall-clock
//!   time (an hourly tick survives sleep/hibernate), only while `general.autoUpdate` is on.
//! - **Skip**: an automatic check never announces the skipped version ([`should_announce`]); a
//!   manual check always reports it.
//! - **Install**: download (progress over a `Channel`, cancellable) → signature verified by the
//!   plugin → staged in memory → the normal quit flow (§8.9, asks about unsaved changes) → after
//!   the sessions are closed, [`apply_staged`] installs (Windows: NSIS `/P` passive, the installer
//!   restarts the app; macOS/Linux AppImage: replace in place, then restart). Cancelling the quit
//!   or `update_cancel` drops the staged bytes; nothing was written to disk before that point.
//! - **Linux .deb/.rpm** cannot replace themselves ([`install_kind`]): the UI offers the release
//!   page instead.

use std::future::Future;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, SystemTime};

use serde_json::Value;
use tauri::utils::config::BundleType;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};
use url::Url;

use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::{
    Platform, UpdateCheckOutcome, UpdateInfo, UpdateInstallKind, UpdateProgress, events,
};

/// The update manifest's file name, the last path segment of the endpoint the updater reads.
pub const MANIFEST_FILE: &str = "stable.json";

/// First automatic check after startup.
pub const FIRST_CHECK_DELAY: Duration = Duration::from_secs(15);
/// Minimum wall-clock time between automatic checks.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// How often the scheduler wakes up to compare wall-clock time (and to notice the setting).
pub const SCHEDULE_TICK: Duration = Duration::from_secs(60 * 60);
/// Timeout of one manifest request.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(30);
/// Timeout of the whole artifact download.
pub const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
/// Release notes longer than this (characters) are cut; the release page has the full text.
pub const MAX_NOTES_CHARS: usize = 10_000;
/// Progress events are sent at most once per this many bytes (plus the final one).
const PROGRESS_STEP_BYTES: u64 = 256 * 1024;

// ---------------------------------------------------------------------------------------------
// Pure logic (unit-tested)
// ---------------------------------------------------------------------------------------------

/// What kind of build this is, for the guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildFlavor {
    /// `cfg(dev)` (no `tauri build`) or a debug build (`debug_assertions`).
    pub dev: bool,
    /// The test-only `e2e` feature (§9.7).
    pub e2e: bool,
}

impl BuildFlavor {
    pub const fn current() -> Self {
        Self {
            dev: cfg!(dev) || cfg!(debug_assertions),
            e2e: cfg!(feature = "e2e"),
        }
    }
}

/// Why this build does not check for updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disabled {
    DevBuild,
    E2eBuild,
    NoEndpoint,
}

/// The HTTPS endpoints in `plugins.updater.endpoints` (anything else is ignored).
pub fn configured_endpoints(updater_config: Option<&Value>) -> Vec<Url> {
    updater_config
        .and_then(|c| c.get("endpoints"))
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .filter_map(|s| Url::parse(s).ok())
        .filter(|u| u.scheme() == "https" && u.host_str().is_some_and(|h| !h.is_empty()))
        .collect()
}

/// The configured endpoint the updater reads: the HTTPS one whose last path segment is
/// [`MANIFEST_FILE`].
pub fn stable_endpoint(endpoints: &[Url]) -> Option<Url> {
    endpoints
        .iter()
        .find(|u| {
            u.scheme() == "https"
                && u.path_segments()
                    .and_then(|mut s| s.next_back())
                    .is_some_and(|last| last == MANIFEST_FILE)
        })
        .cloned()
}

/// The build-flavor half of the guard: dev/debug and `e2e` builds never check.
pub fn build_allows_updates(flavor: BuildFlavor) -> Result<(), Disabled> {
    if flavor.e2e {
        return Err(Disabled::E2eBuild);
    }
    if flavor.dev {
        return Err(Disabled::DevBuild);
    }
    Ok(())
}

/// The guard: dev/debug and `e2e` builds never check; neither does a build without an HTTPS
/// manifest endpoint.
pub fn check_allowed(flavor: BuildFlavor, endpoint: Option<&Url>) -> Result<Url, Disabled> {
    build_allows_updates(flavor)?;
    endpoint
        .filter(|u| u.scheme() == "https")
        .cloned()
        .ok_or(Disabled::NoEndpoint)
}

/// Release notes for the UI: CRLF/CR normalised to LF, at most [`MAX_NOTES_CHARS`] characters,
/// trailing whitespace trimmed. Returns `(notes, truncated)`. The text stays untrusted: the
/// frontend renders it with `displayServerText`.
pub fn truncate_notes(notes: Option<&str>) -> (String, bool) {
    let normalised = notes
        .unwrap_or_default()
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    let mut chars = normalised.chars();
    let cut: String = chars.by_ref().take(MAX_NOTES_CHARS).collect();
    let truncated = chars.next().is_some();
    (cut.trim_end().to_owned(), truncated)
}

/// Linux self-update works only for an AppImage (`APPIMAGE` set by the AppImage runtime and not
/// a .deb/.rpm build); every other supported platform updates itself.
pub fn install_kind(
    platform: Platform,
    appimage_env: bool,
    bundle: Option<BundleType>,
) -> UpdateInstallKind {
    match platform {
        Platform::Windows | Platform::Macos => UpdateInstallKind::SelfUpdate,
        Platform::Linux => {
            let package = matches!(bundle, Some(BundleType::Deb | BundleType::Rpm));
            if appimage_env && !package {
                UpdateInstallKind::SelfUpdate
            } else {
                UpdateInstallKind::ManualDownload
            }
        }
    }
}

/// The installation kind of this process.
pub fn current_install_kind() -> UpdateInstallKind {
    install_kind(
        Platform::current(),
        cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_some_and(|v| !v.is_empty()),
        tauri::utils::platform::bundle_type(),
    )
}

/// The release page of `version` (`<repository>/releases/tag/v<version>`), or `None` for text
/// that is not a plausible version (it ends up in a URL the user may open).
pub fn release_url(version: &str) -> Option<String> {
    crate::storage::recents::valid_version_text(version)
        .then(|| format!("{}/releases/tag/v{version}", env!("CARGO_PKG_REPOSITORY")))
}

/// An automatic check announces `version` unless the user skipped exactly that version.
pub fn should_announce(version: &str, skipped: Option<&str>) -> bool {
    skipped != Some(version)
}

/// Wall-clock bookkeeping of the automatic checks.
#[derive(Debug, Default, Clone, Copy)]
pub struct Schedule {
    last_check: Option<SystemTime>,
}

impl Schedule {
    /// Due when nothing was checked yet, when [`CHECK_INTERVAL`] has passed, or when the clock
    /// went backwards (the interval cannot be measured; checking once more is harmless).
    pub fn due(&self, now: SystemTime) -> bool {
        self.last_check.is_none_or(|last| {
            now.duration_since(last)
                .map_or(true, |elapsed| elapsed >= CHECK_INTERVAL)
        })
    }

    pub fn mark(&mut self, now: SystemTime) {
        self.last_check = Some(now);
    }
}

/// The automatic-check loop: wait [`FIRST_CHECK_DELAY`], then every [`SCHEDULE_TICK`] run `check`
/// if `enabled()` and the [`Schedule`] is due. A disabled setting does not consume the slot, so
/// turning it on leads to a check within one tick.
pub async fn run_schedule<N, E, C, F>(now: N, enabled: E, mut check: C)
where
    N: Fn() -> SystemTime,
    E: Fn() -> bool,
    C: FnMut() -> F,
    F: Future<Output = ()>,
{
    tokio::time::sleep(FIRST_CHECK_DELAY).await;
    let mut schedule = Schedule::default();
    loop {
        if enabled() {
            let t = now();
            if schedule.due(t) {
                schedule.mark(t);
                check().await;
            }
        }
        tokio::time::sleep(SCHEDULE_TICK).await;
    }
}

/// Throttles `Downloading` events to one per [`PROGRESS_STEP_BYTES`].
#[derive(Debug, Default)]
pub struct ProgressThrottle {
    downloaded: u64,
    last_sent: Option<u64>,
}

impl ProgressThrottle {
    /// Account `chunk` bytes; returns the event to send, if one is due.
    pub fn on_chunk(&mut self, chunk: usize, total: Option<u64>) -> Option<UpdateProgress> {
        self.downloaded = self
            .downloaded
            .saturating_add(u64::try_from(chunk).unwrap_or(u64::MAX));
        let done = total.is_some_and(|t| self.downloaded >= t);
        let due = self
            .last_sent
            .is_none_or(|last| self.downloaded.saturating_sub(last) >= PROGRESS_STEP_BYTES);
        (due || done).then(|| {
            self.last_sent = Some(self.downloaded);
            UpdateProgress::Downloading {
                downloaded: self.downloaded,
                total,
            }
        })
    }
}

/// Map a plugin error of the check to `updateCheckFailed` (English detail for logs only).
fn check_failed(err: &tauri_plugin_updater::Error) -> AppError {
    AppError::new(ErrorCode::UpdateCheckFailed).with_detail(err.to_string())
}

/// Map a plugin error of the download: signature problems are `updateVerifyFailed`, the rest
/// `updateDownloadFailed`.
pub fn download_failed(err: &tauri_plugin_updater::Error) -> AppError {
    use tauri_plugin_updater::Error as E;
    let code = match err {
        E::Minisign(_)
        | E::Base64(_)
        | E::SignatureUtf8(_)
        | E::SignedVersionMismatch { .. }
        | E::MissingSignedVersion => ErrorCode::UpdateVerifyFailed,
        _ => ErrorCode::UpdateDownloadFailed,
    };
    AppError::new(code).with_detail(err.to_string())
}

// ---------------------------------------------------------------------------------------------
// Runtime state
// ---------------------------------------------------------------------------------------------

/// A verified download waiting for the end of the quit flow.
pub struct Staged {
    update: Update,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Inner {
    /// The update last reported to the UI (automatic or manual check).
    current: Option<Update>,
    /// A download is running; `cancel` stops it.
    downloading: bool,
    cancel: Option<tokio::sync::oneshot::Sender<()>>,
    staged: Option<Staged>,
}

/// `AppState.updates`.
#[derive(Default)]
pub struct UpdateState {
    inner: Mutex<Inner>,
}

impl std::fmt::Debug for UpdateState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateState").finish_non_exhaustive()
    }
}

/// Clears the `downloading` flag when the download ends in any way (error, cancel, panic).
struct DownloadGuard<'a>(&'a UpdateState);

impl Drop for DownloadGuard<'_> {
    fn drop(&mut self) {
        self.0.with(|i| {
            i.downloading = false;
            i.cancel = None;
        });
    }
}

impl UpdateState {
    fn with<T>(&self, f: impl FnOnce(&mut Inner) -> T) -> T {
        f(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// A download is running or a verified update waits for the quit flow.
    pub fn busy(&self) -> bool {
        self.with(|i| i.downloading || i.staged.is_some())
    }

    fn set_current(&self, update: Option<Update>) {
        self.with(|i| i.current = update);
    }

    /// `update_cancel`: stop a running download and drop a staged one.
    pub fn cancel(&self) {
        self.with(|i| {
            if let Some(tx) = i.cancel.take() {
                let _ = tx.send(());
            }
            i.staged = None;
        });
    }

    /// `app_quit_cancel`: the user kept the app open, so the staged update is dropped.
    pub fn discard_staged(&self) {
        self.with(|i| i.staged = None);
    }

    /// The staged update, removed (end of the quit flow).
    pub fn take_staged(&self) -> Option<Staged> {
        self.with(|i| i.staged.take())
    }

    /// Stage the finished download unless `update_cancel` took the cancel handle meanwhile
    /// (a cancel that raced the end of the download must not start the quit flow). `make` runs
    /// only when the update is staged. Returns whether it was.
    fn stage_unless_cancelled(&self, make: impl FnOnce() -> Staged) -> bool {
        self.with(|i| {
            if !i.downloading || i.cancel.is_none() {
                return false;
            }
            i.staged = Some(make());
            true
        })
    }
}

/// The updater plugin's config section (`plugins.updater` in `tauri.conf.json`).
fn updater_config<R: Runtime>(app: &AppHandle<R>) -> Option<Value> {
    app.config().plugins.0.get("updater").cloned()
}

fn info_for(update: &Update, install: UpdateInstallKind) -> Result<UpdateInfo, AppError> {
    let release_url = release_url(&update.version).ok_or_else(|| {
        AppError::new(ErrorCode::UpdateCheckFailed).with_detail("unexpected version text")
    })?;
    let (notes, notes_truncated) = truncate_notes(update.body.as_deref());
    // The plugin already parsed `pub_date` as RFC 3339; pass the original text on (bounded).
    let date = update
        .raw_json
        .get("pub_date")
        .and_then(Value::as_str)
        .filter(|d| d.len() <= 64 && update.date.is_some())
        .map(str::to_owned);
    Ok(UpdateInfo {
        version: update.version.clone(),
        current_version: update.current_version.clone(),
        date,
        notes,
        notes_truncated,
        install,
        release_url,
    })
}

/// One check of the update manifest. `Ok(None)`: up to date.
async fn check<R: Runtime>(app: &AppHandle<R>) -> Result<Option<(Update, UpdateInfo)>, AppError> {
    let endpoints = configured_endpoints(updater_config(app).as_ref());
    let endpoint = check_allowed(BuildFlavor::current(), stable_endpoint(&endpoints).as_ref())
        .map_err(|d| AppError::new(ErrorCode::UpdatesDisabled).with_detail(format!("{d:?}")))?;
    let updater = app
        .updater_builder()
        .endpoints(vec![endpoint])
        .map_err(|e| check_failed(&e))?
        .timeout(CHECK_TIMEOUT)
        .build()
        .map_err(|e| check_failed(&e))?;
    let Some(update) = updater.check().await.map_err(|e| check_failed(&e))? else {
        return Ok(None);
    };
    let info = info_for(&update, current_install_kind())?;
    Ok(Some((update, info)))
}

/// `update_check` (manual): reports the update even if its version was skipped.
pub async fn manual_check<R: Runtime>(
    app: &AppHandle<R>,
    state: &UpdateState,
) -> Result<UpdateCheckOutcome, AppError> {
    if state.busy() {
        return Err(AppError::new(ErrorCode::UpdateInProgress));
    }
    match check(app).await? {
        Some((update, info)) => {
            tracing::info!(version = %info.version, "update available (manual check)");
            state.set_current(Some(update));
            Ok(UpdateCheckOutcome::Available { update: info })
        }
        None => {
            tracing::info!("no update available (manual check)");
            state.set_current(None);
            Ok(UpdateCheckOutcome::UpToDate)
        }
    }
}

/// One automatic check: errors are logged only; a skipped version is never announced.
async fn automatic_check<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<crate::app_state::AppState>() else {
        return;
    };
    if state.updates.busy() {
        return;
    }
    match check(app).await {
        Ok(Some((update, info))) => {
            let skipped = state
                .stores
                .lock_recents()
                .skipped_update_version()
                .map(str::to_owned);
            if !should_announce(&info.version, skipped.as_deref()) {
                tracing::info!(version = %info.version, "update available but skipped by the user");
                return;
            }
            tracing::info!(version = %info.version, "update available");
            state.updates.set_current(Some(update));
            if let Err(e) = app.emit(events::UPDATE_AVAILABLE, info) {
                tracing::warn!(error = %e, "failed to emit update:available");
            }
        }
        Ok(None) => tracing::info!("no update available"),
        Err(e) => {
            tracing::warn!(code = ?e.kind, detail = ?e.detail, "automatic update check failed")
        }
    }
}

/// Start the automatic-check loop (called from `setup`). Dev/debug and e2e builds start no
/// loop at all; the full guard (with the manifest endpoint) runs again at every check.
pub fn spawn_scheduler<R: Runtime>(app: &AppHandle<R>) {
    if let Err(d) = build_allows_updates(BuildFlavor::current()) {
        tracing::info!(reason = ?d, "automatic update checks are off in this build");
        return;
    }
    let enabled_app = app.clone();
    let check_app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_schedule(
            SystemTime::now,
            move || {
                enabled_app
                    .try_state::<crate::app_state::AppState>()
                    .is_some_and(|s| s.stores.settings().general.auto_update)
            },
            move || {
                let app = check_app.clone();
                async move { automatic_check(&app).await }
            },
        )
        .await;
    });
}

/// `update_install`: download `version` (the one the UI showed), verify, stage, and start the
/// quit flow. Returns once the quit flow was requested.
pub async fn install<R: Runtime>(
    app: &AppHandle<R>,
    state: &UpdateState,
    version: &str,
    progress: impl Fn(UpdateProgress) + Send + Sync,
) -> Result<(), AppError> {
    if current_install_kind() == UpdateInstallKind::ManualDownload {
        return Err(AppError::new(ErrorCode::UpdateManualOnly));
    }
    let (rx, mut update) = state.with(|i| {
        if i.downloading || i.staged.is_some() {
            return Err(AppError::new(ErrorCode::UpdateInProgress));
        }
        let update = i
            .current
            .clone()
            .filter(|u| u.version == version)
            .ok_or_else(|| AppError::new(ErrorCode::UpdateNotFound))?;
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        i.downloading = true;
        i.cancel = Some(tx);
        Ok((rx, update))
    })?;
    let guard = DownloadGuard(state);
    update.timeout = Some(DOWNLOAD_TIMEOUT);
    tracing::info!(version = %update.version, "downloading update");
    let mut throttle = ProgressThrottle::default();
    let download = update.download(
        |chunk, total| {
            if let Some(event) = throttle.on_chunk(chunk, total) {
                progress(event);
            }
        },
        || {},
    );
    let bytes = tokio::select! {
        result = download => result.map_err(|e| {
            let err = download_failed(&e);
            tracing::warn!(code = ?err.kind, detail = ?err.detail, "update download failed");
            err
        })?,
        _ = rx => {
            tracing::info!("update download cancelled");
            return Err(AppError::new(ErrorCode::Cancelled));
        }
    };
    tracing::info!(
        bytes = bytes.len(),
        "update downloaded and signature verified"
    );
    if !state.stage_unless_cancelled(|| Staged { update, bytes }) {
        tracing::info!("update download cancelled");
        return Err(AppError::new(ErrorCode::Cancelled));
    }
    drop(guard);
    progress(UpdateProgress::Ready);
    crate::quit::handle_close_request(app);
    Ok(())
}

/// End of the quit flow with a staged update: install it, then restart. On Windows the plugin
/// starts the NSIS installer in passive mode (`/P /R`: it restarts the app) and exits this
/// process. Returns only on failure (the caller keeps the app open and reports it).
pub async fn apply_staged<R: Runtime>(app: &AppHandle<R>, staged: Staged) -> AppError {
    tracing::info!(version = %staged.update.version, "installing update");
    crate::logging::flush();
    let Staged { update, bytes } = staged;
    let installed = tauri::async_runtime::spawn_blocking(move || {
        update.install(bytes).map_err(|e| e.to_string())
    })
    .await;
    match installed {
        Ok(Ok(())) => {
            tracing::info!("update installed; restarting");
            crate::logging::flush();
            app.restart()
        }
        Ok(Err(detail)) => {
            tracing::error!(detail = %detail, "update install failed");
            AppError::new(ErrorCode::UpdateInstallFailed).with_detail(detail)
        }
        Err(e) => {
            tracing::error!(error = %e, "update install task failed");
            AppError::new(ErrorCode::UpdateInstallFailed).with_detail(e.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::cell::Cell;
    use std::rc::Rc;

    use pretty_assertions::assert_eq;
    use serde_json::json;

    use super::*;

    const RELEASE: BuildFlavor = BuildFlavor {
        dev: false,
        e2e: false,
    };

    fn endpoints() -> Vec<Url> {
        configured_endpoints(Some(&json!({ "endpoints": [
            "https://github.com/shellfin-app/shellfin/releases/download/updater/stable.json",
        ]})))
    }

    #[test]
    fn guard_refuses_dev_e2e_and_missing_endpoints() {
        let stable = stable_endpoint(&endpoints());
        assert_eq!(
            check_allowed(RELEASE, stable.as_ref()).unwrap().as_str(),
            "https://github.com/shellfin-app/shellfin/releases/download/updater/stable.json"
        );
        let dev = BuildFlavor {
            dev: true,
            e2e: false,
        };
        assert_eq!(check_allowed(dev, stable.as_ref()), Err(Disabled::DevBuild));
        let e2e = BuildFlavor {
            dev: true,
            e2e: true,
        };
        assert_eq!(check_allowed(e2e, stable.as_ref()), Err(Disabled::E2eBuild));
        let e2e_release = BuildFlavor {
            dev: false,
            e2e: true,
        };
        assert_eq!(
            check_allowed(e2e_release, stable.as_ref()),
            Err(Disabled::E2eBuild)
        );
        assert_eq!(check_allowed(RELEASE, None), Err(Disabled::NoEndpoint));
        let http = Url::parse("http://example.com/stable.json").unwrap();
        assert_eq!(
            check_allowed(RELEASE, Some(&http)),
            Err(Disabled::NoEndpoint)
        );
        // Unit tests are debug builds: the running test binary itself never checks.
        assert!(BuildFlavor::current().dev);
    }

    #[test]
    fn configured_endpoints_keep_https_only() {
        assert!(configured_endpoints(None).is_empty());
        assert!(configured_endpoints(Some(&json!({}))).is_empty());
        assert!(
            configured_endpoints(Some(&json!({ "endpoints": "https://x/stable.json" }))).is_empty()
        );
        let got = configured_endpoints(Some(&json!({ "endpoints": [
            "http://example.com/stable.json",
            "file:///C:/stable.json",
            "not a url",
            42,
            "https://example.com/stable.json",
        ]})));
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].as_str(), "https://example.com/stable.json");
    }

    #[test]
    fn only_the_stable_manifest_endpoint_is_used() {
        assert!(
            stable_endpoint(&endpoints())
                .unwrap()
                .as_str()
                .ends_with("/updater/stable.json")
        );
        // Only an exact last path segment matches: the legacy `experimental.json` mirror and
        // look-alike names are never read, even when configured first.
        let others = configured_endpoints(Some(&json!({ "endpoints": [
            "https://example.com/experimental.json",
            "https://example.com/stable.json.bak",
            "https://example.com/not-stable.json",
            "https://example.com/stable.json/",
        ]})));
        assert!(stable_endpoint(&others).is_none());
        assert_eq!(
            check_allowed(RELEASE, stable_endpoint(&others).as_ref()),
            Err(Disabled::NoEndpoint)
        );
        let mixed = configured_endpoints(Some(&json!({ "endpoints": [
            "https://example.com/experimental.json",
            "https://example.com/updater/stable.json",
        ]})));
        assert_eq!(
            stable_endpoint(&mixed).unwrap().as_str(),
            "https://example.com/updater/stable.json"
        );
        assert!(stable_endpoint(&[]).is_none());
    }

    /// The shipped config: exactly one endpoint, `stable.json` on the official repository
    /// (HTTPS), the release public key, signed versions required, NSIS passive mode, and updater
    /// artifacts off by default (CI builds without the signing key; the release workflow turns
    /// them on).
    #[test]
    fn shipped_tauri_conf_is_the_trusted_updater_config() {
        let text = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tauri.conf.json"),
        )
        .unwrap();
        let conf: Value = serde_json::from_str(&text).unwrap();
        let updater = &conf["plugins"]["updater"];
        let eps = configured_endpoints(Some(updater));
        assert_eq!(eps.len(), 1, "{updater}");
        assert_eq!(
            updater["endpoints"].as_array().unwrap().len(),
            1,
            "no non-HTTPS endpoint"
        );
        assert_eq!(
            stable_endpoint(&eps).unwrap().as_str(),
            format!(
                "{}/releases/download/updater/stable.json",
                env!("CARGO_PKG_REPOSITORY")
            )
        );
        assert_eq!(
            updater["pubkey"],
            "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWduIHB1YmxpYyBrZXk6IDU1OTI0MEE5MTk4M0MyNzYKUldSMndvTVpxVUNTVmJsMmdrNkFEWWduTEd1L1ZEb0paSTBmL2lna3VuU2lhSWdGWjgxdWVkMEoK"
        );
        assert_eq!(updater["requireSignedVersion"], true);
        assert_eq!(updater["windows"]["installMode"], "passive");
        for dangerous in [
            "dangerousInsecureTransportProtocol",
            "dangerousAcceptInvalidCerts",
            "dangerousAcceptInvalidHostnames",
            "allowDowngrades",
        ] {
            assert!(updater.get(dangerous).is_none(), "{dangerous}");
        }
        assert!(
            conf["bundle"].get("createUpdaterArtifacts").is_none()
                || conf["bundle"]["createUpdaterArtifacts"] == false,
            "updater artifacts are enabled only by the release workflow"
        );
    }

    #[test]
    fn notes_are_normalised_and_truncated() {
        assert_eq!(truncate_notes(None), (String::new(), false));
        assert_eq!(
            truncate_notes(Some("a\r\nb\rc\n\n  ")),
            ("a\nb\nc".to_owned(), false)
        );
        let exact = "ä".repeat(MAX_NOTES_CHARS);
        assert_eq!(truncate_notes(Some(&exact)), (exact.clone(), false));
        let long = format!("{exact}xyz");
        let (cut, truncated) = truncate_notes(Some(&long));
        assert!(truncated);
        assert_eq!(cut.chars().count(), MAX_NOTES_CHARS);
        // Multi-byte boundaries and hostile content stay intact (escaping is the UI's job).
        let hostile = "\u{1b}[2J\u{202e}<script>".repeat(5000);
        let (cut, truncated) = truncate_notes(Some(&hostile));
        assert!(truncated);
        assert!(cut.chars().count() <= MAX_NOTES_CHARS);
    }

    #[test]
    fn linux_self_update_is_appimage_only() {
        use UpdateInstallKind::{ManualDownload, SelfUpdate};
        assert_eq!(install_kind(Platform::Windows, false, None), SelfUpdate);
        assert_eq!(
            install_kind(Platform::Windows, false, Some(BundleType::Msi)),
            SelfUpdate
        );
        assert_eq!(
            install_kind(Platform::Macos, false, Some(BundleType::App)),
            SelfUpdate
        );
        assert_eq!(
            install_kind(Platform::Linux, true, Some(BundleType::AppImage)),
            SelfUpdate
        );
        assert_eq!(install_kind(Platform::Linux, true, None), SelfUpdate);
        // .deb/.rpm (no APPIMAGE), and a stray APPIMAGE variable in a .deb/.rpm install.
        assert_eq!(install_kind(Platform::Linux, false, None), ManualDownload);
        assert_eq!(
            install_kind(Platform::Linux, false, Some(BundleType::Deb)),
            ManualDownload
        );
        assert_eq!(
            install_kind(Platform::Linux, true, Some(BundleType::Deb)),
            ManualDownload
        );
        assert_eq!(
            install_kind(Platform::Linux, true, Some(BundleType::Rpm)),
            ManualDownload
        );
    }

    #[test]
    fn release_url_is_built_from_a_plain_version() {
        let repo = env!("CARGO_PKG_REPOSITORY");
        assert_eq!(
            release_url("0.2.0-experimental.1").unwrap(),
            format!("{repo}/releases/tag/v0.2.0-experimental.1")
        );
        assert!(repo.starts_with("https://github.com/"));
        for bad in [
            "",
            "1.0.0/../../evil",
            "1.0.0?x=1",
            "1.0.0#a",
            "v1.0.0",
            "1 0",
        ] {
            assert!(release_url(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn skip_suppresses_only_that_version() {
        assert!(should_announce("0.2.0", None));
        assert!(!should_announce("0.2.0", Some("0.2.0")));
        assert!(should_announce("0.2.1", Some("0.2.0")));
        assert!(should_announce("0.2.0", Some("0.2.0-experimental.1")));
    }

    #[test]
    fn schedule_due_rules() {
        let t0 = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        let mut s = Schedule::default();
        assert!(s.due(t0));
        s.mark(t0);
        assert!(!s.due(t0));
        assert!(!s.due(t0 + CHECK_INTERVAL - Duration::from_secs(1)));
        assert!(s.due(t0 + CHECK_INTERVAL));
        // The clock went backwards: checking once more is the safe choice.
        assert!(s.due(t0 - Duration::from_secs(10)));
    }

    /// Paused tokio clock: 15 s delay, then once per 24 h; a disabled setting skips without
    /// consuming the slot and a later enable checks within one tick.
    #[tokio::test(start_paused = true)]
    async fn scheduler_checks_after_delay_then_daily_and_honours_the_setting() {
        let base = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
        let start = tokio::time::Instant::now();
        let enabled = Rc::new(Cell::new(true));
        let checks = Rc::new(Cell::new(0u32));
        let (e, c) = (enabled.clone(), checks.clone());
        let local = tokio::task::LocalSet::new();
        local.spawn_local(async move {
            run_schedule(
                move || base + start.elapsed(),
                move || e.get(),
                move || {
                    let c = c.clone();
                    async move { c.set(c.get() + 1) }
                },
            )
            .await;
        });
        local
            .run_until(async {
                tokio::time::sleep(FIRST_CHECK_DELAY - Duration::from_secs(1)).await;
                assert_eq!(checks.get(), 0, "nothing before 15 s");
                tokio::time::sleep(Duration::from_secs(2)).await;
                assert_eq!(checks.get(), 1, "first check at 15 s");
                tokio::time::sleep(CHECK_INTERVAL - Duration::from_secs(60)).await;
                assert_eq!(checks.get(), 1, "not again within 24 h");
                tokio::time::sleep(SCHEDULE_TICK + Duration::from_secs(60)).await;
                assert_eq!(checks.get(), 2, "again after 24 h");

                enabled.set(false);
                tokio::time::sleep(CHECK_INTERVAL * 3).await;
                assert_eq!(checks.get(), 2, "the setting is off");
                enabled.set(true);
                tokio::time::sleep(SCHEDULE_TICK + Duration::from_secs(1)).await;
                assert_eq!(checks.get(), 3, "turning it on checks within one tick");
            })
            .await;
    }

    #[tokio::test(start_paused = true)]
    async fn scheduler_never_checks_when_disabled_from_the_start() {
        let checks = Rc::new(Cell::new(0u32));
        let c = checks.clone();
        let local = tokio::task::LocalSet::new();
        local.spawn_local(async move {
            run_schedule(
                SystemTime::now,
                || false,
                move || {
                    let c = c.clone();
                    async move { c.set(c.get() + 1) }
                },
            )
            .await;
        });
        local
            .run_until(tokio::time::sleep(CHECK_INTERVAL * 2))
            .await;
        assert_eq!(checks.get(), 0);
    }

    #[test]
    fn progress_is_throttled_and_reports_completion() {
        let mut t = ProgressThrottle::default();
        let total = Some(1024 * 1024);
        assert_eq!(
            t.on_chunk(1000, total),
            Some(UpdateProgress::Downloading {
                downloaded: 1000,
                total
            })
        );
        assert_eq!(t.on_chunk(1000, total), None);
        // 2000 bytes so far; 1 MiB - 2000 more in 16 KiB chunks (the last one shorter).
        let mut events = Vec::new();
        let mut left: u64 = 1024 * 1024 - 2000;
        while left > 0 {
            let chunk = left.min(16 * 1024);
            left -= chunk;
            events.extend(t.on_chunk(usize::try_from(chunk).unwrap(), total));
        }
        assert!(events.len() <= 5, "{} events for 1 MiB", events.len());
        assert_eq!(
            events.last(),
            Some(&UpdateProgress::Downloading {
                downloaded: 1024 * 1024,
                total
            }),
            "completion is always reported"
        );
        // Without Content-Length: one event per step.
        let mut t = ProgressThrottle::default();
        let n = (0..64).filter_map(|_| t.on_chunk(64 * 1024, None)).count();
        assert_eq!(n, 16);
    }

    #[test]
    fn signature_errors_map_to_verify_failed() {
        use tauri_plugin_updater::Error as E;
        let verify = [
            E::SignedVersionMismatch {
                signed: "0.1.0".into(),
                announced: "9.9.9".into(),
            },
            E::MissingSignedVersion,
            E::SignatureUtf8("x".into()),
        ];
        for e in &verify {
            assert_eq!(
                download_failed(e).kind,
                ErrorCode::UpdateVerifyFailed,
                "{e}"
            );
        }
        for e in [
            E::Network("503".into()),
            E::ReleaseNotFound,
            E::InvalidUpdaterFormat,
        ] {
            assert_eq!(
                download_failed(&e).kind,
                ErrorCode::UpdateDownloadFailed,
                "{e}"
            );
        }
        assert_eq!(
            check_failed(&E::ReleaseNotFound).kind,
            ErrorCode::UpdateCheckFailed
        );
    }

    #[test]
    fn state_cancel_and_discard() {
        let s = UpdateState::default();
        assert!(!s.busy());
        let (tx, mut rx) = tokio::sync::oneshot::channel();
        s.with(|i| {
            i.downloading = true;
            i.cancel = Some(tx);
        });
        assert!(s.busy());
        s.cancel();
        assert!(
            rx.try_recv().is_ok(),
            "the running download is told to stop"
        );
        // The download finished just as the user cancelled: the cancel wins, nothing is staged.
        assert!(
            !s.stage_unless_cancelled(|| unreachable!("a cancelled download is never staged")),
            "a cancel that raced the end of the download wins"
        );
        {
            let _g = DownloadGuard(&s);
        }
        assert!(!s.busy(), "the guard clears the flag");
        assert!(s.take_staged().is_none());
        s.discard_staged();
        assert!(!s.busy());
    }
}
