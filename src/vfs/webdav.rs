//! WebDAV VfsOps backend — stub.
//! ponytail: impl VfsOps for WebDavFs when reqwest dep added.

use crate::vfs::{Entry, VfsOps};
use anyhow; // ponytail: needed for anyhow! macro
use std::io;
use std::path::Path;

pub struct WebDavFs {
    pub base_url: String,
}

impl VfsOps for WebDavFs {
    fn list(&self) -> anyhow::Result<Vec<Entry>> {
        Err(anyhow::anyhow!("WebDAV backend not yet implemented"))
    }
    fn read_head(&self, _path: &Path, _lines: usize) -> anyhow::Result<Vec<String>> {
        Err(anyhow::anyhow!("WebDAV: not implemented"))
    }
    fn copy_files(&self, _from: &Path, _to: &Path, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDAV: not implemented"))
    }
    fn move_files(&self, _from: &Path, _to: &Path, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDAV: not implemented"))
    }
    fn delete_files(&self, _dir: &Path, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDAV: not implemented"))
    }
}
