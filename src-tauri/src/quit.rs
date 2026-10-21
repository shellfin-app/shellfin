//! Quit flow (§8.9). Every quit path (window close button, Cmd+Q / app-menu Quit, OS shutdown)
//! is prevented in Rust and routed through one state machine:
//!
//! | State          | Close/quit request          | ack          | cancel | app_quit   | Timer (3 s)            |
//! |----------------|-----------------------------|--------------|--------|------------|------------------------|
//! | `idle`         | → requested, emit, arm 3 s  | –            | –      | → quitting | –                      |
//! | `requested`    | ignore (timer running)      | → confirming | → idle | → quitting | → unresponsive         |
//! | `unresponsive` | **forced quit**             | → confirming | → idle | → quitting | –                      |
//! | `confirming`   | re-emit                     | –            | → idle | → quitting | –                      |
//! | `quitting`     | ignore                      | –            | –      | –          | –                      |
//!
//! There is no timer-driven forced quit: only a *repeated* request after ≥ 3 s without an ack.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::app_state::AppState;
use crate::ipc::dto::{EmptyPayload, events};
use crate::logging;

/// How long the frontend has to acknowledge `app:quit-requested` before a repeated request force-quits.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(3);
/// Total budget for disconnecting all sessions while quitting.
pub const DISCONNECT_BUDGET: Duration = Duration::from_millis(500);
/// Budget for removing every session's agent keys first, separate from the above.
pub const AGENT_KEYS_BUDGET: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuitPhase {
    Idle,
    Requested,
    Unresponsive,
    Confirming,
    Quitting,
}

/// What the runtime glue must do after a close/quit request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseAction {
    /// Emit `app:quit-requested` and arm the ack timer for this request generation.
    EmitAndArmTimer {
        generation: u64,
    },
    /// Re-emit `app:quit-requested` (the frontend refocuses its dialog).
    ReEmit,
    /// Quit with empty `lastDirs` (frontend unresponsive and the user repeated the request).
    ForceQuit,
    Ignore,
}

/// Pure state machine (no Tauri types) — unit-tested below.
#[derive(Debug)]
pub struct QuitMachine {
    phase: QuitPhase,
    /// Incremented on every `idle → requested`; a timer only fires for its own generation.
    generation: u64,
}

impl Default for QuitMachine {
    fn default() -> Self {
        Self::new()
    }
}

impl QuitMachine {
    pub const fn new() -> Self {
        Self {
            phase: QuitPhase::Idle,
            generation: 0,
        }
    }

    pub const fn phase(&self) -> QuitPhase {
        self.phase
    }

    pub fn on_close_request(&mut self) -> CloseAction {
        match self.phase {
            QuitPhase::Idle => {
                self.generation = self.generation.wrapping_add(1);
                self.phase = QuitPhase::Requested;
                CloseAction::EmitAndArmTimer {
                    generation: self.generation,
                }
            }
            QuitPhase::Requested | QuitPhase::Quitting => CloseAction::Ignore,
            QuitPhase::Unresponsive => {
                self.phase = QuitPhase::Quitting;
                CloseAction::ForceQuit
            }
            QuitPhase::Confirming => CloseAction::ReEmit,
        }
    }

    /// `app_quit_ack`.
    pub fn on_ack(&mut self) {
        if matches!(self.phase, QuitPhase::Requested | QuitPhase::Unresponsive) {
            self.phase = QuitPhase::Confirming;
        }
    }

    /// `app_quit_cancel`.
    pub fn on_cancel(&mut self) {
        if matches!(
            self.phase,
            QuitPhase::Requested | QuitPhase::Unresponsive | QuitPhase::Confirming
        ) {
            self.phase = QuitPhase::Idle;
        }
    }

    /// `app_quit(lastDirs)`. Returns `true` if the caller must perform the quit.
    pub fn on_quit(&mut self) -> bool {
        if self.phase == QuitPhase::Quitting {
            return false;
        }
        self.phase = QuitPhase::Quitting;
        true
    }

    /// Updater: installing a staged update failed at the end of the quit flow; the app
    /// stays open (`quitting` → `idle`), so a later close/quit request works normally.
    pub fn on_quit_aborted(&mut self) {
        if self.phase == QuitPhase::Quitting {
            self.phase = QuitPhase::Idle;
        }
    }

    /// The 3 s ack timer of request `generation` expired.
    pub fn on_timer(&mut self, generation: u64) {
        if self.phase == QuitPhase::Requested && self.generation == generation {
            self.phase = QuitPhase::Unresponsive;
        }
    }
}

