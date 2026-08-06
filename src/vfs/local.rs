use crate::vfs::{Entry, EntryKind};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Local filesystem backend.
pub struct LocalFs;

impl LocalFs {
    /// List directory contents at `path`. Returns entries sorted: dirs first, then by name.
    pub fn list(path: &Path) -> io::Result<Vec<Entry>> {
        let mut entries: Vec<Entry> = fs::read_dir(path)?
            .filter_map(|e| e.ok())
            .map(|e| {
                let name = e.file_name().to_string_lossy().into_owned();
                let file_type = e.file_type().ok();
                let kind = match file_type {
                    Some(ft) if ft.is_dir() => EntryKind::Directory,
                    Some(ft) if ft.is_symlink() => EntryKind::Symlink,
                    Some(ft) if ft.is_file() => EntryKind::File,
                    _ => EntryKind::Other,
                };
                let size = file_type
                    .filter(|ft| ft.is_file())
                    .and_then(|_| e.metadata().ok())
                    .map(|m| m.len());
                Entry { name, kind, size }
            })
            .collect();

        // ponytail: dirs first, then alphabetical; sort_by_key is stable
        entries.sort_by(|a, b| {
            match (
                a.kind == EntryKind::Directory,
                b.kind == EntryKind::Directory,
            ) {
                (true, false) => std::cmp::Ordering::Less,
                (false, true) => std::cmp::Ordering::Greater,
                _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
            }
        });
        Ok(entries)
    }

    /// Resolve `..` from a path, or stay at root.
    pub fn parent(path: &Path) -> PathBuf {
        path.parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| path.to_path_buf())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn lists_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), b"hello").unwrap();
        fs::create_dir(dir.path().join("sub")).unwrap();

        let entries = LocalFs::list(dir.path()).unwrap();
        assert_eq!(entries.len(), 2, "two entries");
        assert_eq!(entries[0].name, "sub");
        assert_eq!(entries[0].kind, EntryKind::Directory);
        assert_eq!(entries[1].name, "a.txt");
        assert_eq!(entries[1].kind, EntryKind::File);
        assert_eq!(entries[1].size, Some(5));
    }
}
