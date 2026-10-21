//! Terminals (§4.5, §5.8): one PTY shell channel per terminal tab.
//!
//! Per terminal:
//! - a **reader task** per shell channel that never stops draining `ChannelReadHalf::wait()`
//!   (russh has no per-channel back-pressure; a paused reader would stall the whole connection:
//!   other terminals, SFTP, keepalives — §2.3). It pushes into the terminal's [`flow::Flow`],
//!   which never blocks (overflow discards and reports `outputDropped`);
//! - a **pump task** that delivers what the flow releases to the UI ([`TermOutput`]), coalesced
//!   and limited by the frontend's acks (`term_ack`);
//! - an **input task** per shell channel ([`input`]): `term_write` and `term_resize` enqueue
//!   synchronously in arrival order and the task sends them one by one, so input can never be
//!   reordered and the last `window-change` always carries the latest size.
//!
//! Status: `starting → running → exited` (exit status/signal or channel close; the frontend
//! offers `term_restart`), `running → lost` (the transport went away; on the next `connected`
//! the session respawns the shell and sends `reconnected` first, §5.8), any → `closed`
//! (`term_close` / `session_close`). A restart or respawn bumps the channel *generation*, so a
//! late message from an old channel is never mixed into the new stream.
//!
//! Terminal data is never logged.

pub mod flow;
pub mod input;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use bytes::Bytes;
use russh::client::Msg;
use russh::{ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Pty, Sig};
use tokio::sync::{Notify, oneshot, watch};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::{RemotePathStyle, TermEvent, TermSize, TermType};
use crate::session::session::SessionHandle;
use crate::ssh::connect::Connection;

use self::flow::{Flow, Out};
use self::input::{Input, PushError};

/// Default `TERM` sent with the PTY request (§4.5); `terminal.termType` picks it.
pub const TERM: &str = "xterm-256color";

/// How `term_open` starts the shell.
#[derive(Debug, Clone, Default)]
pub struct OpenOptions {
    /// Start directory (`None` = login shell in home).
    pub cwd: Option<String>,
    /// `newTerminalCwd = currentFolder`: an unusable `cwd` or a failing `cd` falls back to home
    /// instead of an error ([`shell_command_or_home`]).
    pub cwd_fallback: bool,
    /// `TERM` of the PTY request (`terminal.termType`).
    pub term_type: TermType,
}
/// PTY modes (§4.5): DEL as erase, UTF-8 input, nominal line speed.
pub const PTY_MODES: [(Pty, u32); 4] = [
    (Pty::VERASE, 127),
    (Pty::IUTF8, 1),
    (Pty::TTY_OP_ISPEED, 38400),
    (Pty::TTY_OP_OSPEED, 38400),
];
/// Budget for channel open + PTY + shell requests.
pub const OPEN_TIMEOUT: Duration = Duration::from_secs(15);
/// Budget for sending CHANNEL_CLOSE on `term_close`/restart.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);
/// Sizes outside these ranges are clamped (xterm.js can briefly propose 0 while hidden).
const MAX_CELLS: u32 = 10_000;
const MAX_PIXELS: u32 = 100_000;
/// Longest signal name reported in `exit{signal}` (server text; sanitized by the UI).
const MAX_SIGNAL_LEN: usize = 64;
/// Largest `term_write` payload per call (the UI sends ≤ 64 KiB chunks).
pub const MAX_WRITE: usize = 1024 * 1024;

/// Where a terminal's output goes: `Raw(bytes)` / `Json(TermEvent)` on a Tauri `Channel` in the
/// app, an in-memory recorder in tests (§3: services only see this trait).
pub trait TermOutput: Send + Sync + 'static {
    fn data(&self, bytes: Vec<u8>);
    fn event(&self, event: TermEvent);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermStatus {
    /// The first shell is being opened.
    Starting,
    Running,
    /// The shell ended (exit status/signal or channel close); `term_restart` starts a new one.
    Exited,
    /// The connection went away while the shell was running (respawned on reconnect, §5.8).
    Lost,
    /// `term_close` / `session_close`.
    Closed,
}

/// See [`Terminal::flow_stats`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FlowStats {
    /// Bytes waiting in the backlog.
    pub queued: usize,
    /// Bytes sent to the UI and not yet acknowledged.
    pub unacked: u64,
    /// Output is currently being discarded (overflow).
    pub dropping: bool,
}

/// The write side of the current shell channel.
#[derive(Clone)]
struct Live {
    writer: Arc<ChannelWriteHalf<Msg>>,
    /// The channel's ordered input queue (writes and window-changes).
    input: Input,
}

/// [`input::Sink`] over a shell channel's write half.
struct ChannelSink(Arc<ChannelWriteHalf<Msg>>);

impl input::Sink for ChannelSink {
    fn data(&self, data: Bytes) -> impl Future<Output = Result<(), String>> + Send {
        let w = self.0.clone();
        async move { w.data_bytes(data).await.map_err(|e| e.to_string()) }
    }

    fn resize(&self, size: TermSize) -> impl Future<Output = Result<(), String>> + Send {
        let w = self.0.clone();
        async move {
            w.window_change(size.cols, size.rows, size.pixel_width, size.pixel_height)
                .await
                .map_err(|e| e.to_string())
        }
    }
}

struct Inner {
    flow: Flow,
    status: TermStatus,
    size: TermSize,
    generation: u64,
    live: Option<Live>,
    alive_tx: Option<watch::Sender<bool>>,
    reader: Option<JoinHandle<()>>,
}

