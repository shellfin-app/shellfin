//! Remote account ids for permission hints: one fixed `exec` of `id -u; id -G`
//! on the destination at connect/reconnect.
//!
//! - The command is a constant: no user input ever reaches the remote shell (§9).
//! - Bounded: 2 s budget, 4 KiB of output. Any failure (non-POSIX server such as Windows
//!   OpenSSH, `exec` disabled by `ForceCommand`/`Subsystem`-only setups, timeout, odd output)
//!   yields `None`; the UI then shows no hint.
//! - `;` (not `&&`) works in POSIX shells, fish ≥ 3, csh and tcsh alike.
//! - Output is parsed strictly: the *last* pair of lines `<uid>` / `<gid> <gid> …` wins, so a
//!   banner or `.bashrc` noise printed before it is ignored.

use std::time::Duration;

use russh::ChannelMsg;
use russh::client::Handle;

use crate::ipc::dto::RemoteIdentity;
use crate::ssh::handler::ClientHandler;

/// The fixed command.
pub const COMMAND: &str = "id -u; id -G";
/// Whole probe budget (channel open, exec, output).
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// More output than this is not `id` output.
const MAX_OUTPUT: usize = 4096;
/// A user in more groups than this gets no hint (not a real account layout).
const MAX_GROUPS: usize = 256;

/// `id -u; id -G` on the destination; `None` on any failure.
pub async fn probe(handle: &Handle<ClientHandler>) -> Option<RemoteIdentity> {
    let result = tokio::time::timeout(PROBE_TIMEOUT, run(handle)).await;
    let identity = match result {
        Ok(Ok(out)) => parse(&out),
        Ok(Err(reason)) => {
            tracing::debug!(reason, "remote identity probe failed");
            None
        }
        Err(_) => {
            tracing::debug!("remote identity probe timed out");
            None
        }
    };
    tracing::info!(known = identity.is_some(), "remote identity probed");
    identity
}

async fn run(handle: &Handle<ClientHandler>) -> Result<String, &'static str> {
    let mut channel = handle
        .channel_open_session()
        .await
        .map_err(|_| "open session")?;
    channel
        .exec(true, COMMAND)
        .await
        .map_err(|_| "exec request")?;
    // EOF right away: a forced command (`internal-sftp`, `command="cat"`, a menu) that reads
    // stdin then ends instead of waiting out the budget.
    let _ = channel.eof().await;
    let mut out: Vec<u8> = Vec::new();
    let mut exit_ok = None;
    while let Some(msg) = channel.wait().await {
        match msg {
            ChannelMsg::Data { data } => {
                if out.len() + data.len() > MAX_OUTPUT {
                    return Err("output too large");
                }
                out.extend_from_slice(&data);
            }
            ChannelMsg::ExitStatus { exit_status } => exit_ok = Some(exit_status == 0),
            ChannelMsg::Failure => return Err("exec refused"),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    let _ = channel.close().await;
    if exit_ok == Some(false) {
        return Err("non-zero exit");
    }
    String::from_utf8(out).map_err(|_| "not UTF-8")
}

/// Parse `id -u` / `id -G` output (see the module docs).
pub fn parse(output: &str) -> Option<RemoteIdentity> {
    let lines: Vec<&str> = output.lines().map(str::trim).collect();
    lines.windows(2).rev().find_map(|pair| {
        let (uid_line, gid_line) = (pair.first()?, pair.get(1)?);
        let uid = uid_line.parse::<u32>().ok()?;
        let gids: Vec<u32> = gid_line
            .split_ascii_whitespace()
            .map(|g| g.parse::<u32>().ok())
            .collect::<Option<_>>()?;
        (!gids.is_empty() && gids.len() <= MAX_GROUPS).then_some(RemoteIdentity { uid, gids })
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn id(uid: u32, gids: &[u32]) -> Option<RemoteIdentity> {
        Some(RemoteIdentity {
            uid,
            gids: gids.to_vec(),
        })
    }

    #[test]
    fn parses_id_output() {
        assert_eq!(parse("1000\n1000 4 24 27\n"), id(1000, &[1000, 4, 24, 27]));
        assert_eq!(parse("0\n0\n"), id(0, &[0]));
        assert_eq!(parse("1000\r\n1000 27\r\n"), id(1000, &[1000, 27]));
    }

    #[test]
    fn ignores_noise_before_the_ids() {
        assert_eq!(
            parse("Welcome to the box\nLast login: today\n501\n20 12 61\n"),
            id(501, &[20, 12, 61])
        );
    }

    #[test]
    fn rejects_everything_else() {
        for bad in [
            "",
            "1000\n",
            "'id' is not recognized as an internal or external command,\n",
            "id: command not found\n",
            "uid=1000(a) gid=1000(a)\n",
            "1000\n-1 5\n",
            "1000\nabc\n",
            "99999999999\n1\n",
        ] {
            assert_eq!(parse(bad), None, "{bad:?}");
        }
        let many = (0..300)
            .map(|n| n.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        assert_eq!(parse(&format!("1\n{many}\n")), None);
    }

    #[test]
    fn command_is_fixed_and_shell_neutral() {
        assert_eq!(COMMAND, "id -u; id -G");
        assert!(!COMMAND.contains("&&"));
    }
}
