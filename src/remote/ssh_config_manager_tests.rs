//! Tests for managed SSH host configuration safety (PACK B B17/B18).
//! These run against temp dirs via env overrides where possible; they never
//! touch the real ~/.ssh.

#[cfg(test)]
mod tests {
    use crate::remote::ssh_config_manager::{
        ManagedHost, add_managed_host, delete_managed_host, ensure_arx_include, list_managed_hosts,
        managed_config_path, update_managed_host, validate_alias,
    };
    use std::io::Write;
    use std::path::PathBuf;

    use std::sync::Mutex;

    /// Serialize HOME manipulation so parallel tests never clobber each other.
    static SSH_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Redirect ARX's ssh dir via a temp HOME so we never touch the real ~/.ssh.
    fn with_temp_ssh<F: FnOnce()>(f: F) {
        let _guard = SSH_TEST_LOCK.lock().unwrap();
        let tid = format!("{:?}", std::thread::current().id());
        let unique = format!("arx-sshtest-{}-{}", std::process::id(), tid);
        let tmp = std::env::temp_dir().join(unique);
        let _ = std::fs::create_dir_all(tmp.join(".ssh"));
        unsafe {
            std::env::set_var("HOME", &tmp);
        }
        f();
        let _ = std::fs::remove_dir_all(&tmp);
        unsafe {
            std::env::remove_var("HOME");
        }
    }

    fn sample_host(alias: &str) -> ManagedHost {
        ManagedHost {
            alias: alias.into(),
            hostname: "example.com".into(),
            user: "deploy".into(),
            port: 22,
            identity_file: Some(PathBuf::from("~/.ssh/arx/prod_ed25519")),
            proxy_jump: None,
            identities_only: true,
        }
    }

    #[test]
    fn alias_newline_rejected() {
        assert!(validate_alias("ok").is_ok());
        assert!(validate_alias("bad\n").is_err());
        assert!(validate_alias("bad\r").is_err());
    }

    #[test]
    fn alias_control_char_rejected() {
        assert!(validate_alias("bad\x00").is_err());
        assert!(validate_alias("bad\x01").is_err());
    }

    #[test]
    fn wildcard_alias_rejected() {
        assert!(validate_alias("web*").is_err());
        assert!(validate_alias("web?").is_err());
    }

    #[test]
    fn port_validation() {
        assert!(sample_host("x").validate().is_ok());
        let mut h = sample_host("x");
        h.port = 0;
        assert!(h.validate().is_err());
    }

    #[test]
    fn managed_add_edit_delete() {
        with_temp_ssh(|| {
            let h = sample_host("prod");
            add_managed_host(&h).expect("add");
            let hosts = list_managed_hosts();
            assert!(hosts.contains_key("prod"));
            assert_eq!(hosts["prod"].user, "deploy");

            let mut h2 = h.clone();
            h2.port = 2222;
            update_managed_host(&h2).expect("edit");
            assert_eq!(list_managed_hosts()["prod"].port, 2222);

            delete_managed_host("prod").expect("delete");
            assert!(!list_managed_hosts().contains_key("prod"));
        });
    }

    #[test]
    fn collision_with_unmanaged_fails_closed() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let cfg = PathBuf::from(&ssh).join(".ssh").join("config");
            let mut f = std::fs::File::create(&cfg).unwrap();
            writeln!(f, "Host existing").unwrap();
            writeln!(f, "    HostName 10.0.0.9").unwrap();
            writeln!(f, "    User root").unwrap();

            let h = sample_host("existing");
            assert!(add_managed_host(&h).is_err(), "collision must fail closed");
        });
    }

    #[test]
    fn include_installed_once_not_duplicated() {
        with_temp_ssh(|| {
            ensure_arx_include().expect("first");
            ensure_arx_include().expect("second idempotent");
            let ssh = std::env::var("HOME").unwrap();
            let cfg =
                std::fs::read_to_string(PathBuf::from(&ssh).join(".ssh").join("config")).unwrap();
            let count = cfg
                .lines()
                .filter(|l| l.trim_start().to_lowercase().starts_with("include"))
                .count();
            assert_eq!(count, 1, "include must appear exactly once");
        });
    }

    #[test]
    fn managed_file_stores_path_only_not_key_contents() {
        with_temp_ssh(|| {
            add_managed_host(&sample_host("nas")).unwrap();
            let content = std::fs::read_to_string(managed_config_path()).unwrap();
            assert!(content.contains("IdentityFile ~/.ssh/arx/prod_ed25519"));
            assert!(!content.contains("BEGIN"));
            assert!(!content.contains("PRIVATE KEY"));
        });
    }
}
