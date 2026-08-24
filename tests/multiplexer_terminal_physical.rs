//! #7 Linux physical acceptance: real PTY + real tmux/GNU screen + the
//! production TuiTerminalSession/ProcessService attach lifecycle.
#![cfg(target_os = "linux")]
#![allow(dead_code)] // included src/tui_terminal.rs carries its own test fns

#[path = "../src/tui_terminal.rs"]
mod tui_terminal;

use arx::effects::EffectEvent;
use arx::process::ProcessService;
use std::io::Write;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tui_terminal::TuiTerminalSession;

const GUARD: &str = "ARX_MULTIPLEXER_PHYSICAL";
const CHILD: &str = "ARX_MULTIPLEXER_PHYSICAL_CHILD";
const SESSION: &str = "ARX_MULTIPLEXER_SESSION";
const REACQUIRED: &str = "ARX_TERMINAL_REACQUIRED=raw,alternate,mouse";

fn physical_enabled() -> bool {
    std::env::var(GUARD).as_deref() == Ok("1")
}

fn require_binary(name: &str) {
    let ok = Command::new("sh")
        .args(["-c", &format!("command -v {name} >/dev/null")])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    assert!(ok, "{GUARD}=1 but required binary '{name}' is unavailable");
}

fn unique(prefix: &str) -> String {
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    format!("{prefix}_arx7_{n}")
}

fn quiet(program: &str, args: &[&str]) -> bool {
    Command::new(program)
        .args(args)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

struct Fixture {
    kind: &'static str,
    name: String,
}
impl Fixture {
    fn cleanup(&self) {
        match self.kind {
            "tmux" => {
                quiet("tmux", &["kill-session", "-t", &self.name]);
            }
            "screen" => {
                // Fixture cleanup only — never used for attach/steal.
                quiet("screen", &["-X", "-S", &self.name, "quit"]);
            }
            _ => unreachable!(),
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.cleanup();
    }
}

/// Child path, launched under script(1), therefore attached to a REAL PTY.
/// This is the actual production lifecycle seam: existing terminal authority
/// + existing ProcessService argv authority.
#[tokio::test]
async fn physical_attach_child() {
    if !physical_enabled() {
        return;
    }
    let Ok(kind) = std::env::var(CHILD) else {
        return; // parent invocation: child test is selected explicitly below
    };
    let session = std::env::var(SESSION).expect("child session id");
    assert!(matches!(kind.as_str(), "tmux" | "screen"));

    let mut terminal = TuiTerminalSession::enter().expect("enter terminal lifecycle");
    let event = terminal
        .suspend_while(|| async { ProcessService::attach_multiplexer(&kind, &session).await })
        .await
        .expect("terminal resume failed");

    assert!(
        matches!(event, EffectEvent::ProcessExited { success: true, .. }),
        "attach event was not successful: {event:?}"
    );
    assert_eq!(
        terminal.lifecycle_state(),
        (true, true, true),
        "raw/alternate/mouse were not reacquired"
    );
    println!("{REACQUIRED}");
}

fn run_child_in_real_pty(kind: &str, session: &str, detach_keys: &[u8]) -> String {
    require_binary("script");
    require_binary("timeout");
    let exe = std::env::current_exe().expect("current physical test executable");
    let command = format!(
        "env TERM=xterm-256color {GUARD}=1 {CHILD}={} {SESSION}={} '{}' --exact physical_attach_child --nocapture",
        kind,
        session,
        exe.display()
    );
    let mut child = Command::new("timeout")
        .args(["45s", "script", "-qfec", &command, "/dev/null"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn real PTY controller");

    // Allow the real multiplexer to own the PTY, then use its normal detach key.
    std::thread::sleep(Duration::from_millis(1500));
    let stdin = child.stdin.as_mut().expect("PTY stdin");
    if let Err(error) = stdin.write_all(detach_keys).and_then(|_| stdin.flush()) {
        // Preserve child output for the real root cause instead of masking it
        // with EPIPE when the child exited before the detach write.
        eprintln!("detach write ended early: {error}");
    }
    drop(child.stdin.take());

    let deadline = Instant::now() + Duration::from_secs(50);
    while child.try_wait().expect("poll child").is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            panic!("{kind} physical attach/detach timed out");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let output = child.wait_with_output().expect("collect PTY output");
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.status.success(), "PTY child failed: {text}");
    assert!(
        text.contains(REACQUIRED),
        "terminal reacquire evidence missing: {text}"
    );
    text
}

#[test]
fn tmux_real_attach_detach_and_terminal_reacquire() {
    if !physical_enabled() {
        return;
    }
    require_binary("tmux");
    let name = unique("tmux");
    assert!(quiet("tmux", &["new-session", "-d", "-s", &name]));
    let fixture = Fixture {
        kind: "tmux",
        name: name.clone(),
    };

    // tmux's DEFAULT detach sequence only; user tmux config is authoritative.
    let evidence = run_child_in_real_pty("tmux", &name, &[0x02, b'd']);
    assert!(evidence.contains(REACQUIRED));
    assert!(quiet("tmux", &["has-session", "-t", &name]));

    fixture.cleanup();
    assert!(!quiet("tmux", &["has-session", "-t", &name]));
}

#[test]
fn screen_real_attach_detach_and_terminal_reacquire() {
    if !physical_enabled() {
        return;
    }
    require_binary("screen");
    let name = unique("screen");
    assert!(quiet("screen", &["-dmS", &name, "sleep", "120"]));
    let fixture = Fixture {
        kind: "screen",
        name: name.clone(),
    };

    // GNU Screen owns its detach binding/configuration (default C-a d).
    let evidence = run_child_in_real_pty("screen", &name, &[0x01, b'd']);
    assert!(evidence.contains(REACQUIRED));

    fixture.cleanup();
    let output = Command::new("screen")
        .arg("-ls")
        .output()
        .expect("screen -ls");
    let listing = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!listing.contains(&name), "screen fixture leaked: {listing}");
}