pub struct Terminal {
    pub id: String,
    pub session_id: String,
    /// The `cd_wrapper` command for "Open terminal here"; `None` = login shell.
    command: Option<String>,
    /// `TERM` of every PTY request of this terminal (restart/respawn keep it).
    term: &'static str,
    inner: Mutex<Inner>,
    /// Wakes the pump (output pushed, ack received, closed).
    wake: Notify,
    /// Serializes restart / respawn.
    lifecycle: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for Terminal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Terminal")
            .field("id", &self.id)
            .field("session_id", &self.session_id)
            .field("status", &self.status())
            .finish_non_exhaustive()
    }
}

fn not_connected(session_id: &str) -> AppError {
    AppError::new(ErrorCode::NotConnected {
        session_id: session_id.to_owned(),
    })
}

fn channel_open_failed(reason: &str) -> AppError {
    AppError::new(ErrorCode::ChannelOpenFailed {
        reason: reason.to_owned(),
    })
}

/// Clamp a frontend-proposed size into sane bounds.
pub fn clamp_size(size: TermSize) -> TermSize {
    TermSize {
        cols: size.cols.clamp(1, MAX_CELLS),
        rows: size.rows.clamp(1, MAX_CELLS),
        pixel_width: size.pixel_width.min(MAX_PIXELS),
        pixel_height: size.pixel_height.min(MAX_PIXELS),
    }
}

/// `exit{signal}` text: the RFC 4254 signal name without "SIG", bounded.
pub fn signal_name(sig: &Sig) -> String {
    let name = match sig {
        Sig::ABRT => "ABRT",
        Sig::ALRM => "ALRM",
        Sig::FPE => "FPE",
        Sig::HUP => "HUP",
        Sig::ILL => "ILL",
        Sig::INT => "INT",
        Sig::KILL => "KILL",
        Sig::PIPE => "PIPE",
        Sig::QUIT => "QUIT",
        Sig::SEGV => "SEGV",
        Sig::TERM => "TERM",
        Sig::USR1 => "USR1",
        Sig::Custom(c) => c.as_str(),
    };
    name.chars().take(MAX_SIGNAL_LEN).collect()
}

/// The command for `term_open(cwd)` (§4.5): `None` ⇒ login shell; a cwd is allowed only on
/// POSIX sessions and must pass `cd_wrapper` (absolute, no NUL/C0) ⇒ else
/// `invalidArgument{field:"cwd"}`.
pub fn shell_command(
    cwd: Option<&str>,
    style: RemotePathStyle,
) -> Result<Option<String>, AppError> {
    let Some(cwd) = cwd else { return Ok(None) };
    if style != RemotePathStyle::Posix {
        return Err(AppError::invalid_argument("cwd").with_detail("session is not POSIX"));
    }
    // Remote paths are at most 4096 bytes (the wrapper is ~4x the path).
    if cwd.len() > crate::ssh::shellquote::MAX_CWD_BYTES {
        return Err(AppError::invalid_argument("cwd").with_detail("path too long"));
    }
    crate::ssh::shellquote::cd_wrapper(cwd)
        .map(Some)
        .map_err(|e| AppError::invalid_argument("cwd").with_detail(e.to_string()))
}

/// Never an error. A cwd is used only on a
/// POSIX session and only when it is absolute, ≤ 4096 bytes and free of NUL/C0 controls;
/// otherwise the login shell starts in home (`None`). A usable cwd gets the
/// [`cd_wrapper_or_home`](crate::ssh::shellquote::cd_wrapper_or_home) wrapper, so a folder that
/// cannot be entered also ends in home.
pub fn shell_command_or_home(cwd: Option<&str>, style: RemotePathStyle) -> Option<String> {
    let cwd = cwd?;
    if style != RemotePathStyle::Posix || cwd.len() > crate::ssh::shellquote::MAX_CWD_BYTES {
        return None;
    }
    crate::ssh::shellquote::cd_wrapper_or_home(cwd).ok()
}

fn map_open_error(err: russh::Error, session_id: &str) -> AppError {
    match err {
        russh::Error::ChannelOpenFailure(f) => {
            let reason = match f {
                russh::ChannelOpenFailure::AdministrativelyProhibited => {
                    "administrativelyProhibited"
                }
                russh::ChannelOpenFailure::ConnectFailed => "connectFailed",
                russh::ChannelOpenFailure::UnknownChannelType => "unknownChannelType",
                russh::ChannelOpenFailure::ResourceShortage => "resourceShortage",
                russh::ChannelOpenFailure::Other { .. } => "other",
            };
            channel_open_failed(reason)
        }
        other => not_connected(session_id).with_detail(format!("channel open: {other}")),
    }
}

/// A freshly opened shell channel.
struct Opened {
    reader: ChannelReadHalf,
    writer: ChannelWriteHalf<Msg>,
    /// Output that arrived while waiting for the request replies (bounded).
    early: Flow,
}

/// Wait for the reply to a `want_reply` request. Output arriving meanwhile is kept in `early`, a
/// flow with the same caps as the backlog (8 MiB, coalesced buffers); the rest is counted as
/// dropped (a server can withhold the reply and flood for the whole open budget).
async fn await_reply(
    ch: &mut russh::Channel<Msg>,
    rejected: &str,
    early: &mut Flow,
    session_id: &str,
) -> Result<(), AppError> {
    loop {
        match ch.wait().await {
            Some(ChannelMsg::Success) => return Ok(()),
            Some(ChannelMsg::Failure) => return Err(channel_open_failed(rejected)),
            Some(ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. }) => {
                early.push_data(&data, Instant::now());
            }
            Some(ChannelMsg::Close | ChannelMsg::Eof) => return Err(channel_open_failed("closed")),
            None => return Err(not_connected(session_id)),
            Some(_) => {}
        }
    }
}

