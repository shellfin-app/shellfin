//! Upload of one local file (§6.6) via the §6.4 temp + rename procedure: never through a remote
//! symlink, conflict policy, exact mode, optional times, throttle.

use std::path::Path;

use crate::error::{AppError, ErrorCode};
use crate::ipc::dto::{FileKind, TransferDirection, TransferEvent};

use super::super::client::{Sftp, kind_of};
use super::super::path;
use super::super::save::{self, Hooks, NEW_FILE_MODE};
use super::local::{LocalSource, open_upload_source, sftp_times, upload_info};
use super::{
    Conflict, Finish, Gate, Item, ItemCtx, Job, Progress, blocking, lost_as_disconnected,
    stop_signal,
};

pub(super) async fn upload(
    job: &Job<'_>,
    local: &Path,
    remote_dir: &str,
    gate: &mut Gate,
) -> Result<Finish, AppError> {
    let Job {
        id,
        session,
        conflict,
        preserve_times,
        sink,
    } = *job;
    let local_owned = local.to_path_buf();
    let (shown_name, total) = blocking(move || Ok(upload_info(&local_owned))).await?;
    sink.send(TransferEvent::Started {
        transfer_id: id.to_owned(),
        name: shown_name,
        direction: TransferDirection::Upload,
        total,
    });
    // Wait for a slot after `started`, before the local file or a remote handle is
    // opened (a burst of 1 000 files holds no handles while queued).
    let _slot = gate.acquire().await?;
    let local_owned = local.to_path_buf();
    let (file, name, size, times) = blocking(move || {
        let (file, name, size) = open_upload_source(&local_owned)?;
        // Times of the opened file (`preserveTimestamps`).
        let times = file.metadata().ok().as_ref().and_then(sftp_times);
        Ok((file, name, size, times))
    })
    .await?;
    let times = times.filter(|_| preserve_times);
    let sftp = Sftp::for_session(session).await?;
    let target = path::join(remote_dir, &name);
    let mut progress = Progress {
        id: id.to_owned(),
        total: Some(size),
        sink: sink.clone(),
        last: None,
    };
    let ctx = ItemCtx {
        session,
        sftp: &sftp,
        gate,
        conflict,
    };
    let source = UploadSource { file, local, times };
    match upload_item(&ctx, source, &target, &mut |n| progress.report(n)).await? {
        Item::Copied(written) => {
            let entry = sftp
                .entry(&target)
                .await
                .map_err(|e| lost_as_disconnected(Some(&sftp), e))?;
            Ok(Finish::Done(written, Some(entry)))
        }
        Item::Skipped => Ok(Finish::Skipped),
    }
}

/// An opened upload source ([`open_upload_source`]) and the times to set on the remote file
/// (`None` = keep the server's).
pub(super) struct UploadSource<'a> {
    pub file: std::fs::File,
    pub local: &'a Path,
    pub times: Option<(u32, u32)>,
}

/// One file's upload to `target` once its slot is held and its source is open: the remote
/// conflict check (a directory or special file refuses, a symlink is replaced as an entry,
/// never written through), then [`save::write_file`]. `progress` hears the bytes written so
/// far. The caller reads the new entry if it needs one.
pub(super) async fn upload_item(
    ctx: &ItemCtx<'_>,
    source: UploadSource<'_>,
    target: &str,
    progress: &mut (dyn FnMut(u64) + Send),
) -> Result<Item, AppError> {
    let ItemCtx {
        session,
        sftp,
        gate,
        conflict,
    } = *ctx;
    let UploadSource { file, local, times } = source;
    let overwrite = conflict == Conflict::Overwrite;
    let cancelled = gate.cancelled_fn();
    let cancelled = &cancelled;
    let window_pool = gate.pool();
    let conn = sftp.conn.clone();
    let (stop, _stop_guard) = stop_signal(gate.cancel_signal(), move || conn.is_gone());
    let result = async {
        // Uploads never write through a symlink. An existing link is replaced as an
        // entry (posix-rename over it); only a regular file is written into (keeping its mode,
        // owner and group).
        let existing = sftp.lstat_opt(target).await?;
        let (cur, exists) = match existing.as_ref().map(kind_of) {
            None => (None, false),
            Some(FileKind::Dir) => {
                return Err(AppError::new(ErrorCode::IsADirectory {
                    path: target.to_owned(),
                }));
            }
            Some(FileKind::Symlink) => (None, true),
            Some(FileKind::File) => (existing.as_ref(), true),
            Some(FileKind::Other) => return Err(save::not_regular(target)),
        };
        if exists && conflict == Conflict::Skip {
            return Ok(Item::Skipped);
        }
        if exists && !overwrite {
            return Err(AppError::new(ErrorCode::RemoteExists {
                path: target.to_owned(),
            }));
        }
        let mut source = LocalSource::new(
            tokio::fs::File::from_std(file),
            local.to_path_buf(),
            Some((&session.throttle.upload, cancelled)),
        );
        let mut hooks = Hooks {
            progress,
            cancelled,
            cancel_signal: Some(stop.clone()),
            window_pool: Some(window_pool.clone()),
        };
        let mut warnings = Vec::new();
        let written = save::write_file(
            sftp,
            &save::WriteSpec {
                target,
                cur,
                exists,
                new_mode: NEW_FILE_MODE,
                replace: overwrite,
                times,
                recheck: false,
            },
            &mut source,
            &mut hooks,
            &mut warnings,
        )
        .await?
        .bytes;
        for w in warnings {
            tracing::info!(code = ?w.kind, "upload warning");
        }
        Ok(Item::Copied(written))
    }
    .await;
    result.map_err(|e| lost_as_disconnected(Some(sftp), e))
}
