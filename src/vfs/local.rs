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

    /// Copy named files/dirs from `src_dir` to `dst_dir`. Returns count of successful copies.
    pub fn copy_files(src_dir: &Path, dst_dir: &Path, names: &[String]) -> io::Result<usize> {
        let mut count = 0;
        for name in names {
            let src = src_dir.join(name);
            let dst = dst_dir.join(name);
            if src.is_dir() {
                copy_dir_recursive(&src, &dst)?;
            } else {
                fs::copy(&src, &dst)?;
            }
            count += 1;
        }
        Ok(count)
    }

    /// Move (rename) named files/dirs from `src_dir` to `dst_dir`. Returns count.
    pub fn move_files(src_dir: &Path, dst_dir: &Path, names: &[String]) -> io::Result<usize> {
        let mut count = 0;
        for name in names {
            let src = src_dir.join(name);
            let dst = dst_dir.join(name);
            fs::rename(&src, &dst)?;
            count += 1;
        }
        Ok(count)
    }

    /// Delete named files/dirs from `dir`. Returns count.
    pub fn delete_files(dir: &Path, names: &[String]) -> io::Result<usize> {
        let mut count = 0;
        for name in names {
            let path = dir.join(name);
            if path.is_dir() {
                fs::remove_dir_all(&path)?;
            } else {
                fs::remove_file(&path)?;
            }
            count += 1;
        }
        Ok(count)
    }

    /// Read first N lines of a text file. Returns empty for binary/large files.
    /// ponytail: 500-line cap; add paging when viewer gets scroll-to-end
    pub fn read_head(path: &Path, max_lines: usize) -> io::Result<Vec<String>> {
        use std::io::{BufRead, BufReader};
        let file = fs::File::open(path)?;
        let reader = BufReader::new(file);
        let lines: Vec<String> = reader
            .lines()
            .take(max_lines)
            .filter_map(|l| l.ok())
            .collect();
        Ok(lines)
    }
}

// ponytail: simple recursive copy; add rsync-based copy for remote operations later
fn copy_dir_recursive(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn copy_and_delete_files() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("x.txt"), b"data").unwrap();
        fs::create_dir(src.path().join("subdir")).unwrap();
        fs::write(src.path().join("subdir/nested.txt"), b"nested").unwrap();

        let names = vec!["x.txt".into(), "subdir".into()];
        let n = LocalFs::copy_files(src.path(), dst.path(), &names).unwrap();
        assert_eq!(n, 2);
        assert!(dst.path().join("x.txt").exists());
        assert!(dst.path().join("subdir/nested.txt").exists());

        let n = LocalFs::delete_files(dst.path(), &names).unwrap();
        assert_eq!(n, 2);
        assert!(!dst.path().join("x.txt").exists());
        assert!(!dst.path().join("subdir").exists());
    }

    #[test]
    fn move_files_renames() {
        let src = tempfile::tempdir().unwrap();
        let dst = tempfile::tempdir().unwrap();
        fs::write(src.path().join("m.txt"), b"move me").unwrap();

        let n = LocalFs::move_files(src.path(), dst.path(), &["m.txt".into()]).unwrap();
        assert_eq!(n, 1);
        assert!(!src.path().join("m.txt").exists());
        assert!(dst.path().join("m.txt").exists());
    }
}
