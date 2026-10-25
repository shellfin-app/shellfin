//! One session (connection tab): handle + supervisor task (§1.3, §5.8).
//!
//! Connect (all hops, auth, eager SFTP), `connected`, probe mode, close, loss detection, automatic
//! reconnect with backoff (`reconnecting{stage}` → `connected` | `disconnected{canRetry}`; a
//! `session_disconnect` stops it), manual retry via `session_reconnect_now`.
//!
//! Invariants:
//! - keys added to an agent are recorded in session-owned state as soon as they are added and
//!   removed on EVERY exit path: connect failure, close during connect, loss, close, panic;
//! - a remembered password is stored (keychain + `passwordSaved`) BEFORE `connected` is emitted;
//! - `session_reconnect_now` while connecting is ignored (no orphaned prompt);
//! - recents updates are applied in order;
//! - a supervisor panic is contained: `disconnected{internal}` + `closed`, agent keys removed,
//!   registry entry dropped (§1.3).

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};

use crate::error::{AppError, DisconnectKind, ErrorCode};
use crate::ipc::dto::{
    HopInfo, HopRef, PromptClosedReason, RecentEntry, RecentTarget, ReconnectStage, SessionEvent,
    SessionInfo, SessionState, TermEvent,
};
use crate::known_hosts::TrustOnceApprovals;
use crate::ssh::agent::{self, AgentEndpoint};
use crate::ssh::auth::HopSecrets;
use crate::ssh::connect::{self, AgentKeyRecord, ConnectCtx, Connection};
use crate::ssh::handler::HostKeyPin;
use crate::ssh::prompt::{EventSink, PromptBroker};
use crate::ssh::target::ResolvedTarget;
use crate::storage::Stores;
use crate::storage::file::now_ms;
use crate::terminal::TerminalSet;

/// One liveness ping (§5.7): the destination's keepalive request travels through every jump.
/// `false` on timeout or a dead transport.
async fn ping(conn: &Connection) -> bool {
    let Some(dest) = conn.hops.last() else {
        return false;
    };
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, dest.handle.send_ping()).await,
        Ok(Ok(()))
    )
}

/// How often the supervisor checks hop handles for loss (backstop to the `disconnected` notify).
const LOSS_POLL: Duration = Duration::from_millis(250);
/// `session_probe_all` (§5.7): one ping's budget, and the gap before the second try.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);
pub const PROBE_RETRY_GAP: Duration = Duration::from_secs(5);
/// Budget for a polite disconnect of a connection the probe declared dead.
const DEAD_DISCONNECT_TIMEOUT: Duration = Duration::from_millis(500);
/// A reconnected connection must stay up this long before the auto-reconnect attempt
/// counter starts again at 1 (a server that accepts and then drops at once ends the loop).
pub const STABLE_AFTER: Duration = Duration::from_secs(30);
/// At most one `connection_lost` OS notification per session in this window.
pub const LOST_NOTICE_GAP: Duration = Duration::from_secs(60);

/// Auto-reconnect attempts still counted after a connection that was up for `up`: all of them
/// unless it stayed up for [`STABLE_AFTER`].
fn attempts_carried(used: u32, up: Duration) -> u32 {
    if up >= STABLE_AFTER { 0 } else { used }
}

/// Whether a `connection_lost` notification may go out now.
fn lost_notice_due(last: Option<tokio::time::Instant>, now: tokio::time::Instant) -> bool {
    last.is_none_or(|t| now.duration_since(t) >= LOST_NOTICE_GAP)
}

pub enum SupervisorCmd {
    Close {
        last_dir: Option<String>,
        done: oneshot::Sender<()>,
    },
    ReconnectNow,
    /// `session_disconnect`: end the connection, keep the session (state `disconnected`).
    Disconnect,
    /// Two consecutive probe timeouts on this connection (§5.7) ⇒ treat it as lost.
    ProbeFailed(Arc<Connection>),
}

/// Process-wide dependencies of every session.
pub struct SessionDeps {
    pub stores: Arc<Stores>,
    pub approvals: Arc<TrustOnceApprovals>,
    pub ctx_template: ConnectCtxTemplate,
    /// Emits `recents:changed` with the full list.
    pub recents_changed: Arc<dyn Fn(Vec<RecentEntry>) + Send + Sync>,
    /// OS notifications (connection lost, transfers done).
    pub notifier: Arc<dyn crate::notify::Notifier>,
}

/// Opening order of sessions (startup restore keeps the tab order).
static OPENED_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// The parts of [`ConnectCtx`] that do not depend on the session.
#[derive(Clone)]
pub struct ConnectCtxTemplate {
    pub profile_root: Option<std::path::PathBuf>,
    pub home: std::path::PathBuf,
    pub user_ssh_dir: std::path::PathBuf,
    pub env: crate::sshconfig::EnvLookup,
    pub keychain_available: bool,
}

/// Removes the session from the manager's registry (idempotent).
pub type OnExit = Arc<dyn Fn(&str) + Send + Sync>;

/// A running session as seen by commands.
pub struct SessionHandle {
    pub id: String,
    pub label: String,
    pub recent: RecentTarget,
    pub broker: Arc<PromptBroker>,
    cmd: mpsc::UnboundedSender<SupervisorCmd>,
    /// The live connection (terminals and SFTP use it); `None` while not connected.
    pub connection: tokio::sync::RwLock<Option<Arc<Connection>>>,
    /// The session's terminals (§4.5).
    pub terminals: TerminalSet,
    /// Bandwidth limiters shared by this session's transfers.
    pub throttle: crate::sftp::throttle::SessionThrottle,
    /// The bookmark this session was opened from (`None` for config aliases and drafts).
    pub bookmark_id: Option<String>,
    /// A probe session (`probeOnly`): never restored, never notified.
    pub probe_only: bool,
    /// Process-wide opening order (startup restore).
    pub opened_seq: u64,
    live: Mutex<super::live::LiveBookmark>,
    /// Keys this session added to an agent (shared with the supervisor), so quit can remove
    /// them in a pass of its own before anything else (§9.2).
    agent_keys: AgentKeyRecord,
}

impl SessionHandle {
    /// Remove the keys this session added to an agent now (quit). Idempotent: the record
    /// is emptied, so the session's own close finds nothing left to do.
    pub async fn remove_agent_keys(&self) {
        remove_agent_keys(&self.agent_keys).await;
    }

