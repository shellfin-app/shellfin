//! Peak heap of one hostile directory listing (the real `ListingBuilder` + the IPC JSON
//! serialisation), measured with a counting global allocator. Own test binary, one test, so
//! nothing else allocates concurrently.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use russh_sftp::protocol::FileAttributes;
use shellfin_lib::sftp::client::{LIST_MAX_BYTES, LIST_MAX_ENTRIES, ListingBuilder};

struct Counting;
static LIVE: AtomicIsize = AtomicIsize::new(0);
static PEAK: AtomicIsize = AtomicIsize::new(0);

fn add(delta: isize) {
    let now = LIVE.fetch_add(delta, Ordering::Relaxed) + delta;
    PEAK.fetch_max(now, Ordering::Relaxed);
}

// SAFETY: forwards to the system allocator; only adds bookkeeping.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        add(layout.size() as isize);
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        add(-(layout.size() as isize));
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        add(new_size as isize - layout.size() as isize);
        // SAFETY: same contract as the caller's.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const MIB: isize = 1024 * 1024;

/// Builds a listing from `names` (as `readdir` would deliver them) and serialises it like the IPC
/// layer; returns (entries, truncated, json bytes, peak heap above the start).
fn measure(dir: &str, names: impl Iterator<Item = String>) -> (usize, bool, usize, isize) {
    let attrs = FileAttributes {
        permissions: Some(0o100_644),
        size: Some(1),
        mtime: Some(1_700_000_000),
        uid: Some(1000),
        gid: Some(1000),
        ..FileAttributes::empty()
    };
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let mut b = ListingBuilder::new(dir);
    for n in names {
        if !b.push(n, &attrs) {
            break;
        }
    }
    let listing = b.finish();
    let json = serde_json::to_string(&listing).unwrap();
    let out = (
        listing.entries.len(),
        listing.truncated,
        json.len(),
        PEAK.load(Ordering::Relaxed) - base,
    );
    drop(json);
    drop(listing);
    out
}

#[test]
fn hostile_listings_stay_within_the_budget() {
    let budget = LIST_MAX_BYTES as isize;

    // A: a 4 KiB directory and 100 000+ tiny names (~16 bytes each on the wire). Before the fix:
    // ~1.3 GB peak.
    let long_dir = format!("/{}", "d".repeat(4095));
    let (n, truncated, json, peak) = measure(
        &long_dir,
        (0..LIST_MAX_ENTRIES + 1).map(|i| format!("{i:x}")),
    );
    eprintln!(
        "A: entries={n} truncated={truncated} json={json} peak={} MiB",
        peak / MIB
    );
    assert!(truncated);
    assert!(json as isize <= budget);
    assert!(peak <= 2 * budget, "peak {peak}");

    // B: control-character names (6-byte JSON escapes, in `name` and `path`).
    let (n, truncated, json, peak) = measure(
        "/tmp",
        (0..LIST_MAX_ENTRIES + 1).map(|i| format!("{}{i}", "\u{1}".repeat(600))),
    );
    eprintln!(
        "B: entries={n} truncated={truncated} json={json} peak={} MiB",
        peak / MIB
    );
    assert!(truncated);
    assert!(json as isize <= budget);
    assert!(peak <= 2 * budget, "peak {peak}");

    // C: the normal case keeps the full 100 000 entries, well below the budget.
    let (n, truncated, json, peak) = measure(
        "/home/user",
        (0..LIST_MAX_ENTRIES + 1).map(|i| format!("file-{i:06}")),
    );
    eprintln!(
        "C: entries={n} truncated={truncated} json={json} peak={} MiB",
        peak / MIB
    );
    assert_eq!((n, truncated), (LIST_MAX_ENTRIES, true));
    assert!(peak <= 2 * budget, "peak {peak}");
}
