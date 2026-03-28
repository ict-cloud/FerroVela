use clap::Parser;
use log::error;
use std::sync::Arc;

use ferrovela::{config, launchd, logger, pac::PacEngine, proxy::Proxy};

#[derive(Parser, Debug)]
#[command(author, version, about = "FerroVela proxy service", long_about = None)]
struct Args {
    #[arg(short, long)]
    config: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let log_path = launchd::log_path();
    if let Some(parent) = log_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Err(e) = logger::init_to(&log_path) {
        eprintln!(
            "Warning: Failed to initialize file logger: {}. Continuing with stderr only.",
            e
        );
    }
    let args = Args::parse();

    let config_path = args
        .config
        .map(std::path::PathBuf::from)
        .unwrap_or_else(config::default_config_path);

    if let Err(e) = config::ensure_user_config(&config_path) {
        eprintln!("Warning: Could not initialise config file: {}.", e);
    }

    let config_str = config_path.to_string_lossy().into_owned();
    let cfg = Arc::new(config::load_config(&config_str).unwrap_or_default());

    let pac_engine = if let Some(ref path) = cfg.proxy.pac_file {
        match PacEngine::new(path).await {
            Ok(engine) => Some(engine),
            Err(e) => {
                error!("Failed to load PAC file: {}", e);
                None
            }
        }
    } else {
        None
    };

    let proxy = Proxy::new(cfg, pac_engine, None);
    if let Err(e) = proxy.run().await {
        error!("Proxy error: {}", e);
        return Err(e.into());
    }

    Ok(())
}
