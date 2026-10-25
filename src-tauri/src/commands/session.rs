//! Session commands (§4.2). Thin wrappers over `session::manager`.

use std::sync::Arc;

use tauri::State;
use tauri::ipc::Channel;

use crate::app_state::AppState;
use crate::commands::guarded;
use crate::error::AppError;
use crate::ipc::dto::{OpenSessionRequest, PromptAnswer, SessionEvent};
use crate::ssh::prompt::EventSink;

/// `Channel<SessionEvent>` as the session's event sink (§3: services see only `EventSink`).
struct ChannelSink(Channel<SessionEvent>);

impl EventSink for ChannelSink {
    fn send(&self, event: SessionEvent) {
        if let Err(e) = self.0.send(event) {
            tracing::debug!(error = %e, "session event channel closed");
        }
    }
}

/// Returns the sessionId immediately; progress arrives on `events`. Resolution errors
/// (`notFound`, `invalidHostSpec`, `invalidHostname`, `proxyJump*`, `proxyCommandUnsupported`)
/// reject the command; connection errors arrive as `state: disconnected`.
#[tauri::command]
pub async fn session_open(
    state: State<'_, AppState>,
    req: OpenSessionRequest,
    events: Channel<SessionEvent>,
) -> Result<String, AppError> {
    guarded("session_open", async move {
        state
            .sessions
            .open(req, Arc::new(ChannelSink(events)))
            .await
    })
    .await
}

#[tauri::command]
pub async fn session_answer_prompt(
    state: State<'_, AppState>,
    session_id: String,
    prompt_id: String,
    answer: PromptAnswer,
) -> Result<(), AppError> {
    guarded("session_answer_prompt", async move {
        state
            .sessions
            .answer_prompt(&session_id, &prompt_id, answer)
    })
    .await
}

#[tauri::command]
pub async fn session_reconnect_now(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), AppError> {
    guarded("session_reconnect_now", async move {
        state.sessions.reconnect_now(&session_id)
    })
    .await
}

/// Real Disconnect: closes the SSH connection but keeps the session. State becomes
/// `disconnected{reason: userRequested, canRetry: true}`, no auto-reconnect; terminals are kept
/// as lost and respawn on `session_reconnect_now`. Idempotent on an already disconnected session.
#[tauri::command]
pub async fn session_disconnect(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), AppError> {
    guarded("session_disconnect", async move {
        state.sessions.disconnect(&session_id).await
    })
    .await
}

/// Idempotent; records recents and emits `recents:changed`.
#[tauri::command]
pub async fn session_close(
    state: State<'_, AppState>,
    session_id: String,
    last_dir: Option<String>,
) -> Result<(), AppError> {
    guarded("session_close", async move {
        state.sessions.close(&session_id, last_dir).await;
        Ok(())
    })
    .await
}

/// Liveness probe of all sessions (§5.7): returns at once; each connected session is pinged
/// (5 s budget, a second try 5 s later). Two consecutive timeouts ⇒ that session is lost and
/// auto-reconnect starts (§5.8). Rate-limited in the backend too: while a probe round
/// runs, or within [`crate::session::manager::PROBE_MIN_GAP`] of the last one, the call is a
/// no-op.
#[tauri::command]
pub async fn session_probe_all(state: State<'_, AppState>) -> Result<(), AppError> {
    guarded("session_probe_all", async move {
        let sessions = state.sessions.clone();
        let Some(round) = sessions.begin_probe_round() else {
            tracing::debug!("probe round skipped (rate limit)");
            return Ok(());
        };
        tauri::async_runtime::spawn(async move {
            let _round = round;
            let lost = sessions.probe_all().await;
            if !lost.is_empty() {
                tracing::info!(count = lost.len(), "probe declared sessions lost");
            }
        });
        Ok(())
    })
    .await
}
