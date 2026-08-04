use tracing::info;

fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    info!("starting ARX");
    println!("ARX foundation ready. TUI shell lands in the next implementation step.");
}
