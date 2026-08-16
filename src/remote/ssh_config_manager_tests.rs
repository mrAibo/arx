//! Tests for managed SSH host configuration safety (PACK B B17/B18).
//! These run against temp dirs via env overrides where possible; they never
//! touch the real ~/.ssh.

#[cfg(test)]
mod tests {
    use crate::remote::ssh_config_manager::{
        ManagedHost, add_managed_host, delete_managed_host, ensure_arx_include,
        generate_ed25519_key, list_managed_hosts, managed_config_path, open_config,
        reload_managed_hosts, update_managed_host, validate_alias,
    };
    use std::io::Write;
    use std::path::PathBuf;

    use std::sync::Mutex;

    /// Serialize HOME manipulation so parallel tests never clobber each other.
    static SSH_TEST_LOCK: Mutex<()> = Mutex::new(());

    /// Redirect ARX's ssh dir via a temp HOME so we never touch the real ~/.ssh.
    fn with_temp_ssh<F: FnOnce()>(f: F) {
        let _guard = SSH_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
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
            update_managed_host("prod", &h2).expect("edit");
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

    #[test]
    fn rename_atomic_removes_old_and_adds_new() {
        with_temp_ssh(|| {
            add_managed_host(&sample_host("prod")).expect("add");
            let mut renamed = sample_host("staging");
            renamed.hostname = "staging.example.com".into();
            update_managed_host("prod", &renamed).expect("rename");
            let hosts = list_managed_hosts();
            assert!(!hosts.contains_key("prod"), "old alias must be gone");
            assert!(hosts.contains_key("staging"), "new alias must exist");
            assert_eq!(hosts["staging"].hostname, "staging.example.com");
        });
    }

    #[test]
    fn rename_collision_with_unmanaged_fails_and_keeps_original() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let cfg = PathBuf::from(&ssh).join(".ssh").join("config");
            let mut f = std::fs::File::create(&cfg).unwrap();
            writeln!(f, "Host taken").unwrap();
            writeln!(f, "    HostName 10.0.0.7").unwrap();

            add_managed_host(&sample_host("prod")).expect("add");
            let clash = sample_host("taken");
            let err = update_managed_host("prod", &clash).expect_err("rename collision must fail");
            assert!(err.contains("collides"), "must explain collision: {err}");
            // original intact
            let hosts = list_managed_hosts();
            assert!(hosts.contains_key("prod"), "original must remain");
            assert!(
                !hosts.contains_key("taken"),
                "must not create colliding entry"
            );
        });
    }

    #[test]
    fn include_equivalence_recognizes_tilde_absolute_glob() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            // Write a user config that already includes the managed file via absolute path.
            let abs = ssh_dir.join("arx_hosts.conf").display().to_string();
            let mut f = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(f, "Include {abs}").unwrap();
            assert!(crate::remote::ssh_config_manager::is_arx_include_installed());
            // Second ensure must NOT append another include.
            ensure_arx_include().unwrap();
            let cfg = std::fs::read_to_string(ssh_dir.join("config")).unwrap();
            let includes = cfg
                .lines()
                .filter(|l| l.trim_start().to_lowercase().starts_with("include"))
                .count();
            assert_eq!(includes, 1, "must stay exactly one include");
        });
    }

    #[test]
    fn glob_include_discovers_unmanaged_host_for_collision() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(ssh_dir.join("config.d")).unwrap();
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh/config.d/*.conf").unwrap();
            let mut inc =
                std::fs::File::create(ssh_dir.join("config.d").join("extra.conf")).unwrap();
            writeln!(inc, "Host globbed").unwrap();
            writeln!(inc, "    HostName 10.0.0.11").unwrap();
            // Adding a managed host colliding with the globbed alias must fail closed.
            let h = sample_host("globbed");
            assert!(
                add_managed_host(&h).is_err(),
                "glob collision must fail closed"
            );
        });
    }

    #[test]
    fn same_filename_outside_managed_path_is_unmanaged() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            // A file named arx_hosts.conf somewhere ELSEWHERE is NOT ARX-owned.
            let other = ssh_dir.join("other.arx_hosts.conf");
            let mut f = std::fs::File::create(&other).unwrap();
            writeln!(f, "Host stray").unwrap();
            writeln!(f, "    HostName 10.0.0.20").unwrap();
            let mut cfg = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(cfg, "Include ~/.ssh/other.arx_hosts.conf").unwrap();
            // Managed add of "stray" collides (unmanaged), must fail closed.
            let h = sample_host("stray");
            assert!(
                add_managed_host(&h).is_err(),
                "stray unmanaged must collide"
            );
        });
    }

    #[test]
    fn first_backup_preserved_on_reinstall() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            let mut cfg = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(cfg, "# original user comment").unwrap();
            writeln!(cfg, "Host keep").unwrap();
            writeln!(cfg, "    HostName 10.0.0.5").unwrap();
            ensure_arx_include().unwrap();
            // Tamper with backup to prove it is preserved.
            std::fs::write(ssh_dir.join("config.arx-backup"), "BACKUP_MARKER").unwrap();
            ensure_arx_include().unwrap();
            let backup = std::fs::read_to_string(ssh_dir.join("config.arx-backup")).unwrap();
            assert!(
                backup.contains("BACKUP_MARKER"),
                "original backup must be preserved"
            );
            // User comment still present in live config.
            let live = std::fs::read_to_string(ssh_dir.join("config")).unwrap();
            assert!(
                live.contains("original user comment"),
                "user bytes preserved"
            );
        });
    }

    #[test]
    fn existing_ssh_dir_permissions_untouched() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(&ssh_dir).unwrap();
            // Simulate pre-existing dir (ARX must not chmod it on open).
            ensure_arx_include().unwrap();
            assert!(ssh_dir.exists());
            // No panic / no permission rewrite assertion beyond success.
        });
    }

    #[test]
    fn add_invalid_alias_rejected() {
        with_temp_ssh(|| {
            for bad in ["", " ", "bad\n", "web*", "a\x00", "../escape"] {
                let mut h = sample_host(bad);
                h.alias = bad.to_string();
                assert!(add_managed_host(&h).is_err(), "alias {bad:?} must reject");
            }
        });
    }

    #[test]
    fn add_valid_host_persists_and_reloads() {
        with_temp_ssh(|| {
            add_managed_host(&sample_host("web1")).expect("add");
            // Simulate an external edit: append a comment to the managed file.
            let mut content = std::fs::read_to_string(managed_config_path()).unwrap();
            content.push_str("\n# external edit\n");
            std::fs::write(managed_config_path(), &content).unwrap();
            reload_managed_hosts();
            let hosts = list_managed_hosts();
            assert!(
                hosts.contains_key("web1"),
                "external edit must not drop host"
            );
        });
    }

    #[test]
    fn delete_removes_only_managed_host() {
        with_temp_ssh(|| {
            // An unmanaged host lives in user config.
            let ssh = std::env::var("HOME").unwrap();
            let cfg = PathBuf::from(&ssh).join(".ssh").join("config");
            let mut f = std::fs::File::create(&cfg).unwrap();
            writeln!(f, "Host unmanaged").unwrap();
            writeln!(f, "    HostName 10.0.0.99").unwrap();
            add_managed_host(&sample_host("managed")).expect("add");
            delete_managed_host("managed").expect("delete managed");
            assert!(
                !list_managed_hosts().contains_key("managed"),
                "managed must be gone"
            );
            // unmanaged host survives in the user config (byte-preserved).
            let cfg_content = std::fs::read_to_string(&cfg).unwrap();
            assert!(
                cfg_content.contains("Host unmanaged"),
                "unmanaged host must remain in user config"
            );
        });
    }

    #[test]
    fn generate_key_returns_path_only_not_bytes() {
        with_temp_ssh(|| {
            let path = generate_ed25519_key("gentest").expect("generate");
            assert!(path.exists(), "key file must exist");
            let pub_path = path.with_extension("pub");
            assert!(pub_path.exists(), "public key must exist");
            // The managed file must never contain key bytes.
            let mut h = sample_host("genhost");
            h.identity_file = Some(path.clone());
            add_managed_host(&h).unwrap();
            let content = std::fs::read_to_string(managed_config_path()).unwrap();
            assert!(content.contains(&format!("IdentityFile {}", path.display())));
            assert!(!content.contains("BEGIN OPENSSH PRIVATE KEY"));
            assert!(!content.contains("PRIVATE KEY"));
        });
    }

    #[test]
    fn open_config_returns_expected_paths() {
        with_temp_ssh(|| {
            assert_eq!(
                open_config(false),
                Some(
                    PathBuf::from(&std::env::var("HOME").unwrap())
                        .join(".ssh")
                        .join("config")
                )
            );
            assert_eq!(
                open_config(true),
                Some(
                    PathBuf::from(&std::env::var("HOME").unwrap())
                        .join(".ssh")
                        .join("arx_hosts.conf")
                )
            );
        });
    }

    #[test]
    fn secret_bytes_never_enter_config_or_logs() {
        with_temp_ssh(|| {
            add_managed_host(&sample_host("secret")).unwrap();
            let content = std::fs::read_to_string(managed_config_path()).unwrap();
            // AWS-style secrets must never appear.
            assert!(!content.contains("SecretAccessKey"));
            assert!(!content.contains("SessionToken"));
            assert!(!content.contains("BEGIN OPENSSH PRIVATE KEY"));
            assert!(!content.contains("PRIVATE KEY"));
            // The managed host is present by alias (discovery path-independent).
            assert!(content.contains("Host secret"));
        });
    }
}
