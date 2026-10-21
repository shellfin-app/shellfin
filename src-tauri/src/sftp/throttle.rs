//! Bandwidth limits for file transfers.
//!
//! One [`SessionThrottle`] per session holds a [`Limiter`] per direction, shared by all
//! concurrent transfers of the session (the per-session concurrency gate of `transfer.rs` stays
//! as it is). The effective rate of a direction is the bookmark override (when the session was
//! opened from a bookmark that has one) or the global `settings.transfers` value; a settings
//! update re-applies the global values to every open session immediately.
//!
//! Scope: uploads and downloads (`transfer_*`) only. Editor reads/saves and tree listings are
//! small, interactive and never throttled.
//!
//! Algorithm: a "next free instant" bucket without banked credit. A chunk of `n` bytes at rate
//! `R` waits until the previous chunk's cost has elapsed, then books `n / R` seconds. The
//! first chunk goes at once; the SFTP window (8, growing to at most 64 × 32 KiB) bounds
//! the initial burst. Waits are cancel-aware (≤ 100 ms granularity), and a cancelled or
//! abandoned wait gives its booking back.

use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::TransferSettings;

/// Longest uninterrupted sleep, so a cancel is noticed quickly.
const SLICE: Duration = Duration::from_millis(100);

/// Per-bookmark overrides in KiB/s (`None` = use the global setting, `Some(0)` = unlimited).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LimitOverrides {
    pub upload_kibps: Option<u32>,
    pub download_kibps: Option<u32>,
}

#[derive(Debug)]
struct State {
    /// Bytes per second; 0 = unlimited.
    rate: u64,
    next_free: Instant,
}

/// A token bucket for one direction of one session.
#[derive(Debug)]
pub struct Limiter {
    state: Mutex<State>,
}

