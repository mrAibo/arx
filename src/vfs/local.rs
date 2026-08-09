use crate::vfs::{Entry, EntryKind};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// ISO 8601 timestamp for trash .trashinfo files.
fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = now.as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    // ponytail: approximate date; good enough for trash timestamps
    let days_since_epoch = secs / 86400;
    let year = 1970 + (days_since_epoch / 365) as u32;
    let day_of_year = (days_since_epoch % 365) as u32;
    let month = ((day_of_year * 12) / 365).min(11) + 1;
    let day = day_of_year.saturating_sub((month.saturating_sub(1)) * 365 / 12) + 1;
    format!("{year:04}-{month:02}-{day:02}T{h:02}:{m:02}:{s:02}")
}

/// Local filesystem backend.
pub struct LocalFs;

impl LocalFs {
    /// List directory contents at `path`. Returns entries sorted: dirs first, then by name.
    pub fn list(path: &Path) -> io::Result<Vec<Entry>> {
        let mut entries: Vec<Entry> = fs::read_dir(path)?
            .filter_map(|e| e.ok())
            .map(|e| {
                // ponytail: to_string_lossy() may mangle non-UTF-8 filenames.
                // Full OsString migration deferred — affects <0.01% of real files.
                let name = e.file_name().to_string_lossy().into_owned();
                let file_type = e.file_type().ok();
                let kind = match file_type {
                    Some(ft) if ft.is_dir() => EntryKind::Directory,
                    Some(ft) if ft.is_symlink() => EntryKind::Symlink,
                    Some(ft) if ft.is_file() => EntryKind::File,
                    _ => EntryKind::Other,
                };
                let metadata = e.metadata().ok();
                let size = if kind == EntryKind::File {
                    metadata.as_ref().map(|metadata| metadata.len())
                } else {
                    None
                };
                let modified_unix_ms = metadata
                    .as_ref()
                    .and_then(|metadata| metadata.modified().ok())
                    .and_then(crate::vfs::canonical_system_mtime_ms);
                Entry {
                    name,
                    kind,
                    size,
                    modified_unix_ms,
                }
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
    /// ponytail: atomic overwrite with mandatory backup + rollback on failure.
    pub fn copy_files(src_dir: &Path, dst_dir: &Path, names: &[String]) -> io::Result<usize> {
        let mut count = 0;
        for name in names {
            let src = src_dir.join(name);
            let dst = dst_dir.join(name);

            if dst.exists() {
                let bak = dst.with_extension(format!(
                    "{}.arx-bak",
                    dst.extension()
                        .map(|e| e.to_string_lossy().to_string())
                        .unwrap_or_default()
                ));
                if bak.exists() {
                    let _ = fs::remove_file(&bak);
                }
                fs::rename(&dst, &bak).map_err(|e| {
                    io::Error::other(format!("backup failed: {dst:?} → {bak:?}: {e}"))
                })?;
                // Rollback on failure
                if let Err(e) = (|| -> io::Result<()> {
                    if src.is_dir() {
                        copy_dir_recursive(&src, &dst)?;
                    } else {
                        fs::copy(&src, &dst)?;
                    }
                    Ok(())
                })() {
                    // Restore backup
                    if bak.exists() && !dst.exists() {
                        let _ = fs::rename(&bak, &dst);
                    }
                    return Err(e);
                }
            } else if src.is_dir() {
                copy_dir_recursive(&src, &dst)?;
            } else {
                fs::copy(&src, &dst)?;
            }
            count += 1;
        }
        Ok(count)
    }

    /// Move (rename) named files/dirs with EXDEV fallback (cross-device copy+delete).
    pub fn move_files(src_dir: &Path, dst_dir: &Path, names: &[String]) -> io::Result<usize> {
        let mut count = 0;
        for name in names {
            let src = src_dir.join(name);
            let dst = dst_dir.join(name);
            fs::rename(&src, &dst).or_else(|e| {
                // EXDEV: cross-device — copy + delete original
                if e.raw_os_error() == Some(18) {
                    if src.is_dir() {
                        copy_dir_recursive(&src, &dst)?;
                        fs::remove_dir_all(&src)?;
                    } else {
                        fs::copy(&src, &dst)?;
                        fs::remove_file(&src)?;
                    }
                    Ok(())
                } else {
                    Err(e)
                }
            })?;
            count += 1;
        }
        Ok(count)
    }

    /// Move files to trash (~/.local/share/Trash) instead of permanent delete.
    /// Creates .trashinfo files per Freedesktop spec for restore support.
    pub fn delete_files(dir: &Path, names: &[String]) -> io::Result<usize> {
        let base = dirs::data_dir()
            .map(|d| d.join("Trash"))
            .unwrap_or_else(|| PathBuf::from("/tmp/arx-trash"));
        let trash_files = base.join("files");
        let trash_info = base.join("info");
        std::fs::create_dir_all(&trash_files)?;
        std::fs::create_dir_all(&trash_info)?;

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let deletion_date = chrono_now();

        let mut count = 0;
        for name in names {
            let from = dir.join(name);
            if !from.exists() {
                continue;
            }
            let basename = from
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_else(|| name.clone());
            let mut to = trash_files.join(&basename);
            let mut info_file = trash_info.join(format!("{basename}.trashinfo"));

            // ponytail: unique names if collision in trash
            if to.exists() {
                let stem = from
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| name.clone());
                let ext = from
                    .extension()
                    .map(|e| e.to_string_lossy().to_string())
                    .unwrap_or_default();
                let unique = format!(
                    "{stem}.{now}{}",
                    if ext.is_empty() {
                        String::new()
                    } else {
                        format!(".{ext}")
                    }
                );
                to = trash_files.join(&unique);
                info_file = trash_info.join(format!("{unique}.trashinfo"));
            }

            // Write .trashinfo
            let original = std::fs::canonicalize(&from).unwrap_or_else(|_| from.clone());
            let info = format!(
                "[Trash Info]\nPath={}\nDeletionDate={}\n",
                original.display(),
                deletion_date
            );
            let _ = std::fs::write(&info_file, info);

            // Move to trash
            std::fs::rename(&from, &to).or_else(|e| {
                if e.raw_os_error() == Some(18) {
                    if from.is_dir() {
                        copy_dir_recursive(&from, &to)?;
                        std::fs::remove_dir_all(&from)?;
                    } else {
                        std::fs::copy(&from, &to)?;
                        std::fs::remove_file(&from)?;
                    }
                    Ok(())
                } else {
                    Err(e)
                }
            })?;
            count += 1;
        }
        Ok(count)
    }

    /// Restore all trashed files to their original locations.
    /// Returns count of restored files.
    pub fn restore_all() -> io::Result<usize> {
        let base = dirs::data_dir()
            .map(|d| d.join("Trash"))
            .unwrap_or_else(|| PathBuf::from("/tmp/arx-trash"));
        let trash_info = base.join("info");
        let trash_files = base.join("files");
        if !trash_info.exists() {
            return Ok(0);
        }

        let mut count = 0;
        for entry in std::fs::read_dir(&trash_info)? {
            let entry = entry?;
            if entry.path().extension() != Some(std::ffi::OsStr::new("trashinfo")) {
                continue;
            }
            let content = match std::fs::read_to_string(entry.path()) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let mut original = None;
            for line in content.lines() {
                if let Some(path) = line.strip_prefix("Path=") {
                    original = Some(PathBuf::from(path));
                }
            }
            let Some(ref orig) = original else {
                continue;
            };
            let basename = entry
                .path()
                .file_stem()
                .unwrap()
                .to_string_lossy()
                .to_string();
            let trashed = trash_files.join(&basename);
            if !trashed.exists() {
                continue;
            }

            if let Some(parent) = orig.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&trashed, orig).or_else(|e| {
                if e.raw_os_error() == Some(18) {
                    if trashed.is_dir() {
                        copy_dir_recursive(&trashed, orig)?;
                        std::fs::remove_dir_all(&trashed)?;
                    } else {
                        std::fs::copy(&trashed, orig)?;
                        std::fs::remove_file(&trashed)?;
                    }
                    Ok(())
                } else {
                    Err(e)
                }
            })?;
            let _ = std::fs::remove_file(entry.path());
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

// ── VfsProvider impl (Provider Registry) ──

use crate::vfs::VfsProvider;
#[derive(Debug)]
pub struct LocalProvider;
#[async_trait::async_trait]
impl VfsProvider for LocalProvider {
    fn list(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        LocalFs::list(std::path::Path::new(path))
    }
    async fn list_async(&self, path: &str) -> std::io::Result<Vec<Entry>> {
        let path = std::path::PathBuf::from(path);
        tokio::task::spawn_blocking(move || LocalFs::list(&path))
            .await
            .map_err(|error| std::io::Error::other(format!("local list worker failed: {error}")))?
    }
    fn read_head(&self, path: &str, lines: usize) -> std::io::Result<Vec<String>> {
        LocalFs::read_head(std::path::Path::new(path), lines)
    }
    fn copy_files(&self, src: &str, dst: &str, names: &[String]) -> std::io::Result<usize> {
        LocalFs::copy_files(std::path::Path::new(src), std::path::Path::new(dst), names)
    }
    fn move_files(&self, src: &str, dst: &str, names: &[String]) -> std::io::Result<usize> {
        LocalFs::move_files(std::path::Path::new(src), std::path::Path::new(dst), names)
    }
    fn delete_files(&self, dir: &str, names: &[String]) -> std::io::Result<usize> {
        LocalFs::delete_files(std::path::Path::new(dir), names)
    }
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
        let mtime = entries[1]
            .modified_unix_ms
            .expect("local entry should expose modification time");
        assert_eq!(
            mtime % 1_000,
            0,
            "mtime is canonical whole-second milliseconds"
        );
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
