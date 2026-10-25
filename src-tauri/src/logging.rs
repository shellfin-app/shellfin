//! Logging (§9.6): `tracing` to a daily-rolling file in `app_log_dir` (7 files kept, at most
//! [`MAX_LOG_BYTES_PER_DAY`] each), the `log` → `tracing` bridge (captures russh), a panic hook
//! that records the location only, and redaction helpers.
//!
//! Never log terminal data, file contents, secrets (type-enforced via `Secret`), kbd-int
//! responses or key material. Every field value (and so any server-originated text inside an
//! error) goes through [`sanitize_for_log`] in the field formatter.

use std::path::Path;
use std::sync::{Mutex, PoisonError};

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{Builder as RollingBuilder, Rotation};
use tracing_subscriber::field::MakeExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Registry, fmt, reload};

use crate::ipc::dto::LogLevel;

/// Default filter; `RUST_LOG` overrides it in debug builds only.
pub const DEFAULT_FILTER: &str = "info,shellfin=info,russh=warn,russh_sftp=warn";
pub const LOG_FILE_PREFIX: &str = "shellfin";
pub const LOG_FILE_SUFFIX: &str = "log";
pub const MAX_LOG_FILES: usize = 7;

/// Keeps the non-blocking writer alive; dropped by [`flush`] before exit.
static GUARD: Mutex<Option<WorkerGuard>> = Mutex::new(None);

#[derive(Debug, thiserror::Error)]
pub enum LogInitError {
    #[error("cannot create the log file appender: {0}")]
    Appender(#[from] tracing_appender::rolling::InitError),
    #[error("a global tracing subscriber is already installed: {0}")]
    Subscriber(#[from] tracing_subscriber::util::TryInitError),
}

/// Filter for `advanced.logLevel = "debug"`: the app's own debug lines; dependencies (russh may
/// trace protocol details) stay at `warn`.
pub const DEBUG_FILTER: &str = "info,shellfin=debug,shellfin_lib=debug,russh=warn,russh_sftp=warn";

type FilterHandle = reload::Handle<EnvFilter, Registry>;

/// Handle to change the filter at runtime (`advanced.logLevel`).
static FILTER_HANDLE: Mutex<Option<FilterHandle>> = Mutex::new(None);

fn filter() -> EnvFilter {
    if cfg!(debug_assertions) {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER))
    } else {
        EnvFilter::new(DEFAULT_FILTER)
    }
}

/// The filter text for a level.
pub const fn filter_text(level: LogLevel) -> &'static str {
    match level {
        LogLevel::Info => DEFAULT_FILTER,
        LogLevel::Debug => DEBUG_FILTER,
    }
}

/// Apply `advanced.logLevel` live. A no-op before the subscriber is installed.
pub fn set_level(level: LogLevel) {
    let guard = FILTER_HANDLE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(handle) = guard.as_ref() else {
        return;
    };
    let new = match level {
        LogLevel::Info => filter(),
        LogLevel::Debug => EnvFilter::new(DEBUG_FILTER),
    };
    if let Err(e) = handle.reload(new) {
        tracing::warn!(error = %e, "could not change the log level");
    } else {
        tracing::info!(level = ?level, "log level changed");
    }
}

/// Longest field value written to the log (characters, after escaping starts).
pub const MAX_FIELD_CHARS: usize = 4096;

/// Field formatter of every log layer: each field value, including the message, goes
/// through [`sanitize_for_log`], so text that came from a server (an SFTP status message inside
/// an error, a disconnect reason) can neither forge log lines with newlines nor carry terminal
/// escapes or bidi controls, and is bounded in length.
fn safe_fields() -> impl for<'w> fmt::FormatFields<'w> + Send + Sync + 'static {
    fmt::format::debug_fn(|w, field, value| {
        let text = sanitize_for_log(&format!("{value:?}"), MAX_FIELD_CHARS);
        if field.name() == "message" {
            write!(w, "{text}")
        } else {
            write!(w, "{}={text}", field.name())
        }
    })
    .delimited(" ")
}

/// Most bytes written to the log per (UTC) day: with [`MAX_LOG_FILES`] daily files the
/// log directory stays below 7 × 32 MiB even if something logs in a loop.
pub const MAX_LOG_BYTES_PER_DAY: u64 = 32 * 1024 * 1024;

/// Days since the Unix epoch (UTC), the period of the daily rotation.
fn utc_day() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() / 86_400)
        .unwrap_or(0)
}

