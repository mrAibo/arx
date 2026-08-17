use std::io;
use std::pin::Pin;
use std::process::Stdio;
use std::task::{Context, Poll};
use std::time::Duration;

use russh_sftp::client::SftpSession;
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time::timeout;

/// SFTP connection whose SSH transport is the user's system OpenSSH client.
///
/// ARX deliberately lets OpenSSH own ssh_config, ProxyJump, agent, identities,
/// known_hosts and host-key policy. russh-sftp only speaks the SFTP protocol
/// over the already-authenticated subsystem stream.
pub struct OpenSshSftpConnection {
    pub session: Option<SftpSession>,
    child: Child,
}

impl OpenSshSftpConnection {
    pub async fn connect(alias: &str) -> io::Result<Self> {
        super::validate_ssh_alias(alias)?;

        let mut child = Command::new("ssh")
            .args([
                "-T",
                "-o",
                "BatchMode=yes",
                "-o",
                "StrictHostKeyChecking=yes",
                "-o",
                "ConnectTimeout=5",
                "-s",
                alias,
                "sftp",
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("OpenSSH SFTP stdin unavailable"))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("OpenSSH SFTP stdout unavailable"))?;

        let stream = SshSubsystemStream { stdin, stdout };
        let session = SftpSession::new(stream)
            .await
            .map_err(|error| io::Error::other(format!("SFTP handshake failed: {error}")))?;
        session.set_timeout(30);

        Ok(Self {
            session: Some(session),
            child,
        })
    }

    /// Create a transaction directory with mode 0700 in the creating syscall.
    /// SFTP v3's MKDIR wrapper does not expose attributes, so use the same
    /// validated OpenSSH transport for this one operation.
    pub async fn create_private_dir(alias: &str, path: &str) -> io::Result<()> {
        super::validate_ssh_alias(alias)?;
        if path.contains('\0') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "remote directory path contains NUL",
            ));
        }

        let script = format!("set -eu; umask 077; mkdir -m 700 -- {}", shell_quote(path));
        let remote_command = format!("sh -c {}", shell_quote(&script));
        let status = timeout(
            Duration::from_secs(10),
            Command::new("ssh")
                .args([
                    "-T",
                    "-o",
                    "BatchMode=yes",
                    "-o",
                    "StrictHostKeyChecking=yes",
                    "-o",
                    "ConnectTimeout=5",
                    alias,
                    &remote_command,
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .kill_on_drop(true)
                .status(),
        )
        .await
        .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "SSH mkdir outcome uncertain"))??;

        match status.code() {
            Some(0) => Ok(()),
            Some(255) | None => Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "SSH mkdir transport outcome uncertain",
            )),
            Some(code) => Err(io::Error::other(format!(
                "remote private mkdir failed with status {code}"
            ))),
        }
    }

    pub async fn close(mut self) -> io::Result<()> {
        if let Some(session) = self.session.take() {
            let _ = session.close().await;
        }
        match timeout(Duration::from_secs(2), self.child.wait()).await {
            Ok(result) => {
                let _ = result?;
            }
            Err(_) => {
                let _ = self.child.kill().await;
                let _ = self.child.wait().await?;
            }
        }
        Ok(())
    }

    pub fn session(&self) -> &SftpSession {
        self.session.as_ref().expect("connected session")
    }

    pub async fn abort(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }

    /// Test-only connection used by the deterministic pooled-acquire matrix.
    /// It owns a dummy child process (so `abort()` has something to kill) but
    /// holds NO live SFTP session — the injected test probe ignores the
    /// session, so no handshake (no SSH, no network, no external sftp-server
    /// binary) runs. This keeps #48's acquire/probe/reconnect tests fully local
    /// and flake-free across environments, per the "no network / no sleeps in
    /// deterministic tests" contract.
    #[cfg(test)]
    pub(crate) async fn test_stub() -> Self {
        use tokio::process::Command;
        // ponytail: portable dummy child (no sftp-server needed) — only exists
        // so abort() can be exercised; the pooled tests never touch a session.
        let child = Command::new("true")
            .kill_on_drop(true)
            .spawn()
            .expect("test_stub dummy child spawns");
        Self {
            session: None,
            child,
        }
    }
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

struct SshSubsystemStream {
    stdin: ChildStdin,
    stdout: ChildStdout,
}

impl AsyncRead for SshSubsystemStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stdout).poll_read(cx, buf)
    }
}

impl AsyncWrite for SshSubsystemStream {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<Result<usize, io::Error>> {
        Pin::new(&mut self.stdin).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn transport_rejects_ssh_option_alias_before_spawn() {
        let error = match OpenSshSftpConnection::connect("-oProxyCommand=bad").await {
            Ok(_) => panic!("unsafe SSH alias reached process spawn"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    }

    #[test]
    fn shell_quote_preserves_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    }
}