    /// A bookmark of this session was saved: follow label, color and bandwidth overrides now
    /// (the running connection keeps its target) and tell the UI (`bookmarkUpdated`).
    pub fn apply_bookmark(
        &self,
        bookmark: &crate::ipc::dto::Bookmark,
        global: &crate::ipc::dto::TransferSettings,
    ) {
        let applied = self
            .live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .apply(bookmark);
        self.throttle.set_overrides(applied.overrides, global);
        self.broker.sink().send(SessionEvent::BookmarkUpdated {
            label: applied.label,
            color: applied.color,
            limit_overrides: crate::ipc::dto::LimitOverridesInfo {
                upload_kibps: applied.overrides.upload_kibps,
                download_kibps: applied.overrides.download_kibps,
            },
            needs_reconnect: applied.needs_reconnect,
        });
    }

    fn live(&self) -> super::live::LiveBookmark {
        self.live
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    pub fn reconnect_now(&self) {
        let _ = self.cmd.send(SupervisorCmd::ReconnectNow);
    }

    /// `session_disconnect`: close the SSH connection but keep the session entry (idempotent).
    pub fn disconnect(&self) {
        let _ = self.cmd.send(SupervisorCmd::Disconnect);
    }

    /// `session_probe_all` for this session (§5.7): ping through all hops with a 5 s budget;
    /// on a timeout wait 5 s and try once more; two consecutive timeouts ⇒ the supervisor treats
    /// the connection as lost. Returns `false` if loss was declared. A busy terminal cannot delay
    /// the ping: terminal readers never pause (§4.5).
    pub async fn probe(&self) -> bool {
        let Some(conn) = self.connection.read().await.clone() else {
            return true;
        };
        for attempt in 0..2 {
            if attempt > 0 {
                tokio::time::sleep(PROBE_RETRY_GAP).await;
            }
            if ping(&conn).await {
                return true;
            }
        }
        tracing::info!("probe: two consecutive ping timeouts; connection treated as lost");
        let _ = self.cmd.send(SupervisorCmd::ProbeFailed(conn));
        false
    }

    /// Close and wait until the supervisor finished (idempotent).
    pub async fn close(&self, last_dir: Option<String>) {
        let (done, wait) = oneshot::channel();
        if self
            .cmd
            .send(SupervisorCmd::Close { last_dir, done })
            .is_ok()
        {
            let _ = wait.await;
        }
    }
}

struct Supervisor {
    handle: Arc<SessionHandle>,
    deps: Arc<SessionDeps>,
    target: ResolvedTarget,
    probe_only: bool,
    secrets: Vec<HopSecrets>,
    pins: Vec<Option<HostKeyPin>>,
    rx: mpsc::UnboundedReceiver<SupervisorCmd>,
    recorded_connect: bool,
    agent_keys: AgentKeyRecord,
    on_exit: OnExit,
    /// When the last `connection_lost` notification went out (rate limit).
    last_lost_notice: Option<tokio::time::Instant>,
}

/// A created, not yet started session (the manager registers the handle before starting it).
pub struct PendingSession {
    pub handle: Arc<SessionHandle>,
    supervisor: Supervisor,
}

impl PendingSession {
    /// Spawn the supervisor task plus a watchdog that contains a supervisor panic.
    pub fn start(self) {
        let handle = self.handle.clone();
        let on_exit = self.supervisor.on_exit.clone();
        let agent_keys = self.supervisor.agent_keys.clone();
        let task = tokio::spawn(self.supervisor.run());
        tokio::spawn(contain_panic(task, handle, on_exit, agent_keys));
    }
}

/// If the supervisor task panicked: withdraw prompts, drop the connection, remove agent keys,
/// emit `disconnected{internal}` + `closed`, and remove the session from the registry.
async fn contain_panic(
    task: tokio::task::JoinHandle<()>,
    handle: Arc<SessionHandle>,
    on_exit: OnExit,
    agent_keys: AgentKeyRecord,
) {
    let Err(e) = task.await else { return };
    if !e.is_panic() {
        return;
    }
    tracing::error!("session supervisor panicked; session closed");
    handle.broker.cancel_pending(PromptClosedReason::Cancelled);
    remove_agent_keys(&agent_keys).await;
    handle
        .terminals
        .close_all(Some(TermEvent::Closed {
            error: Some(AppError::internal("session task panicked")),
        }))
        .await;
    // End the transport too (bounded); the stats ticker died with the task (its guard
    // aborts on drop) and `on_exit` cancels the session's transfers.
    let conn = handle.connection.write().await.take();
    if let Some(conn) = conn {
        conn.abandoned.store(true, Ordering::Relaxed);
        let _ = tokio::time::timeout(DEAD_DISCONNECT_TIMEOUT, conn.disconnect()).await;
    }
    let sink = handle.broker.sink();
    sink.send(SessionEvent::State {
        state: SessionState::Disconnected {
            error: Some(AppError::internal("session task panicked")),
            can_retry: false,
        },
    });
    sink.send(SessionEvent::State {
        state: SessionState::Closed,
    });
    on_exit(&handle.id);
}

/// Remove every key this session added to an agent, and clear the record.
async fn remove_agent_keys(record: &AgentKeyRecord) {
    let keys = std::mem::take(&mut *record.lock().unwrap_or_else(PoisonError::into_inner));
    let mut by_endpoint: HashMap<String, (AgentEndpoint, Vec<russh::keys::PublicKey>)> =
        HashMap::new();
    for (ep, key) in keys {
        by_endpoint
            .entry(ep.display())
            .or_insert_with(|| (ep.clone(), Vec::new()))
            .1
            .push(key);
    }
    for (_, (ep, keys)) in by_endpoint {
        // A wedged (but owner-verified) agent must not hold up close or quit.
        match tokio::time::timeout(agent::AGENT_OP_TIMEOUT, agent::connect_verified(&ep)).await {
            Ok(Ok(mut a)) => {
                for k in keys {
                    match tokio::time::timeout(
                        agent::AGENT_OP_TIMEOUT,
                        a.client.remove_identity(&k),
                    )
                    .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(e)) => {
                            tracing::warn!(error = %e, "could not remove a key from the agent");
                        }
                        Err(_) => {
                            tracing::warn!("agent did not answer while removing session keys");
                            break;
                        }
                    }
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "agent unavailable while removing session keys")
            }
            Err(_) => tracing::warn!("agent did not answer while removing session keys"),
        }
    }
}

