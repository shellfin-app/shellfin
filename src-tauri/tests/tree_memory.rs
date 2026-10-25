//! Peak heap of the folder-download scan (`sftp::tree::scan`) on hostile trees, measured with a
//! counting global allocator (like `listing_memory`): a million entries with 4 KiB names, and
//! a wide tree of tiny names, stay within the manifest budget (64 MiB charged) plus bounded
//! slack, and are refused before anything else happens. Own test binary, one test, so nothing
//! else allocates concurrently.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use russh_sftp::protocol::FileAttributes;
use shellfin_lib::error::{AppError, ErrorCode, TreeLimit};
use shellfin_lib::ipc::dto::SkipReason;
use shellfin_lib::sftp::tree::{
    MAX_ENTRIES, MAX_MANIFEST_BYTES, RawEntry, RemoteDirSource, ScanEvents, ScanStats, scan,
};

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

/// A generated tree: the root holds `dirs` directories, each with `files` files whose names
/// are `name_len` bytes. Nothing is materialised up front.
struct Gen {
    dirs: usize,
    files: usize,
    name_len: usize,
}

struct H {
    root: bool,
    next: Mutex<usize>,
}

fn attrs(perm: u32) -> FileAttributes {
    FileAttributes {
        permissions: Some(perm),
        size: Some(1),
        mtime: Some(1_700_000_000),
        ..FileAttributes::empty()
    }
}

impl RemoteDirSource for Gen {
    type Handle = H;
    async fn open_dir(&self, path: &str) -> Result<H, AppError> {
        Ok(H {
            root: path == "/r",
            next: Mutex::new(0),
        })
    }
    async fn read_batch(&self, h: &H, _: &str) -> Result<Option<Vec<RawEntry>>, AppError> {
        let mut next = h.next.lock().unwrap();
        let (count, perm) = if h.root {
            (self.dirs, 0o040_755)
        } else {
            (self.files, 0o100_644)
        };
        if *next >= count {
            return Ok(None);
        }
        let end = (*next + 200).min(count);
        // Unique in the first bytes (local names are cut to 200 bytes); short directory names
        // keep the paths under 4096 bytes.
        let batch = (*next..end)
            .map(|i| RawEntry {
                name: if h.root {
                    format!("d{i}")
                } else {
                    format!("{i:08}{}", "x".repeat(self.name_len.saturating_sub(8)))
                },
                attrs: attrs(perm),
            })
            .collect();
        *next = end;
        Ok(Some(batch))
    }
    async fn close_dir(&self, _: H) {}
    async fn lstat(&self, _: &str) -> Result<FileAttributes, AppError> {
        Err(AppError::new(ErrorCode::Internal))
    }
}

struct Quiet;
impl ScanEvents for Quiet {
    fn progress(&mut self, _: &ScanStats) {}
    fn skipped(&mut self, _: &str, _: SkipReason) {}
    fn failed(&mut self, _: u32, _: &str, _: &AppError) {}
}

/// Scan `g`; returns the error and the peak heap above the start.
async fn measure(g: &Gen) -> (AppError, isize) {
    let (_tx, rx) = tokio::sync::watch::channel(false);
    let base = LIVE.load(Ordering::Relaxed);
    PEAK.store(base, Ordering::Relaxed);
    let r = scan(g, "/r", "r", rx, &mut Quiet).await;
    let peak = PEAK.load(Ordering::Relaxed) - base;
    (r.expect_err("the tree must be refused"), peak)
}

#[tokio::test]
async fn hostile_trees_stay_within_the_manifest_budget() {
    // 1 000 × 1 000 entries with 4 KiB names: the memory budget refuses it.
    let (e, peak) = measure(&Gen {
        dirs: 1_000,
        files: 1_000,
        name_len: 4096,
    })
    .await;
    assert_eq!(
        e.kind,
        ErrorCode::TreeTooLarge {
            limit: TreeLimit::Memory,
            value: MAX_MANIFEST_BYTES as u64
        }
    );
    eprintln!("4 KiB names: peak {} MiB", peak / MIB);
    assert!(peak < 64 * MIB + 32 * MIB, "peak {} MiB", peak / MIB);

    // 1 000 × 1 000 tiny names: the entry cap (200 000) refuses it.
    let (e, peak) = measure(&Gen {
        dirs: 1_000,
        files: 1_000,
        name_len: 8,
    })
    .await;
    assert_eq!(
        e.kind,
        ErrorCode::TreeTooLarge {
            limit: TreeLimit::Entries,
            value: MAX_ENTRIES as u64
        }
    );
    eprintln!("tiny names: peak {} MiB", peak / MIB);
    assert!(peak < 64 * MIB, "peak {} MiB", peak / MIB);
}
