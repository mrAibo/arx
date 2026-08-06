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
    pub session: SftpSession,
    child: Child,
}

impl OpenSshSftpConnection {
    pub async fn connect(alias: &str) -> io::Result<Self> {
        if alias.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "SSH alias must not be empty",
            ));
        }

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

        Ok(Self { session, child })
    }

    pub async fn close(mut self) -> io::Result<()> {
        let _ = self.session.close().await;
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

    pub async fn abort(&mut self) {
        let _ = self.child.kill().await;
        let _ = self.child.wait().await;
    }
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

    fn poll_flush(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.stdin).poll_flush(cx)
    }

    fn poll_shutdown(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), io::Error>> {
        Pin::new(&mut self.stdin).poll_shutdown(cx)
    }
}