/// Open a session channel on the destination hop: PTY (§4.5 modes) then the login shell, or
/// `exec(cd_wrapper)` for a cwd.
async fn open_shell<H: russh::client::Handler>(
    handle: &russh::client::Handle<H>,
    size: TermSize,
    command: Option<&str>,
    term: &str,
    session_id: &str,
) -> Result<Opened, AppError> {
    let open = async {
        let mut ch = handle
            .channel_open_session()
            .await
            .map_err(|e| map_open_error(e, session_id))?;
        let lost = |e: russh::Error| not_connected(session_id).with_detail(e.to_string());
        ch.request_pty(
            true,
            term,
            size.cols,
            size.rows,
            size.pixel_width,
            size.pixel_height,
            &PTY_MODES,
        )
        .await
        .map_err(lost)?;
        let mut early = Flow::new();
        await_reply(&mut ch, "ptyRejected", &mut early, session_id).await?;
        match command {
            None => ch.request_shell(true).await,
            Some(c) => ch.exec(true, c).await,
        }
        .map_err(lost)?;
        await_reply(&mut ch, "shellRejected", &mut early, session_id).await?;
        let (reader, writer) = ch.split();
        Ok(Opened {
            reader,
            writer,
            early,
        })
    };
    tokio::time::timeout(OPEN_TIMEOUT, open)
        .await
        .unwrap_or_else(|_| Err(channel_open_failed("timeout")))
}

impl Terminal {
    #[cfg(test)]
    fn new(id: String, session_id: String, size: TermSize, command: Option<String>) -> Self {
        Self::with_term(id, session_id, size, command, TERM)
    }

    fn with_term(
        id: String,
        session_id: String,
        size: TermSize,
        command: Option<String>,
        term: &'static str,
    ) -> Self {
        Self {
            id,
            session_id,
            command,
            term,
            inner: Mutex::new(Inner {
                flow: Flow::new(),
                status: TermStatus::Starting,
                size: clamp_size(size),
                generation: 0,
                live: None,
                alive_tx: None,
                reader: None,
            }),
            wake: Notify::new(),
            lifecycle: tokio::sync::Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn status(&self) -> TermStatus {
        self.lock().status
    }

    pub fn size(&self) -> TermSize {
        self.lock().size
    }

    /// Backlog diagnostics (byte counts only, never content).
    pub fn flow_stats(&self) -> FlowStats {
        let i = self.lock();
        FlowStats {
            queued: i.flow.queued_bytes(),
            unacked: i.flow.unacked(),
            dropping: i.flow.is_dropping(),
        }
    }

    /// Open a new shell channel and make it current. `first` is queued before any of its output
    /// (`reconnected` on respawn).
    async fn start(
        self: &Arc<Self>,
        conn: &Connection,
        first: Option<TermEvent>,
    ) -> Result<(), AppError> {
        let size = self.size();
        let hop = conn
            .hops
            .last()
            .ok_or_else(|| not_connected(&self.session_id))?;
        let opened = open_shell(
            &hop.handle,
            size,
            self.command.as_deref(),
            self.term,
            &self.session_id,
        )
        .await?;
        let (alive_tx, alive) = watch::channel(true);
        let writer = Arc::new(opened.writer);
        let opened_ok = {
            let mut i = self.lock();
            if i.status == TermStatus::Closed {
                false
            } else {
                i.generation += 1;
                let generation = i.generation;
                // Output the old shell lost is reported before anything of the new
                // one, and the new shell's output is never discarded as the old one's.
                i.flow.boundary();
                if let Some(ev) = first {
                    i.flow.push_event(ev);
                }
                i.flow.absorb(opened.early, Instant::now());
                let input = Input::spawn(ChannelSink(writer.clone()), alive);
                // A resize that arrived while the channel was opening; queued under the lock,
                // so a later `term_resize` is always sent after it.
                if i.size != size {
                    let _ = input.push_resize(i.size);
                }
                i.live = Some(Live {
                    writer: writer.clone(),
                    input,
                });
                i.alive_tx = Some(alive_tx);
                i.status = TermStatus::Running;
                let reader = tokio::spawn(read_loop(opened.reader, self.clone(), generation));
                // A reader left over from a lost channel (`mark_lost` keeps it so it can
                // drain what already arrived) belongs to an older generation, whose output is
                // ignored from here on; stop it instead of detaching it.
                if let Some(old) = i.reader.replace(reader) {
                    old.abort();
                }
                true
            }
        };
        if !opened_ok {
            // Closed while the channel was opening.
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, writer.close()).await;
            return Err(not_connected(&self.session_id));
        }
        self.wake.notify_one();
        Ok(())
    }

    /// Output from channel `generation` (ignored if that channel is no longer current).
    /// The pump is woken only when the push changes what it would deliver now (/// [`Flow::push_data_wake`]), not on every packet.
    fn push_output(&self, generation: u64, data: Bytes) {
        let wake = {
            let mut i = self.lock();
            if i.generation != generation || i.status == TermStatus::Closed {
                return;
            }
            i.flow.push_data_wake(&data, Instant::now())
        };
        if wake {
            self.wake.notify_one();
        }
    }

    /// Channel `generation` ended: exit status/signal ⇒ `exit`; close without exit ⇒
    /// `closed{error:null}`; transport gone ⇒ `lost` (no event, §5.8).
    fn channel_ended(&self, generation: u64, exit: Option<TermEvent>, closed: bool) {
        {
            let mut i = self.lock();
            if i.generation != generation || i.status == TermStatus::Closed {
                return;
            }
            i.live = None;
            if let Some(tx) = i.alive_tx.take() {
                let _ = tx.send(false);
            }
            i.reader = None;
            match (exit, closed) {
                (Some(ev), _) => {
                    i.flow.push_event(ev);
                    i.status = TermStatus::Exited;
                }
                (None, true) => {
                    i.flow.push_event(TermEvent::Closed { error: None });
                    i.status = TermStatus::Exited;
                }
                (None, false) => i.status = TermStatus::Lost,
            }
        }
        self.wake.notify_one();
    }

    /// The transport is gone (supervisor): a running shell becomes `lost`.
    fn mark_lost(&self) {
        let mut i = self.lock();
        if matches!(i.status, TermStatus::Running | TermStatus::Starting) {
            i.status = TermStatus::Lost;
            i.live = None;
            if let Some(tx) = i.alive_tx.take() {
                let _ = tx.send(false);
            }
        }
    }

    /// `term_ack`. Wakes the pump only when the ack released held-back output.
    pub fn ack(&self, bytes: u64) {
        let released = self.lock().flow.ack(bytes);
        if released {
            self.wake.notify_one();
        }
    }

    /// `term_write` (synchronous): queue `data` behind every earlier write of this
    /// terminal and return. `notConnected` unless a shell is running; more than [`MAX_WRITE`]
    /// bytes per call ⇒ `invalidArgument{field:"data"}` (the UI chunks to 64 KiB); more than
    /// [`input::MAX_QUEUED`] bytes waiting (the remote stopped reading) ⇒
    /// `invalidArgument{field:"data"}` too.
    pub fn enqueue_write(&self, data: Vec<u8>) -> Result<(), AppError> {
        self.push_write(data, None)
    }

    /// [`Self::enqueue_write`], then wait until the data was handed to the channel (tests and
    /// the integration suite; the command does not wait).
    pub async fn write(&self, data: Vec<u8>) -> Result<(), AppError> {
        let (tx, rx) = oneshot::channel();
        self.push_write(data, Some(tx))?;
        match rx.await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(not_connected(&self.session_id).with_detail(e)),
            Err(_) => Err(not_connected(&self.session_id)),
        }
    }

