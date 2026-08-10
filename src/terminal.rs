// Embedded terminal pane — PTY-backed shell running inside a pane.
// ponytail: uses portable-pty; add custom VT parser only when needed.

use anyhow::Result;
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use std::io::{Read, Write};
use std::sync::mpsc::{Receiver, Sender};

pub struct TermPane {
    pub master: Box<dyn portable_pty::MasterPty + Send>,
    pub writer: Box<dyn Write + Send>,
    pub child: Box<dyn portable_pty::Child + Send + Send>,
    pub reader: Receiver<String>,
    pub buffer: Vec<String>,
    pub scroll: usize,
    size: PtySize,
}

impl std::fmt::Debug for TermPane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermPane")
            .field("buffer_lines", &self.buffer.len())
            .field("scroll", &self.scroll)
            .finish()
    }
}

impl TermPane {
    pub fn spawn(cwd: &std::path::Path) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let mut cmd =
            CommandBuilder::new(std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into()));
        cmd.cwd(cwd);
        cmd.env("TERM", "xterm-256color");
        let child = pair.slave.spawn_command(cmd)?;
        drop(pair.slave);

        let size = pair.master.get_size()?;
        let writer = pair.master.take_writer()?;
        let mut reader = pair.master.try_clone_reader()?;
        let (tx, rx): (Sender<String>, Receiver<String>) = std::sync::mpsc::channel();

        // Background reader thread
        std::thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        let s = String::from_utf8_lossy(&buf[..n]).to_string();
                        if tx.send(s).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(TermPane {
            master: pair.master,
            writer,
            child,
            reader: rx,
            buffer: Vec::new(),
            scroll: 0,
            size,
        })
    }

    /// Drain PTY output into buffer. Call in the event loop.
    pub fn drain(&mut self) {
        while let Ok(chunk) = self.reader.try_recv() {
            let cleaned = strip_ansi(&chunk);
            for part in cleaned.split_inclusive('\n') {
                if let Some(last) = self.buffer.last_mut() {
                    last.push_str(part);
                    if part.ends_with('\n') {
                        self.buffer.push(String::new());
                    }
                } else {
                    self.buffer.push(part.to_string());
                }
            }
            if self.buffer.last().map(|s| s.is_empty()).unwrap_or(false) {
                self.buffer.pop();
            }
        }
        if self.buffer.len() > self.size.rows as usize {
            self.scroll = self.buffer.len().saturating_sub(self.size.rows as usize);
        }
    }

    /// Send data to the PTY master (stdin of the shell)
    pub fn write(&mut self, data: &str) {
        let _ = self.writer.write_all(data.as_bytes());
    }

    pub fn resize(&mut self, rows: u16, cols: u16) -> Result<()> {
        self.size = PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        };
        self.master.resize(self.size)?;
        Ok(())
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
    }
}

impl Drop for TermPane {
    fn drop(&mut self) {
        self.kill();
    }
}

/// Strip basic ANSI escape sequences. ponytail: regex-free, covers CSI sequences.
fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.peek() == Some(&'[') {
            chars.next(); // consume '['
            while let Some(&nc) = chars.peek() {
                if nc.is_ascii_digit() || nc == ';' || nc == '?' {
                    chars.next();
                } else {
                    break;
                }
            }
            // Consume final byte (0x40-0x7E)
            #[allow(clippy::collapsible_if)]
            {
                if let Some(&nc) = chars.peek() {
                    if (0x40..=0x7E).contains(&(nc as u32)) {
                        chars.next();
                    }
                }
            }
        } else if c != '\r' {
            result.push(c);
        }
    }
    result
}
