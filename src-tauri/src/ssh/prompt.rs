//! Prompts (§5.4): `PromptBroker::ask` registers a `oneshot`, emits `SessionEvent::Prompt`, and
//! waits up to `promptTimeoutSecs` (default 110 s < OpenSSH `LoginGraceTime` 120 s). Timeout or
//! loss → `promptClosed` (+ `promptTimeout` / `disconnected` decided by the caller). One open
//! prompt per session.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::sync::oneshot;

use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::{Prompt, PromptAnswer, PromptClosedReason, SessionEvent};

/// Where session events go: the Tauri `Channel<SessionEvent>` in the app, an in-memory sink in
/// tests (§3: services do not depend on Tauri types).
pub trait EventSink: Send + Sync + 'static {
    fn send(&self, event: SessionEvent);
}

/// Result of [`PromptBroker::ask`].
#[derive(Debug)]
pub enum PromptOutcome {
    Answer(PromptAnswer),
    Timeout,
    /// The prompt was withdrawn (`cancel_pending`), e.g. the session is closing or the transport died.
    Withdrawn,
}

struct Pending {
    id: String,
    tx: oneshot::Sender<PromptAnswer>,
}

pub struct PromptBroker {
    sink: Arc<dyn EventSink>,
    pending: Mutex<Option<Pending>>,
    timeout: Duration,
}

impl PromptBroker {
    pub fn new(sink: Arc<dyn EventSink>, timeout: Duration) -> Self {
        Self {
            sink,
            pending: Mutex::new(None),
            timeout,
        }
    }

    pub fn sink(&self) -> &Arc<dyn EventSink> {
        &self.sink
    }

    /// Emit a prompt built by `build(prompt_id)` and wait for the answer. A previous pending
    /// prompt (should not happen: one per session) is withdrawn first.
    pub async fn ask(&self, build: impl FnOnce(String) -> Prompt) -> PromptOutcome {
        let id = uuid::Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        let previous = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(Pending { id: id.clone(), tx });
        if let Some(prev) = previous {
            self.closed(prev.id, PromptClosedReason::Cancelled);
        }
        self.sink.send(SessionEvent::Prompt {
            prompt: build(id.clone()),
        });
        // If this future is dropped while waiting (attempt abandoned), withdraw our own prompt:
        // no orphan stays pending and pauses the handshake timer.
        let _guard = AbandonGuard {
            broker: self,
            id: id.clone(),
        };

        let outcome = match tokio::time::timeout(self.timeout, rx).await {
            Ok(Ok(answer)) => PromptOutcome::Answer(answer),
            Ok(Err(_)) => PromptOutcome::Withdrawn,
            Err(_) => {
                self.take_if(&id);
                self.closed(id, PromptClosedReason::Timeout);
                return PromptOutcome::Timeout;
            }
        };
        if matches!(outcome, PromptOutcome::Answer(_)) {
            self.closed(id, PromptClosedReason::Answered);
        }
        outcome
    }

    /// `session_answer_prompt`. Unknown or stale ids → `promptNotFound`.
    /// (See also [`AbandonGuard`]: an `ask` dropped mid-wait withdraws its prompt.)
    pub fn answer(&self, prompt_id: &str, answer: PromptAnswer) -> Result<(), AppError> {
        let pending = self
            .take_if(prompt_id)
            .ok_or_else(|| AppError::new(ErrorCode::PromptNotFound))?;
        pending
            .tx
            .send(answer)
            .map_err(|_| AppError::new(ErrorCode::PromptNotFound))
    }

    /// Withdraw the open prompt (session closing → `cancelled`, transport lost → `connectionLost`).
    pub fn cancel_pending(&self, reason: PromptClosedReason) {
        let pending = self
            .pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take();
        if let Some(p) = pending {
            drop(p.tx);
            self.closed(p.id, reason);
        }
    }

    pub fn has_pending(&self) -> bool {
        self.pending
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some()
    }

    fn take_if(&self, id: &str) -> Option<Pending> {
        let mut guard = self.pending.lock().unwrap_or_else(PoisonError::into_inner);
        if guard.as_ref().is_some_and(|p| p.id == id) {
            guard.take()
        } else {
            None
        }
    }

    fn closed(&self, prompt_id: String, reason: PromptClosedReason) {
        self.sink
            .send(SessionEvent::PromptClosed { prompt_id, reason });
    }
}

/// Withdraws the prompt of an `ask` future that is dropped before it completes. After a normal
/// completion the entry is already gone, so the guard is a no-op.
struct AbandonGuard<'a> {
    broker: &'a PromptBroker,
    id: String,
}

impl Drop for AbandonGuard<'_> {
    fn drop(&mut self) {
        if self.broker.take_if(&self.id).is_some() {
            self.broker
                .closed(self.id.clone(), PromptClosedReason::Cancelled);
        }
    }
}

