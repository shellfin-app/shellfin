//! 1 Hz traffic stats (§1.3 "stats ticker"): deltas of the outer TCP stream counters.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::time::MissedTickBehavior;

use crate::ipc::dto::SessionEvent;
use crate::ssh::connect::Traffic;
use crate::ssh::prompt::EventSink;

/// The running ticker; dropping it stops the task (also when the supervisor panics and
/// its locals are dropped during unwinding, so no `Stats` follow `closed`).
#[derive(Debug)]
pub struct Ticker(tokio::task::JoinHandle<()>);

impl Ticker {
    pub fn abort(&self) {
        self.0.abort();
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Emit `SessionEvent::Stats` every second until the returned [`Ticker`] is dropped or aborted.
pub fn spawn_ticker(traffic: Arc<Traffic>, sink: Arc<dyn EventSink>) -> Ticker {
    Ticker(tokio::spawn(async move {
        let mut last = (
            traffic.tx.load(Ordering::Relaxed),
            traffic.rx.load(Ordering::Relaxed),
        );
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        // After a system sleep (or a stalled runtime) skip the missed ticks instead of
        // firing a burst of catch-up events into the webview.
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
        tick.tick().await;
        loop {
            tick.tick().await;
            let now = (
                traffic.tx.load(Ordering::Relaxed),
                traffic.rx.load(Ordering::Relaxed),
            );
            sink.send(SessionEvent::Stats {
                tx_bytes_per_sec: now.0.saturating_sub(last.0),
                rx_bytes_per_sec: now.1.saturating_sub(last.1),
            });
            last = now;
        }
    }))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Count(Mutex<usize>);
    impl EventSink for Count {
        fn send(&self, _event: SessionEvent) {
            *self.0.lock().unwrap() += 1;
        }
    }

    /// A long stall produces one tick, not a burst; dropping the ticker stops it.
    #[tokio::test(start_paused = true)]
    async fn missed_ticks_are_skipped_and_drop_stops_the_ticker() {
        let sink = Arc::new(Count::default());
        let ticker = spawn_ticker(Arc::new(Traffic::default()), sink.clone());
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(*sink.0.lock().unwrap(), 1);
        // Simulate a 1 h suspend: the clock jumps without the task running in between.
        tokio::time::advance(Duration::from_secs(3600)).await;
        tokio::time::sleep(Duration::from_millis(10)).await;
        assert!(*sink.0.lock().unwrap() <= 3, "{}", *sink.0.lock().unwrap());
        drop(ticker);
        tokio::task::yield_now().await;
        let after = *sink.0.lock().unwrap();
        tokio::time::sleep(Duration::from_secs(10)).await;
        assert_eq!(*sink.0.lock().unwrap(), after);
    }
}
