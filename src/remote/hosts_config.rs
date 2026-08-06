use crate::remote::Host;
use std::path::PathBuf;

/// Load hosts from `~/.config/arx/hosts.toml`. Falls back to empty vec.
pub fn load_hosts() -> Vec<Host> {
    let path = hosts_path();
    if !path.exists() {
        return default_hosts();
    }
    match std::fs::read_to_string(&path) {
        Ok(content) => toml::from_str::<HostsFile>(&content)
            .map(|h| h.hosts.into_iter().map(Host::from).collect())
            .unwrap_or_else(|e| {
                eprintln!("arx: bad hosts.toml: {e}");
                default_hosts()
            }),
        Err(_) => default_hosts(),
    }
}

fn hosts_path() -> PathBuf {
    dirs::config_dir()
        .map(|d| d.join("arx").join("hosts.toml"))
        .unwrap_or_else(|| PathBuf::from("hosts.toml"))
}

fn default_hosts() -> Vec<Host> {
    // ponytail: empty default; user fills ~/.config/arx/hosts.toml
    vec![]
}

#[derive(serde::Deserialize)]
struct HostsFile {
    #[serde(default)]
    hosts: Vec<HostToml>,
}

#[derive(serde::Deserialize)]
struct HostToml {
    id: String,
    name: String,
    #[serde(default)]
    ssh_alias: Option<String>,
    hostname: String,
    #[serde(default = "default_port")]
    port: u16,
    #[serde(default = "default_user")]
    user: String,
    #[serde(default)]
    groups: Vec<String>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    favorite: bool,
    default_path: Option<String>,
    #[serde(default)]
    transfer_preference: Option<String>,
    notes: Option<String>,
}

fn default_port() -> u16 {
    22
}
fn default_user() -> String {
    std::env::var("USER").unwrap_or_else(|_| "root".into())
}

impl From<HostToml> for Host {
    fn from(h: HostToml) -> Self {
        let ssh_alias = h.ssh_alias.unwrap_or_else(|| h.id.clone());
        let transfer_preference = match h.transfer_preference.as_deref() {
            Some("rsync") => crate::remote::TransferPreference::Rsync,
            Some("sftp") => crate::remote::TransferPreference::Sftp,
            _ => crate::remote::TransferPreference::Auto,
        };
        Self {
            id: h.id,
            name: h.name,
            ssh_alias,
            hostname: h.hostname,
            port: h.port,
            user: h.user,
            group_ids: h.groups.into_iter().collect(),
            tags: h.tags.into_iter().collect(),
            favorite: h.favorite,
            default_path: h.default_path,
            transfer_preference,
            notes: h.notes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_host() {
        let toml = r#"
[[hosts]]
id = "nuc"
name = "Headless NUC"
hostname = "192.168.1.10"
"#;
        let hf: HostsFile = toml::from_str(toml).unwrap();
        assert_eq!(hf.hosts.len(), 1);
        let host: Host = hf.hosts.into_iter().next().unwrap().into();
        assert_eq!(host.id, "nuc");
        assert_eq!(host.hostname, "192.168.1.10");
        assert_eq!(host.port, 22);
    }
}
