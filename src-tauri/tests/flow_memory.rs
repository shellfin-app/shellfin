//! Real heap held by the terminal backlog under a flood of tiny packets, measured with a
//! counting global allocator (its own test binary, one test, so nothing else allocates
//! concurrently).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use shellfin_lib::terminal::flow::{Flow, MAX_BACKLOG, MAX_IN_FLIGHT};

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);

// SAFETY: forwards to the system allocator; only adds bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        LIVE.fetch_add(
            new_size as isize - layout.size() as isize,
            Ordering::Relaxed,
        );
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

#[test]
fn one_byte_packet_flood_keeps_heap_near_the_cap() {
    let now = tokio::time::Instant::now();
    let mut flow = Flow::new();
    // Stall the UI: nothing is flushed, everything goes to the backlog.
    let filler = vec![b'f'; MAX_IN_FLIGHT as usize];
    flow.push_data(&filler, now);
    let _ = flow.poll(now);
    drop(filler);

    let before = LIVE.load(Ordering::Relaxed);
    // 9 MiB as 1-byte packets, built the way russh hands them over (one allocation each).
    for i in 0..(9 * 1024 * 1024usize) {
        let packet = bytes::Bytes::from(vec![(i % 251) as u8]);
        flow.push_data(&packet, now);
    }
    let held = LIVE.load(Ordering::Relaxed) - before;
    assert_eq!(flow.queued_bytes(), MAX_BACKLOG);
    assert!(flow.is_dropping());
    eprintln!("flow_memory: {held} heap bytes for {MAX_BACKLOG} payload bytes");
    assert!(
        held <= (MAX_BACKLOG as isize) * 2,
        "heap {held} > 2 × MAX_BACKLOG"
    );
}
