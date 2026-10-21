//! Per-terminal output flow control (§4.5). Pure state: no I/O, no tasks, unit-tested.
//!
//! The reader task pushes everything the channel delivers ([`Flow::push_data`] never blocks and
//! never refuses: excess output is *discarded*, not back-pressured, because russh has no
//! per-channel flow control and a paused reader would stall the whole connection, §2.3). The
//! pump task calls [`Flow::poll`] and sends what it returns to the UI.
//!
//! Rules (§4.5):
//! - data is flushed once ≥ [`FLUSH_BYTES`] are buffered, or [`FLUSH_DELAY`] after the oldest
//!   unflushed byte arrived, or right away when an event is waiting behind it;
//! - data is flushed only while fewer than [`MAX_IN_FLIGHT`] bytes are unacknowledged; the
//!   frontend acknowledges with `term_ack` ([`Flow::ack`]);
//! - when the backlog would exceed [`MAX_BACKLOG`], new output is discarded (the oldest output is
//!   kept) and counted; once the backlog is below [`RESUME_BELOW`] again, an
//!   `outputDropped{bytes}` event is queued **behind** the kept output and new output is accepted
//!   again (hysteresis);
//! - an overflow also ends at every stream boundary: an event pushed while discarding (`exit`,
//!   `closed`, `reconnected`) and a new shell ([`Flow::boundary`]) first queue the
//!   `outputDropped` for the bytes discarded before them, so the marker sits exactly where the
//!   bytes were lost and a new shell's output is never counted as the old one's;
//! - events ([`TermEvent`]) keep their position in the stream relative to the data and are not
//!   subject to the in-flight limit.
//!
//! Memory: the cap counts payload bytes, so the payload must not carry per-packet
//! overhead. Small packets are copied into the tail buffer ([`TAIL_MAX`] each); every buffer but
//! the last is full, so the heap held is ≈ payload + one tail buffer + one slot per buffer, no
//! matter how small the server's packets are.

use std::collections::VecDeque;
use std::time::Duration;

use bytes::BytesMut;
use tokio::time::Instant;

use crate::ipc::dto::TermEvent;

/// Flush as soon as this much data is buffered.
pub const FLUSH_BYTES: usize = 64 * 1024;
/// Flush at the latest this long after the oldest unflushed byte arrived.
pub const FLUSH_DELAY: Duration = Duration::from_millis(4);
/// No data is sent while this many bytes are unacknowledged by the frontend.
pub const MAX_IN_FLIGHT: u64 = 4 * 1024 * 1024;
/// Backlog cap: output beyond it is discarded.
pub const MAX_BACKLOG: usize = 8 * 1024 * 1024;
/// After an overflow, output is accepted again once the backlog is below this.
pub const RESUME_BELOW: usize = 1024 * 1024;
/// Largest single `Raw` message sent to the UI (so in-flight ≤ MAX_IN_FLIGHT + MAX_CHUNK).
pub const MAX_CHUNK: usize = 256 * 1024;
/// Size of one backlog buffer: packets are coalesced into the tail buffer until it is full.
pub const TAIL_MAX: usize = 64 * 1024;

#[derive(Debug)]
enum Item {
    Data(BytesMut),
    Event(TermEvent),
}

/// Something to deliver to the UI, in order.
#[derive(Debug)]
pub enum Out {
    Data(Vec<u8>),
    Event(TermEvent),
}

/// Result of [`Flow::poll`].
#[derive(Debug, Default)]
pub struct Poll {
    /// Deliver these in order.
    pub out: Vec<Out>,
    /// Poll again at this time (pending data waits for [`FLUSH_DELAY`]); `None` = only when
    /// something is pushed or acknowledged.
    pub wake_at: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct Flow {
    queue: VecDeque<Item>,
    /// Data bytes in `queue` (events not counted).
    queued: usize,
    /// Events in `queue`.
    events: usize,
    /// Bytes sent to the UI and not yet acknowledged.
    unacked: u64,
    /// `Some(n)` while discarding: n bytes dropped since the overflow began.
    dropping: Option<u64>,
    /// Arrival time of the oldest unflushed data byte.
    oldest: Option<Instant>,
}

impl Flow {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append channel output. Returns `false` if it was discarded (backlog overflow).
    pub fn push_data(&mut self, data: &[u8], now: Instant) -> bool {
        let n = data.len();
        if n == 0 {
            return true;
        }
        if let Some(dropped) = self.dropping.as_mut() {
            if self.queued >= RESUME_BELOW {
                *dropped = dropped.saturating_add(n as u64);
                return false;
            }
            self.end_overflow();
        }
        if self.queued.saturating_add(n) > MAX_BACKLOG {
            self.dropping = Some(n as u64);
            return false;
        }
        if self.oldest.is_none() {
            self.oldest = Some(now);
        }
        self.queued += n;
        self.append(data);
        true
    }

