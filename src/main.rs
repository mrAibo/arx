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

    /// Print the effective keymap (managed contexts) and exit
    #[arg(long)]
    print_keymap: bool,
}

/// Build the ONE effective keymap: library defaults + user overrides, then the
/// real browser legacy collision validator. No overrides == default behavior.
fn build_effective_keymap(config: &arx::config::ArxConfig) -> Result<arx::input::Keymap, String> {
    let keymap = arx::input::Keymap::effective(&config.keybindings).map_err(|e| e.to_string())?;
    tui::validate_user_browser_bindings(&keymap)?;
    Ok(keymap)
}

/// Deterministic --print-keymap rows for the KeyRouter-managed contexts only.
fn keymap_rows(keymap: &arx::input::Keymap) -> Vec<String> {
    use arx::app::InputContext;
    let contexts = [
        ("browser", InputContext::Browser),
        ("sync_preview", InputContext::SyncPreview),
        ("sync_confirmation", InputContext::SyncConfirmation),
        ("sync_job", InputContext::SyncJob),
    ];
    let mut rows = Vec::new();
    for (name, context) in contexts {
        for binding in keymap.bindings() {
            if binding.context != context {
                continue;
            }
            let sequence = binding
                .sequence
                .iter()
                .map(|stroke| stroke.label())
                .collect::<Vec<_>>()
                .join(" ");
            let source = match binding.source {
                arx::input::BindingSource::BuiltIn => "built-in",
                arx::input::BindingSource::User => "user",
            };
            let visibility = if binding.discoverable {
                "discoverable"
            } else {
                "alias"
            };
            rows.push(format!(
                "{:<18} {:<14} {:<28} {:<9} {}",
                name,
                sequence,
                binding.action.id().config_name(),
                source,
                visibility,
            ));
        }
    }
    rows
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    // Explicit --config is authoritative: never silently fall back to defaults.
    let config = match &cli.config {
        Some(path) => match arx::config::load_from_path(path) {
            Ok(config) => config,
            Err(error) => {
                eprintln!("arx: {error}");
                std::process::exit(2);
            }
        },
        None => arx::config::load(),
    };

    let keymap = match build_effective_keymap(&config) {
        Ok(keymap) => keymap,
        Err(error) => {
            eprintln!("arx: invalid keybindings: {error}");
            std::process::exit(2);
        }
    };

    if cli.print_keymap {
        println!(
            "{:<18} {:<14} {:<28} {:<9} visibility",
            "context", "keys", "action", "source"
        );
        for row in keymap_rows(&keymap) {
            println!("{row}");
        }
        return;
    }

    // X11 forwarding is session-owned. If the SSH client/server established
    // forwarding, DISPLAY is already present and is inherited unchanged by ARX and
    // child processes. If it is absent, ARX must not invent a display endpoint.
    tui::run(config, keymap)
        .await
        .expect("TUI exited with error");
}