/// Create a session. `on_exit(id)` removes it from the manager after `closed`.
pub fn create(
    id: String,
    target: ResolvedTarget,
    probe_only: bool,
    sink: Arc<dyn EventSink>,
    prompt_timeout: Duration,
    deps: Arc<SessionDeps>,
    on_exit: OnExit,
) -> PendingSession {
    let (cmd, rx) = mpsc::unbounded_channel();
    let broker = Arc::new(PromptBroker::new(sink, prompt_timeout));
    let handle = Arc::new(SessionHandle {
        id,
        label: target.label.clone(),
        recent: target.recent.clone(),
        broker,
        cmd,
        connection: tokio::sync::RwLock::new(None),
        terminals: TerminalSet::default(),
        throttle: crate::sftp::throttle::SessionThrottle::new(
            target.limits,
            &deps.stores.settings().transfers,
        ),
        bookmark_id: target.bookmark_id.clone(),
        probe_only,
        opened_seq: OPENED_SEQ.fetch_add(1, Ordering::Relaxed),
        live: Mutex::new(super::live::LiveBookmark::new(&target)),
        agent_keys: AgentKeyRecord::default(),
    });
    let hops = target.hops.len();
    let mut secrets: Vec<HopSecrets> = (0..hops).map(|_| HopSecrets::default()).collect();
    // §5.1 rule 4: a draft password goes to the destination's SessionSecrets.
    if let (Some(pw), Some(dest)) = (&target.draft_password, secrets.last_mut()) {
        dest.set_password(zeroize::Zeroizing::new(pw.expose().clone()));
    }
    let sup = Supervisor {
        handle: handle.clone(),
        deps,
        target,
        probe_only,
        secrets,
        pins: vec![None; hops],
        rx,
        recorded_connect: false,
        agent_keys: handle.agent_keys.clone(),
        on_exit,
        last_lost_notice: None,
    };
    PendingSession {
        handle,
        supervisor: sup,
    }
}

/// Outcome of one connect attempt (or what interrupted it).
enum Attempt {
    Done(Box<Result<Connection, AppError>>),
    Close(Option<String>, oneshot::Sender<()>),
    /// `session_disconnect` during an automatic reconnect attempt.
    Stopped,
}

/// `recv()` on the command channel: the supervisor's own [`SessionHandle`] holds a sender, so
/// the channel never closes while the supervisor runs. Should it ever, that is handled as a
/// close without `lastDir` (no separate, unreachable "gone" paths).
async fn next_cmd(rx: &mut mpsc::UnboundedReceiver<SupervisorCmd>) -> SupervisorCmd {
    rx.recv().await.unwrap_or_else(|| SupervisorCmd::Close {
        last_dir: None,
        done: oneshot::channel().0,
    })
}

/// The `userRequested` disconnect error.
fn user_disconnect() -> AppError {
    AppError::new(ErrorCode::Disconnected {
        reason: DisconnectKind::UserRequested,
        message: None,
    })
}

/// Drive a connect attempt against the command channel: `close` interrupts it, `reconnect_now`
/// is ignored — a second retry must not drop the attempt and orphan its open prompt.
/// With `stoppable` (an automatic reconnect), `session_disconnect` ends the attempt;
/// the caller withdraws its prompts.
async fn race_attempt(
    fut: impl std::future::Future<Output = Result<Connection, AppError>>,
    rx: &mut mpsc::UnboundedReceiver<SupervisorCmd>,
    stoppable: bool,
) -> Attempt {
    tokio::pin!(fut);
    loop {
        tokio::select! {
            res = &mut fut => return Attempt::Done(Box::new(res)),
            cmd = next_cmd(rx) => match cmd {
                SupervisorCmd::Close { last_dir, done } => return Attempt::Close(last_dir, done),
                SupervisorCmd::Disconnect if stoppable => return Attempt::Stopped,
                SupervisorCmd::ReconnectNow | SupervisorCmd::Disconnect | SupervisorCmd::ProbeFailed(_) => {}
            },
        }
    }
}

impl Supervisor {
    fn emit(&self, event: SessionEvent) {
        self.handle.broker.sink().send(event);
    }

    fn state(&self, state: SessionState) {
        self.emit(SessionEvent::State { state });
    }

    fn connect_ctx(&self) -> ConnectCtx {
        let t = &self.deps.ctx_template;
        ConnectCtx {
            agent_keys: self.agent_keys.clone(),
            broker: self.handle.broker.clone(),
            approvals: self.deps.approvals.clone(),
            profile_root: t.profile_root.clone(),
            home: t.home.clone(),
            user_ssh_dir: t.user_ssh_dir.clone(),
            env: t.env.clone(),
            keychain_available: t.keychain_available,
            remember_passwords: self.deps.stores.settings().connection.remember_passwords,
        }
    }

    /// Run one connect attempt; a close interrupts it, `reconnect_now` is ignored meanwhile.
    /// With `reconnect = Some((attempt, max, cause))` the hop stages are reported as
    /// `reconnecting{stage: connecting|authenticating, hop}` (§5.8: the overlay stays up), and a
    /// `session_disconnect` stops the attempt.
    async fn attempt(&mut self, reconnect: Option<(u32, u32, AppError)>) -> Attempt {
        let stoppable = reconnect.is_some();
        let ctx = self.connect_ctx();
        let sink = self.handle.broker.sink().clone();
        let on_state = move |s: SessionState| {
            let state = match (&reconnect, s) {
                (Some((attempt, max, cause)), SessionState::Connecting { hop }) => reconnecting(
                    ReconnectStage::Connecting,
                    Some(hop),
                    *attempt,
                    *max,
                    None,
                    cause,
                ),
                (Some((attempt, max, cause)), SessionState::Authenticating { hop }) => {
                    reconnecting(
                        ReconnectStage::Authenticating,
                        Some(hop),
                        *attempt,
                        *max,
                        None,
                        cause,
                    )
                }
                // A reconnect keeps showing its last hop stage until `connected`.
                (Some(_), SessionState::OpeningChannels { .. }) => return,
                (_, other) => other,
            };
            sink.send(SessionEvent::State { state });
        };
        let fut = connect::connect(
            &self.target,
            &ctx,
            &mut self.secrets,
            &mut self.pins,
            &on_state,
        );
        race_attempt(fut, &mut self.rx, stoppable).await
    }