impl Limiter {
    pub fn new(kibps: u32) -> Self {
        Self {
            state: Mutex::new(State {
                rate: rate_bytes(kibps),
                next_free: Instant::now(),
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Change the rate (KiB/s, 0 = unlimited). Booked time is forgotten so a lowered or raised
    /// limit takes effect on the next chunk.
    pub fn set_kibps(&self, kibps: u32) {
        let mut s = self.lock();
        s.rate = rate_bytes(kibps);
        s.next_free = Instant::now();
    }

    pub fn kibps(&self) -> u32 {
        u32::try_from(self.lock().rate / 1024).unwrap_or(u32::MAX)
    }

    /// Book `bytes` and return how long the caller must wait before using them.
    pub fn reserve(&self, bytes: usize, now: Instant) -> Duration {
        self.book(bytes, now).0
    }

    /// [`Limiter::reserve`], also returning the booked cost (for [`Limiter::refund`]).
    fn book(&self, bytes: usize, now: Instant) -> (Duration, Duration) {
        let mut s = self.lock();
        if s.rate == 0 {
            return (Duration::ZERO, Duration::ZERO);
        }
        let start = s.next_free.max(now);
        let cost = Duration::from_secs_f64(bytes as f64 / s.rate as f64);
        s.next_free = start + cost;
        (start.saturating_duration_since(now), cost)
    }

    /// Give back `cost` that was booked but never used (a cancelled transfer), so the
    /// session's other transfers do not wait for it. Never moves the next free instant into the
    /// past.
    pub fn refund(&self, cost: Duration, now: Instant) {
        let mut s = self.lock();
        s.next_free = s.next_free.checked_sub(cost).map_or(now, |t| t.max(now));
    }

    /// Wait for `bytes` of budget; `cancelled` aborts the wait with `cancelled`. A wait that is
    /// cancelled (or dropped, e.g. by a cancel signal racing it) refunds its booking.
    pub async fn acquire(
        &self,
        bytes: usize,
        cancelled: &(impl Fn() -> bool + ?Sized),
    ) -> Result<(), AppError> {
        let (mut wait, cost) = self.book(bytes, Instant::now());
        let mut booking = Booking {
            limiter: self,
            cost,
            used: false,
        };
        while !wait.is_zero() {
            if cancelled() {
                return Err(AppError::new(ErrorCode::Cancelled));
            }
            let step = wait.min(SLICE);
            tokio::time::sleep(step).await;
            wait = wait.saturating_sub(step);
        }
        booking.used = true;
        Ok(())
    }
}

/// A booking that is refunded unless it was used.
struct Booking<'a> {
    limiter: &'a Limiter,
    cost: Duration,
    used: bool,
}

impl Drop for Booking<'_> {
    fn drop(&mut self) {
        if !self.used && !self.cost.is_zero() {
            self.limiter.refund(self.cost, Instant::now());
        }
    }
}

fn rate_bytes(kibps: u32) -> u64 {
    u64::from(kibps) * 1024
}

/// Upload and download limiters of a session.
#[derive(Debug)]
pub struct SessionThrottle {
    pub upload: Limiter,
    pub download: Limiter,
    overrides: Mutex<LimitOverrides>,
}

impl SessionThrottle {
    pub fn new(overrides: LimitOverrides, global: &TransferSettings) -> Self {
        let t = Self {
            upload: Limiter::new(0),
            download: Limiter::new(0),
            overrides: Mutex::new(overrides),
        };
        t.apply_global(global);
        t
    }

    fn overrides(&self) -> LimitOverrides {
        *self
            .overrides
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }

    /// Replace the bookmark overrides of a running session (the bookmark was edited) and apply
    /// them at once.
    pub fn set_overrides(&self, overrides: LimitOverrides, global: &TransferSettings) {
        *self
            .overrides
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = overrides;
        self.apply_global(global);
    }

    /// Re-apply the global settings (bookmark overrides keep precedence).
    pub fn apply_global(&self, global: &TransferSettings) {
        let overrides = self.overrides();
        let up = overrides.upload_kibps.unwrap_or(global.upload_limit_kibps);
        let down = overrides
            .download_kibps
            .unwrap_or(global.download_limit_kibps);
        if up != self.upload.kibps() {
            self.upload.set_kibps(up);
        }
        if down != self.download.kibps() {
            self.download.set_kibps(down);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::atomic::{AtomicBool, Ordering};

    use super::*;

    fn global(up: u32, down: u32) -> TransferSettings {
        TransferSettings {
            upload_limit_kibps: up,
            download_limit_kibps: down,
            ..crate::storage::settings::default_settings().transfers
        }
    }

    #[test]
    fn unlimited_never_waits() {
        let l = Limiter::new(0);
        let now = Instant::now();
        for _ in 0..10 {
            assert_eq!(l.reserve(10_000_000, now), Duration::ZERO);
        }
    }

    #[test]
    fn books_cost_after_a_free_first_chunk() {
        let l = Limiter::new(100); // 102 400 B/s
        let now = Instant::now();
        assert_eq!(l.reserve(102_400, now), Duration::ZERO);
        let w = l.reserve(51_200, now);
        assert!((w.as_secs_f64() - 1.0).abs() < 1e-6, "{w:?}");
        let w = l.reserve(1, now);
        assert!((w.as_secs_f64() - 1.5).abs() < 1e-3, "{w:?}");
        // No credit is banked while idle.
        let later = now + Duration::from_secs(60);
        assert_eq!(l.reserve(102_400, later), Duration::ZERO);
    }

    #[test]
    fn rate_change_resets_the_booking() {
        let l = Limiter::new(1);
        let now = Instant::now();
        l.reserve(1024 * 1024, now);
        assert!(l.reserve(1, now) > Duration::from_secs(100));
        l.set_kibps(0);
        assert_eq!(l.kibps(), 0);
        assert_eq!(l.reserve(1, Instant::now()), Duration::ZERO);
    }

    #[test]
    fn overrides_win_over_global_and_global_updates_apply() {
        let t = SessionThrottle::new(
            LimitOverrides {
                upload_kibps: Some(0),
                download_kibps: None,
            },
            &global(500, 300),
        );
        assert_eq!((t.upload.kibps(), t.download.kibps()), (0, 300));
        t.apply_global(&global(900, 100));
        assert_eq!((t.upload.kibps(), t.download.kibps()), (0, 100));
        let plain = SessionThrottle::new(LimitOverrides::default(), &global(500, 0));
        assert_eq!((plain.upload.kibps(), plain.download.kibps()), (500, 0));
        plain.apply_global(&global(0, 64));
        assert_eq!((plain.upload.kibps(), plain.download.kibps()), (0, 64));
        // A bookmark edit changes the overrides of the running session.
        plain.set_overrides(
            LimitOverrides {
                upload_kibps: Some(10),
                download_kibps: Some(0),
            },
            &global(0, 64),
        );
        assert_eq!((plain.upload.kibps(), plain.download.kibps()), (10, 0));
        plain.set_overrides(LimitOverrides::default(), &global(7, 64));
        assert_eq!((plain.upload.kibps(), plain.download.kibps()), (7, 64));
    }

    #[tokio::test(start_paused = true)]
    async fn acquire_sleeps_and_obeys_cancel() {
        let l = Limiter::new(10); // 10 240 B/s
        let never = || false;
        l.acquire(10_240, &never).await.unwrap(); // first chunk is free
        let t0 = tokio::time::Instant::now();
        l.acquire(10_240, &never).await.unwrap(); // waits for the first one's cost: 1 s
        assert!(
            t0.elapsed() >= Duration::from_millis(990),
            "{:?}",
            t0.elapsed()
        );

        let flag = AtomicBool::new(false);
        let cancelled = || flag.load(Ordering::Relaxed);
        l.reserve(10_240 * 3600, Instant::now()); // book an hour
        let fut = l.acquire(1, &cancelled);
        tokio::pin!(fut);
        tokio::select! {
            _ = &mut fut => panic!("should still be waiting"),
            () = tokio::time::sleep(Duration::from_secs(1)) => {}
        }
        flag.store(true, Ordering::Relaxed);
        let err = fut.await.unwrap_err();
        assert!(matches!(err.kind, ErrorCode::Cancelled));
    }

    /// A cancelled transfer's booked time goes back, so the session's other transfers
    /// do not wait for bytes that were never sent; a used booking is kept.
    #[tokio::test(start_paused = true)]
    async fn cancelled_bookings_are_refunded() {
        let l = Limiter::new(10); // 10 240 B/s
        let never = || false;
        l.acquire(10_240, &never).await.unwrap(); // free, books 1 s
        // A transfer books 100 s and is cancelled while it waits.
        let yes = || true;
        let err = l.acquire(1_024_000, &yes).await.unwrap_err();
        assert!(matches!(err.kind, ErrorCode::Cancelled));
        // Only the first booking (≤ 1 s) is left.
        let w = l.reserve(1, Instant::now());
        assert!(w <= Duration::from_secs(1), "{w:?}");

        // A wait dropped by a racing cancel signal (`or_cancelled`) also refunds.
        let l = Limiter::new(10);
        l.acquire(10_240, &never).await.unwrap();
        {
            let fut = l.acquire(1_024_000, &never);
            tokio::pin!(fut);
            tokio::select! {
                _ = &mut fut => panic!("should still be waiting"),
                () = tokio::time::sleep(Duration::from_millis(10)) => {}
            }
        }
        let w = l.reserve(1, Instant::now());
        assert!(w <= Duration::from_secs(1), "{w:?}");

        // Refunds never move the next free instant into the past.
        let l = Limiter::new(10);
        l.refund(Duration::from_secs(3600), Instant::now());
        assert_eq!(l.reserve(10_240, Instant::now()), Duration::ZERO);
        assert!(l.reserve(1, Instant::now()) > Duration::from_millis(900));
    }
}