    fn push_write(&self, data: Vec<u8>, done: Option<input::Done>) -> Result<(), AppError> {
        if data.len() > MAX_WRITE {
            return Err(AppError::invalid_argument("data")
                .with_detail(format!("{} bytes > {MAX_WRITE}", data.len())));
        }
        if data.is_empty() {
            if let Some(done) = done {
                let _ = done.send(Ok(()));
            }
            return Ok(());
        }
        // Queued under the terminal lock: the order of the calls is the order on the wire.
        let i = self.lock();
        let Some(live) = i.live.as_ref() else {
            return Err(not_connected(&self.session_id));
        };
        match live.input.push_data(Bytes::from(data), done) {
            Ok(()) => Ok(()),
            Err(PushError::Full) => Err(AppError::invalid_argument("data")
                .with_detail("input backlog full (the remote is not reading)")),
            Err(PushError::Closed) => Err(not_connected(&self.session_id)),
        }
    }

    /// `term_resize` (synchronous): remember the size (also for restart/respawn) and queue a
    /// `window-change` behind the input sent before it (the last one sent is always the
    /// latest size).
    pub fn resize(&self, size: TermSize) {
        let size = clamp_size(size);
        let mut i = self.lock();
        i.size = size;
        if let Some(live) = i.live.as_ref() {
            let _ = live.input.push_resize(size);
        }
    }

    /// End the current channel without reporting it (restart).
    async fn end_current(&self) {
        let (live, tx, reader) = {
            let mut i = self.lock();
            i.generation += 1;
            if matches!(
                i.status,
                TermStatus::Running | TermStatus::Lost | TermStatus::Starting
            ) {
                i.status = TermStatus::Exited;
            }
            (i.live.take(), i.alive_tx.take(), i.reader.take())
        };
        if let Some(tx) = tx {
            let _ = tx.send(false);
        }
        if let Some(r) = reader {
            r.abort();
        }
        if let Some(live) = live {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, live.writer.close()).await;
        }
    }

    /// Close for good: pending output is dropped, `last` (if any) is the final message. Returns
    /// whether the shell was running.
    async fn close(&self, last: Option<TermEvent>) -> bool {
        let (was_running, live, tx, reader) = {
            let mut i = self.lock();
            if i.status == TermStatus::Closed {
                return false;
            }
            let was_running = i.status == TermStatus::Running;
            i.status = TermStatus::Closed;
            i.generation += 1;
            i.flow.clear();
            if let Some(ev) = last {
                i.flow.push_event(ev);
            }
            (
                was_running,
                i.live.take(),
                i.alive_tx.take(),
                i.reader.take(),
            )
        };
        self.wake.notify_one();
        if let Some(tx) = tx {
            let _ = tx.send(false);
        }
        if let Some(r) = reader {
            r.abort();
        }
        if let Some(live) = live {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, live.writer.close()).await;
        }
        was_running
    }

    /// Record a failed respawn: the terminal ends with `closed{error}`.
    fn fail(&self, error: AppError) {
        {
            let mut i = self.lock();
            if i.status == TermStatus::Closed {
                return;
            }
            i.status = TermStatus::Exited;
            i.flow.push_event(TermEvent::Closed { error: Some(error) });
        }
        self.wake.notify_one();
    }
}

