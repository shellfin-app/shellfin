//! Session registry (§3 `session::manager`): `session_open` resolves the target (command
//! errors), registers a session and starts its supervisor; prompts are answered directly through
//! the session's broker; `session_close` is idempotent.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError, RwLock};
use std::time::Duration;

use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::{OpenSessionRequest, PromptAnswer};
use crate::sftp::transfer::TransferRegistry;
use crate::ssh::prompt::EventSink;
use crate::ssh::target::{self, ResolveInput};
use crate::sshconfig::service::SshConfigService;

use super::session::{self, SessionDeps, SessionHandle};

type Registry = Arc<RwLock<HashMap<String, Arc<SessionHandle>>>>;

/// Most bookmarks recorded for startup restore.
pub const MAX_RESTORE: usize = 64;

/// `(opened_seq, bookmark id)` → ids in opening order, first occurrence wins, capped.
fn restore_order(open: &mut [(u64, String)]) -> Vec<String> {
    open.sort_by_key(|(seq, _)| *seq);
    let mut ids: Vec<String> = Vec::new();
    for (_, id) in open.iter() {
        if ids.len() >= MAX_RESTORE {
            break;
        }
        if !ids.contains(id) {
            ids.push(id.clone());
        }
    }
    ids
}

/// Shortest time between the starts of two `session_probe_all` rounds in the backend
/// (the frontend already waits 10 s; this bounds a misbehaving or compromised WebView).
pub const PROBE_MIN_GAP: Duration = Duration::from_secs(5);

/// Rate limit of `session_probe_all`: one round at a time, rounds at least [`PROBE_MIN_GAP`]
/// apart.
#[derive(Debug, Default)]
struct ProbeGate {
    running: bool,
    last_start: Option<tokio::time::Instant>,
}

impl ProbeGate {
    /// Start a round at `now` if allowed.
    fn try_begin(&mut self, now: tokio::time::Instant) -> bool {
        if self.running || self.last_start.is_some_and(|t| now < t + PROBE_MIN_GAP) {
            return false;
        }
        self.running = true;
        self.last_start = Some(now);
        true
    }
}

/// A running probe round; ends (lets the next one start) when dropped.
pub struct ProbeRound(Arc<Mutex<ProbeGate>>);

impl Drop for ProbeRound {
    fn drop(&mut self) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .running = false;
    }
}

pub struct SessionManager {
    deps: Arc<SessionDeps>,
    ssh_config: Arc<SshConfigService>,
    sessions: Registry,
    /// Transfers of all sessions (§6.6); cancelled per session on `session_close`.
    transfers: Arc<TransferRegistry>,
    probe_gate: Arc<Mutex<ProbeGate>>,
}

impl SessionManager {
    pub fn new(deps: Arc<SessionDeps>, ssh_config: Arc<SshConfigService>) -> Self {
        let transfers = Arc::new(TransferRegistry::default());
        transfers.set_max_active(
            usize::try_from(deps.stores.settings().transfers.max_concurrent)
                .unwrap_or(crate::sftp::transfer::MAX_ACTIVE_PER_SESSION),
        );
        transfers.set_notifier(deps.notifier.clone());
        transfers.set_preserve_timestamps(deps.stores.settings().transfers.preserve_timestamps);
        Self {
            deps,
            ssh_config,
            sessions: Arc::new(RwLock::new(HashMap::new())),
            transfers,
            probe_gate: Arc::default(),
        }
    }

    /// Start a `session_probe_all` round, or `None` while one runs or the last one
    /// started less than [`PROBE_MIN_GAP`] ago. Keep the returned guard for the round.
    pub fn begin_probe_round(&self) -> Option<ProbeRound> {
        let mut gate = self
            .probe_gate
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        gate.try_begin(tokio::time::Instant::now())
            .then(|| ProbeRound(self.probe_gate.clone()))
    }

    pub fn transfers(&self) -> &Arc<TransferRegistry> {
        &self.transfers
    }

    /// Settings snapshot (limits for SFTP reads, §6.3).
    pub fn settings(&self) -> crate::ipc::dto::Settings {
        self.deps.stores.settings()
    }

    /// Apply changed settings to open sessions (bandwidth limits take effect on the next
    /// chunk of every running transfer).
    pub fn apply_settings(&self, settings: &crate::ipc::dto::Settings) {
        self.transfers.set_max_active(
            usize::try_from(settings.transfers.max_concurrent)
                .unwrap_or(crate::sftp::transfer::MAX_ACTIVE_PER_SESSION),
        );
        self.transfers
            .set_preserve_timestamps(settings.transfers.preserve_timestamps);
        let sessions: Vec<Arc<SessionHandle>> = self
            .sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for s in sessions {
            s.throttle.apply_global(&settings.transfers);
        }
    }

