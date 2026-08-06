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

fn main() {
    let _cli = Cli::parse();
    // ponytail: config path from CLI not yet wired — default path only
    let _config = arx::config::load();

    tui::run().expect("TUI exited with error");
}
