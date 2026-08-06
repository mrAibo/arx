//! WebDAV VfsProvider via reqwest (PROPFIND).
//! ponytail: manual XML parsing, no XML crate. HTTP Basic auth.
use crate::vfs::{Entry, EntryKind, VfsProvider};
use std::io;

#[derive(Debug, Clone)]
pub struct WebDavConfig {
    pub url: String, // e.g. https://webdav.example.com/remote.php/dav/files/user/
    pub user: String,
    pub password: String,
}

#[derive(Debug)]
pub struct WebDavProvider {
    pub config: WebDavConfig,
}

impl WebDavProvider {
    fn propfind(&self, path: &str, depth: &str) -> io::Result<String> {
        let url = format!("{}{}", self.config.url.trim_end_matches('/'), path);
        let body = r#"<?xml version="1.0" encoding="utf-8"?><D:propfind xmlns:D="DAV:"><D:prop><D:resourcetype/><D:getcontentlength/><D:displayname/></D:prop></D:propfind>"#;
        let resp = reqwest::blocking::Client::new()
            .request(reqwest::Method::from_bytes(b"PROPFIND").unwrap(), &url)
            .basic_auth(&self.config.user, Some(&self.config.password))
            .header("Depth", depth)
            .header("Content-Type", "application/xml")
            .body(body)
            .send()
            .map_err(|e| io::Error::other(format!("WebDAV: {e}")))?;
        resp.text().map_err(io::Error::other)
    }
}

impl VfsProvider for WebDavProvider {
    fn list(&self, path: &str) -> io::Result<Vec<Entry>> {
        let xml = self.propfind(path, "1")?;
        // ponytail: manual XML extraction for DAV response elements
        let mut entries = Vec::new();
        for resp_block in xml.split("<D:response>").skip(1) {
            let resp = resp_block.split("</D:response>").next().unwrap_or("");
            let href = resp
                .split("<D:href>")
                .nth(1)
                .and_then(|s| s.split("</D:href>").next())
                .map(|s| s.trim().to_string())
                .unwrap_or_default();
            let is_dir = resp.contains("<D:collection/>") || resp.contains("<D:collection />");
            let name = href
                .split('/')
                .rfind(|s| !s.is_empty())
                .unwrap_or(&href)
                .to_string();
            if name.is_empty() {
                continue;
            }
            entries.push(Entry {
                name,
                kind: if is_dir {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
                size: None,
            });
        }
        Ok(entries)
    }

    fn read_head(&self, path: &str, lines: usize) -> io::Result<Vec<String>> {
        let url = format!("{}{}", self.config.url.trim_end_matches('/'), path);
        let resp = reqwest::blocking::Client::new()
            .get(&url)
            .basic_auth(&self.config.user, Some(&self.config.password))
            .send()
            .map_err(|e| io::Error::other(format!("WebDAV GET: {e}")))?;
        let body = resp.text().map_err(io::Error::other)?;
        Ok(body.lines().take(lines).map(|s| s.to_string()).collect())
    }

    fn copy_files(&self, _src: &str, _dst: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDAV copy: use COPY method"))
    }
    fn move_files(&self, _src: &str, _dst: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDAV move: use MOVE method"))
    }
    fn delete_files(&self, _dir: &str, _names: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDAV delete: use DELETE method"))
    }
}

// Old VfsOps stub
pub struct WebDavFs;
use crate::vfs::VfsOps;

impl VfsOps for WebDavFs {
    fn list(&self) -> anyhow::Result<Vec<Entry>> {
        Err(anyhow::anyhow!("WebDavFs: use WebDavProvider"))
    }
    fn read_head(&self, _: &str, _: usize) -> anyhow::Result<Vec<String>> {
        Err(anyhow::anyhow!("WebDavFs: use WebDavProvider"))
    }
    fn copy_files(&self, _: &str, _: &str, _: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDavFs: use WebDavProvider"))
    }
    fn move_files(&self, _: &str, _: &str, _: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDavFs: use WebDavProvider"))
    }
    fn delete_files(&self, _: &str, _: &[String]) -> io::Result<usize> {
        Err(io::Error::other("WebDavFs: use WebDavProvider"))
    }
}