    async fn run(mut self) {
        self.state(SessionState::Resolving);
        loop {
            let mut result = match self.attempt(None).await {
                Attempt::Done(result) => *result,
                Attempt::Close(last_dir, done) => {
                    self.close_from_attempt(last_dir, done).await;
                    return;
                }
                // Not stoppable: a first connect (or a manual retry) ignores Disconnect.
                Attempt::Stopped => Err(user_disconnect()),
            };

            // Connected ⇄ automatic reconnect (§5.8) until a failure ends it.
            // Attempts used by reconnects whose connection did not stay up for
            // `STABLE_AFTER` carry over, so a flapping server cannot loop forever.
            let mut used_attempts = 0u32;
            let failure = loop {
                let conn = match result {
                    Ok(conn) => conn,
                    Err(error) => break error,
                };
                let served_at = tokio::time::Instant::now();
                let served = self.serve(conn).await;
                used_attempts = attempts_carried(used_attempts, served_at.elapsed());
                let cause = match served {
                    Served::Lost(cause) => cause,
                    // Disconnect: no auto-reconnect, wait for `session_reconnect_now`.
                    Served::UserDisconnect(error) => break error,
                    Served::Close(conn, last_dir, done) => {
                        self.finish(conn, last_dir).await;
                        let _ = done.send(());
                        return;
                    }
                };
                match self.auto_reconnect(cause, used_attempts).await {
                    Auto::Connected(conn, attempt) => {
                        used_attempts = attempt;
                        result = Ok(*conn);
                    }
                    Auto::Failed(error) => break error,
                    Auto::Close(last_dir, done) => {
                        self.close_from_attempt(last_dir, done).await;
                        return;
                    }
                }
            };
            tracing::info!(code = ?failure.kind, "session disconnected");
            // Keys added for a connection that did not come up are removed now.
            remove_agent_keys(&self.agent_keys).await;
            let can_retry = !is_host_key_fatal(&failure);
            self.state(SessionState::Disconnected {
                error: Some(failure),
                can_retry,
            });

            // Disconnected: wait for retry or close.
            loop {
                match next_cmd(&mut self.rx).await {
                    SupervisorCmd::ReconnectNow => break,
                    SupervisorCmd::ProbeFailed(_) | SupervisorCmd::Disconnect => {}
                    SupervisorCmd::Close { last_dir, done } => {
                        self.finish(None, last_dir).await;
                        let _ = done.send(());
                        return;
                    }
                }
            }
        }
    }

    /// A close that interrupted a connect attempt (or a reconnect wait).
    async fn close_from_attempt(self, last_dir: Option<String>, done: oneshot::Sender<()>) {
        self.handle
            .broker
            .cancel_pending(PromptClosedReason::Cancelled);
        self.finish(None, last_dir).await;
        let _ = done.send(());
    }

    /// The `connected` phase of one connection: publish it, then wait for close or loss. On loss
    /// the connection is given up (terminals lost, prompts withdrawn, agent keys removed).
    async fn serve(&mut self, conn: Connection) -> Served {
        let conn = Arc::new(conn);
        let info = self.session_info(&conn);
        *self.handle.connection.write().await = Some(conn.clone());
        if let Some(w) = &conn.start_dir_warning {
            self.emit(SessionEvent::Warning { warning: w.clone() });
        }
        // The store is up to date when the UI hears `connected`.
        self.store_remembered_passwords(&conn).await;
        self.state(SessionState::Connected { info });
        if self.probe_only {
            let (done, _) = oneshot::channel();
            return Served::Close(Some(conn), None, done);
        }
        self.record_connected().await;
        // §5.8: shells that were running when the connection was lost come back (same size and
        // cwd), each announcing `reconnected` first.
        self.handle.terminals.respawn_lost(&conn);
        let stats = crate::session::stats::spawn_ticker(
            conn.traffic.clone(),
            self.handle.broker.sink().clone(),
        );
        let outcome = self.wait_connected(&conn).await;
        stats.abort();
        match outcome {
            Waited::Close(last_dir, done) => Served::Close(Some(conn), last_dir, done),
            Waited::UserDisconnect(error) => {
                // Terminals are marked lost (respawned on reconnect, the UI keeps its buffers);
                // SFTP dies with the connection.
                conn.abandoned.store(true, Ordering::Relaxed);
                self.handle.terminals.mark_lost();
                *self.handle.connection.write().await = None;
                self.handle
                    .broker
                    .cancel_pending(PromptClosedReason::Cancelled);
                let _ = tokio::time::timeout(DEAD_DISCONNECT_TIMEOUT, conn.disconnect()).await;
                remove_agent_keys(&self.agent_keys).await;
                Served::UserDisconnect(error)
            }
            Waited::Lost(error) => {
                conn.abandoned.store(true, Ordering::Relaxed);
                // One OS notification per unexpected loss (probe sessions never get
                // here: they close right after `connected`), at most one per `LOST_NOTICE_GAP`
                // (a flapping server would otherwise notify every cycle).
                let now = tokio::time::Instant::now();
                if lost_notice_due(self.last_lost_notice, now) {
                    self.last_lost_notice = Some(now);
                    self.deps
                        .notifier
                        .connection_lost(&self.handle.live().label);
                }
                self.handle.terminals.mark_lost();
                *self.handle.connection.write().await = None;
                if matches!(
                    error.kind,
                    ErrorCode::Disconnected {
                        reason: DisconnectKind::KeepaliveTimeout,
                        ..
                    }
                ) {
                    // Declared dead (keepalive or probe): the transport may still be half-open,
                    // so end it (bounded).
                    let _ = tokio::time::timeout(DEAD_DISCONNECT_TIMEOUT, conn.disconnect()).await;
                }
                self.handle
                    .broker
                    .cancel_pending(PromptClosedReason::ConnectionLost);
                remove_agent_keys(&self.agent_keys).await;
                Served::Lost(error)
            }
        }
    }