/// `AppState.quit`.
#[derive(Debug, Clone, Default)]
pub struct QuitState(Arc<Mutex<QuitMachine>>);

impl QuitState {
    pub fn with<T>(&self, f: impl FnOnce(&mut QuitMachine) -> T) -> T {
        let mut guard = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        f(&mut guard)
    }
}

/// Entry point for every close/quit request (window close button, `ExitRequested{code: None}`).
pub fn handle_close_request<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<AppState>() else {
        tracing::warn!("quit requested before startup completed; ignoring");
        return;
    };
    let action = state.quit.with(QuitMachine::on_close_request);
    tracing::info!(?action, "quit requested");
    match action {
        CloseAction::EmitAndArmTimer { generation } => {
            emit_quit_requested(app);
            let quit = state.quit.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(ACK_TIMEOUT).await;
                quit.with(|m| m.on_timer(generation));
            });
        }
        CloseAction::ReEmit => emit_quit_requested(app),
        CloseAction::ForceQuit => {
            tracing::warn!("frontend did not acknowledge the quit request; forcing quit");
            let app = app.clone();
            tauri::async_runtime::spawn(async move { perform_quit(&app, HashMap::new()).await });
        }
        CloseAction::Ignore => {}
    }
}

fn emit_quit_requested<R: Runtime>(app: &AppHandle<R>) {
    if let Err(err) = app.emit(events::QUIT_REQUESTED, EmptyPayload {}) {
        tracing::error!(error = %err, "failed to emit quit request");
    }
}

/// `quitting`: close every session with its `lastDir` (500 ms budget), flush logs, exit(0).
/// `AppHandle::exit` raises `ExitRequested{code: Some(0)}`, which is not prevented.
pub async fn perform_quit<R: Runtime>(
    app: &AppHandle<R>,
    last_dirs: HashMap<String, Option<String>>,
) {
    tracing::info!(sessions = last_dirs.len(), "quitting");
    record_open_bookmarks(app).await;
    // (§9.2): keys added to the user's agent are removed in a pass of their own, with
    // their own budget, before anything that can stall (transfers, terminals, disconnects).
    if let Some(state) = app.try_state::<AppState>() {
        let sessions = state.sessions.clone();
        if tokio::time::timeout(AGENT_KEYS_BUDGET, sessions.remove_all_agent_keys())
            .await
            .is_err()
        {
            tracing::warn!("removing agent keys exceeded the quit budget");
        }
    }
    if tokio::time::timeout(DISCONNECT_BUDGET, close_all_sessions(app, last_dirs))
        .await
        .is_err()
    {
        tracing::warn!("session shutdown exceeded the quit budget");
    }
    logging::flush();
    // Updater (§9.8): "Update and restart" staged a verified update before requesting the
    // quit. Install it now that the sessions are closed; success restarts (never returns).
    let staged = app
        .try_state::<AppState>()
        .and_then(|s| s.updates.take_staged());
    if let Some(staged) = staged {
        let err = crate::updater::apply_staged(app, staged).await;
        if let Some(state) = app.try_state::<AppState>() {
            state.quit.with(QuitMachine::on_quit_aborted);
        }
        if let Err(e) = app.emit(events::UPDATE_INSTALL_FAILED, err) {
            tracing::error!(error = %e, "failed to emit update:install-failed");
        }
        return;
    }
    app.exit(0);
}

/// Startup restore: remember which bookmarks are open, before the sessions
/// close. A failure (read-only `recents.json`) is logged; quitting goes on.
async fn record_open_bookmarks<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let ids = state.sessions.open_bookmark_ids();
    let stores = state.stores.clone();
    let recorded =
        tauri::async_runtime::spawn_blocking(move || stores.lock_recents().set_open_at_quit(ids))
            .await;
    match recorded {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(code = ?e.kind, "could not record the open bookmarks"),
        Err(e) => tracing::warn!(error = %e, "could not record the open bookmarks"),
    }
}

