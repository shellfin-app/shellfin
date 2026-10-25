//! Download of one remote file (§6.6): `.part` file, pipelined read, throttle, provenance mark,
//! remote times, rename over the chosen path; the `.part` is removed on failure or cancel.

use std::path::Path;
use std::time::Duration;

use tokio::io::AsyncWriteExt;

use crate::error::{AppError, ErrorCode, IoOp};
use crate::ipc::dto::{TransferDirection, TransferEvent};

use super::super::client::{PipelinedReader, Sftp, ensure_regular};
use super::super::path;
use super::super::save;
use super::local::{create_part, finish_part_with, local_error, local_target_exists, local_times};
use super::{
    Conflict, Finish, Item, ItemCtx, Job, Progress, blocking, cancel_or_stop, lost_as_disconnected,
    stop_signal,
};

/// How long a cancelled download waits for its remote CLOSE before removing the `.part`.
const CANCEL_CLOSE_WAIT: Duration = Duration::from_millis(500);
/// Local write buffer for downloads.
const LOCAL_WRITE: usize = 256 * 1024;

pub(super) async fn download(
    job: &Job<'_>,
    remote: &str,
    local: &Path,
    gate: &mut super::Gate,
) -> Result<Finish, AppError> {
    let Job {
        id,
        session,
        conflict,
        preserve_times,
        sink,
    } = *job;
    let name = path::basename(remote).to_owned();
    let sftp = Sftp::for_session(session).await;
    let attrs = match &sftp {
        Ok(s) => s.stat(remote).await.map(Some),
        Err(e) => Err(e.clone()),
    };
    let total = attrs
        .as_ref()
        .ok()
        .and_then(|a| a.as_ref())
        .and_then(|a| a.size);
    sink.send(TransferEvent::Started {
        transfer_id: id.to_owned(),
        name,
        direction: TransferDirection::Download,
        total,
    });
    let sftp = sftp?;
    let attrs = attrs.map_err(|e| lost_as_disconnected(Some(&sftp), e))?;
    // Only a regular file (after links) is downloaded; a FIFO would block the server,
    // `/dev/zero` would fill the disk.
    if let Some(a) = &attrs {
        ensure_regular(a, remote)?;
    }
    let times = attrs
        .as_ref()
        .filter(|_| preserve_times)
        .and_then(|a| local_times(a.atime, a.mtime));
    // Wait for a slot after `started`, before any handle is opened.
    let _slot = gate.acquire().await?;
    let mut progress = Progress {
        id: id.to_owned(),
        total,
        sink: sink.clone(),
        last: None,
    };
    let ctx = ItemCtx {
        session,
        sftp: &sftp,
        gate,
        conflict,
    };
    match download_item(&ctx, remote, local, times, &mut |n| progress.report(n)).await? {
        Item::Copied(bytes) => Ok(Finish::Done(bytes, None)),
        Item::Skipped => Ok(Finish::Skipped),
    }
}

/// One file's download once its slot is held: the `Skip` check, the `.part` copy and the
/// finish (mark, `times`, rename over `local`). `remote` was checked to be a regular file by
/// the caller; `progress` hears the bytes written so far.
pub(super) async fn download_item(
    ctx: &ItemCtx<'_>,
    remote: &str,
    local: &Path,
    times: Option<std::fs::FileTimes>,
    progress: &mut (dyn FnMut(u64) + Send),
) -> Result<Item, AppError> {
    let ItemCtx {
        session,
        sftp,
        gate,
        conflict,
    } = *ctx;
    let cancelled = gate.cancelled_fn();
    let cancelled = &cancelled;
    if conflict == Conflict::Skip {
        let chosen = local.to_path_buf();
        if blocking(move || Ok(local_target_exists(&chosen))).await? {
            return Ok(Item::Skipped);
        }
    }
    let chosen = local.to_path_buf();
    let (part, file) = blocking(move || create_part(&chosen)).await?;
    // One local write per 256 KiB instead of per 32 KiB chunk.
    let mut file =
        tokio::io::BufWriter::with_capacity(LOCAL_WRITE, tokio::fs::File::from_std(file));

    let conn = sftp.conn.clone();
    let (stop, _stop_guard) = stop_signal(gate.cancel_signal(), move || conn.is_gone());
    // A throttle wait (up to a chunk's cost at the lowest limit, more behind the session's
    // other transfers) also ends when the connection is lost, not only on a user cancel.
    let throttle_stop = cancel_or_stop(cancelled, stop.clone());
    let mut cancel = Some(stop);
    let copy = async {
        let handle = save::or_cancelled(&mut cancel, sftp.open_read(remote)).await?;
        let mut reader = PipelinedReader::with_window(
            sftp.raw.clone(),
            handle.clone(),
            sftp.read_chunk(),
            None,
            gate.window(),
        );
        let result = async {
            loop {
                if cancelled() {
                    return Err(AppError::new(ErrorCode::Cancelled));
                }
                // A read stuck on a stalled server ends as soon as the transfer is
                // cancelled (not after the 30 s request timeout).
                let next = tokio::select! {
                    biased;
                    () = save::until_cancelled(cancel.as_mut()) => {
                        return Err(AppError::new(ErrorCode::Cancelled));
                    }
                    next = reader.next() => next,
                };
                match next {
                    Ok(Some(chunk)) => {
                        session
                            .throttle
                            .download
                            .acquire(chunk.len(), &throttle_stop)
                            .await?;
                        file.write_all(&chunk)
                            .await
                            .map_err(|e| local_error(&e, &part, IoOp::WriteLocalFile))?;
                        progress(reader.position());
                    }
                    Ok(None) => return Ok(reader.position()),
                    Err(e) => return Err(sftp.map_err(&e, Some(remote))),
                }
            }
        }
        .await;
        drop(reader);
        if cancelled() || sftp.conn.is_gone() {
            // Do not let a stalled server or a dead link keep the `.part` file around.
            let _ = tokio::time::timeout(CANCEL_CLOSE_WAIT, sftp.close_quietly(handle)).await;
        } else {
            sftp.close_quietly(handle).await;
        }
        let n = result?;
        // The buffered tail reaches the file before it is synced, marked and renamed.
        file.flush()
            .await
            .map_err(|e| local_error(&e, &part, IoOp::WriteLocalFile))?;
        file.get_mut()
            .sync_all()
            .await
            .map_err(|e| local_error(&e, &part, IoOp::WriteLocalFile))?;
        Ok::<u64, AppError>(n)
    }
    .await;
    // On failure or cancel the unflushed buffer is simply dropped with the `.part` file.
    let file = file.into_inner().into_std().await;
    let bytes = match copy {
        Ok(n) if !cancelled() => n,
        other => {
            drop(file);
            let part = part.clone();
            let _ = blocking(move || {
                let _ = std::fs::remove_file(&part);
                Ok(())
            })
            .await;
            let e = other
                .err()
                .unwrap_or_else(|| AppError::new(ErrorCode::Cancelled));
            return Err(lost_as_disconnected(Some(sftp), e));
        }
    };
    let chosen = local.to_path_buf();
    let part_done = part.clone();
    let with_times = times.map(|t| (file, t));
    if let Err(e) = blocking(move || finish_part_with(&part_done, &chosen, with_times)).await {
        let _ = blocking(move || {
            let _ = std::fs::remove_file(&part);
            Ok(())
        })
        .await;
        return Err(e);
    }
    Ok(Item::Copied(bytes))
}
