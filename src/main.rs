use clap::Parser;

mod tui;
mod tui_terminal;
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
    // X11 forwarding is session-owned. If the SSH client/server established
    // forwarding, DISPLAY is already present and is inherited unchanged by ARX and
    // child processes. If it is absent, ARX must not invent a display endpoint.
    let config = arx::config::load();
    tui::run(config).await.expect("TUI exited with error");
}
