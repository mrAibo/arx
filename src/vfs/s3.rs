//! S3/MinIO VfsOps backend — stub.
//! ponytail: full implementation deferred. Impl VfsOps for S3Fs when ready.

use crate::vfs::{Entry, VfsOps};
use anyhow; // ponytail: needed for anyhow! macro
use std::io;
use std::path::Path;

pub struct S3Fs {
    pub bucket: String,
    pub prefix: String,
}

impl VfsOps for S3Fs {
    fn list(&self) -> anyhow::Result<Vec<Entry>> {
        Err(anyhow::anyhow!("S3 backend not yet implemented"))
    }
    fn read_head(&self, _path: &Path, _lines: usize) -> anyhow::Result<Vec<String>> {
        Err(anyhow::anyhow!("S3: not implemented"))
    }
    fn copy_files(&self, _from: &Path, _to: &Path, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("S3: not implemented"))
    }
    fn move_files(&self, _from: &Path, _to: &Path, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("S3: not implemented"))
    }
    fn delete_files(&self, _dir: &Path, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("S3: not implemented"))
    }
}
