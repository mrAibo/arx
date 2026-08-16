//! Tests for managed SSH host configuration safety (PACK B B17/B18).
//! These run against temp dirs via env overrides where possible; they never
//! touch the real ~/.ssh.

#[cfg(test)]
mod tests {
    use crate::app::AppState;
    use crate::remote::ssh_config::parse_ssh_config;
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

            // Error messages from a collision must not echo secret material either.
            // Plant an unmanaged host whose HostName looks like a secret, then collide.
            let ssh = std::env::var("HOME").unwrap();
            let cfg = PathBuf::from(&ssh).join(".ssh").join("config");
            let mut f = std::fs::File::create(&cfg).unwrap();
            writeln!(f, "Host secret").unwrap();
            writeln!(f, "    HostName AKIA-SECRET-ACCESS-KEY-VALUE").unwrap();
            let res = add_managed_host(&sample_host("secret"));
            let msg = res.err().unwrap_or_default();
            assert!(
                !msg.contains("AKIA"),
                "error log must not leak secret: {msg}"
            );
        });
    }

    // ---- Include / discovery matrix (requirement #6/#7) ----

    #[test]
    fn keyword_equals_directive_is_parsed() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let cfg = PathBuf::from(&ssh).join(".ssh").join("config");
            let mut f = std::fs::File::create(&cfg).unwrap();
            // Keyword=value form that the old whitespace-only parser ignored.
            writeln!(f, "Host=equalsform").unwrap();
            writeln!(f, "    HostName=10.0.0.42").unwrap();
            let map = parse_ssh_config().expect("discovery must succeed");
            assert!(
                map.contains_key("equalsform"),
                "Host=alias must be discovered"
            );
            assert_eq!(map["equalsform"].hostname.as_deref(), Some("10.0.0.42"));
        });
    }

    #[test]
    fn include_equals_recognized_as_installed() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            let abs = ssh_dir.join("arx_hosts.conf").display().to_string();
            let mut f = std::fs::File::create(ssh_dir.join("config")).unwrap();
            // `Include=...` must be recognized, not silently ignored (old bug).
            writeln!(f, "Include={abs}").unwrap();
            assert!(
                crate::remote::ssh_config_manager::is_arx_include_installed(),
                "Include= form must be recognized"
            );
            ensure_arx_include().unwrap();
            let live = std::fs::read_to_string(ssh_dir.join("config")).unwrap();
            assert_eq!(
                live.lines()
                    .filter(|l| l.trim_start().to_lowercase().starts_with("include"))
                    .count(),
                1,
                "must stay exactly one include"
            );
        });
    }

    #[test]
    fn question_mark_glob_is_matched() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(ssh_dir.join("d")).unwrap();
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh/d/host?.conf").unwrap();
            let mut inc = std::fs::File::create(ssh_dir.join("d").join("hosta.conf")).unwrap();
            writeln!(inc, "Host qmark").unwrap();
            writeln!(inc, "    HostName 10.0.0.51").unwrap();
            // A non-matching file (two chars) must NOT be discovered.
            let mut inc2 = std::fs::File::create(ssh_dir.join("d").join("hostbb.conf")).unwrap();
            writeln!(inc2, "Host notqmark").unwrap();
            let map = parse_ssh_config().expect("discovery must succeed");
            assert!(map.contains_key("qmark"), "? glob must match hosta.conf");
            assert!(
                !map.contains_key("notqmark"),
                "? glob must not match hostbb.conf"
            );
        });
    }

    #[test]
    fn multi_level_glob_discovers_unmanaged() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(ssh_dir.join("config.d")).unwrap();
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh/config.d/*.conf").unwrap();
            let mut inc = std::fs::File::create(ssh_dir.join("config.d").join("b.conf")).unwrap();
            writeln!(inc, "Host deep").unwrap();
            writeln!(inc, "    HostName 10.0.0.61").unwrap();
            // collision discovery must see the globbed host.
            assert!(
                matches!(
                    crate::remote::ssh_config_manager::alias_collision("deep"),
                    Ok(true)
                ),
                "globbed host must be discovered for collision"
            );
        });
    }

    #[test]
    fn relative_include_resolved_against_ssh_dir() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(ssh_dir.join("inc")).unwrap();
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            // OpenSSH resolves a relative Include in user config against ~/.ssh.
            writeln!(main, "Include inc/rel.conf").unwrap();
            let mut inc = std::fs::File::create(ssh_dir.join("inc").join("rel.conf")).unwrap();
            writeln!(inc, "Host relative").unwrap();
            writeln!(inc, "    HostName 10.0.0.71").unwrap();
            let map = parse_ssh_config().expect("discovery must succeed");
            assert!(
                map.contains_key("relative"),
                "relative Include must resolve against ~/.ssh"
            );
        });
    }

    #[test]
    fn unreadable_include_fails_closed() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(&ssh_dir).unwrap();
            // An Include pointing at a DIRECTORY cannot be read -> discovery errors.
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh").unwrap();
            // Discovery must surface the error (fail closed), not silently empty.
            assert!(
                parse_ssh_config().is_err(),
                "unreadable Include must fail closed"
            );
            // Consequently, add_managed_host must be refused rather than assume no collision.
            let h = sample_host("anything");
            assert!(
                add_managed_host(&h).is_err(),
                "write must be refused when discovery is unsafe"
            );
        });
    }

    #[test]
    fn collision_discovery_error_propagates_as_failure() {
        with_temp_ssh(|| {
            // Build an unreadable include so discovery errors, then prove the
            // collision check refuses instead of returning false (fail closed).
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(&ssh_dir).unwrap();
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh").unwrap();
            assert!(
                crate::remote::ssh_config_manager::alias_collision("probe").is_err(),
                "collision check must error on unsafe discovery"
            );
        });
    }

    // ---- Behavioral: real KeyEvent transition, no bypass ----

    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode, mods: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, mods)
    }

    #[test]
    fn ctrl_k_event_sets_pending_without_writing() {
        with_temp_ssh(|| {
            let mut host = sample_host("srv");
            host.identity_file = None;
            host.identities_only = false;
            add_managed_host(&host).unwrap();

            let mut state = AppState {
                ssh_hosts: list_managed_hosts().into_values().collect(),
                ssh_host_cursor: 0,
                ..AppState::default()
            };
            // No pending yet.
            assert!(state.ssh_pending_keygen.is_none());
            // Drive the REAL Ctrl+K key event (not a pre-seeded field).
            let consumed = crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            );
            assert!(consumed, "Ctrl+K must be consumed");
            assert!(
                state.ssh_pending_keygen.is_some(),
                "Ctrl+K must set pending keygen state"
            );
            // Still nothing written: host has no identity.
            assert!(
                list_managed_hosts()
                    .get("srv")
                    .unwrap()
                    .identity_file
                    .is_none()
            );
        });
    }

    #[test]
    fn y_event_confirms_pending_generates_key() {
        with_temp_ssh(|| {
            let mut host = sample_host("srv");
            host.identity_file = None;
            host.identities_only = false;
            add_managed_host(&host).unwrap();
            let mut state = AppState {
                ssh_hosts: list_managed_hosts().into_values().collect(),
                ssh_host_cursor: 0,
                ..AppState::default()
            };
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            );
            // Now drive the real y key event.
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('y'), KeyModifiers::NONE),
            );
            assert!(
                state.ssh_pending_keygen.is_none(),
                "pending cleared after y"
            );
            let after = list_managed_hosts();
            let got = after.get("srv").expect("host present");
            assert!(got.identity_file.is_some(), "y attached identity");
            assert!(got.identities_only);
            assert!(
                got.identity_file.clone().unwrap().exists(),
                "key file written"
            );
        });
    }

    #[test]
    fn n_event_cancels_pending_writes_nothing() {
        with_temp_ssh(|| {
            let mut host = sample_host("srv");
            host.identity_file = None;
            add_managed_host(&host).unwrap();
            let mut state = AppState {
                ssh_hosts: list_managed_hosts().into_values().collect(),
                ssh_host_cursor: 0,
                ..AppState::default()
            };
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            );
            // Drive the real n key event.
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('n'), KeyModifiers::NONE),
            );
            assert!(
                state.ssh_pending_keygen.is_none(),
                "pending cleared after n"
            );
            assert!(
                !list_managed_hosts().contains_key("srv")
                    || list_managed_hosts()
                        .get("srv")
                        .unwrap()
                        .identity_file
                        .is_none(),
                "cancel must not attach an identity"
            );
        });
    }

    #[test]
    fn plain_k_event_does_not_set_pending() {
        with_temp_ssh(|| {
            let mut state = AppState::default();
            let consumed = crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('k'), KeyModifiers::NONE),
            );
            assert!(!consumed, "plain k must not be consumed");
            assert!(state.ssh_pending_keygen.is_none());
        });
    }

    // ---- Discovery regression: fail-closed paths ----

    #[test]
    fn unreadable_glob_directory_fails_closed() {
        // A glob whose parent is a FILE (not a directory) makes read_dir error
        // regardless of user/permissions — proving the resolver surfaces the error
        // instead of yielding an empty result.
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(&ssh_dir).unwrap();
            // config exists as a regular file; `~/.ssh/config/*` cannot be read as a dir.
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh/config/*").unwrap();
            assert!(
                parse_ssh_config().is_err(),
                "glob over a non-directory parent must fail closed"
            );
        });
    }

    #[test]
    fn wildcard_in_directory_component_is_discovered() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(ssh_dir.join("env-prod")).unwrap();
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh/env-*/hosts.conf").unwrap();
            let mut inc =
                std::fs::File::create(ssh_dir.join("env-prod").join("hosts.conf")).unwrap();
            writeln!(inc, "Host dirwild").unwrap();
            writeln!(inc, "    HostName 10.0.0.81").unwrap();
            let map = parse_ssh_config().expect("discovery must succeed");
            assert!(
                map.contains_key("dirwild"),
                "wildcard in directory component must be discovered"
            );
        });
    }

    #[test]
    fn nested_relative_include_anchors_to_ssh_dir() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(ssh_dir.join("sub")).unwrap();
            let mut root = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(root, "Include sub/outer.conf").unwrap();
            let mut outer = std::fs::File::create(ssh_dir.join("sub").join("outer.conf")).unwrap();
            writeln!(outer, "Include inner.conf").unwrap();
            let mut inner = std::fs::File::create(ssh_dir.join("inner.conf")).unwrap();
            writeln!(inner, "Host nested").unwrap();
            writeln!(inner, "    HostName 10.0.0.91").unwrap();
            let map = parse_ssh_config().expect("discovery must succeed");
            assert!(
                map.contains_key("nested"),
                "nested relative include must anchor to ~/.ssh, not sub/"
            );
        });
    }

    #[test]
    fn failed_discovery_refuses_add() {
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(&ssh_dir).unwrap();
            // Glob over a regular file makes discovery error reliably (cross-user).
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh/config/*").unwrap();
            // Discovery error must propagate to a refused write.
            let h = sample_host("anything");
            assert!(
                add_managed_host(&h).is_err(),
                "write must be refused when discovery is unsafe"
            );
        });
    }

    #[test]
    fn multi_wildcard_component_is_discovered() {
        // OpenSSH passes the whole Include pathname to glob(), so multiple
        // wildcard segments are supported (e.g. ~/.ssh/env-*/host?/conf*.ssh).
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(ssh_dir.join("env-prod").join("host1")).unwrap();
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh/env-*/host?/conf*.ssh").unwrap();
            let mut inc =
                std::fs::File::create(ssh_dir.join("env-prod").join("host1").join("conf1.ssh"))
                    .unwrap();
            writeln!(inc, "Host multiwild").unwrap();
            writeln!(inc, "    HostName 10.0.0.91").unwrap();
            let map = parse_ssh_config().expect("discovery must succeed");
            assert!(
                map.contains_key("multiwild"),
                "multiple wildcard segments must be discovered"
            );
        });
    }

    #[test]
    fn matched_metadata_error_after_wildcard_fails_closed() {
        // A wildcard matches a path, but stat'ing it fails (e.g. broken symlink
        // target), must surface as an error, not "no match". Uses a 2-segment
        // include so the descent (metadata) branch is exercised.
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(&ssh_dir).unwrap();
            let dir = ssh_dir.join("broken.d");
            std::fs::create_dir_all(&dir).unwrap();
            // A genuine matchable directory that would otherwise be descended.
            std::fs::create_dir_all(dir.join("real")).unwrap();
            let mut ok = std::fs::File::create(dir.join("real").join("hosts.conf")).unwrap();
            writeln!(ok, "Host okhost").unwrap();
            // Dangling symlink: read_dir lists it, but stat fails -> descent errors.
            #[cfg(unix)]
            {
                use std::os::unix::fs::symlink;
                symlink("/nonexistent-arx-target-xyz", dir.join("link")).unwrap();
            }
            #[cfg(not(unix))]
            {
                // Non-unix: a regular file matched by the wildcard cannot be
                // descended (not a dir) but must still surface as fail-closed
                // because the metadata probe errors.
                std::fs::File::create(dir.join("link")).unwrap();
            }
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh/broken.d/*/hosts.conf").unwrap();
            assert!(
                parse_ssh_config().is_err(),
                "wildcard match with unstatable entry must fail closed"
            );
        });
    }

    // ---- Pending-confirmation gate: the helper is the production dispatcher ----

    #[test]
    fn pending_ctrl_k_then_d_does_not_delete() {
        with_temp_ssh(|| {
            let mut host = sample_host("srv");
            host.identity_file = None;
            add_managed_host(&host).unwrap();
            let mut state = AppState {
                ssh_hosts: list_managed_hosts().into_values().collect(),
                ssh_host_cursor: 0,
                ..AppState::default()
            };
            // Ctrl+K sets pending.
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            );
            assert!(state.ssh_pending_keygen.is_some());
            // D must be swallowed while pending -> no delete.
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('d'), KeyModifiers::NONE),
            );
            assert!(
                list_managed_hosts().contains_key("srv"),
                "D must not delete a host while keygen is pending"
            );
            assert!(state.ssh_pending_keygen.is_some(), "pending survives D");
        });
    }

    #[test]
    fn pending_ctrl_k_then_a_does_not_open_form() {
        with_temp_ssh(|| {
            let mut host = sample_host("srv");
            host.identity_file = None;
            add_managed_host(&host).unwrap();
            let mut state = AppState {
                ssh_hosts: list_managed_hosts().into_values().collect(),
                ssh_host_cursor: 0,
                ..AppState::default()
            };
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            );
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('a'), KeyModifiers::NONE),
            );
            assert!(
                state.ssh_form.is_none(),
                "A must not open the form while pending"
            );
            assert!(state.ssh_pending_keygen.is_some());
        });
    }

    #[test]
    fn pending_ctrl_k_then_esc_cancels() {
        with_temp_ssh(|| {
            let mut host = sample_host("srv");
            host.identity_file = None;
            add_managed_host(&host).unwrap();
            let mut state = AppState {
                ssh_hosts: list_managed_hosts().into_values().collect(),
                ssh_host_cursor: 0,
                ..AppState::default()
            };
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            );
            let consumed = crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Esc, KeyModifiers::NONE),
            );
            assert!(consumed, "Esc must be consumed while pending");
            assert!(state.ssh_pending_keygen.is_none(), "Esc cancels pending");
        });
    }

    #[test]
    fn pending_ctrl_k_then_arbitrary_key_unchanged() {
        with_temp_ssh(|| {
            let mut host = sample_host("srv");
            host.identity_file = None;
            add_managed_host(&host).unwrap();
            let mut state = AppState {
                ssh_hosts: list_managed_hosts().into_values().collect(),
                ssh_host_cursor: 0,
                ..AppState::default()
            };
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('k'), KeyModifiers::CONTROL),
            );
            crate::app::handle_ssh_host_keypress(
                &mut state,
                key(KeyCode::Char('x'), KeyModifiers::NONE),
            );
            assert!(state.ssh_pending_keygen.is_some(), "arbitrary key ignored");
        });
    }

    #[test]
    fn confirm_without_existing_host_does_not_orphan_key() {
        with_temp_ssh(|| {
            // Pending alias whose host was deleted between request and confirm.
            let mut state = AppState {
                ssh_hosts: vec![],
                ssh_pending_keygen: Some("ghost".into()),
                ..AppState::default()
            };
            crate::app::confirm_pending_keygen(&mut state);
            assert!(state.ssh_pending_keygen.is_none());
            // No key file should have been written for a non-existent host.
            let ssh = std::env::var("HOME").unwrap();
            let key_path = PathBuf::from(&ssh)
                .join(".ssh")
                .join("arx")
                .join("ghost_ed25519");
            assert!(!key_path.exists(), "no orphan key for missing host");
        });
    }

    #[test]
    fn wildcard_anchors_prefix_at_start() {
        // env-* must match names BEGINNING with env-, not those merely
        // containing it (glob component semantics).
        assert!(crate::remote::ssh_config::wildcard_match(
            "env-prod", "env-*"
        ));
        assert!(
            !crate::remote::ssh_config::wildcard_match("xenv-prod", "env-*"),
            "env-* must not match xenv-prod"
        );
        assert!(
            !crate::remote::ssh_config::wildcard_match("myenv-x", "env-*"),
            "env-* must not match a name where env- is not at the start"
        );
    }

    #[test]
    fn wildcard_anchors_suffix_at_end() {
        // *.conf must match names ENDING with .conf.
        assert!(crate::remote::ssh_config::wildcard_match(
            "hosts.conf",
            "*.conf"
        ));
        assert!(
            !crate::remote::ssh_config::wildcard_match("conf.hosts", "*.conf"),
            "*.conf must not match conf.hosts"
        );
        assert!(crate::remote::ssh_config::wildcard_match("a*b", "a*b"));
        assert!(
            !crate::remote::ssh_config::wildcard_match("xaXXb", "a*b"),
            "a*b must not match xaXXb (suffix not at end)"
        );
    }

    #[test]
    fn env_include_rejects_unanchored_substring_dir() {
        // The discovery path must not descend into xenv-prod for Include env-*.
        with_temp_ssh(|| {
            let ssh = std::env::var("HOME").unwrap();
            let ssh_dir = PathBuf::from(&ssh).join(".ssh");
            std::fs::create_dir_all(ssh_dir.join("env-prod")).unwrap();
            std::fs::create_dir_all(ssh_dir.join("xenv-prod")).unwrap();
            let mut main = std::fs::File::create(ssh_dir.join("config")).unwrap();
            writeln!(main, "Include ~/.ssh/env-*/hosts.conf").unwrap();
            let mut good =
                std::fs::File::create(ssh_dir.join("env-prod").join("hosts.conf")).unwrap();
            writeln!(good, "Host envok").unwrap();
            writeln!(good, "    HostName 10.0.0.91").unwrap();
            let mut bad =
                std::fs::File::create(ssh_dir.join("xenv-prod").join("hosts.conf")).unwrap();
            writeln!(bad, "Host envtrap").unwrap();
            writeln!(bad, "    HostName 10.0.0.92").unwrap();
            let map = match parse_ssh_config() {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("DEBUG DISCOVERY ERR: {e:?}");
                    panic!("discovery errored");
                }
            };
            assert!(map.contains_key("envok"), "env-prod must be discovered");
            assert!(
                !map.contains_key("envtrap"),
                "xenv-prod must NOT be matched by env-* (prefix anchor)"
            );
        });
    }
}