/// Close every session with its `lastDir` (recents updated; DISCONNECT per hop).
async fn close_all_sessions<R: Runtime>(
    app: &AppHandle<R>,
    last_dirs: HashMap<String, Option<String>>,
) {
    if let Some(state) = app.try_state::<AppState>() {
        let sessions = state.sessions.clone();
        sessions.close_all(last_dirs).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn requested() -> (QuitMachine, u64) {
        let mut m = QuitMachine::new();
        match m.on_close_request() {
            CloseAction::EmitAndArmTimer { generation } => (m, generation),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn idle_transitions() {
        let (m, _) = requested();
        assert_eq!(m.phase(), QuitPhase::Requested);

        let mut m = QuitMachine::new();
        m.on_ack();
        m.on_cancel();
        m.on_timer(0);
        assert_eq!(m.phase(), QuitPhase::Idle);
        assert!(m.on_quit());
        assert_eq!(m.phase(), QuitPhase::Quitting);
    }

    #[test]
    fn requested_ignores_repeats_until_timer_then_no_forced_quit_without_repeat() {
        let (mut m, g) = requested();
        assert_eq!(m.on_close_request(), CloseAction::Ignore);
        assert_eq!(m.phase(), QuitPhase::Requested);
        m.on_timer(g);
        // The timer alone never quits: it only marks the frontend unresponsive.
        assert_eq!(m.phase(), QuitPhase::Unresponsive);
        assert_eq!(m.on_close_request(), CloseAction::ForceQuit);
        assert_eq!(m.phase(), QuitPhase::Quitting);
        assert_eq!(m.on_close_request(), CloseAction::Ignore);
    }

    #[test]
    fn requested_ack_cancel_quit() {
        let (mut m, _) = requested();
        m.on_ack();
        assert_eq!(m.phase(), QuitPhase::Confirming);

        let (mut m, _) = requested();
        m.on_cancel();
        assert_eq!(m.phase(), QuitPhase::Idle);

        let (mut m, _) = requested();
        assert!(m.on_quit());
        assert_eq!(m.phase(), QuitPhase::Quitting);
    }

    #[test]
    fn late_ack_from_unresponsive_goes_to_confirming() {
        let (mut m, g) = requested();
        m.on_timer(g);
        m.on_ack();
        assert_eq!(m.phase(), QuitPhase::Confirming);
        // While confirming, a repeated request re-emits and never force-quits.
        assert_eq!(m.on_close_request(), CloseAction::ReEmit);
        m.on_timer(g);
        assert_eq!(m.phase(), QuitPhase::Confirming);
    }

    #[test]
    fn unresponsive_cancel_and_quit() {
        let (mut m, g) = requested();
        m.on_timer(g);
        m.on_cancel();
        assert_eq!(m.phase(), QuitPhase::Idle);

        let (mut m, g) = requested();
        m.on_timer(g);
        assert!(m.on_quit());
        assert_eq!(m.phase(), QuitPhase::Quitting);
    }

    #[test]
    fn confirming_transitions() {
        let (mut m, _) = requested();
        m.on_ack();
        assert_eq!(m.on_close_request(), CloseAction::ReEmit);
        m.on_ack();
        assert_eq!(m.phase(), QuitPhase::Confirming);
        m.on_cancel();
        assert_eq!(m.phase(), QuitPhase::Idle);

        let (mut m, _) = requested();
        m.on_ack();
        assert!(m.on_quit());
        assert_eq!(m.phase(), QuitPhase::Quitting);
    }

    #[test]
    fn quitting_ignores_everything() {
        let mut m = QuitMachine::new();
        assert!(m.on_quit());
        assert_eq!(m.on_close_request(), CloseAction::Ignore);
        m.on_ack();
        m.on_cancel();
        m.on_timer(1);
        assert!(!m.on_quit());
        assert_eq!(m.phase(), QuitPhase::Quitting);
    }

    /// Updater: a failed update install ends `quitting`; nothing else is affected.
    #[test]
    fn aborted_quit_returns_to_idle_only_from_quitting() {
        let mut m = QuitMachine::new();
        assert!(m.on_quit());
        m.on_quit_aborted();
        assert_eq!(m.phase(), QuitPhase::Idle);
        assert!(matches!(
            m.on_close_request(),
            CloseAction::EmitAndArmTimer { .. }
        ));
        m.on_quit_aborted();
        assert_eq!(m.phase(), QuitPhase::Requested);
        m.on_ack();
        m.on_quit_aborted();
        assert_eq!(m.phase(), QuitPhase::Confirming);
    }

    #[test]
    fn stale_timer_does_not_affect_a_new_request() {
        let (mut m, g1) = requested();
        m.on_cancel();
        let g2 = match m.on_close_request() {
            CloseAction::EmitAndArmTimer { generation } => generation,
            other => panic!("unexpected {other:?}"),
        };
        assert_ne!(g1, g2);
        m.on_timer(g1); // timer of the cancelled request fires late
        assert_eq!(m.phase(), QuitPhase::Requested);
        assert_eq!(m.on_close_request(), CloseAction::Ignore);
        m.on_timer(g2);
        assert_eq!(m.phase(), QuitPhase::Unresponsive);
    }
}