    /// Copy `data` into the tail buffer, opening new full-size buffers as needed.
    fn append(&mut self, mut data: &[u8]) {
        if let Some(Item::Data(tail)) = self.queue.back_mut() {
            let room = TAIL_MAX.saturating_sub(tail.len());
            let (head, rest) = data.split_at(room.min(data.len()));
            tail.extend_from_slice(head);
            data = rest;
        }
        if !data.is_empty() {
            // The remainder starts a new buffer: a full-size tail for small packets, exact size
            // for a large packet.
            let mut buf = BytesMut::with_capacity(data.len().max(TAIL_MAX));
            buf.extend_from_slice(data);
            self.queue.push_back(Item::Data(buf));
        }
    }

    /// Append a control event (kept in stream order behind the data before it). Output discarded
    /// before it is reported first (`outputDropped` precedes the event).
    pub fn push_event(&mut self, event: TermEvent) {
        self.end_overflow();
        self.events += 1;
        self.queue.push_back(Item::Event(event));
    }

    /// A new shell starts (restart/respawn). Without an overflow nothing changes (the old
    /// shell's queued output is still delivered first). During an overflow the UI is ≥ 1 MiB
    /// behind and the old shell's queued output is stale: it is discarded too and counted in the
    /// same `outputDropped`, so the new shell starts with an empty backlog and its first output
    /// (the prompt) is never lost. Queued events keep their order.
    pub fn boundary(&mut self) {
        if let Some(dropped) = self.dropping.as_mut() {
            let mut stale = 0usize;
            self.queue.retain(|item| match item {
                Item::Data(d) => {
                    stale += d.len();
                    false
                }
                Item::Event(_) => true,
            });
            *dropped = dropped.saturating_add(stale as u64);
            self.queued = 0;
            self.oldest = None;
        }
        self.end_overflow();
    }

    /// Append the output collected in `early` (a flow that buffered a new shell's output before
    /// its PTY/shell replies), followed by `outputDropped` for what `early` discarded.
    pub fn absorb(&mut self, mut early: Flow, now: Instant) {
        for item in early.queue.drain(..) {
            match item {
                Item::Data(d) => {
                    self.push_data(&d, now);
                }
                Item::Event(e) => self.push_event(e),
            }
        }
        if let Some(bytes) = early.dropping.take() {
            self.push_event(TermEvent::OutputDropped { bytes });
        }
    }

    /// Bytes discarded by the current overflow (0 if none).
    pub fn dropped_bytes(&self) -> u64 {
        self.dropping.unwrap_or(0)
    }

    /// Heap held by queued data (buffer capacities + queue slots); diagnostics and tests.
    pub fn heap_bytes(&self) -> usize {
        let buffers: usize = self
            .queue
            .iter()
            .map(|i| match i {
                Item::Data(d) => d.capacity(),
                Item::Event(_) => 0,
            })
            .sum();
        buffers + self.queue.capacity() * std::mem::size_of::<Item>()
    }

    /// [`Self::push_data`], then whether the pump has to be woken. The pump sleeps until
    /// the time [`Self::poll`] gave it, or until woken; a wake is needed only when `poll` would
    /// now return something earlier than that: the first unflushed byte (no flush timer is
    /// running yet), the buffer reaching [`FLUSH_BYTES`] (flush now, not after [`FLUSH_DELAY`]),
    /// or an overflow that ended (an `outputDropped` was queued). A flood of tiny packets thus
    /// wakes the pump about once per flush instead of once per packet.
    pub fn push_data_wake(&mut self, data: &[u8], now: Instant) -> bool {
        let had_pending = self.oldest.is_some();
        let queued = self.queued;
        let events = self.events;
        self.push_data(data, now);
        (!had_pending && self.oldest.is_some())
            || (queued < FLUSH_BYTES && self.queued >= FLUSH_BYTES)
            || self.events != events
    }

