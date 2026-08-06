//! WebDAV VfsProvider stub.
use crate::vfs::{Entry, VfsOps, VfsProvider};
use std::io;
use std::path::Path;

pub struct WebDavFs;
#[derive(Debug)]
pub struct WebDavProvider;

impl VfsProvider for WebDavProvider {
    fn list(&self, _path: &str) -> io::Result<Vec<Entry>> {
        Err(io::Error::other("WebDAV: not implemented"))
    }
    fn read_head(&self, _path: &str, _lines: usize) -> io::Result<Vec<String>> {
        Err(io::Error::other("WebDAV: not implemented"))
    }
    fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDAV: not implemented"))
    }
    fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDAV: not implemented"))
    }
    fn delete_files(&self, _dir: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDAV: not implemented"))
    }
}

impl VfsOps for WebDavFs {
    fn list(&self) -> anyhow::Result<Vec<Entry>> {
        Err(anyhow::anyhow!("WebDAV: not implemented"))
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