    /// §5.8 auto-reconnect after an unexpected loss: up to `maxAttempts` attempts with backoff
    /// 1, 2, 4, 8, 16 s (±20 %, cap 30 s), reported as `reconnecting{waiting}` then
    /// `reconnecting{connecting|authenticating, hop}`. Host-key pins and the session's cached
    /// secrets apply (`connect` re-reads known_hosts, so revoked/changed keys are caught); SFTP
    /// is re-initialised by `connect`. Fatal errors stop at once; `cause` is the latest error.
    /// `used` attempts were already spent on this unstable streak: numbering continues
    /// after them, and none are left when `used >= maxAttempts`.
    async fn auto_reconnect(&mut self, cause: AppError, used: u32) -> Auto {
        let settings = self.deps.stores.settings().connection.reconnect;
        if !settings.enabled || settings.max_attempts == 0 {
            return Auto::Failed(cause);
        }
        let max = settings.max_attempts;
        let mut cause = cause;
        for attempt in used.saturating_add(1)..=max {
            let delay = backoff(attempt, jitter());
            let next = now_ms().saturating_add(i64::try_from(delay.as_millis()).unwrap_or(0));
            self.state(reconnecting(
                ReconnectStage::Waiting,
                None,
                attempt,
                max,
                Some(next),
                &cause,
            ));
            match self.wait_backoff(delay).await {
                Backoff::Go => {}
                Backoff::Close(last_dir, done) => return Auto::Close(last_dir, done),
                // Disconnect while waiting ends the automatic reconnect.
                Backoff::Stopped => return Auto::Failed(user_disconnect()),
            }
            match self.attempt(Some((attempt, max, cause.clone()))).await {
                Attempt::Done(result) => match *result {
                    Ok(conn) => {
                        tracing::info!(attempt, "reconnected");
                        return Auto::Connected(Box::new(conn), attempt);
                    }
                    Err(e) => {
                        tracing::info!(attempt, code = ?e.kind, "reconnect attempt failed");
                        remove_agent_keys(&self.agent_keys).await;
                        if !is_transport_error(&e) {
                            return Auto::Failed(e);
                        }
                        cause = e;
                    }
                },
                Attempt::Close(last_dir, done) => return Auto::Close(last_dir, done),
                Attempt::Stopped => {
                    // The dropped attempt's prompt (if any) is withdrawn and keys it
                    // added to the agent are removed by the caller's failure path.
                    self.handle
                        .broker
                        .cancel_pending(PromptClosedReason::Cancelled);
                    return Auto::Failed(user_disconnect());
                }
            }
        }
        Auto::Failed(cause)
    }

    /// Wait out a backoff delay; `reconnect_now` cuts it short, `close` ends the session,
    /// `session_disconnect` stops reconnecting.
    async fn wait_backoff(&mut self, delay: Duration) -> Backoff {
        let sleep = tokio::time::sleep(delay);
        tokio::pin!(sleep);
        loop {
            tokio::select! {
                () = &mut sleep => return Backoff::Go,
                cmd = next_cmd(&mut self.rx) => match cmd {
                    SupervisorCmd::ReconnectNow => return Backoff::Go,
                    SupervisorCmd::Disconnect => return Backoff::Stopped,
                    SupervisorCmd::ProbeFailed(_) => {}
                    SupervisorCmd::Close { last_dir, done } => {
                        return Backoff::Close(last_dir, done);
                    }
                },
            }
        }
    }

    fn session_info(&self, conn: &Connection) -> SessionInfo {
        let live = self.handle.live();
        SessionInfo {
            session_id: self.handle.id.clone(),
            label: live.label,
            hops: conn
                .hops
                .iter()
                .map(|h| h.info.clone())
                .collect::<Vec<HopInfo>>(),
            auth: conn.auth.clone(),
            sftp_available: conn.sftp.is_some(),
            home_dir: conn.home_dir.clone(),
            start_dir: conn.start_dir.clone(),
            remote_path_style: conn.remote_path_style,
            remote_identity: conn.remote_identity.clone(),
            limit_overrides: crate::ipc::dto::LimitOverridesInfo {
                upload_kibps: live.overrides.upload_kibps,
                download_kibps: live.overrides.download_kibps,
            },
            color: live.color,
        }
    }

    async fn wait_connected(&mut self, conn: &Arc<Connection>) -> Waited {
        loop {
            tokio::select! {
                cmd = next_cmd(&mut self.rx) => match cmd {
                    SupervisorCmd::Close { last_dir, done } => return Waited::Close(last_dir, done),
                    SupervisorCmd::ReconnectNow => {} // connected: nothing to do
                    SupervisorCmd::Disconnect => return Waited::UserDisconnect(user_disconnect()),
                    // A probe of an earlier connection must not end this one.
                    SupervisorCmd::ProbeFailed(probed) if Arc::ptr_eq(&probed, conn) => {
                        return Waited::Lost(AppError::new(ErrorCode::Disconnected {
                            reason: DisconnectKind::KeepaliveTimeout,
                            message: None,
                        }));
                    }
                    SupervisorCmd::ProbeFailed(_) => {}
                },
                () = tokio::time::sleep(LOSS_POLL) => {
                    if let Some(error) = loss_of(conn) {
                        return Waited::Lost(error);
                    }
                }
            }
        }
    }

    /// First `connected` of the session: upsert recents (awaited, so a following close cannot
    /// overtake it).
    async fn record_connected(&mut self) {
        if self.recorded_connect {
            return;
        }
        self.recorded_connect = true;
        let Some(dest) = self.target.destination() else {
            return;
        };
        let (recent, label, uah, via) = (
            self.target.recent.clone(),
            self.target.label.clone(),
            dest.user_at_host(),
            self.target.via(),
        );
        let stores = self.deps.stores.clone();
        let notify = self.deps.recents_changed.clone();
        let _ = tokio::task::spawn_blocking(move || {
            match stores
                .lock_recents()
                .upsert_connected(recent, label, uah, via, now_ms())
            {
                Ok(list) => notify(list),
                Err(e) => tracing::warn!(code = ?e.kind, "could not record recent connection"),
            }
        })
        .await;
    }