    /// `term_ack(bytes)`: the frontend has written `bytes` more bytes. Returns whether this
    /// released data held back by [`MAX_IN_FLIGHT`] (the only case where an ack changes what
    /// [`Self::poll`] returns, so the only one that needs to wake the pump).
    pub fn ack(&mut self, bytes: u64) -> bool {
        let blocked = self.unacked >= MAX_IN_FLIGHT;
        self.unacked = self.unacked.saturating_sub(bytes);
        blocked && self.unacked < MAX_IN_FLIGHT
    }

    /// Forget all pending output and flow state (terminal closed). A restart or respawn does
    /// not clear: it marks a [`Self::boundary`] and keeps the in-flight count.
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.dropping.is_none()
    }

    pub fn unacked(&self) -> u64 {
        self.unacked
    }

    pub fn queued_bytes(&self) -> usize {
        self.queued
    }

    pub fn is_dropping(&self) -> bool {
        self.dropping.is_some()
    }

    /// What can be delivered now, and when to poll again.
    pub fn poll(&mut self, now: Instant) -> Poll {
        let mut out = Vec::new();
        loop {
            if self.dropping.is_some() && self.queued < RESUME_BELOW {
                self.end_overflow();
            }
            match self.queue.front() {
                None => return Poll { out, wake_at: None },
                Some(Item::Event(_)) => {
                    if let Some(Item::Event(e)) = self.queue.pop_front() {
                        self.events -= 1;
                        out.push(Out::Event(e));
                    }
                }
                Some(Item::Data(_)) => {
                    if self.unacked >= MAX_IN_FLIGHT {
                        return Poll { out, wake_at: None };
                    }
                    let deadline = self.oldest.map(|t| t + FLUSH_DELAY);
                    let due = self.events > 0
                        || self.queued >= FLUSH_BYTES
                        || deadline.is_some_and(|d| now >= d);
                    if !due {
                        return Poll {
                            out,
                            wake_at: deadline,
                        };
                    }
                    let chunk = self.take_chunk();
                    self.unacked = self.unacked.saturating_add(chunk.len() as u64);
                    out.push(Out::Data(chunk));
                }
            }
        }
    }

    /// Coalesce consecutive data items from the head, up to [`MAX_CHUNK`] bytes.
    fn take_chunk(&mut self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(self.queued.min(MAX_CHUNK));
        while let Some(Item::Data(d)) = self.queue.front_mut() {
            let room = MAX_CHUNK - buf.len();
            if room == 0 {
                break;
            }
            if d.len() <= room {
                buf.extend_from_slice(d);
                self.queue.pop_front();
            } else {
                let head = d.split_to(room);
                buf.extend_from_slice(&head);
            }
        }
        self.queued -= buf.len();
        if self.queued == 0 {
            self.oldest = None;
        }
        buf
    }

