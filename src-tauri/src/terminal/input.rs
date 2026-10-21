//! Ordered terminal input (§4.5).
//!
//! Every shell channel has one writer task fed by an ordered queue. `term_write` and
//! `term_resize` only *enqueue* (synchronously, in the order Tauri delivers the invokes), so two
//! writes can never overtake each other, whatever the frontend does: before, each async invoke
//! deserialised its payload in its own task and the smaller one could take the write lock first.
//! A `window-change` is queued behind the input typed before it, and the last one queued always
//! carries the latest size.
//!
//! The queue is bounded in bytes ([`MAX_QUEUED`]): a remote that stops reading (closed window)
//! makes further writes fail instead of growing memory. When the channel ends, or one write
//! fails, the task stops and the rest of the queue is dropped: nothing is ever written after a
//! gap.
//!
//! Terminal data is never logged.

use std::future::Future;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use bytes::Bytes;
use tokio::sync::{mpsc, oneshot, watch};

use crate::ipc::dto::TermSize;

/// Most input bytes waiting for one shell channel (a paste into a shell whose remote window is
/// closed). The UI sends ≤ 64 KiB per call and one call at a time, so this is never reached in
/// normal use.
pub const MAX_QUEUED: usize = 8 * 1024 * 1024;

/// Completion of one queued write (`Err` = channel ended or the send failed).
pub type Done = oneshot::Sender<Result<(), String>>;

/// Where queued input goes: the shell channel's write half in the app, a recorder in tests.
pub trait Sink: Send + Sync + 'static {
    fn data(&self, data: Bytes) -> impl Future<Output = Result<(), String>> + Send;
    fn resize(&self, size: TermSize) -> impl Future<Output = Result<(), String>> + Send;
}

enum Job {
    Data(Bytes, Option<Done>),
    Resize(TermSize),
}

/// Why [`Input::push_data`] refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushError {
    /// More than [`MAX_QUEUED`] bytes would be waiting.
    Full,
    /// The writer task has stopped (channel ended or a write failed).
    Closed,
}

/// The sending side of one channel's input queue (cheap to clone).
#[derive(Clone)]
pub struct Input {
    tx: mpsc::UnboundedSender<Job>,
    queued: Arc<AtomicUsize>,
}

impl Input {
    /// Start the writer task. It ends when every `Input` clone is dropped, when `alive` turns
    /// `false` (or its sender goes away), or after the first failed write.
    pub fn spawn<S: Sink>(sink: S, alive: watch::Receiver<bool>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let queued = Arc::new(AtomicUsize::new(0));
        tokio::spawn(run(sink, rx, alive, queued.clone()));
        Self { tx, queued }
    }

    /// Queue `data` behind everything queued before. `done` (optional) receives the outcome.
    pub fn push_data(&self, data: Bytes, done: Option<Done>) -> Result<(), PushError> {
        let len = data.len();
        let before = self.queued.fetch_add(len, Ordering::AcqRel);
        if before.saturating_add(len) > MAX_QUEUED {
            self.queued.fetch_sub(len, Ordering::AcqRel);
            return Err(PushError::Full);
        }
        if self.tx.send(Job::Data(data, done)).is_err() {
            self.queued.fetch_sub(len, Ordering::AcqRel);
            return Err(PushError::Closed);
        }
        Ok(())
    }

    /// Queue a `window-change` behind the input queued before it.
    pub fn push_resize(&self, size: TermSize) -> Result<(), PushError> {
        self.tx
            .send(Job::Resize(size))
            .map_err(|_| PushError::Closed)
    }

    /// Bytes waiting (diagnostics and tests).
    pub fn queued_bytes(&self) -> usize {
        self.queued.load(Ordering::Acquire)
    }
}

/// Resolves once the channel is gone (`alive == false` or its sender dropped).
async fn ended(alive: &mut watch::Receiver<bool>) {
    let _ = alive.wait_for(|a| !*a).await;
}