    /// §5.6: "success && remember ⇒ keychain set with binding (bookmark: passwordSaved = true)".
    async fn store_remembered_passwords(&self, conn: &Connection) {
        // The setting may have been turned off while the prompt was open.
        if !self.deps.stores.settings().connection.remember_passwords {
            return;
        }
        for (index, password) in &conn.remember_passwords {
            let Some(hop) = self.target.hops.get(*index) else {
                continue;
            };
            let Some(keychain) = hop.keychain.clone() else {
                continue;
            };
            let binding = hop.binding();
            let password = password.clone();
            let stores = self.deps.stores.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::secrets::set_password(&keychain.account, &password)?;
                if let Some(id) = keychain.account.strip_prefix("bookmark:") {
                    let saved =
                        stores
                            .lock_bookmarks()
                            .set_password_saved(id, Some(binding), now_ms());
                    if let Err(e) = saved {
                        // No keychain entry without a binding in the store.
                        let _ = crate::secrets::delete_password(&keychain.account);
                        return Err(e);
                    }
                }
                Ok::<(), AppError>(())
            })
            .await;
            match result {
                Ok(Ok(())) => {}
                Ok(Err(e)) => self.emit(SessionEvent::Warning { warning: e }),
                // Never `%e`: a `JoinError`'s text carries the panic payload.
                Err(e) => tracing::warn!(panicked = e.is_panic(), "keychain task failed"),
            }
        }
    }

    /// `session_close` / probe end (§5.8): cancel prompts, remove agent keys, disconnect hops in
    /// reverse, record recents, emit `closed` (+ `recents:changed`), drop secrets and pins.
    async fn finish(mut self, conn: Option<Arc<Connection>>, last_dir: Option<String>) {
        self.handle
            .broker
            .cancel_pending(PromptClosedReason::Cancelled);
        // (§9.2): agent keys first, so nothing that can stall below (terminal closes on a
        // dead link, the disconnect) can keep them in the user's agent.
        remove_agent_keys(&self.agent_keys).await;
        // Terminals next (their channels close while the transport is still up); the count of
        // shells still running goes into recents (§7.3 `terminalCount`).
        let running_terminals = self
            .handle
            .terminals
            .close_all(Some(TermEvent::Closed { error: None }))
            .await;
        if let Some(conn) = &conn {
            conn.abandoned.store(true, Ordering::Relaxed);
            // Bounded like the loss paths; a stalled transport must not keep the tab.
            let _ = tokio::time::timeout(DEAD_DISCONNECT_TIMEOUT, conn.disconnect()).await;
        }
        *self.handle.connection.write().await = None;
        if !self.probe_only && self.recorded_connect {
            let stores = self.deps.stores.clone();
            let notify = self.deps.recents_changed.clone();
            let recent = self.target.recent.clone();
            let _ = tokio::task::spawn_blocking(move || {
                match stores.lock_recents().record_close(
                    &recent,
                    last_dir,
                    running_terminals,
                    now_ms(),
                ) {
                    Ok(list) => notify(list),
                    Err(e) => tracing::warn!(code = ?e.kind, "could not record session close"),
                }
            })
            .await;
        }
        self.secrets.clear();
        self.pins.clear();
        self.state(SessionState::Closed);
        (self.on_exit)(&self.handle.id);
    }
}

enum Waited {
    Close(Option<String>, oneshot::Sender<()>),
    Lost(AppError),
    UserDisconnect(AppError),
}

/// How a `connected` phase ended.
enum Served {
    Close(Option<Arc<Connection>>, Option<String>, oneshot::Sender<()>),
    Lost(AppError),
    UserDisconnect(AppError),
}

/// How automatic reconnecting ended.
enum Auto {
    /// The new connection and the attempt number that made it.
    Connected(Box<Connection>, u32),
    /// Gave up (or `session_disconnect`): the session goes to `disconnected`.
    Failed(AppError),
    Close(Option<String>, oneshot::Sender<()>),
}

enum Backoff {
    Go,
    Close(Option<String>, oneshot::Sender<()>),
    /// `session_disconnect` while waiting.
    Stopped,
}

fn reconnecting(
    stage: ReconnectStage,
    hop: Option<HopRef>,
    attempt: u32,
    max_attempts: u32,
    next_attempt_at_ms: Option<i64>,
    cause: &AppError,
) -> SessionState {
    SessionState::Reconnecting {
        stage,
        hop,
        attempt,
        max_attempts,
        next_attempt_at_ms,
        cause: cause.clone(),
    }
}

/// §5.8: auto-reconnect continues only after transport errors.
pub fn is_transport_error(e: &AppError) -> bool {
    matches!(
        e.kind,
        ErrorCode::Disconnected { .. }
            | ErrorCode::ConnectRefused { .. }
            | ErrorCode::ConnectTimeout { .. }
            | ErrorCode::NetworkUnreachable { .. }
            | ErrorCode::DnsFailed { .. }
            | ErrorCode::JumpChannelFailed { .. }
    )
}

/// Host-key failures are never retried (`canRetry: false`, §5.3/§9.3).
pub fn is_host_key_fatal(e: &AppError) -> bool {
    matches!(
        e.kind,
        ErrorCode::HostKeyChanged { .. }
            | ErrorCode::HostKeyRevoked { .. }
            | ErrorCode::HostKeyCaManaged { .. }
            | ErrorCode::HostKeyMismatchOnReconnect { .. }
    )
}

/// Backoff before attempt `attempt` (1-based): 1, 2, 4, 8, 16, 30, … s, scaled by
/// `0.8 + 0.4·r` (±20 %, `r ∈ [0, 1)`), capped at 30 s (§5.8).
pub fn backoff(attempt: u32, r: f64) -> Duration {
    let exp = i32::try_from(attempt.saturating_sub(1).min(5)).unwrap_or(5);
    let base = 2f64.powi(exp).min(MAX_BACKOFF_SECS);
    let jittered = base * (0.8 + 0.4 * r.clamp(0.0, 1.0));
    Duration::from_secs_f64(jittered.min(MAX_BACKOFF_SECS))
}

const MAX_BACKOFF_SECS: f64 = 30.0;

/// A uniform value in `[0, 1)` for the backoff jitter.
fn jitter() -> f64 {
    use ring::rand::SecureRandom;
    let mut b = [0u8; 2];
    if ring::rand::SystemRandom::new().fill(&mut b).is_err() {
        return 0.5;
    }
    f64::from(u16::from_le_bytes(b)) / 65_536.0
}

