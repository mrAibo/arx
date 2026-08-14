use std::collections::HashMap;
use std::io;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::ExecutorAvailability;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct LocalToolAvailability {
    pub ssh: bool,
    pub rsync: bool,
    pub sftp: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RemoteToolAvailability {
    pub reachable: bool,
    pub rsync: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ProbeError {
    #[error("OpenSSH client is not available")]
    SshUnavailable,
    #[error("SSH capability probe failed for {alias}")]
    RemoteUnavailable { alias: String },
    #[error(transparent)]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone, Copy)]
struct CachedRemoteTools {
    value: RemoteToolAvailability,
    observed_at: Instant,
}

#[derive(Debug)]
pub struct RemoteCapabilityCache {
    ttl: Duration,
    entries: HashMap<String, CachedRemoteTools>,
}

impl RemoteCapabilityCache {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            entries: HashMap::new(),
        }
    }

    pub fn get(&mut self, alias: &str) -> Option<RemoteToolAvailability> {
        let cached = self.entries.get(alias).copied()?;
        if cached.observed_at.elapsed() < self.ttl {
            Some(cached.value)
        } else {
            self.entries.remove(alias);
            None
        }
    }

    pub fn insert(&mut self, alias: impl Into<String>, value: RemoteToolAvailability) {
        self.entries.insert(
            alias.into(),
            CachedRemoteTools {
                value,
                observed_at: Instant::now(),
            },
        );
    }

    pub fn invalidate(&mut self, alias: &str) {
        self.entries.remove(alias);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn get_or_probe(&mut self, alias: &str) -> Result<RemoteToolAvailability, ProbeError> {
        if let Some(cached) = self.get(alias) {
            return Ok(cached);
        }

        let detected = detect_remote_tools(alias)?;
        self.insert(alias, detected);
        Ok(detected)
    }
}

impl Default for RemoteCapabilityCache {
    fn default() -> Self {
        Self::new(Duration::from_secs(60))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CommandOutcome {
    success: bool,
    code: Option<i32>,
}

trait CommandRunner {
    fn run(&self, program: &str, args: &[String]) -> io::Result<CommandOutcome>;
}

#[derive(Debug, Default)]
struct SystemCommandRunner;

impl CommandRunner for SystemCommandRunner {
    fn run(&self, program: &str, args: &[String]) -> io::Result<CommandOutcome> {
        let status = Command::new(program)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()?;
        Ok(CommandOutcome {
            success: status.success(),
            code: status.code(),
        })
    }
}

pub fn detect_local_tools() -> LocalToolAvailability {
    detect_local_tools_with(&SystemCommandRunner)
}

pub fn detect_remote_tools(alias: &str) -> Result<RemoteToolAvailability, ProbeError> {
    detect_remote_tools_with(&SystemCommandRunner, alias)
}

pub fn local_executors(local: LocalToolAvailability) -> ExecutorAvailability {
    ExecutorAvailability {
        native: true,
        rsync: local.rsync,
        sftp: false,
        s3: false,
    }
}

pub fn local_remote_executors(
    local: LocalToolAvailability,
    remote: RemoteToolAvailability,
    sftp_executor_available: bool,
) -> ExecutorAvailability {
    ExecutorAvailability {
        native: false,
        rsync: local.ssh && local.rsync && remote.reachable && remote.rsync,
        sftp: local.ssh && local.sftp && remote.reachable && sftp_executor_available,
        s3: false,
    }
}

fn detect_local_tools_with(runner: &impl CommandRunner) -> LocalToolAvailability {
    LocalToolAvailability {
        ssh: command_succeeds(runner, "ssh", &["-V"]),
        rsync: command_succeeds(runner, "rsync", &["--version"]),
        // OpenSSH sftp has no portable --version flag. For availability we only
        // care whether the executable can be spawned; `-h` may exit non-zero.
        sftp: command_available(runner, "sftp", &["-h"]),
    }
}

fn detect_remote_tools_with(
    runner: &impl CommandRunner,
    alias: &str,
) -> Result<RemoteToolAvailability, ProbeError> {
    if !command_succeeds(runner, "ssh", &["-V"]) {
        return Err(ProbeError::SshUnavailable);
    }

    let args = [
        "-T",
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=yes",
        "-o",
        "ConnectTimeout=5",
        alias,
        "rsync",
        "--version",
    ]
    .into_iter()
    .map(str::to_string)
    .collect::<Vec<_>>();

    let outcome = runner.run("ssh", &args)?;
    if outcome.success {
        return Ok(RemoteToolAvailability {
            reachable: true,
            rsync: true,
        });
    }

    if outcome.code == Some(255) || outcome.code.is_none() {
        return Err(ProbeError::RemoteUnavailable {
            alias: alias.to_string(),
        });
    }

    Ok(RemoteToolAvailability {
        reachable: true,
        rsync: false,
    })
}

fn command_succeeds(runner: &impl CommandRunner, program: &str, args: &[&str]) -> bool {
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    runner
        .run(program, &args)
        .map(|outcome| outcome.success)
        .unwrap_or(false)
}

fn command_available(runner: &impl CommandRunner, program: &str, args: &[&str]) -> bool {
    let args = args
        .iter()
        .map(|arg| (*arg).to_string())
        .collect::<Vec<_>>();
    runner.run(program, &args).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::VecDeque;

    #[derive(Debug)]
    struct FakeRunner {
        outcomes: RefCell<VecDeque<io::Result<CommandOutcome>>>,
        calls: RefCell<Vec<(String, Vec<String>)>>,
    }

    impl FakeRunner {
        fn new(outcomes: impl IntoIterator<Item = CommandOutcome>) -> Self {
            Self::new_results(outcomes.into_iter().map(Ok))
        }

        fn new_results(outcomes: impl IntoIterator<Item = io::Result<CommandOutcome>>) -> Self {
            Self {
                outcomes: RefCell::new(outcomes.into_iter().collect()),
                calls: RefCell::new(Vec::new()),
            }
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[String]) -> io::Result<CommandOutcome> {
            self.calls
                .borrow_mut()
                .push((program.to_string(), args.to_vec()));
            self.outcomes
                .borrow_mut()
                .pop_front()
                .unwrap_or_else(|| Err(io::Error::other("missing fake outcome")))
        }
    }

    const OK: CommandOutcome = CommandOutcome {
        success: true,
        code: Some(0),
    };
    const MISSING: CommandOutcome = CommandOutcome {
        success: false,
        code: Some(127),
    };
    const SSH_FAILED: CommandOutcome = CommandOutcome {
        success: false,
        code: Some(255),
    };

    #[test]
    fn detects_local_transfer_tools() {
        let runner = FakeRunner::new([OK, OK, MISSING]);
        let tools = detect_local_tools_with(&runner);
        assert_eq!(
            tools,
            LocalToolAvailability {
                ssh: true,
                rsync: true,
                // Exit status is irrelevant for `sftp -h`; successful spawn is enough.
                sftp: true,
            }
        );
    }

    #[test]
    fn missing_sftp_executable_is_not_advertised() {
        let runner = FakeRunner::new_results([
            Ok(OK),
            Ok(OK),
            Err(io::Error::new(io::ErrorKind::NotFound, "sftp missing")),
        ]);
        let tools = detect_local_tools_with(&runner);
        assert!(!tools.sftp);
    }

    #[test]
    fn remote_probe_reuses_openssh_and_requires_known_host() {
        let runner = FakeRunner::new([OK, OK]);
        let remote = detect_remote_tools_with(&runner, "prod-db").unwrap();
        assert_eq!(
            remote,
            RemoteToolAvailability {
                reachable: true,
                rsync: true
            }
        );
        let calls = runner.calls.borrow();
        let (_, args) = &calls[1];
        assert!(args.iter().any(|arg| arg == "BatchMode=yes"));
        assert!(args.iter().any(|arg| arg == "StrictHostKeyChecking=yes"));
        assert!(args.iter().any(|arg| arg == "prod-db"));
    }

    #[test]
    fn missing_remote_rsync_is_not_connection_failure() {
        let runner = FakeRunner::new([OK, MISSING]);
        let remote = detect_remote_tools_with(&runner, "prod-db").unwrap();
        assert!(remote.reachable);
        assert!(!remote.rsync);
    }

    #[test]
    fn ssh_exit_255_is_remote_unavailable() {
        let runner = FakeRunner::new([OK, SSH_FAILED]);
        let error = detect_remote_tools_with(&runner, "prod-db").unwrap_err();
        assert!(matches!(
            error,
            ProbeError::RemoteUnavailable { ref alias } if alias == "prod-db"
        ));
    }

    #[test]
    fn executor_availability_requires_tools_on_both_ends() {
        let local = LocalToolAvailability {
            ssh: true,
            rsync: true,
            sftp: true,
        };
        let remote = RemoteToolAvailability {
            reachable: true,
            rsync: false,
        };
        let executors = local_remote_executors(local, remote, true);
        assert!(!executors.rsync);
        assert!(executors.sftp);
        assert!(!executors.native);
    }

    #[test]
    fn sftp_executor_is_not_advertised_without_local_client() {
        let local = LocalToolAvailability {
            ssh: true,
            rsync: false,
            sftp: false,
        };
        let remote = RemoteToolAvailability {
            reachable: true,
            rsync: false,
        };
        assert!(!local_remote_executors(local, remote, true).sftp);
    }

    #[test]
    fn remote_cache_returns_fresh_entry_and_can_invalidate() {
        let mut cache = RemoteCapabilityCache::new(Duration::from_secs(60));
        let value = RemoteToolAvailability {
            reachable: true,
            rsync: true,
        };
        cache.insert("prod-db", value);
        assert_eq!(cache.get("prod-db"), Some(value));
        cache.invalidate("prod-db");
        assert_eq!(cache.get("prod-db"), None);
    }

    #[test]
    fn remote_cache_expires_entries() {
        let mut cache = RemoteCapabilityCache::new(Duration::ZERO);
        cache.insert(
            "prod-db",
            RemoteToolAvailability {
                reachable: true,
                rsync: true,
            },
        );
        assert_eq!(cache.get("prod-db"), None);
    }
}