    /// Leave overflow mode: queue `outputDropped` behind the output that was kept.
    fn end_overflow(&mut self) {
        if let Some(bytes) = self.dropping.take() {
            self.events += 1;
            self.queue
                .push_back(Item::Event(TermEvent::OutputDropped { bytes }));
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    fn data_len(p: &Poll) -> usize {
        p.out
            .iter()
            .map(|o| match o {
                Out::Data(d) => d.len(),
                Out::Event(_) => 0,
            })
            .sum()
    }

    fn bytes(n: usize, fill: u8) -> Vec<u8> {
        vec![fill; n]
    }

    /// Push `total` bytes in 32 KiB packets (russh's typical packet size).
    fn push_n(f: &mut Flow, total: usize, now: Instant) -> usize {
        let mut accepted = 0;
        let mut left = total;
        while left > 0 {
            let n = left.min(32 * 1024);
            if f.push_data(&bytes(n, b'y'), now) {
                accepted += n;
            }
            left -= n;
        }
        accepted
    }

    /// A pump that polls only when [`Flow::push_data_wake`] / [`Flow::ack`] say so, or
    /// when its `wake_at` is reached, never misses deliverable output. Checked after every step
    /// of a pseudo-random mix of packet sizes, acks and clock advances: whenever the lazy pump
    /// would stay asleep, an eager poll must deliver nothing.
    #[test]
    fn wake_hints_never_miss_deliverable_output() {
        let mut seed: u64 = 0x5eed_1234_abcd_0001;
        let mut rand = move |n: u64| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % n
        };
        let mut now = Instant::now();
        let mut f = Flow::new();
        let mut wake_at: Option<Instant> = None;
        let mut pump_wakes = 0u32;
        let mut pushes = 0u32;
        let mut delivered = 0usize;
        for step in 0..20_000 {
            let woken = match rand(10) {
                0..=5 => {
                    // Mostly tiny packets, sometimes large ones (up to 3 chunks).
                    let n = if rand(8) == 0 {
                        1 + rand(3 * MAX_CHUNK as u64) as usize
                    } else {
                        1 + rand(64) as usize
                    };
                    pushes += 1;
                    f.push_data_wake(&bytes(n, b'z'), now)
                }
                6 | 7 => f.ack(rand(2 * MAX_CHUNK as u64)),
                8 => {
                    now += Duration::from_millis(rand(6));
                    false
                }
                _ => {
                    f.push_event(TermEvent::Reconnected);
                    true // push_event callers always wake
                }
            };
            let due = wake_at.is_some_and(|at| now >= at);
            if woken || due {
                pump_wakes += 1;
                let p = f.poll(now);
                delivered += data_len(&p);
                wake_at = p.wake_at;
            } else {
                let p = f.poll(now);
                assert!(
                    p.out.is_empty(),
                    "step {step}: the pump slept through deliverable output"
                );
                let in_time = match (p.wake_at, wake_at) {
                    (None, _) => true,
                    (Some(_), None) => false,
                    (Some(needed), Some(planned)) => planned <= needed,
                };
                assert!(in_time, "step {step}: a sleeping pump would wake too late");
            }
        }
        assert!(delivered > 0);
        assert!(
            pump_wakes < pushes,
            "tiny packets must not wake the pump one by one ({pump_wakes} wakes, {pushes} pushes)"
        );
    }

    #[test]
    fn tiny_packets_wake_the_pump_once_per_flush() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        assert!(f.push_data_wake(b"a", t0), "first byte starts the timer");
        for _ in 0..100 {
            assert!(!f.push_data_wake(b"b", t0));
        }
        let p = f.poll(t0 + FLUSH_DELAY);
        assert_eq!(data_len(&p), 101);
        assert!(f.push_data_wake(b"c", t0 + FLUSH_DELAY), "empty again");
        // Reaching FLUSH_BYTES wakes for an immediate flush.
        assert!(!f.push_data_wake(&bytes(FLUSH_BYTES - 2, b'd'), t0));
        assert!(f.push_data_wake(b"e", t0));
        assert!(!f.push_data_wake(b"f", t0), "already over the mark");
    }

