//! Terminal commands (§4.2, §4.5). Thin wrappers over `terminal::*`.
//! `output` carries `Raw(bytes)` (→ `ArrayBuffer`) and `Json(TermEvent)`.

use std::sync::Arc;

use tauri::State;
use tauri::http::HeaderMap;
use tauri::ipc::{Channel, InvokeBody, InvokeResponseBody, Request};

use crate::app_state::AppState;
use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::{TermEvent, TermSize};
use crate::terminal::{self, TermOutput};

/// `Channel<TermMessage>` as a terminal's output (§3: services only see `TermOutput`).
struct ChannelOutput(Channel<InvokeResponseBody>);

impl TermOutput for ChannelOutput {
    fn data(&self, bytes: Vec<u8>) {
        if let Err(e) = self.0.send(InvokeResponseBody::Raw(bytes)) {
            tracing::debug!(error = %e, "terminal output channel closed");
        }
    }

    fn event(&self, event: TermEvent) {
        match serde_json::to_string(&event) {
            Ok(json) => {
                if let Err(e) = self.0.send(InvokeResponseBody::Json(json)) {
                    tracing::debug!(error = %e, "terminal output channel closed");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not serialize a terminal event"),
        }
    }
}

/// PTY + login shell (or the `cd_wrapper` exec for `cwd`, POSIX sessions only); returns the
/// termId. `TERM` = `settings.terminal.termType`. `cwdFallback` (optional, default
/// `false`): an unusable `cwd` or a failing `cd` starts the shell in home instead of an error.
/// Errors: `sessionNotFound`, `notConnected`, `channelOpenFailed{reason}`,
/// `invalidArgument{field:"cwd"}` (only without `cwdFallback`).
#[tauri::command]
pub async fn term_open(
    state: State<'_, AppState>,
    session_id: String,
    size: TermSize,
    cwd: Option<String>,
    output: Channel<InvokeResponseBody>,
    cwd_fallback: Option<bool>,
) -> Result<String, AppError> {
    let session = state.sessions.get(&session_id).ok_or_else(|| {
        AppError::new(ErrorCode::SessionNotFound {
            session_id: session_id.clone(),
        })
    })?;
    let options = terminal::OpenOptions {
        cwd,
        cwd_fallback: cwd_fallback.unwrap_or(false),
        term_type: state.sessions.settings().terminal.term_type,
    };
    terminal::open_with(&session, size, &options, Arc::new(ChannelOutput(output))).await
}

/// Header that carries the terminal id of a raw-body `term_write`.
pub const TERM_ID_HEADER: &str = "shellfin-term-id";

/// Longest accepted terminal id in [`TERM_ID_HEADER`] (ids are UUIDs).
const MAX_TERM_ID_LEN: usize = 64;

/// The input bytes are the raw request body (`InvokeBody::Raw`, an `ArrayBuffer` /
/// `Uint8Array` on the JS side), not a JSON `number[]` (about 4× the size and parse cost); the
/// terminal id is in the [`TERM_ID_HEADER`] header. Tauri's postMessage fallback (used only when
/// the IPC custom protocol fails) turns the body into a JSON array of numbers, which is accepted
/// too, with every element checked to be a byte.
///
/// Errors: `terminalNotFound`, `notConnected` (no running shell: lost connection or exited),
/// `invalidArgument{field:"termId"}` (missing or malformed header), `invalidArgument{field:"data"}`
/// (not bytes, over 1 MiB, or the input backlog is full).
///
/// Deliberately **synchronous**: Tauri parses and runs a sync command inline, in the
/// order the invokes arrive, so the payload is queued on the terminal's ordered input queue
/// before the next `term_write` is even parsed. An async command would parse each payload in
/// its own task, and a small write could overtake a large one. The command only queues; the
/// terminal's input task does the sending.
#[tauri::command]
pub fn term_write(state: State<'_, AppState>, request: Request<'_>) -> Result<(), AppError> {
    let (term_id, data) = write_args(request.body(), request.headers())?;
    let (_, term) = state.sessions.terminal(term_id)?;
    term.enqueue_write(data)
}

/// Terminal id and input bytes of a `term_write` request (pure, tested below).
fn write_args<'a>(
    body: &InvokeBody,
    headers: &'a HeaderMap,
) -> Result<(&'a str, Vec<u8>), AppError> {
    let term_id = headers
        .get(TERM_ID_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|id| !id.is_empty() && id.len() <= MAX_TERM_ID_LEN)
        .ok_or_else(|| invalid("termId"))?;
    let data = match body {
        InvokeBody::Raw(bytes) => {
            check_len(bytes.len())?;
            bytes.clone()
        }
        InvokeBody::Json(serde_json::Value::Array(items)) => {
            check_len(items.len())?;
            items
                .iter()
                .map(|v| {
                    v.as_u64()
                        .and_then(|n| u8::try_from(n).ok())
                        .ok_or_else(|| invalid("data"))
                })
                .collect::<Result<Vec<u8>, _>>()?
        }
        InvokeBody::Json(_) => return Err(invalid("data")),
    };
    Ok((term_id, data))
}

/// Rejects an oversized payload before it is copied (the input queue checks again).
fn check_len(len: usize) -> Result<(), AppError> {
    if len > terminal::MAX_WRITE {
        return Err(invalid("data").with_detail(format!("{len} bytes > {}", terminal::MAX_WRITE)));
    }
    Ok(())
}

fn invalid(field: &'static str) -> AppError {
    AppError::invalid_argument(field)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use serde_json::json;
    use tauri::http::HeaderValue;

    use super::*;

    fn headers(id: Option<&str>) -> HeaderMap {
        let mut h = HeaderMap::new();
        if let Some(id) = id {
            h.insert(TERM_ID_HEADER, HeaderValue::from_str(id).unwrap());
        }
        h
    }

    fn field(e: &AppError) -> String {
        match &e.kind {
            ErrorCode::InvalidArgument { field } => field.clone(),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn raw_body_with_header_is_accepted_as_is() {
        let h = headers(Some("7f1c1b5e-0000-4000-8000-000000000001"));
        let bytes: Vec<u8> = (0..=255).collect();
        let (id, data) = write_args(&InvokeBody::Raw(bytes.clone()), &h).unwrap();
        assert_eq!(id, "7f1c1b5e-0000-4000-8000-000000000001");
        assert_eq!(data, bytes);
        let (_, empty) = write_args(&InvokeBody::Raw(Vec::new()), &h).unwrap();
        assert!(empty.is_empty());
    }

    #[test]
    fn post_message_fallback_json_bytes_are_accepted() {
        let h = headers(Some("t"));
        let (_, data) = write_args(&InvokeBody::Json(json!([0, 104, 105, 255])), &h).unwrap();
        assert_eq!(data, vec![0, 104, 105, 255]);
    }

    #[test]
    fn hostile_json_bodies_are_rejected() {
        let h = headers(Some("t"));
        for body in [
            json!([256]),
            json!([-1]),
            json!([1.5]),
            json!(["a"]),
            json!([null]),
            json!({ "termId": "t", "data": [1] }),
            json!("abc"),
            json!(null),
        ] {
            let e = write_args(&InvokeBody::Json(body.clone()), &h).unwrap_err();
            assert_eq!(field(&e), "data", "{body}");
        }
    }

    #[test]
    fn missing_or_bad_term_id_header_is_rejected() {
        let body = InvokeBody::Raw(vec![1]);
        for h in [
            headers(None),
            headers(Some("")),
            headers(Some(&"x".repeat(MAX_TERM_ID_LEN + 1))),
        ] {
            assert_eq!(field(&write_args(&body, &h).unwrap_err()), "termId");
        }
        let mut h = HeaderMap::new();
        h.insert(TERM_ID_HEADER, HeaderValue::from_bytes(b"t\xff").unwrap());
        assert_eq!(field(&write_args(&body, &h).unwrap_err()), "termId");
        // The id is only looked up, so any other header name is ignored.
        let mut h = HeaderMap::new();
        h.insert("x-term-id", HeaderValue::from_static("t"));
        assert_eq!(field(&write_args(&body, &h).unwrap_err()), "termId");
    }

    #[test]
    fn oversized_payloads_are_rejected_before_copying() {
        let h = headers(Some("t"));
        let max = terminal::MAX_WRITE;
        assert_eq!(
            write_args(&InvokeBody::Raw(vec![0; max]), &h)
                .unwrap()
                .1
                .len(),
            max
        );
        let e = write_args(&InvokeBody::Raw(vec![0; max + 1]), &h).unwrap_err();
        assert_eq!(field(&e), "data");
        let e = write_args(&InvokeBody::Json(json!(vec![0; max + 1])), &h).unwrap_err();
        assert_eq!(field(&e), "data");
    }
}

/// Stores the size (used by restart/respawn) and queues `window-change` to a running shell.
/// Synchronous for the same reason as `term_write` (the last size wins).
#[tauri::command]
pub fn term_resize(
    state: State<'_, AppState>,
    term_id: String,
    size: TermSize,
) -> Result<(), AppError> {
    let (_, term) = state.sessions.terminal(&term_id)?;
    term.resize(size);
    Ok(())
}

/// Flow-control ack (§4.5; no errors by contract: an unknown terminal is ignored).
#[tauri::command]
pub async fn term_ack(
    state: State<'_, AppState>,
    term_id: String,
    bytes: u64,
) -> Result<(), AppError> {
    if let Some((_, term)) = state.sessions.find_terminal(&term_id) {
        term.ack(bytes);
    }
    Ok(())
}

/// A new shell in the same terminal (same size and cwd). Errors: `terminalNotFound`,
/// `notConnected`, `channelOpenFailed{reason}`.
#[tauri::command]
pub async fn term_restart(state: State<'_, AppState>, term_id: String) -> Result<(), AppError> {
    let (session, term) = state.sessions.terminal(&term_id)?;
    terminal::restart(&session, &term).await
}

/// Idempotent close (no errors by contract).
#[tauri::command]
pub async fn term_close(state: State<'_, AppState>, term_id: String) -> Result<(), AppError> {
    if let Some((session, _)) = state.sessions.find_terminal(&term_id) {
        terminal::close(&session, &term_id).await;
    }
    Ok(())
}
