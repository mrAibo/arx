use clap::Parser;

mod tui;
mod workspace;

/// Terminal commander for local and remote files, archives, transfers, and jobs.
#[derive(Parser)]
#[command(name = "arx", version, about)]
struct Cli {
    /// Path to config file
    #[arg(short, long)]
    config: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() {
    let _cli = Cli::parse();
    // ponytail: auto-set DISPLAY for Windows SSH clients (PuTTY/RoyalTS/Xshell)
    if std::env::var("DISPLAY").is_err() && std::env::var("SSH_CLIENT").is_ok() {
        // SAFETY: single-threaded at startup, no concurrent env access
        unsafe {
            std::env::set_var("DISPLAY", "localhost:0.0");
        }
    }
    let config = arx::config::load();
    tui::run(config).await.expect("TUI exited with error");
}
