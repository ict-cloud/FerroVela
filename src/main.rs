use clap::Parser;
use log::{error, info};

mod auth;
mod config;
mod launchd;
mod logger;
mod pac;
mod proxy;
mod ui;

#[cfg(test)]
mod tests;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    config: Option<String>,
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Err(e) = logger::init() {
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

    // Single-instance check via Unix socket.
    // If another UI instance is already running, connecting to its socket signals it
    // to bring itself to the front, then we exit.
    if std::os::unix::net::UnixStream::connect(launchd::UI_SOCKET_PATH).is_ok() {
        info!("Existing UI instance found. Signaling and exiting.");
        return Ok(());
    }

    // Run the UI
    match ui::run_ui(config_str) {
        Ok(_) => Ok(()),
        Err(e) => {
            error!("UI Error: {}", e);
            Err(Box::new(e))
        }
    }
}