/// `YYYY-MM-DD` of a day number (the date in the rolling appender's file names).
pub(crate) fn civil_date(day: u64) -> String {
    // Howard Hinnant's days-to-civil algorithm, for days since 1970-01-01.
    let z = i64::try_from(day).unwrap_or(0) + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Wraps the rolling file writer and stops writing once [`MAX_LOG_BYTES_PER_DAY`] were written
/// today (one note line marks the cut); the next day starts fresh. The count starts from the size
/// of today's file, so restarts do not reset it.
struct CappedWriter<W> {
    inner: W,
    cap: u64,
    day: u64,
    written: u64,
    noted: bool,
}

impl<W: std::io::Write> CappedWriter<W> {
    fn write_on(&mut self, buf: &[u8], day: u64) -> std::io::Result<usize> {
        if day != self.day {
            self.day = day;
            self.written = 0;
            self.noted = false;
        }
        if self.written.saturating_add(buf.len() as u64) > self.cap {
            if !self.noted {
                self.noted = true;
                let note = b"log size limit for today reached; further lines are dropped\n";
                self.inner.write_all(note)?;
                self.written = self.written.saturating_add(note.len() as u64);
            }
            return Ok(buf.len());
        }
        let n = self.inner.write(buf)?;
        self.written = self.written.saturating_add(n as u64);
        Ok(n)
    }
}

impl<W: std::io::Write> std::io::Write for CappedWriter<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.write_on(buf, utc_day())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Install the global subscriber writing to `<log_dir>/shellfin.<date>.log`.
/// The log directory must already exist with private permissions (`platform::harden_process`).
pub fn init(log_dir: &Path) -> Result<(), LogInitError> {
    let appender = RollingBuilder::new()
        .rotation(Rotation::DAILY)
        .filename_prefix(LOG_FILE_PREFIX)
        .filename_suffix(LOG_FILE_SUFFIX)
        .max_log_files(MAX_LOG_FILES)
        .build(log_dir)?;
    let day = utc_day();
    let today = log_dir.join(format!(
        "{LOG_FILE_PREFIX}.{}.{LOG_FILE_SUFFIX}",
        civil_date(day)
    ));
    let capped = CappedWriter {
        inner: appender,
        cap: MAX_LOG_BYTES_PER_DAY,
        day,
        written: std::fs::metadata(today).map(|m| m.len()).unwrap_or(0),
        noted: false,
    };
    let (writer, guard) = tracing_appender::non_blocking(capped);
    let stderr_layer = cfg!(debug_assertions).then(|| {
        fmt::layer()
            .fmt_fields(safe_fields())
            .with_writer(std::io::stderr)
    });
    let (filter_layer, handle) = reload::Layer::new(filter());
    tracing_subscriber::registry()
        .with(filter_layer)
        .with(
            fmt::layer()
                .fmt_fields(safe_fields())
                .with_writer(writer)
                .with_ansi(false),
        )
        .with(stderr_layer)
        .try_init()?;
    *FILTER_HANDLE.lock().unwrap_or_else(PoisonError::into_inner) = Some(handle);
    *GUARD.lock().unwrap_or_else(PoisonError::into_inner) = Some(guard);
    Ok(())
}

/// Fallback when the file appender cannot be created: stderr only (debug builds show it).
pub fn init_stderr_only() {
    let _ = tracing_subscriber::registry()
        .with(filter())
        .with(
            fmt::layer()
                .fmt_fields(safe_fields())
                .with_writer(std::io::stderr),
        )
        .try_init();
}

/// Flush and stop the background log writer (called right before `AppHandle::exit`).
pub fn flush() {
    let guard = GUARD.lock().unwrap_or_else(PoisonError::into_inner).take();
    drop(guard);
}

/// Most lines of the log tail an error report may include (§9.6).
pub const LOG_TAIL_MAX_LINES: usize = 200;
/// Most bytes read from the end of the log file for an error report.
pub const LOG_TAIL_MAX_BYTES: u64 = 64 * 1024;

/// `shellfin.YYYY-MM-DD.log`, the names the daily appender writes.
fn is_log_file_name(name: &str) -> bool {
    let Some(date) = name
        .strip_prefix(LOG_FILE_PREFIX)
        .and_then(|r| r.strip_prefix('.'))
        .and_then(|r| r.strip_suffix(LOG_FILE_SUFFIX))
        .and_then(|r| r.strip_suffix('.'))
    else {
        return false;
    };
    date.len() == 10
        && date.char_indices().all(|(i, c)| match i {
            4 | 7 => c == '-',
            _ => c.is_ascii_digit(),
        })
}

/// The current log file: the newest `shellfin.<date>.log` in the app's own log directory that is
/// a regular file (not a link). Never a path from the WebView (§9.4).
pub fn current_log_file(log_dir: &Path) -> Option<std::path::PathBuf> {
    let entries = std::fs::read_dir(log_dir).ok()?;
    entries
        .filter_map(Result::ok)
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            let regular = std::fs::symlink_metadata(e.path())
                .ok()?
                .file_type()
                .is_file();
            (regular && is_log_file_name(&name)).then_some(name)
        })
        // The date sorts lexicographically.
        .max()
        .map(|name| log_dir.join(name))
}

