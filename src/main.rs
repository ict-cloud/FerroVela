use clap::Parser;
use log::{error, info};
use std::sync::Arc;

mod config;
mod logger;
mod pac;
mod proxy;
mod ui;
mod auth;

#[cfg(test)]
mod tests;

use crate::config::load_config;
use crate::pac::PacEngine;
use crate::proxy::Proxy;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "config.toml")]
    config: String,

    /// Launch the configuration UI
    #[arg(long)]
    ui: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    logger::init().expect("Failed to initialize logger");
    let args = Args::parse();

    if args.ui {
        match ui::run_ui(args.config) {
            Ok(_) => Ok(()),
            Err(e) => {
                error!("UI Error: {}", e);
                Err(Box::new(e))
            }
        }
    } else {
        let rt = tokio::runtime::Runtime::new()?;
        rt.block_on(run_proxy(args.config))
    }
}

async fn run_proxy(config_path: String) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    info!("Loading configuration from {}", config_path);
    let config = match load_config(&config_path) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            error!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    let pac_engine = if let Some(path) = &config.proxy.pac_file {
        info!("Loading PAC file from {}", path);
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

    let proxy = Proxy::new(config.clone(), pac_engine);

    info!("Starting proxy on port {}", config.proxy.port);
    proxy.run().await?;

    Ok(())
}
