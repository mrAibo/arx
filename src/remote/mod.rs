use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostId(String);

impl HostId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct HostGroupId(String);

impl HostGroupId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Host {
    pub id: HostId,
    pub name: String,
    pub ssh_alias: String,
    pub group_ids: BTreeSet<HostGroupId>,
    pub tags: BTreeSet<String>,
    pub favorite: bool,
    pub default_path: Option<String>,
    pub transfer_preference: TransferPreference,
    pub notes: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostGroup {
    pub id: HostGroupId,
    pub name: String,
    pub parent_id: Option<HostGroupId>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TransferPreference {
    #[default]
    Auto,
    Rsync,
    Sftp,
}

impl Host {
    pub fn belongs_to(&self, group_id: &HostGroupId) -> bool {
        self.group_ids.contains(group_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_can_belong_to_multiple_groups() {
        let database = HostGroupId::new("database");
        let project_a = HostGroupId::new("project-a");
        let production = HostGroupId::new("production");
        let host = Host {
            id: HostId::new("ora-prod-01"),
            name: "Oracle Production 01".into(),
            ssh_alias: "ora-prod-01".into(),
            group_ids: BTreeSet::from([
                database.clone(),
                project_a.clone(),
                production.clone(),
            ]),
            tags: BTreeSet::new(),
            favorite: true,
            default_path: Some("/opt/oracle".into()),
            transfer_preference: TransferPreference::Auto,
            notes: None,
        };

        assert!(host.belongs_to(&database));
        assert!(host.belongs_to(&project_a));
        assert!(host.belongs_to(&production));
        assert_eq!(host.id.as_str(), "ora-prod-01");
    }
}