/// The last complete lines of a log file: at most `max_lines` lines from the last `max_bytes`
/// bytes, oldest first. A line cut by the byte window and an unterminated last line (a write in
/// progress) are dropped, so no value is ever cut in half; invalid UTF-8 becomes U+FFFD. A missing
/// file is an error; an empty file gives no lines.
pub fn read_log_tail(
    path: &Path,
    max_lines: usize,
    max_bytes: u64,
) -> std::io::Result<Vec<String>> {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    let start = len.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::new();
    file.take(max_bytes).read_to_end(&mut bytes)?;
    let mut window = bytes.as_slice();
    if start > 0 {
        // The first line started before the window.
        window = match window.iter().position(|&b| b == b'\n') {
            Some(i) => window.get(i + 1..).unwrap_or_default(),
            None => &[],
        };
    }
    // Only complete lines (the last one ends with a newline).
    window = match window.iter().rposition(|&b| b == b'\n') {
        Some(i) => window.get(..i).unwrap_or_default(),
        None => &[],
    };
    if window.is_empty() {
        return Ok(Vec::new());
    }
    let text = String::from_utf8_lossy(window);
    let lines: Vec<&str> = text.split('\n').collect();
    let skip = lines.len().saturating_sub(max_lines);
    Ok(lines
        .into_iter()
        .skip(skip)
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_owned())
        .collect())
}

/// Where a panic happened (`file`, `line`, `column`); never its message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanicLocation {
    /// Source file as compiled (relative for this crate, a build-machine path for dependencies).
    pub file: String,
    pub line: u32,
    pub column: u32,
}

impl std::fmt::Display for PanicLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.column)
    }
}

thread_local! {
    /// The location of the last panic on this thread (set by the hook, taken by the code that
    /// catches the panic: `commands::guarded`). The unwinding runs on the panicking thread, so the
    /// catch site reads its own panic.
    static LAST_PANIC: std::cell::Cell<Option<PanicLocation>> = const { std::cell::Cell::new(None) };
}

/// Record a panic location for [`take_panic_location`] (the panic hook calls it).
pub fn record_panic_location(location: Option<PanicLocation>) {
    LAST_PANIC.with(|last| last.set(location));
}

/// The location of the last panic on this thread, if any (cleared by the call).
pub fn take_panic_location() -> Option<PanicLocation> {
    LAST_PANIC.with(std::cell::Cell::take)
}

/// Tests: record panic locations like [`install_panic_hook`], but keep the default hook's output
/// so failing tests still print their messages. Idempotent.
#[cfg(test)]
pub(crate) fn install_test_panic_hook() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            record_panic_location(info.location().map(|loc| PanicLocation {
                file: loc.file().to_owned(),
                line: loc.line(),
                column: loc.column(),
            }));
            previous(info);
        }));
    });
}

/// Panic hook: logs file:line only — never the payload (it may contain secrets or server data).
/// The location is also kept for the thread's catch site (`commands::guarded`).
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        match info.location() {
            Some(loc) => tracing::error!(
                file = loc.file(),
                line = loc.line(),
                column = loc.column(),
                "panic"
            ),
            None => tracing::error!("panic at an unknown location"),
        }
        record_panic_location(info.location().map(|loc| PanicLocation {
            file: loc.file().to_owned(),
            line: loc.line(),
            column: loc.column(),
        }));
    }));
}

/// Placeholder written instead of a redacted value.
pub const REDACTED: &str = "***";

fn is_bidi_control(c: char) -> bool {
    matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{200E}' | '\u{200F}' | '\u{061C}')
}