    /// `bookmark_save` of a bookmark: update every open session that was opened from it
    /// (live limits, label, color; `needsReconnect` for connection-relevant edits).
    pub fn apply_bookmark(&self, bookmark: &crate::ipc::dto::Bookmark) {
        let sessions: Vec<Arc<SessionHandle>> = self
            .sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|s| s.bookmark_id.as_deref() == Some(bookmark.id.as_str()))
            .cloned()
            .collect();
        if sessions.is_empty() {
            return;
        }
        let global = self.deps.stores.settings().transfers;
        for s in sessions {
            s.apply_bookmark(bookmark, &global);
        }
    }

    /// The session, or `sessionNotFound`.
    pub fn session(&self, id: &str) -> Result<Arc<SessionHandle>, AppError> {
        self.get(id).ok_or_else(|| {
            AppError::new(ErrorCode::SessionNotFound {
                session_id: id.to_owned(),
            })
        })
    }

    /// Resolve and start a session; returns the session id immediately (connecting runs in the
    /// background and reports through `sink`).
    pub async fn open(
        &self,
        req: OpenSessionRequest,
        sink: Arc<dyn EventSink>,
    ) -> Result<String, AppError> {
        let stores = self.deps.stores.clone();
        let service = self.ssh_config.clone();
        let keychain_available = self.deps.ctx_template.keychain_available;
        // A panic is caught on the blocking thread (the `JoinError` text would carry its
        // payload); `session_open` fills in its name.
        let (resolved, prompt_timeout) = tokio::task::spawn_blocking(move || {
            crate::error::catch_panic("", || {
                let config = service.current();
                let settings = stores.settings();
                let bookmarks = stores.lock_bookmarks();
                let input = ResolveInput {
                    config: &config,
                    env: service.env(),
                    bookmarks: &*bookmarks,
                    settings: &settings,
                    keychain_available,
                };
                let resolved = target::resolve(req.target, &input)?;
                Ok::<_, AppError>((
                    resolved,
                    Duration::from_secs(u64::from(settings.connection.prompt_timeout_secs)),
                ))
            })
        })
        .await
        .map_err(|e| crate::error::join_failed(e.is_panic(), ""))???;

        let id = uuid::Uuid::new_v4().to_string();
        let registry = self.sessions.clone();
        let transfers = self.transfers.clone();
        let on_exit: session::OnExit = Arc::new(move |id: &str| {
            registry
                .write()
                .unwrap_or_else(PoisonError::into_inner)
                .remove(id);
            // A session that ended without `session_close` (supervisor panic) must not
            // leave transfers running; after a normal close there are none left (no-op).
            transfers.signal_cancel_session(id);
        });
        let pending = session::create(
            id.clone(),
            resolved,
            req.probe_only,
            sink,
            prompt_timeout,
            self.deps.clone(),
            on_exit,
        );
        self.sessions
            .write()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.clone(), pending.handle.clone());
        pending.start();
        Ok(id)
    }

    pub fn get(&self, id: &str) -> Option<Arc<SessionHandle>> {
        self.sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
            .cloned()
    }

    /// Startup restore: the bookmark ids of the open, non-probe sessions in
    /// opening order, without duplicates, at most [`MAX_RESTORE`].
    pub fn open_bookmark_ids(&self) -> Vec<String> {
        let mut open: Vec<(u64, String)> = self
            .sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .filter(|s| !s.probe_only)
            .filter_map(|s| s.bookmark_id.clone().map(|id| (s.opened_seq, id)))
            .collect();
        restore_order(&mut open)
    }

    pub fn ids(&self) -> Vec<String> {
        self.sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// The session that owns terminal `term_id`, and the terminal.
    pub fn find_terminal(
        &self,
        term_id: &str,
    ) -> Option<(Arc<SessionHandle>, Arc<crate::terminal::Terminal>)> {
        let sessions: Vec<Arc<SessionHandle>> = self
            .sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        sessions
            .into_iter()
            .find_map(|s| s.terminals.get(term_id).map(|t| (s, t)))
    }

    /// Like [`Self::find_terminal`], or `terminalNotFound`.
    pub fn terminal(
        &self,
        term_id: &str,
    ) -> Result<(Arc<SessionHandle>, Arc<crate::terminal::Terminal>), AppError> {
        self.find_terminal(term_id).ok_or_else(|| {
            AppError::new(ErrorCode::TerminalNotFound {
                term_id: term_id.to_owned(),
            })
        })
    }

    /// `session_probe_all` (§5.7): probe every connected session concurrently; returns the ids
    /// of sessions declared lost.
    pub async fn probe_all(&self) -> Vec<String> {
        let sessions: Vec<Arc<SessionHandle>> = self
            .sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        let probes = sessions.into_iter().map(|s| async move {
            let alive = s.probe().await;
            (!alive).then(|| s.id.clone())
        });
        futures_util::future::join_all(probes)
            .await
            .into_iter()
            .flatten()
            .collect()
    }

    /// `session_answer_prompt`: unknown session or prompt ⇒ `promptNotFound`.
    pub fn answer_prompt(
        &self,
        session_id: &str,
        prompt_id: &str,
        answer: PromptAnswer,
    ) -> Result<(), AppError> {
        let session = self
            .get(session_id)
            .ok_or_else(|| AppError::new(ErrorCode::PromptNotFound))?;
        session.broker.answer(prompt_id, answer)
    }

    /// `session_reconnect_now`.
    pub fn reconnect_now(&self, session_id: &str) -> Result<(), AppError> {
        let session = self.get(session_id).ok_or_else(|| {
            AppError::new(ErrorCode::SessionNotFound {
                session_id: session_id.to_owned(),
            })
        })?;
        session.reconnect_now();
        Ok(())
    }

    /// `session_disconnect`: end the connection, keep the session; running transfers are
    /// cancelled first (while the connection can still clean their temp files).
    pub async fn disconnect(&self, session_id: &str) -> Result<(), AppError> {
        let session = self.get(session_id).ok_or_else(|| {
            AppError::new(ErrorCode::SessionNotFound {
                session_id: session_id.to_owned(),
            })
        })?;
        // A disconnect is not a close; failed retryable transfers stay for retry (§6.6).
        self.transfers.cancel_session_active(session_id).await;
        session.disconnect();
        Ok(())
    }

    /// `session_close` (idempotent). Transfers are cancelled first, while the connection can
    /// still remove their remote temp files (§5.8).
    pub async fn close(&self, session_id: &str, last_dir: Option<String>) {
        self.transfers.cancel_session(session_id).await;
        if let Some(session) = self.get(session_id) {
            session.close(last_dir).await;
        }
    }

    /// Ids of every open session.
    pub fn session_ids(&self) -> Vec<String> {
        self.sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .cloned()
            .collect()
    }

    /// The page was reloaded; close the sessions the old page opened (`ids`, taken
    /// before the new page can open any), like `session_close` without a `lastDir` (transfers
    /// cancelled first, agent keys removed).
    pub async fn close_orphaned(&self, ids: &[String]) {
        futures_util::future::join_all(ids.iter().map(|id| self.close(id, None))).await;
    }

    /// Quit (§9.2): remove the agent keys of every session now, concurrently (each
    /// session's own close then finds nothing left to remove).
    pub async fn remove_all_agent_keys(&self) {
        let sessions: Vec<_> = self
            .sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        futures_util::future::join_all(sessions.iter().map(|s| s.remove_agent_keys())).await;
    }

    /// Quit (§8.9): close every session with its `lastDir`, concurrently. Transfers are only
    /// signalled to stop: waiting for them could use up the whole quit budget.
    pub async fn close_all(&self, mut last_dirs: HashMap<String, Option<String>>) {
        let sessions: Vec<_> = self
            .sessions
            .read()
            .unwrap_or_else(PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        for s in &sessions {
            self.transfers.signal_cancel_session(&s.id);
        }
        let closes = sessions.into_iter().map(|s| {
            let last_dir = last_dirs.remove(&s.id).flatten();
            async move {
                s.close(last_dir).await;
            }
        });
        futures_util::future::join_all(closes).await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::indexing_slicing)]

    use super::*;

    /// One probe round at a time, and rounds at least `PROBE_MIN_GAP` apart.
    #[tokio::test(start_paused = true)]
    async fn probe_rounds_are_rate_limited() {
        let gate = Arc::new(Mutex::new(ProbeGate::default()));
        let begin = || {
            gate.lock()
                .unwrap_or_else(PoisonError::into_inner)
                .try_begin(tokio::time::Instant::now())
                .then(|| ProbeRound(gate.clone()))
        };
        let first = begin();
        assert!(first.is_some());
        assert!(begin().is_none(), "a round is running");
        drop(first);
        assert!(begin().is_none(), "too soon after the last start");
        tokio::time::advance(PROBE_MIN_GAP - Duration::from_millis(1)).await;
        assert!(begin().is_none());
        tokio::time::advance(Duration::from_millis(1)).await;
        let second = begin();
        assert!(second.is_some());
        // A long round still blocks the next one after the gap.
        tokio::time::advance(PROBE_MIN_GAP * 3).await;
        assert!(begin().is_none());
        drop(second);
        assert!(begin().is_some());
    }

    #[test]
    fn restore_order_sorts_dedups_and_caps() {
        let mut open = vec![
            (5, "c".to_owned()),
            (1, "a".to_owned()),
            (3, "b".to_owned()),
            (4, "a".to_owned()),
        ];
        assert_eq!(restore_order(&mut open), vec!["a", "b", "c"]);
        let mut many: Vec<(u64, String)> = (0..100).map(|i| (i, format!("id{i}"))).collect();
        let ids = restore_order(&mut many);
        assert_eq!(ids.len(), MAX_RESTORE);
        assert_eq!(ids.first().map(String::as_str), Some("id0"));
        assert!(restore_order(&mut []).is_empty());
    }
}