async fn run<S: Sink>(
    sink: S,
    mut rx: mpsc::UnboundedReceiver<Job>,
    mut alive: watch::Receiver<bool>,
    queued: Arc<AtomicUsize>,
) {
    while let Some(job) = rx.recv().await {
        match job {
            Job::Data(data, done) => {
                let len = data.len();
                let res = tokio::select! {
                    biased;
                    () = ended(&mut alive) => Err("channel ended".to_owned()),
                    r = sink.data(data) => r,
                };
                queued.fetch_sub(len, Ordering::AcqRel);
                let failed = res.is_err();
                if let Some(done) = done {
                    let _ = done.send(res);
                }
                if failed {
                    // Never write anything after a gap; the rest of the queue is dropped (their
                    // `done` senders with it, which reads as "not connected").
                    return;
                }
            }
            Job::Resize(size) => {
                let res = tokio::select! {
                    biased;
                    () = ended(&mut alive) => return,
                    r = sink.resize(size) => r,
                };
                if let Err(e) = res {
                    tracing::debug!(error = %e, "window-change not sent");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use std::sync::Mutex;
    use std::time::Duration;

    use super::*;

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Rec {
        Data(Vec<u8>),
        Resize(u32, u32),
    }

    /// A sink whose writes take a pseudo-random time (so a later, smaller write would overtake
    /// an earlier one if anything ran them concurrently).
    #[derive(Clone, Default)]
    struct Recorder {
        out: Arc<Mutex<Vec<Rec>>>,
        fail_on: Option<u8>,
    }

    impl Sink for Recorder {
        fn data(&self, data: Bytes) -> impl Future<Output = Result<(), String>> + Send {
            let out = self.out.clone();
            let fail = self.fail_on.is_some_and(|b| data.first() == Some(&b));
            async move {
                let delay = u64::from(data.first().copied().unwrap_or(0) % 7);
                tokio::time::sleep(Duration::from_millis(delay)).await;
                if fail {
                    return Err("boom".into());
                }
                out.lock().unwrap().push(Rec::Data(data.to_vec()));
                Ok(())
            }
        }
        fn resize(&self, size: TermSize) -> impl Future<Output = Result<(), String>> + Send {
            let out = self.out.clone();
            async move {
                out.lock().unwrap().push(Rec::Resize(size.cols, size.rows));
                Ok(())
            }
        }
    }

    fn size(cols: u32, rows: u32) -> TermSize {
        TermSize {
            cols,
            rows,
            pixel_width: 0,
            pixel_height: 0,
        }
    }

    /// Big chunks alternating with small tails (the >64 KiB paste case), with sends that
    /// take different times on a multi-thread runtime, arrive in exactly the order queued.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn writes_and_resizes_keep_their_order() {
        let (_alive_tx, alive) = watch::channel(true);
        let rec = Recorder::default();
        let input = Input::spawn(rec.clone(), alive);
        let mut expected = Vec::new();
        for i in 0..200u32 {
            let n = u8::try_from(i % 251).unwrap();
            let len = if i % 2 == 0 { 64 * 1024 } else { 3 };
            let data = vec![n; len];
            input.push_data(Bytes::from(data.clone()), None).unwrap();
            expected.push(Rec::Data(data));
            if i % 50 == 0 {
                input.push_resize(size(80 + i, 24)).unwrap();
                expected.push(Rec::Resize(80 + i, 24));
            }
        }
        let (tx, rx) = oneshot::channel();
        input
            .push_data(Bytes::from_static(b"end"), Some(tx))
            .unwrap();
        expected.push(Rec::Data(b"end".to_vec()));
        rx.await.unwrap().unwrap();
        assert_eq!(*rec.out.lock().unwrap(), expected);
        assert_eq!(input.queued_bytes(), 0);
    }

    /// The byte budget bounds memory when the remote stops reading.
    #[tokio::test]
    async fn the_queue_is_bounded() {
        let (_alive_tx, alive) = watch::channel(true);
        let input = Input::spawn(Stalled, alive);
        let chunk = Bytes::from(vec![0u8; 1024 * 1024]);
        for _ in 0..(MAX_QUEUED / chunk.len()) {
            input.push_data(chunk.clone(), None).unwrap();
        }
        assert_eq!(
            input.push_data(Bytes::from_static(b"x"), None),
            Err(PushError::Full)
        );
        assert_eq!(input.queued_bytes(), MAX_QUEUED);
    }

    struct Stalled;
    impl Sink for Stalled {
        fn data(&self, _data: Bytes) -> impl Future<Output = Result<(), String>> + Send {
            std::future::pending()
        }
        fn resize(&self, _size: TermSize) -> impl Future<Output = Result<(), String>> + Send {
            std::future::pending()
        }
    }

    /// A write stuck on a closed window is released when the channel ends; nothing queued after
    /// it is written, and further pushes fail.
    #[tokio::test]
    async fn a_channel_end_stops_the_queue() {
        let (alive_tx, alive) = watch::channel(true);
        let input = Input::spawn(Stalled, alive);
        let (tx1, rx1) = oneshot::channel();
        let (tx2, rx2) = oneshot::channel();
        input
            .push_data(Bytes::from_static(b"a"), Some(tx1))
            .unwrap();
        input
            .push_data(Bytes::from_static(b"b"), Some(tx2))
            .unwrap();
        alive_tx.send(false).unwrap();
        assert!(rx1.await.unwrap().is_err());
        assert!(rx2.await.is_err(), "dropped, never written");
        tokio::time::timeout(Duration::from_secs(1), async {
            while input.push_data(Bytes::from_static(b"c"), None).is_ok() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the writer task stops");
    }

    /// After one failed write nothing behind it is written (no spliced input).
    #[tokio::test]
    async fn nothing_is_written_after_a_failed_write() {
        let (_alive_tx, alive) = watch::channel(true);
        let rec = Recorder {
            fail_on: Some(b'X'),
            ..Recorder::default()
        };
        let input = Input::spawn(rec.clone(), alive);
        let (tx, rx) = oneshot::channel();
        input.push_data(Bytes::from_static(b"1"), None).unwrap();
        input.push_data(Bytes::from_static(b"X"), Some(tx)).unwrap();
        input.push_data(Bytes::from_static(b"2"), None).unwrap();
        assert!(rx.await.unwrap().is_err());
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(*rec.out.lock().unwrap(), vec![Rec::Data(b"1".to_vec())]);
        assert_eq!(
            input.push_data(Bytes::from_static(b"3"), None),
            Err(PushError::Closed)
        );
    }
}
