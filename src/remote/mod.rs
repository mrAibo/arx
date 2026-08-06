use std::collections::BTreeSet;

pub mod hosts_config;
pub mod ssh_config;
#[cfg(target_os = "linux")]
pub mod watch;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: String,
    pub name: String,
    pub ssh_alias: String,
    /// Resolved hostname for connection (falls back to ssh_alias).
    pub hostname: String,
    /// SSH port (default 22).
    pub port: u16,
    /// SSH user.
    pub user: String,
    pub group_ids: BTreeSet<String>,
    pub tags: BTreeSet<String>,
    pub favorite: bool,
    pub default_path: Option<String>,
    pub transfer_preference: TransferPreference,
    pub notes: Option<String>,
}

impl Host {
    /// Quick-construct a Host from an SSH alias (for internal use).
    pub fn from_alias(alias: &str) -> Self {
        Self {
            id: alias.to_string(),
            name: alias.to_string(),
            ssh_alias: alias.to_string(),
            hostname: alias.to_string(),
            port: 22,
            user: std::env::var("USER").unwrap_or_else(|_| "root".into()),
            group_ids: BTreeSet::new(),
            tags: BTreeSet::new(),
            favorite: false,
            default_path: None,
            transfer_preference: TransferPreference::Auto,
            notes: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostGroup {
    pub id: String,
    pub name: String,
    pub parent_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TransferPreference {
    #[default]
    Auto,
    Rsync,
    Sftp,
}

impl Host {
    pub fn belongs_to(&self, group_id: &str) -> bool {
        self.group_ids.contains(group_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_can_belong_to_multiple_groups() {
        let host = Host {
            id: "ora-prod-01".into(),
            name: "Oracle Production 01".into(),
            ssh_alias: "ora-prod-01".into(),
            hostname: "10.0.0.5".into(),
            port: 22,
            user: "oracle".into(),
            group_ids: BTreeSet::from(["database".into(), "project-a".into(), "production".into()]),
            tags: BTreeSet::new(),
            favorite: true,
            default_path: Some("/opt/oracle".into()),
            transfer_preference: TransferPreference::Auto,
            notes: None,
        };

        assert!(host.belongs_to("database"));
        assert!(host.belongs_to("project-a"));
        assert!(host.belongs_to("production"));
    }
}