#[cfg(test)]
pub mod test_support {
    //! In-memory sink that records events and can auto-answer prompts (scripted broker).
    use super::*;

    #[derive(Default)]
    pub struct RecordingSink {
        pub events: Mutex<Vec<SessionEvent>>,
    }

    impl EventSink for RecordingSink {
        fn send(&self, event: SessionEvent) {
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(event);
        }
    }

    impl RecordingSink {
        pub fn prompt_ids(&self) -> Vec<String> {
            self.events
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .iter()
                .filter_map(|e| match e {
                    SessionEvent::Prompt { prompt } => Some(prompt_id(prompt).to_owned()),
                    _ => None,
                })
                .collect()
        }
    }

    pub fn prompt_id(p: &Prompt) -> &str {
        match p {
            Prompt::HostKeyUnknown { id, .. }
            | Prompt::Passphrase { id, .. }
            | Prompt::Password { id, .. }
            | Prompt::KeyboardInteractive { id, .. } => id,
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::test_support::*;
    use super::*;
    use crate::ipc::dto::HopRef;

    fn hop() -> HopRef {
        HopRef {
            index: 0,
            label: "h".into(),
            user: "alice".into(),
            host: "example.com".into(),
            port: 22,
        }
    }

    fn kbd(id: String) -> Prompt {
        Prompt::KeyboardInteractive {
            id,
            hop: hop(),
            name: String::new(),
            instructions: String::new(),
            fields: vec![],
        }
    }

    #[tokio::test]
    async fn answer_round_trip() {
        let sink = Arc::new(RecordingSink::default());
        let broker = Arc::new(PromptBroker::new(sink.clone(), Duration::from_secs(5)));
        let b = broker.clone();
        let s = sink.clone();
        let answering = tokio::spawn(async move {
            loop {
                if let Some(id) = s.prompt_ids().first().cloned() {
                    assert!(b.answer("wrong-id", PromptAnswer::Skip).is_err());
                    b.answer(&id, PromptAnswer::Cancel).unwrap();
                    // A second answer to the same id is stale.
                    assert_eq!(
                        b.answer(&id, PromptAnswer::Cancel).unwrap_err().kind,
                        ErrorCode::PromptNotFound
                    );
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        });
        let outcome = broker.ask(kbd).await;
        answering.await.unwrap();
        assert!(matches!(
            outcome,
            PromptOutcome::Answer(PromptAnswer::Cancel)
        ));
        let events = sink.events.lock().unwrap();
        assert!(matches!(
            events.last(),
            Some(SessionEvent::PromptClosed {
                reason: PromptClosedReason::Answered,
                ..
            })
        ));
        assert!(!broker.has_pending());
    }

    #[tokio::test(start_paused = true)]
    async fn timeout_closes_prompt() {
        let sink = Arc::new(RecordingSink::default());
        let broker = PromptBroker::new(sink.clone(), Duration::from_secs(110));
        let outcome = broker.ask(kbd).await;
        assert!(matches!(outcome, PromptOutcome::Timeout));
        let events = sink.events.lock().unwrap();
        assert!(matches!(
            events.last(),
            Some(SessionEvent::PromptClosed {
                reason: PromptClosedReason::Timeout,
                ..
            })
        ));
        assert!(!broker.has_pending());
    }

    #[tokio::test]
    async fn withdrawn_on_cancel() {
        let sink = Arc::new(RecordingSink::default());
        let broker = Arc::new(PromptBroker::new(sink.clone(), Duration::from_secs(5)));
        let b = broker.clone();
        let task = tokio::spawn(async move { b.ask(kbd).await });
        while !broker.has_pending() {
            tokio::task::yield_now().await;
        }
        broker.cancel_pending(PromptClosedReason::ConnectionLost);
        assert!(matches!(task.await.unwrap(), PromptOutcome::Withdrawn));
        let events = sink.events.lock().unwrap();
        assert!(matches!(
            events.last(),
            Some(SessionEvent::PromptClosed {
                reason: PromptClosedReason::ConnectionLost,
                ..
            })
        ));
    }

    /// Dropping an `ask` mid-wait withdraws its prompt (no orphan keeps `has_pending`
    /// true and pauses the handshake timer).
    #[tokio::test]
    async fn abandoned_ask_withdraws_its_prompt() {
        let sink = Arc::new(RecordingSink::default());
        let broker = Arc::new(PromptBroker::new(sink.clone(), Duration::from_secs(60)));
        let b = broker.clone();
        let task = tokio::spawn(async move { b.ask(kbd).await });
        while !broker.has_pending() {
            tokio::task::yield_now().await;
        }
        task.abort();
        let _ = task.await;
        assert!(!broker.has_pending());
        let events = sink.events.lock().unwrap();
        assert!(matches!(
            events.last(),
            Some(SessionEvent::PromptClosed {
                reason: PromptClosedReason::Cancelled,
                ..
            })
        ));
    }
}
