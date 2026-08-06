use clap::Parser;

mod tui;

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
    let config = arx::config::load();
    tui::run(config).await.expect("TUI exited with error");
}