/// If any hop's connection ended: `disconnected{reason}` (a lost jump ⇒ `jumpHostLost`).
fn loss_of(conn: &Connection) -> Option<AppError> {
    conn.loss()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::ssh::prompt::test_support::RecordingSink;

    fn deps(dir: &std::path::Path) -> Arc<SessionDeps> {
        let (stores, _) = Stores::open(dir);
        Arc::new(SessionDeps {
            stores: Arc::new(stores),
            approvals: Arc::default(),
            ctx_template: ConnectCtxTemplate {
                profile_root: None,
                home: dir.to_path_buf(),
                user_ssh_dir: dir.join(".ssh"),
                env: Arc::new(|_| None),
                keychain_available: false,
            },
            recents_changed: Arc::new(|_| {}),
            notifier: Arc::new(crate::notify::NoopNotifier),
        })
    }

    fn empty_target() -> ResolvedTarget {
        ResolvedTarget {
            label: "x".into(),
            recent: RecentTarget::Quick {
                spec: "alice@example.com".into(),
            },
            hops: vec![],
            start_dir: None,
            bookmark_id: None,
            bookmark: None,
            draft_password: None,
            limits: crate::sftp::throttle::LimitOverrides::default(),
        }
    }

    /// §5.8: 1, 2, 4, 8, 16 s (±20 %), cap 30 s.
    #[test]
    fn backoff_schedule() {
        let secs = |attempt, r| backoff(attempt, r).as_secs_f64();
        let nominal: Vec<f64> = (1..=7).map(|a| secs(a, 0.5)).collect();
        assert_eq!(nominal, vec![1.0, 2.0, 4.0, 8.0, 16.0, 30.0, 30.0]);
        assert!((secs(1, 0.0) - 0.8).abs() < 1e-9);
        assert!((secs(5, 0.999_99) - 19.2).abs() < 1e-3);
        assert!(secs(6, 0.999) <= 30.0);
        assert!(secs(40, 0.0) >= 24.0 - 1e-9);
        for _ in 0..100 {
            let r = jitter();
            assert!((0.0..1.0).contains(&r));
        }
    }

    /// Attempts carry over unless the connection stayed up for `STABLE_AFTER`; with all
    /// attempts used, auto-reconnect gives up at once; `connection_lost` is rate-limited.
    #[tokio::test(start_paused = true)]
    async fn flapping_connections_exhaust_the_attempts() {
        assert_eq!(attempts_carried(3, Duration::from_secs(2)), 3);
        assert_eq!(attempts_carried(3, STABLE_AFTER), 0);
        let t0 = tokio::time::Instant::now();
        assert!(lost_notice_due(None, t0));
        assert!(!lost_notice_due(Some(t0), t0 + Duration::from_secs(5)));
        assert!(lost_notice_due(Some(t0), t0 + LOST_NOTICE_GAP));

        let dir = tempfile::tempdir().unwrap();
        let mut pending = create(
            "s1".into(),
            empty_target(),
            false,
            Arc::new(RecordingSink::default()),
            Duration::from_secs(5),
            deps(dir.path()),
            Arc::new(|_| {}),
        );
        let max = pending
            .supervisor
            .deps
            .stores
            .settings()
            .connection
            .reconnect
            .max_attempts;
        let cause = AppError::new(ErrorCode::Disconnected {
            reason: DisconnectKind::ServerClosed,
            message: None,
        });
        let started = tokio::time::Instant::now();
        let out = pending.supervisor.auto_reconnect(cause, max).await;
        assert!(matches!(out, Auto::Failed(_)));
        assert_eq!(started.elapsed(), Duration::ZERO, "no backoff, no attempt");
    }

    /// `session_reconnect_now` during `reconnecting{waiting}` starts the attempt at once; a close
    /// during the wait ends it.
    #[tokio::test(start_paused = true)]
    async fn reconnect_now_cuts_the_backoff_short() {
        let dir = tempfile::tempdir().unwrap();
        let mut pending = create(
            "s1".into(),
            empty_target(),
            false,
            Arc::new(RecordingSink::default()),
            Duration::from_secs(5),
            deps(dir.path()),
            Arc::new(|_| {}),
        );
        let handle = pending.handle.clone();
        let started = tokio::time::Instant::now();
        handle.reconnect_now();
        let go = pending
            .supervisor
            .wait_backoff(Duration::from_secs(30))
            .await;
        assert!(matches!(go, Backoff::Go));
        assert!(started.elapsed() < Duration::from_secs(1));
        // Without a command the full delay passes (paused clock auto-advances).
        let go = pending
            .supervisor
            .wait_backoff(Duration::from_secs(16))
            .await;
        assert!(matches!(go, Backoff::Go));
        assert!(started.elapsed() >= Duration::from_secs(16));
        let closer = tokio::spawn(async move { handle.close(None).await });
        let closed = pending
            .supervisor
            .wait_backoff(Duration::from_secs(30))
            .await;
        match closed {
            Backoff::Close(None, done) => {
                let _ = done.send(());
            }
            _ => panic!("expected close"),
        }
        closer.await.unwrap();
    }

    /// `session_disconnect` while auto-reconnect waits ends it as a user disconnect
    /// (retryable), and stops a running reconnect attempt.
    #[tokio::test(start_paused = true)]
    async fn disconnect_stops_a_pending_auto_reconnect() {
        let dir = tempfile::tempdir().unwrap();
        let mut pending = create(
            "s1".into(),
            empty_target(),
            false,
            Arc::new(RecordingSink::default()),
            Duration::from_secs(5),
            deps(dir.path()),
            Arc::new(|_| {}),
        );
        let started = tokio::time::Instant::now();
        pending.handle.disconnect();
        let stopped = pending
            .supervisor
            .wait_backoff(Duration::from_secs(30))
            .await;
        assert!(matches!(stopped, Backoff::Stopped));
        assert!(started.elapsed() < Duration::from_secs(1));

        // Through auto_reconnect: Disconnect during the wait ⇒ Failed(userRequested), at once.
        pending.handle.disconnect();
        let cause = AppError::new(ErrorCode::Disconnected {
            reason: DisconnectKind::ServerClosed,
            message: None,
        });
        match pending.supervisor.auto_reconnect(cause, 0).await {
            Auto::Failed(AppError {
                kind:
                    ErrorCode::Disconnected {
                        reason: DisconnectKind::UserRequested,
                        ..
                    },
                ..
            }) => {}
            _ => panic!("expected a user disconnect"),
        }
        assert!(started.elapsed() < Duration::from_secs(1));

        // A stoppable attempt that never finishes ends on Disconnect.
        let (tx, mut rx) = mpsc::unbounded_channel();
        tx.send(SupervisorCmd::ReconnectNow).unwrap();
        tx.send(SupervisorCmd::Disconnect).unwrap();
        let never = std::future::pending::<Result<Connection, AppError>>();
        assert!(matches!(
            race_attempt(never, &mut rx, true).await,
            Attempt::Stopped
        ));
    }

    /// §5.8 fatal set: only transport errors are retried automatically; host-key failures are
    /// never retryable.
    #[test]
    fn transport_and_fatal_errors() {
        let e = |k| AppError::new(k);
        let transport = [
            ErrorCode::Disconnected {
                reason: DisconnectKind::KeepaliveTimeout,
                message: None,
            },
            ErrorCode::ConnectRefused {
                host: "h".into(),
                port: 22,
            },
            ErrorCode::ConnectTimeout {
                host: "h".into(),
                port: 22,
                seconds: 3,
            },
            ErrorCode::NetworkUnreachable { host: "h".into() },
            ErrorCode::DnsFailed { host: "h".into() },
            ErrorCode::JumpChannelFailed {
                via: "b".into(),
                host: "h".into(),
                port: 22,
            },
        ];
        for k in transport {
            assert!(is_transport_error(&e(k.clone())), "{k:?}");
            assert!(!is_host_key_fatal(&e(k)));
        }
        let mismatch = ErrorCode::HostKeyMismatchOnReconnect {
            host: "h".into(),
            port: 22,
            expected: "a".into(),
            actual: "b".into(),
        };
        let revoked = ErrorCode::HostKeyRevoked {
            host: "h".into(),
            port: 22,
            fingerprint: "f".into(),
        };
        for k in [mismatch, revoked] {
            assert!(!is_transport_error(&e(k.clone())));
            assert!(is_host_key_fatal(&e(k)));
        }
        for k in [
            ErrorCode::Cancelled,
            ErrorCode::TooManyAuthFailures,
            ErrorCode::PromptTimeout,
            ErrorCode::KexFailed,
            ErrorCode::AuthFailed {
                user: "u".into(),
                host: "h".into(),
                tried: vec![],
                server_methods: vec![],
            },
        ] {
            assert!(!is_transport_error(&e(k.clone())), "{k:?}");
            assert!(!is_host_key_fatal(&e(k)));
        }
    }

    /// A panicking supervisor still yields `disconnected{internal}` + `closed` and the
    /// registry entry is removed.
    #[tokio::test]
    async fn supervisor_panic_is_contained() {
        let dir = tempfile::tempdir().unwrap();
        let sink = Arc::new(RecordingSink::default());
        let exits = Arc::new(AtomicUsize::new(0));
        let e2 = exits.clone();
        let pending = create(
            "s1".into(),
            empty_target(),
            false,
            sink.clone(),
            Duration::from_secs(5),
            deps(dir.path()),
            Arc::new(move |id| {
                assert_eq!(id, "s1");
                e2.fetch_add(1, Ordering::SeqCst);
            }),
        );
        let handle = pending.handle.clone();
        let task = tokio::spawn(async { panic!("injected supervisor panic") });
        contain_panic(
            task,
            handle,
            pending.supervisor.on_exit.clone(),
            Arc::default(),
        )
        .await;
        assert_eq!(exits.load(Ordering::SeqCst), 1);
        let events = sink.events.lock().unwrap();
        let states: Vec<&SessionState> = events
            .iter()
            .filter_map(|e| match e {
                SessionEvent::State { state } => Some(state),
                _ => None,
            })
            .collect();
        assert!(matches!(
            states[0],
            SessionState::Disconnected {
                error: Some(AppError {
                    kind: ErrorCode::Internal,
                    ..
                }),
                can_retry: false
            }
        ));
        assert!(matches!(states[1], SessionState::Closed));
    }

    /// `reconnect_now` while connecting neither restarts nor drops the attempt (whose
    /// prompt stays answerable); a close then interrupts it.
    #[tokio::test]
    async fn reconnect_now_while_connecting_keeps_the_prompt() {
        use crate::ipc::dto::{HopRef, Prompt, PromptAnswer};
        let sink = Arc::new(RecordingSink::default());
        let broker = Arc::new(PromptBroker::new(sink.clone(), Duration::from_secs(30)));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let polled = Arc::new(Mutex::new(0usize));
        let b = broker.clone();
        let p = polled.clone();
        // A stand-in connect attempt that waits on a prompt.
        let attempt = async move {
            *p.lock().unwrap() += 1;
            let _ = b
                .ask(|id| Prompt::KeyboardInteractive {
                    id,
                    hop: HopRef {
                        index: 0,
                        label: "h".into(),
                        user: "alice".into(),
                        host: "example.com".into(),
                        port: 22,
                    },
                    name: String::new(),
                    instructions: String::new(),
                    fields: vec![],
                })
                .await;
            Err(AppError::new(ErrorCode::Cancelled))
        };
        let driver = tokio::spawn(async move { race_attempt(attempt, &mut rx, false).await });
        while !broker.has_pending() {
            tokio::task::yield_now().await;
        }
        tx.send(SupervisorCmd::ReconnectNow).unwrap();
        tx.send(SupervisorCmd::ReconnectNow).unwrap();
        // A first connect is not stopped by Disconnect either (only automatic reconnects are).
        tx.send(SupervisorCmd::Disconnect).unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(broker.has_pending(), "the prompt must survive retry clicks");
        assert_eq!(
            *polled.lock().unwrap(),
            1,
            "the attempt must not be restarted"
        );
        // The prompt is still answerable (no promptNotFound).
        let id = sink.prompt_ids().pop().unwrap();
        broker.answer(&id, PromptAnswer::Cancel).unwrap();
        assert!(matches!(driver.await.unwrap(), Attempt::Done(r) if r.is_err()));
    }
}
