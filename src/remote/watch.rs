//! Remote Watch — sync local changes to remote via inotify + rsync.
//! ponytail: shells out to inotifywait + rsync. No crate dependency.
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};

/// Watch a local directory for changes and rsync them to a remote host.
/// Spawns a background process pair: inotifywait monitors, rsync syncs on each change.
pub fn start_watch(local: &Path, remote_host: &str, remote_path: &str) -> io::Result<()> {
    let target = format!("{remote_host}:{remote_path}");
    // Initial rsync
    let status = Command::new("rsync")
        .args(["-avz", "--progress", &local.to_string_lossy(), &target])
        .status()?;
    if !status.success() {
        return Err(io::Error::other("initial rsync failed"));
    }

    // Background inotify + rsync loop
    let local = local.to_path_buf();
    let remote_host = remote_host.to_string();
    let remote_path = remote_path.to_string();

    std::thread::spawn(move || {
        let mut child = Command::new("inotifywait")
            .args([
                "-m",
                "-r",
                "-e",
                "modify,create,delete,move",
                "--format",
                "%w%f",
                &local.to_string_lossy(),
            ])
            .stdout(Stdio::piped())
            .spawn()
            .expect("inotifywait");

        let stdout = child.stdout.take().expect("pipe");
        use std::io::BufRead;
        for _line in std::io::BufReader::new(stdout).lines() {
            let target = format!("{}:{}", remote_host, remote_path);
            let _ = Command::new("rsync")
                .args(["-avz", "--delete", &local.to_string_lossy(), &target])
                .status();
        }
        let _ = child.wait();
    });

    Ok(())
}
