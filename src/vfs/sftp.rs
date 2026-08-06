use super::Entry;
use std::io;

/// SFTP filesystem backend — stub. Full implementation pending host-config loading.
pub struct SftpFs;

impl SftpFs {
    /// List directory on remote host. Returns empty until host config is wired.
    pub fn list(_host: &str, _path: &str) -> io::Result<Vec<Entry>> {
        // ponytail: stub; wire russh after hosts.toml config is loadable
        Err(io::Error::other(
            "SFTP: host config not yet wired. Configure in ~/.config/arx/hosts.toml",
        ))
    }
}