    #[test]
    fn ack_wakes_only_when_it_releases_held_output() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        assert!(!f.ack(10), "nothing in flight");
        push_n(&mut f, 6 * 1024 * 1024, t0);
        while f.unacked() < MAX_IN_FLIGHT {
            assert!(!f.poll(t0).out.is_empty());
        }
        assert!(
            f.poll(t0).out.is_empty(),
            "held back by the in-flight limit"
        );
        let over = f.unacked() - MAX_IN_FLIGHT;
        assert!(!f.ack(over), "still at the limit");
        assert!(f.ack(1), "now below the limit");
        assert!(!f.ack(1), "already released");
    }

    #[test]
    fn small_output_waits_for_the_flush_delay() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        assert!(f.push_data(b"hello", t0));
        let p = f.poll(t0);
        assert!(p.out.is_empty());
        assert_eq!(p.wake_at, Some(t0 + FLUSH_DELAY));
        let p = f.poll(t0 + FLUSH_DELAY);
        assert_eq!(data_len(&p), 5);
        assert!(p.wake_at.is_none());
        assert_eq!(f.unacked(), 5);
        assert!(f.is_empty());
    }

    #[test]
    fn large_output_is_flushed_immediately_and_coalesced() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        for _ in 0..3 {
            f.push_data(&bytes(30_000, b'a'), t0);
        }
        let p = f.poll(t0);
        assert_eq!(
            p.out.len(),
            1,
            "consecutive packets coalesce into one message"
        );
        assert_eq!(data_len(&p), 90_000);
    }

    #[test]
    fn chunks_are_capped() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        push_n(&mut f, 1024 * 1024, t0);
        let p = f.poll(t0);
        assert_eq!(p.out.len(), 4);
        assert!(
            p.out
                .iter()
                .all(|o| matches!(o, Out::Data(d) if d.len() == MAX_CHUNK))
        );
    }

    #[test]
    fn a_packet_larger_than_a_chunk_is_split_in_order() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        let big: Vec<u8> = (0..(MAX_CHUNK + 10)).map(|i| (i % 251) as u8).collect();
        f.push_data(&big, t0);
        // The full chunk goes out at once; the 10-byte remainder waits for the flush delay.
        let first = f.poll(t0);
        assert_eq!(data_len(&first), MAX_CHUNK);
        assert_eq!(first.wake_at, Some(t0 + FLUSH_DELAY));
        let rest = f.poll(t0 + FLUSH_DELAY);
        let mut joined = Vec::new();
        for o in first.out.into_iter().chain(rest.out) {
            if let Out::Data(d) = o {
                joined.extend(d);
            }
        }
        assert!(joined == big, "binary-safe and in order");
    }

    #[test]
    fn in_flight_limit_holds_data_until_acked() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        push_n(&mut f, 6 * 1024 * 1024, t0);
        let p = f.poll(t0);
        assert_eq!(data_len(&p) as u64, MAX_IN_FLIGHT);
        assert!(p.wake_at.is_none(), "blocked on acks, not on time");
        assert!(f.poll(t0 + Duration::from_secs(1)).out.is_empty());
        f.ack(MAX_CHUNK as u64);
        assert_eq!(data_len(&f.poll(t0)), MAX_CHUNK);
        f.ack(u64::MAX);
        assert_eq!(f.unacked(), 0);
        assert_eq!(data_len(&f.poll(t0)), 2 * 1024 * 1024 - MAX_CHUNK);
        assert!(f.is_empty());
    }

    #[test]
    fn overflow_discards_new_output_and_reports_once_drained() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        // Nothing acked: 4 MiB go out, then the backlog fills to 8 MiB.
        let accepted = push_n(&mut f, 4 * 1024 * 1024, t0);
        assert_eq!(data_len(&f.poll(t0)) as u64, MAX_IN_FLIGHT);
        let more = push_n(&mut f, 10 * 1024 * 1024, t0);
        assert_eq!(accepted + more - (MAX_IN_FLIGHT as usize), MAX_BACKLOG);
        assert!(f.is_dropping());
        assert_eq!(f.queued_bytes(), MAX_BACKLOG);
        let dropped = 10 * 1024 * 1024 - more;
        // Still nothing acked: more output is discarded, nothing is sent.
        assert!(!f.push_data(&bytes(100, b'z'), t0));
        assert!(f.poll(t0).out.is_empty());
        // The UI catches up: the kept 8 MiB flow out, then the event, then new data.
        let mut events = Vec::new();
        let mut delivered = 0usize;
        while delivered < MAX_BACKLOG {
            f.ack(u64::MAX);
            for o in f.poll(t0).out {
                match o {
                    Out::Data(d) => {
                        assert!(events.is_empty(), "the marker must follow all kept output");
                        delivered += d.len();
                    }
                    Out::Event(e) => events.push(e),
                }
            }
        }
        assert_eq!(delivered, MAX_BACKLOG);
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0], TermEvent::OutputDropped { bytes } if bytes == (dropped + 100) as u64)
        );
        assert!(!f.is_dropping());
        assert!(f.push_data(b"new", t0));
    }

    #[test]
    fn overflow_ends_when_new_output_arrives_below_the_resume_mark() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.unacked = MAX_IN_FLIGHT; // UI stalled
        push_n(&mut f, MAX_BACKLOG, t0);
        assert!(!f.push_data(&bytes(10, b'x'), t0));
        // The UI drains all but 512 KiB of the backlog.
        f.ack(u64::MAX);
        while f.queued_bytes() > 512 * 1024 {
            let chunk = f.take_chunk();
            assert!(!chunk.is_empty());
        }
        // New output: marker first (behind the remaining kept bytes), then the new bytes.
        assert!(f.push_data(b"after", t0));
        f.ack(u64::MAX);
        let p = f.poll(t0 + FLUSH_DELAY);
        let kinds: Vec<&str> = p
            .out
            .iter()
            .map(|o| match o {
                Out::Data(_) => "data",
                Out::Event(TermEvent::OutputDropped { .. }) => "dropped",
                Out::Event(_) => "event",
            })
            .collect();
        assert_eq!(kinds.first(), Some(&"data"));
        let marker = kinds.iter().position(|k| *k == "dropped").unwrap();
        assert_eq!(kinds.len(), marker + 2, "new data follows the marker");
        match &p.out[marker + 1] {
            Out::Data(d) => assert_eq!(d.as_slice(), b"after"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn memory_stays_bounded_under_a_flood() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        for _ in 0..50 {
            push_n(&mut f, 1024 * 1024, t0);
            let _ = f.poll(t0);
            assert!(f.queued_bytes() <= MAX_BACKLOG);
            assert!(f.unacked() <= MAX_IN_FLIGHT + MAX_CHUNK as u64);
        }
    }

    #[test]
    fn events_flush_pending_data_first() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.push_data(b"bye\r\n", t0);
        f.push_event(TermEvent::Exit {
            code: Some(3),
            signal: None,
        });
        let p = f.poll(t0);
        assert_eq!(p.out.len(), 2);
        assert!(matches!(&p.out[0], Out::Data(d) if d.as_slice() == b"bye\r\n"));
        assert!(matches!(
            p.out[1],
            Out::Event(TermEvent::Exit {
                code: Some(3),
                signal: None
            })
        ));
    }

    #[test]
    fn events_wait_behind_unacked_data() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.unacked = MAX_IN_FLIGHT;
        f.push_data(b"tail", t0);
        f.push_event(TermEvent::Closed { error: None });
        assert!(f.poll(t0).out.is_empty(), "order is kept even when blocked");
        f.ack(1);
        let p = f.poll(t0);
        assert_eq!(p.out.len(), 2);
    }

    #[test]
    fn leading_events_go_out_without_acks() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.unacked = MAX_IN_FLIGHT;
        f.push_event(TermEvent::Reconnected);
        let p = f.poll(t0);
        assert!(matches!(p.out[..], [Out::Event(TermEvent::Reconnected)]));
    }

    fn kinds(out: &[Out]) -> Vec<&'static str> {
        out.iter()
            .map(|o| match o {
                Out::Data(_) => "data",
                Out::Event(TermEvent::OutputDropped { .. }) => "dropped",
                Out::Event(TermEvent::Exit { .. }) => "exit",
                Out::Event(TermEvent::Reconnected) => "reconnected",
                Out::Event(TermEvent::Closed { .. }) => "closed",
            })
            .collect()
    }

    /// Drain everything (acking as the UI does); data runs are merged to one "data" entry.
    fn drain(f: &mut Flow, t: Instant) -> (Vec<&'static str>, Vec<Out>) {
        let mut all = Vec::new();
        loop {
            f.ack(u64::MAX);
            let p = f.poll(t + FLUSH_DELAY);
            if p.out.is_empty() {
                break;
            }
            all.extend(p.out);
        }
        let mut k = kinds(&all);
        k.dedup();
        (k, all)
    }

    /// The shell exits while output is being discarded ⇒ `outputDropped` comes before
    /// `exit` (the lost bytes preceded it), after all kept output.
    #[test]
    fn overflow_then_exit_reports_the_drop_before_the_exit() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.unacked = MAX_IN_FLIGHT; // UI stalled
        push_n(&mut f, MAX_BACKLOG + 2 * 1024 * 1024, t0);
        assert!(f.is_dropping());
        f.push_event(TermEvent::Exit {
            code: Some(0),
            signal: None,
        });
        assert!(!f.is_dropping(), "the event is a boundary");
        let (k, all) = drain(&mut f, t0);
        assert_eq!(k, ["data", "dropped", "exit"]);
        assert!(all.iter().any(|o| matches!(
            o,
            Out::Event(TermEvent::OutputDropped { bytes }) if *bytes == 2 * 1024 * 1024
        )));
    }

    /// After a respawn (`boundary` + `reconnected`, as `Terminal::start` does) or a
    /// restart (`boundary`) during an overflow, the stale backlog of the old shell is dropped and
    /// counted, the marker comes first, and the new shell's output is accepted and delivered.
    #[test]
    fn overflow_ends_at_a_new_shell() {
        for respawn in [true, false] {
            let t0 = Instant::now();
            let mut f = Flow::new();
            f.unacked = MAX_IN_FLIGHT;
            push_n(&mut f, MAX_BACKLOG + 1024, t0);
            assert!(f.is_dropping());
            f.boundary();
            if respawn {
                f.push_event(TermEvent::Reconnected);
            }
            assert!(f.push_data(b"new-shell$ ", t0), "new output is accepted");
            let (k, all) = drain(&mut f, t0);
            if respawn {
                assert_eq!(k, ["dropped", "reconnected", "data"]);
            } else {
                assert_eq!(k, ["dropped", "data"]);
            }
            assert!(all.iter().any(|o| matches!(
                o,
                Out::Event(TermEvent::OutputDropped { bytes }) if *bytes == (MAX_BACKLOG + 1024) as u64
            )));
            match all.last() {
                Some(Out::Data(d)) => assert!(d.ends_with(b"new-shell$ ")),
                other => panic!("unexpected {other:?}"),
            }
        }
    }

    /// A boundary without an overflow queues nothing and keeps the old shell's output (delivered
    /// before the new shell's).
    #[test]
    fn boundary_without_overflow_keeps_old_output() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.boundary();
        assert!(f.is_empty());
        f.push_data(b"old-exit\r\n", t0);
        f.push_event(TermEvent::Exit {
            code: Some(0),
            signal: None,
        });
        f.boundary();
        f.push_data(b"new$ ", t0);
        let (k, all) = drain(&mut f, t0);
        assert_eq!(k, ["data", "exit", "data"]);
        assert!(matches!(all.first(), Some(Out::Data(d)) if d.as_slice() == b"old-exit\r\n"));
    }

    /// A flood of 1-byte packets (≥ 9 MiB) while the UI is stalled keeps the heap close
    /// to the payload cap: full 64 KiB buffers, not one allocation per packet.
    #[test]
    fn tiny_packets_do_not_multiply_memory() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.unacked = MAX_IN_FLIGHT;
        for i in 0..(9 * 1024 * 1024 + 7) {
            f.push_data(&[(i % 251) as u8], t0);
        }
        assert_eq!(f.queued_bytes(), MAX_BACKLOG);
        assert!(f.is_dropping());
        let items = f.queue.len();
        assert!(items <= MAX_BACKLOG / TAIL_MAX + 1, "{items} items");
        let heap = f.heap_bytes();
        assert!(
            heap <= MAX_BACKLOG + TAIL_MAX + 64 * 1024,
            "heap {heap} for {} payload bytes",
            f.queued_bytes()
        );
        // Content and order survive the coalescing.
        f.ack(u64::MAX);
        let first = f.take_chunk();
        assert!(first.iter().enumerate().all(|(i, b)| *b == (i % 251) as u8));
    }

    #[test]
    fn mixed_packet_sizes_fill_buffers_completely() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        f.unacked = MAX_IN_FLIGHT;
        let mut expected = Vec::new();
        for (i, n) in [1usize, 40_000, 3, 100_000, 7, 32_768, 1]
            .iter()
            .cycle()
            .take(70)
            .enumerate()
        {
            let chunk = vec![(i % 256) as u8; *n];
            f.push_data(&chunk, t0);
            expected.extend(chunk);
        }
        assert!(f.heap_bytes() <= f.queued_bytes() + 2 * TAIL_MAX + 64 * 1024);
        let (_, all) = drain(&mut f, t0);
        let got: Vec<u8> = all
            .into_iter()
            .filter_map(|o| match o {
                Out::Data(d) => Some(d),
                Out::Event(_) => None,
            })
            .flatten()
            .collect();
        assert!(got == expected, "byte stream unchanged");
    }

    #[test]
    fn clear_forgets_everything() {
        let t0 = Instant::now();
        let mut f = Flow::new();
        push_n(&mut f, 9 * 1024 * 1024, t0);
        assert!(!f.poll(t0).out.is_empty());
        f.clear();
        assert_eq!(f.unacked(), 0);
        assert!(f.is_empty());
        assert_eq!(f.queued_bytes(), 0);
        assert!(!f.is_dropping());
    }
}
