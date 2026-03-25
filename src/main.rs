use clap::Parser;
use log::{error, info};
use std::io::{Read, Write};
use std::net::TcpStream;

mod auth;
mod config;
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

    // Single Instance Check / IPC via Proxy Port (Default 3128)
    // We try to connect to the default port. If we can talk to our proxy, we signal it to show UI.
    // If not (connection refused, or not our proxy), we start a new instance.

    // Note: If the user changed the port in config, we should check THAT port.
    // So we should load config first.
    let config_port = match config::load_config(&config_str) {
        Ok(c) => c.proxy.port,
        Err(_) => config::default_port(), // Default fallback
    };

    let addr = format!("127.0.0.1:{}", config_port);
    info!("Checking for existing instance on {}", addr);

    if let Ok(mut stream) = TcpStream::connect(&addr) {
        // Send Magic Request
        let request = crate::proxy::MAGIC_SHOW_REQUEST;
        if stream.write_all(request.as_bytes()).is_ok() {
            let mut buffer = [0; 1024];
            if let Ok(n) = stream.read(&mut buffer) {
                let response = String::from_utf8_lossy(&buffer[..n]);
                if response.contains("200 OK") {
                    info!("Existing instance found and signaled. Exiting.");
                    return Ok(());
                }
            }
        }
        info!("Port {} is open but did not respond correctly. Starting new instance (User might need to change port).", config_port);
    } else {
        info!(
            "No instance found on {}. Starting new instance.",
            config_port
        );
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