/// The reader task: drains the channel until it ends, never pausing (§4.5).
async fn read_loop(mut rx: ChannelReadHalf, term: Arc<Terminal>, generation: u64) {
    let mut exit = None;
    let mut closed = false;
    while let Some(msg) = rx.wait().await {
        match msg {
            ChannelMsg::Data { data } | ChannelMsg::ExtendedData { data, .. } => {
                term.push_output(generation, data);
            }
            ChannelMsg::ExitStatus { exit_status } => {
                exit = Some(TermEvent::Exit {
                    code: Some(exit_status),
                    signal: None,
                });
            }
            ChannelMsg::ExitSignal { signal_name, .. } => {
                exit = Some(TermEvent::Exit {
                    code: None,
                    signal: Some(self::signal_name(&signal_name)),
                });
            }
            ChannelMsg::Close => {
                closed = true;
                break;
            }
            _ => {}
        }
    }
    term.channel_ended(generation, exit, closed);
}

/// The pump task: delivers released output/events to the UI until the terminal is closed and
/// everything (incl. the final event) has been delivered.
async fn pump(term: Arc<Terminal>, output: Arc<dyn TermOutput>) {
    loop {
        let (poll, done) = {
            let mut i = term.lock();
            let poll = i.flow.poll(Instant::now());
            let done = i.status == TermStatus::Closed && i.flow.is_empty();
            (poll, done)
        };
        for o in poll.out {
            match o {
                Out::Data(bytes) => output.data(bytes),
                Out::Event(ev) => output.event(ev),
            }
        }
        if done {
            return;
        }
        match poll.wake_at {
            Some(at) => {
                tokio::select! {
                    () = term.wake.notified() => {}
                    () = tokio::time::sleep_until(at) => {}
                }
            }
            None => term.wake.notified().await,
        }
    }
}

// ---------------------------------------------------------------------------------------------
// per-session set
// ---------------------------------------------------------------------------------------------

#[derive(Default)]
struct SetInner {
    map: HashMap<String, Arc<Terminal>>,
    /// The session is closing/closed: no new terminals.
    closed: bool,
}

/// The terminals of one session.
#[derive(Default)]
pub struct TerminalSet {
    inner: Mutex<SetInner>,
}

impl std::fmt::Debug for TerminalSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalSet")
            .field("len", &self.lock().map.len())
            .finish()
    }
}