/// Make untrusted (server-originated) text safe for a single log line: C0/C1 controls and bidi
/// controls become visible escapes, and the result is truncated to `max_chars` with `…`.
pub fn sanitize_for_log(s: &str, max_chars: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max_chars.saturating_mul(4)));
    for (count, c) in s.chars().enumerate() {
        if count >= max_chars {
            out.push('…');
            break;
        }
        if c.is_control() || is_bidi_control(c) {
            let code = u32::from(c);
            if code <= 0xFF {
                out.push_str(&format!("\\x{code:02x}"));
            } else {
                out.push_str(&format!("\\u{{{code:04x}}}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn debug_filter_keeps_dependencies_quiet() {
        assert_eq!(filter_text(LogLevel::Info), DEFAULT_FILTER);
        // russh / russh_sftp must never be raised above warn by the debug level.
        assert!(DEBUG_FILTER.contains("russh=warn") && DEBUG_FILTER.contains("russh_sftp=warn"));
        assert!(EnvFilter::try_new(DEBUG_FILTER).is_ok());
        // Without an installed subscriber this is a harmless no-op.
        set_level(LogLevel::Debug);
    }

    #[test]
    fn sanitize_escapes_controls_and_bidi() {
        assert_eq!(sanitize_for_log("a\x1b[31mb\nc", 100), "a\\x1b[31mb\\x0ac");
        assert_eq!(
            sanitize_for_log("x\u{202E}y\u{9b}", 100),
            "x\\u{202e}y\\x9b"
        );
        assert_eq!(sanitize_for_log("Привіт ✓", 100), "Привіт ✓");
    }

    /// Server text inside a logged field cannot forge a log line or carry escapes.
    #[test]
    fn log_fields_are_sanitized() {
        use std::sync::{Arc, Mutex};
        #[derive(Clone, Default)]
        struct Buf(Arc<Mutex<Vec<u8>>>);
        impl std::io::Write for Buf {
            fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(b);
                Ok(b.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let buf = Buf::default();
        let sink = buf.clone();
        let subscriber = tracing_subscriber::registry().with(
            fmt::layer()
                .fmt_fields(safe_fields())
                .with_ansi(false)
                .with_writer(move || sink.clone()),
        );
        let server_text = "denied\nFAKE ERROR injected\x1b[2J\u{202E}";
        tracing::subscriber::with_default(subscriber, || {
            tracing::warn!(error = %server_text, n = 3, "save failed: {}", server_text);
        });
        let out = String::from_utf8(buf.0.lock().unwrap().clone()).unwrap();
        assert_eq!(out.lines().count(), 1, "{out}");
        assert!(!out.contains('\x1b') && !out.contains('\u{202E}'), "{out}");
        assert!(out.contains("error=denied\\x0aFAKE ERROR"), "{out}");
        assert!(out.contains("save failed: denied\\x0a"), "{out}");
        assert!(out.contains(" n=3"), "{out}");
        assert!(out.contains("\\u{202e}"), "{out}");
    }

    #[test]
    fn capped_writer_stops_at_the_daily_limit_and_resets_next_day() {
        let mut w = CappedWriter {
            inner: Vec::new(),
            cap: 100,
            day: 10,
            written: 90,
            noted: false,
        };
        assert_eq!(w.write_on(b"0123456789", 10).unwrap(), 10);
        assert_eq!(w.inner.len(), 10);
        // Over the cap: reported as written (the logger never fails), one note, then nothing.
        assert_eq!(w.write_on(b"x", 10).unwrap(), 1);
        assert_eq!(w.write_on(b"y", 10).unwrap(), 1);
        let text = String::from_utf8(w.inner.clone()).unwrap();
        assert!(text.ends_with("further lines are dropped\n"), "{text}");
        assert_eq!(text.matches("limit").count(), 1);
        // A new day starts fresh.
        let before = w.inner.len();
        assert_eq!(w.write_on(b"next day", 11).unwrap(), 8);
        assert_eq!(w.inner.len(), before + 8);
    }

    #[test]
    fn civil_dates_match_the_rolling_file_names() {
        assert_eq!(civil_date(0), "1970-01-01");
        assert_eq!(civil_date(11_016), "2000-02-29");
        assert_eq!(civil_date(20_747), "2026-10-21");
        assert_eq!(civil_date(utc_day()).len(), 10);
    }

    fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    #[test]
    fn log_file_names_are_recognised_strictly() {
        assert!(is_log_file_name("shellfin.2026-10-23.log"));
        for bad in [
            "shellfin.log",
            "shellfin.2026-10-2.log",
            "shellfin.2026_10_23.log",
            "shellfin.2026-10-23.log.bak",
            "other.2026-10-23.log",
            "shellfin.2026-10-23.txt",
            "shellfin.../../x.log",
            "shellfin.２０２６-10-23.log",
        ] {
            assert!(!is_log_file_name(bad), "{bad}");
        }
    }

    #[test]
    fn current_log_file_is_the_newest_dated_file() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(current_log_file(dir.path()), None, "empty dir");
        assert_eq!(current_log_file(&dir.path().join("missing")), None);
        write(dir.path(), "shellfin.2026-10-21.log", b"old\n");
        let newest = write(dir.path(), "shellfin.2026-10-23.log", b"new\n");
        write(dir.path(), "shellfin.2026-10-22.log", b"mid\n");
        write(dir.path(), "zzz.2099-01-01.log", b"not ours\n");
        std::fs::create_dir(dir.path().join("shellfin.2099-01-01.log")).unwrap();
        assert_eq!(current_log_file(dir.path()), Some(newest));
    }

    #[test]
    fn log_tail_keeps_complete_lines_only() {
        let dir = tempfile::tempdir().unwrap();
        // Missing file: an error, the report then says "no log".
        assert!(read_log_tail(&dir.path().join("none.log"), 10, 1024).is_err());
        // Empty file, and a file with no complete line.
        let p = write(dir.path(), "a.log", b"");
        assert!(read_log_tail(&p, 10, 1024).unwrap().is_empty());
        let p = write(dir.path(), "b.log", b"no newline yet");
        assert!(read_log_tail(&p, 10, 1024).unwrap().is_empty());
        // The unterminated last line (a write in progress) is dropped; CRLF is trimmed.
        let p = write(dir.path(), "c.log", b"one\r\ntwo\nthree");
        assert_eq!(read_log_tail(&p, 10, 1024).unwrap(), ["one", "two"]);
        // Line limit: the newest lines win.
        let p = write(dir.path(), "d.log", b"1\n2\n3\n4\n");
        assert_eq!(read_log_tail(&p, 2, 1024).unwrap(), ["3", "4"]);
        // Byte window: the first line in it may be cut, so it is dropped (here the window starts
        // at the newline after `aaaaaaaaaa`, then inside `bbb`).
        let p = write(dir.path(), "e.log", b"aaaaaaaaaa\nbbb\ncc\n");
        assert_eq!(read_log_tail(&p, 10, 8).unwrap(), ["bbb", "cc"]);
        assert_eq!(read_log_tail(&p, 10, 6).unwrap(), ["cc"]);
        // A single line longer than the window gives nothing (never half a line).
        let p = write(
            dir.path(),
            "f.log",
            format!("{}\n", "x".repeat(100)).as_bytes(),
        );
        assert!(read_log_tail(&p, 10, 50).unwrap().is_empty());
        // Invalid UTF-8 is replaced, not an error.
        let p = write(dir.path(), "g.log", b"ok\n\xff\xfe bad\n");
        assert_eq!(
            read_log_tail(&p, 10, 1024).unwrap(),
            ["ok", "\u{FFFD}\u{FFFD} bad"]
        );
    }

    #[test]
    fn log_tail_of_a_huge_file_reads_only_the_window() {
        let dir = tempfile::tempdir().unwrap();
        let line = format!("{}\n", "y".repeat(99));
        let p = write(dir.path(), "h.log", line.repeat(20_000).as_bytes());
        let tail = read_log_tail(&p, LOG_TAIL_MAX_LINES, LOG_TAIL_MAX_BYTES).unwrap();
        assert_eq!(tail.len(), LOG_TAIL_MAX_LINES);
        assert!(tail.iter().all(|l| l.len() == 99));
        let bytes: usize = tail.iter().map(|l| l.len() + 1).sum();
        assert!(bytes as u64 <= LOG_TAIL_MAX_BYTES);
        // 64 KiB of 100-byte lines is more than 200 lines; with longer lines the byte cap wins.
        let long = format!("{}\n", "z".repeat(999));
        let p = write(dir.path(), "i.log", long.repeat(1000).as_bytes());
        let tail = read_log_tail(&p, LOG_TAIL_MAX_LINES, LOG_TAIL_MAX_BYTES).unwrap();
        assert_eq!(
            tail.len(),
            65,
            "65 536 bytes hold 65 whole 1000-byte lines after the cut one"
        );
    }

    #[test]
    fn panic_location_is_kept_per_thread() {
        record_panic_location(Some(PanicLocation {
            file: "src/x.rs".into(),
            line: 3,
            column: 9,
        }));
        let other = std::thread::spawn(take_panic_location).join().unwrap();
        assert_eq!(other, None);
        let loc = take_panic_location().unwrap();
        assert_eq!(loc.to_string(), "src/x.rs:3:9");
        assert_eq!(take_panic_location(), None, "taken once");
    }

    #[test]
    fn sanitize_truncates_by_chars() {
        assert_eq!(sanitize_for_log("abcdef", 3), "abc…");
        assert_eq!(sanitize_for_log("abc", 3), "abc");
        assert_eq!(sanitize_for_log("", 0), "");
    }
}