impl TerminalSet {
    fn lock(&self) -> MutexGuard<'_, SetInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn get(&self, term_id: &str) -> Option<Arc<Terminal>> {
        self.lock().map.get(term_id).cloned()
    }

    fn all(&self) -> Vec<Arc<Terminal>> {
        self.lock().map.values().cloned().collect()
    }

    /// Terminals whose shell is running (`RecentEntry.terminalCount`).
    pub fn running_count(&self) -> u32 {
        let n = self
            .all()
            .iter()
            .filter(|t| t.status() == TermStatus::Running)
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    /// The connection is gone: running shells become `lost` (respawned on reconnect).
    pub fn mark_lost(&self) {
        for t in self.all() {
            t.mark_lost();
        }
    }

    /// Session close (§5.8): every terminal gets `last` (normally `closed{error:null}`) as its
    /// final message. Returns how many shells were still running. Idempotent.
    pub async fn close_all(&self, last: Option<TermEvent>) -> u32 {
        let terms: Vec<Arc<Terminal>> = {
            let mut s = self.lock();
            s.closed = true;
            s.map.drain().map(|(_, t)| t).collect()
        };
        let closes = terms.iter().map(|t| t.close(last.clone()));
        let running = futures_util::future::join_all(closes)
            .await
            .into_iter()
            .filter(|r| *r)
            .count();
        u32::try_from(running).unwrap_or(u32::MAX)
    }

    /// After a (re)connect: open a new shell for every `lost` terminal (same size and cwd),
    /// `reconnected` first (§5.8). Failures end the terminal with `closed{error}`.
    pub fn respawn_lost(&self, conn: &Arc<Connection>) {
        for t in self.all() {
            if t.status() != TermStatus::Lost {
                continue;
            }
            let conn = conn.clone();
            tokio::spawn(async move {
                let _lifecycle = t.lifecycle.lock().await;
                if t.status() != TermStatus::Lost {
                    return;
                }
                if let Err(e) = t.start(&conn, Some(TermEvent::Reconnected)).await {
                    tracing::info!(code = ?e.kind, "terminal respawn failed");
                    t.fail(e);
                }
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------
// service functions (called by `commands::terminal` through `SessionManager`)
// ---------------------------------------------------------------------------------------------

/// `term_open` (§4.2): PTY + login shell (or `cd_wrapper` exec for `cwd`) on the session's
/// destination; returns the termId. Output flows to `output` from now on.
pub async fn open(
    session: &SessionHandle,
    size: TermSize,
    cwd: Option<&str>,
    output: Arc<dyn TermOutput>,
) -> Result<String, AppError> {
    let options = OpenOptions {
        cwd: cwd.map(str::to_owned),
        ..OpenOptions::default()
    };
    open_with(session, size, &options, output).await
}

/// [`open`] with the settings options (`TERM`, cwd fallback).
pub async fn open_with(
    session: &SessionHandle,
    size: TermSize,
    options: &OpenOptions,
    output: Arc<dyn TermOutput>,
) -> Result<String, AppError> {
    let conn = session
        .connection
        .read()
        .await
        .clone()
        .ok_or_else(|| not_connected(&session.id))?;
    let command = if options.cwd_fallback {
        shell_command_or_home(options.cwd.as_deref(), conn.remote_path_style)
    } else {
        shell_command(options.cwd.as_deref(), conn.remote_path_style)?
    };
    let term = Arc::new(Terminal::with_term(
        uuid::Uuid::new_v4().to_string(),
        session.id.clone(),
        size,
        command,
        options.term_type.as_term(),
    ));
    term.start(&conn, None).await?;
    let inserted = {
        let mut s = session.terminals.lock();
        if s.closed {
            false
        } else {
            s.map.insert(term.id.clone(), term.clone());
            true
        }
    };
    if !inserted {
        term.close(None).await;
        return Err(not_connected(&session.id));
    }
    tokio::spawn(pump(term.clone(), output));
    Ok(term.id.clone())
}

/// `term_restart`: a new shell (same size and cwd) in the same terminal; a running shell is
/// closed first. `notConnected` while the session has no connection.
pub async fn restart(session: &SessionHandle, term: &Arc<Terminal>) -> Result<(), AppError> {
    let conn = session
        .connection
        .read()
        .await
        .clone()
        .ok_or_else(|| not_connected(&session.id))?;
    let _lifecycle = term.lifecycle.lock().await;
    if term.status() == TermStatus::Closed {
        return Err(AppError::new(ErrorCode::TerminalNotFound {
            term_id: term.id.clone(),
        }));
    }
    term.end_current().await;
    term.start(&conn, None).await
}

/// `term_close` (idempotent): closes the channel; no further messages.
pub async fn close(session: &SessionHandle, term_id: &str) {
    let term = session.terminals.lock().map.remove(term_id);
    if let Some(term) = term {
        term.close(None).await;
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

    use super::*;

    #[derive(Default)]
    struct Recorder {
        out: Mutex<Vec<Result<Vec<u8>, TermEvent>>>,
    }
    impl TermOutput for Recorder {
        fn data(&self, bytes: Vec<u8>) {
            self.out.lock().unwrap().push(Ok(bytes));
        }
        fn event(&self, event: TermEvent) {
            self.out.lock().unwrap().push(Err(event));
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

    fn running_terminal() -> Arc<Terminal> {
        let t = Arc::new(Terminal::new("t".into(), "s".into(), size(80, 24), None));
        {
            let mut i = t.lock();
            i.generation = 1;
            i.status = TermStatus::Running;
        }
        t
    }

    #[test]
    fn sizes_are_clamped() {
        assert_eq!(clamp_size(size(0, 0)), size(1, 1));
        let big = clamp_size(TermSize {
            cols: u32::MAX,
            rows: 50,
            pixel_width: u32::MAX,
            pixel_height: 7,
        });
        assert_eq!(big.cols, MAX_CELLS);
        assert_eq!(big.rows, 50);
        assert_eq!(big.pixel_width, MAX_PIXELS);
        assert_eq!(big.pixel_height, 7);
    }

    #[test]
    fn signal_names_are_bounded() {
        assert_eq!(signal_name(&Sig::TERM), "TERM");
        assert_eq!(signal_name(&Sig::Custom("XCPU@x".into())), "XCPU@x");
        assert_eq!(signal_name(&Sig::Custom("A".repeat(500))).len(), 64);
    }

    #[test]
    fn cwd_rules() {
        assert_eq!(shell_command(None, RemotePathStyle::Windows).unwrap(), None);
        let cmd = shell_command(Some("/tmp/a b"), RemotePathStyle::Posix)
            .unwrap()
            .unwrap();
        assert!(cmd.starts_with("exec /bin/sh -c '"));
        for (cwd, style) in [
            ("/tmp", RemotePathStyle::Windows),
            ("/tmp", RemotePathStyle::Unknown),
            ("relative", RemotePathStyle::Posix),
            ("/tmp/\nx", RemotePathStyle::Posix),
            ("", RemotePathStyle::Posix),
        ] {
            let e = shell_command(Some(cwd), style).unwrap_err();
            assert_eq!(
                e.kind,
                ErrorCode::InvalidArgument {
                    field: "cwd".into()
                }
            );
        }
    }

    /// The currentFolder fallback never errors and never puts an unsafe path on the
    /// wire; hostile names are carried only as octal escapes.
    #[test]
    fn cwd_fallback_rules() {
        assert_eq!(shell_command_or_home(None, RemotePathStyle::Posix), None);
        for (cwd, style) in [
            ("/tmp", RemotePathStyle::Windows),
            ("/tmp", RemotePathStyle::Unknown),
            ("relative", RemotePathStyle::Posix),
            ("C:\\Users", RemotePathStyle::Posix),
            ("/tmp/\nx", RemotePathStyle::Posix),
            ("/tmp/x\n", RemotePathStyle::Posix),
            ("/tmp/\u{0}", RemotePathStyle::Posix),
            ("/tmp/\u{1b}[2J", RemotePathStyle::Posix),
            ("", RemotePathStyle::Posix),
        ] {
            assert_eq!(shell_command_or_home(Some(cwd), style), None, "{cwd:?}");
        }
        let long = format!("/{}", "a".repeat(4096));
        assert_eq!(
            shell_command_or_home(Some(&long), RemotePathStyle::Posix),
            None
        );
        let ok = format!("/{}", "a".repeat(4095));
        assert!(shell_command_or_home(Some(&ok), RemotePathStyle::Posix).is_some());
        let cmd = shell_command_or_home(Some("/srv/x';reboot;#"), RemotePathStyle::Posix).unwrap();
        assert!(
            cmd.ends_with(r#"")" 2>/dev/null; exec "${SHELL:-/bin/sh}" -l'"#),
            "{cmd}"
        );
        assert!(!cmd.contains("reboot"), "{cmd}");
        // The strict variant keeps the error for "Open terminal here".
        assert!(shell_command(Some("/tmp/\nx"), RemotePathStyle::Posix).is_err());
        assert!(shell_command(Some(&ok), RemotePathStyle::Posix).is_ok());
        let too_long = format!("/{}", "a".repeat(4096));
        assert!(shell_command(Some(&too_long), RemotePathStyle::Posix).is_err());
        // TERM per terminal (restart/respawn reuse it).
        let t = Terminal::with_term("t".into(), "s".into(), size(80, 24), None, "vt100");
        assert_eq!(t.term, "vt100");
        assert_eq!(
            Terminal::new("t".into(), "s".into(), size(1, 1), None).term,
            TERM
        );
        assert_eq!(OpenOptions::default().term_type.as_term(), TERM);
    }

    #[tokio::test]
    async fn exit_is_delivered_after_the_output_and_ends_the_shell() {
        let t = running_terminal();
        let rec = Arc::new(Recorder::default());
        let pump = tokio::spawn(pump(t.clone(), rec.clone()));
        t.push_output(1, Bytes::from_static(b"\xff\xfe raw"));
        t.channel_ended(
            1,
            Some(TermEvent::Exit {
                code: Some(3),
                signal: None,
            }),
            true,
        );
        assert_eq!(t.status(), TermStatus::Exited);
        tokio::time::sleep(Duration::from_millis(50)).await;
        {
            let out = rec.out.lock().unwrap();
            assert_eq!(out.len(), 2);
            assert_eq!(out[0].as_ref().unwrap().as_slice(), b"\xff\xfe raw");
            assert!(matches!(
                out[1],
                Err(TermEvent::Exit {
                    code: Some(3),
                    signal: None
                })
            ));
        }
        assert!(!t.close(None).await, "an exited shell is not running");
        tokio::time::timeout(Duration::from_secs(1), pump)
            .await
            .expect("pump ends after close")
            .unwrap();
    }

    #[tokio::test]
    async fn close_without_exit_reports_closed_and_loss_reports_nothing() {
        let t = running_terminal();
        t.channel_ended(1, None, true);
        assert_eq!(t.status(), TermStatus::Exited);
        assert!(matches!(
            t.lock().flow.poll(Instant::now()).out[..],
            [Out::Event(TermEvent::Closed { error: None })]
        ));

        let t = running_terminal();
        t.channel_ended(1, None, false);
        assert_eq!(t.status(), TermStatus::Lost);
        assert!(t.lock().flow.poll(Instant::now()).out.is_empty());
        assert_eq!(
            t.write(b"x".to_vec()).await.unwrap_err().kind,
            ErrorCode::NotConnected {
                session_id: "s".into()
            }
        );
    }

    #[tokio::test]
    async fn stale_generations_are_ignored() {
        let t = running_terminal();
        t.lock().generation = 2; // restarted
        t.push_output(1, Bytes::from_static(b"old"));
        t.channel_ended(
            1,
            Some(TermEvent::Exit {
                code: Some(0),
                signal: None,
            }),
            true,
        );
        assert_eq!(t.status(), TermStatus::Running);
        assert!(t.lock().flow.is_empty());
    }

    #[tokio::test]
    async fn session_close_sends_closed_last_and_drops_pending_output() {
        let set = TerminalSet::default();
        let t = running_terminal();
        let rec = Arc::new(Recorder::default());
        set.lock().map.insert(t.id.clone(), t.clone());
        // Withheld acks: 1 MiB stays pending behind the in-flight limit and is never delivered.
        for _ in 0..(5 * 32) {
            t.push_output(1, Bytes::from(vec![b'y'; 32 * 1024]));
        }
        let pump = tokio::spawn(pump(t.clone(), rec.clone()));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(t.lock().flow.queued_bytes(), 1024 * 1024);
        let exited = running_terminal();
        exited.lock().status = TermStatus::Exited;
        set.lock().map.insert("e".into(), exited);
        assert_eq!(set.running_count(), 1);
        assert_eq!(
            set.close_all(Some(TermEvent::Closed { error: None })).await,
            1
        );
        tokio::time::timeout(Duration::from_secs(1), pump)
            .await
            .expect("pump ends")
            .unwrap();
        let out = rec.out.lock().unwrap();
        let delivered: usize = out
            .iter()
            .filter_map(|o| o.as_ref().ok())
            .map(Vec::len)
            .sum();
        assert_eq!(delivered as u64, flow::MAX_IN_FLIGHT);
        assert!(matches!(
            out.last(),
            Some(Err(TermEvent::Closed { error: None }))
        ));
        assert!(set.get("t").is_none());
        assert!(set.lock().closed);
    }

    #[test]
    fn mark_lost_only_affects_running_shells() {
        let set = TerminalSet::default();
        let running = running_terminal();
        let exited = Arc::new(Terminal::new("x".into(), "s".into(), size(1, 1), None));
        exited.lock().status = TermStatus::Exited;
        set.lock().map.insert("a".into(), running.clone());
        set.lock().map.insert("b".into(), exited.clone());
        set.mark_lost();
        assert_eq!(running.status(), TermStatus::Lost);
        assert_eq!(exited.status(), TermStatus::Exited);
    }

    /// Note: `term_write` payloads are capped per call.
    #[tokio::test]
    async fn oversized_writes_are_rejected() {
        let t = running_terminal();
        let err = t.write(vec![b'x'; MAX_WRITE + 1]).await.unwrap_err();
        assert_eq!(
            err.kind,
            ErrorCode::InvalidArgument {
                field: "data".into()
            }
        );
        // At the limit it is only refused because no shell is attached in this unit test.
        let err = t.write(vec![b'x'; MAX_WRITE]).await.unwrap_err();
        assert!(matches!(err.kind, ErrorCode::NotConnected { .. }));
    }

    /// A server that withholds the PTY reply and floods output meanwhile cannot make the
    /// client buffer more than the backlog cap; the excess is counted and reported as
    /// `outputDropped` once the shell starts. In-process russh server over a duplex stream.
    #[tokio::test(flavor = "multi_thread")]
    async fn withheld_pty_reply_flood_is_bounded() {
        use russh::keys::PrivateKey;
        use russh::keys::ssh_key::private::Ed25519Keypair;
        use russh::server::{self, Auth, Msg as SMsg, Session};

        const FLOOD: usize = 12 * 1024 * 1024;
        const PACKET: usize = 1024;

        struct Hostile;
        impl server::Handler for Hostile {
            type Error = russh::Error;
            async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
                Ok(Auth::Accept)
            }
            async fn channel_open_session(
                &mut self,
                _ch: russh::Channel<SMsg>,
                reply: server::ChannelOpenHandle,
                _s: &mut Session,
            ) -> Result<(), Self::Error> {
                reply.accept().await;
                Ok(())
            }
            #[allow(clippy::too_many_arguments)]
            async fn pty_request(
                &mut self,
                channel: russh::ChannelId,
                _: &str,
                _: u32,
                _: u32,
                _: u32,
                _: u32,
                _: &[(Pty, u32)],
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                // Flood first, reply afterwards.
                let h = session.handle();
                tokio::spawn(async move {
                    let packet = Bytes::from(vec![b'x'; PACKET]);
                    for _ in 0..(FLOOD / PACKET) {
                        if h.data(channel, packet.clone()).await.is_err() {
                            return;
                        }
                    }
                    let _ = h.channel_success(channel).await;
                });
                Ok(())
            }
            async fn shell_request(
                &mut self,
                channel: russh::ChannelId,
                session: &mut Session,
            ) -> Result<(), Self::Error> {
                session.channel_success(channel)?;
                Ok(())
            }
        }

        struct Cli;
        impl russh::client::Handler for Cli {
            type Error = russh::Error;
            async fn check_server_key(
                &mut self,
                _: &russh::keys::PublicKeyOrCertificate,
            ) -> Result<bool, Self::Error> {
                Ok(true)
            }
        }

        let cfg = Arc::new(server::Config {
            keys: vec![PrivateKey::from(Ed25519Keypair::from_seed(&[9u8; 32]))],
            ..Default::default()
        });
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let _server = tokio::spawn(server::run_stream(cfg, server_io, Hostile));
        let mut client = russh::client::connect_stream(
            Arc::new(russh::client::Config::default()),
            client_io,
            Cli,
        )
        .await
        .unwrap();
        assert!(client.authenticate_none("u").await.unwrap().success());

        let opened = tokio::time::timeout(
            Duration::from_secs(60),
            open_shell(&client, size(80, 24), None, TERM, "s"),
        )
        .await
        .expect("open within the budget")
        .expect("shell opened");
        let early = opened.early;
        assert_eq!(
            early.queued_bytes(),
            flow::MAX_BACKLOG,
            "oldest output kept"
        );
        assert_eq!(
            early.dropped_bytes() as usize,
            FLOOD - flow::MAX_BACKLOG,
            "the rest is counted"
        );
        assert!(
            early.heap_bytes() <= flow::MAX_BACKLOG + 2 * flow::TAIL_MAX,
            "heap {}",
            early.heap_bytes()
        );

        // Handed to the terminal: kept output, then `outputDropped` with the count.
        let mut f = Flow::new();
        f.absorb(early, Instant::now());
        let mut data = 0usize;
        let mut dropped = None;
        loop {
            f.ack(u64::MAX);
            let p = f.poll(Instant::now() + flow::FLUSH_DELAY);
            if p.out.is_empty() {
                break;
            }
            for o in p.out {
                match o {
                    Out::Data(d) => {
                        assert!(dropped.is_none(), "marker after the kept output");
                        data += d.len();
                    }
                    Out::Event(TermEvent::OutputDropped { bytes }) => dropped = Some(bytes),
                    Out::Event(other) => panic!("unexpected {other:?}"),
                }
            }
        }
        assert_eq!(data, flow::MAX_BACKLOG);
        assert_eq!(dropped, Some((FLOOD - flow::MAX_BACKLOG) as u64));
    }
}
