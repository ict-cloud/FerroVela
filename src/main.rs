use clap::Parser;
use log::{error, info};
use std::net::SocketAddr;
use std::sync::Arc;

mod config;
mod pac;
mod proxy;

use crate::config::{load_config, Config};
use crate::pac::PacEngine;
use crate::proxy::Proxy;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "config.toml")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    env_logger::init();
    let args = Args::parse();

    info!("Loading configuration from {}", args.config);
    let config = match load_config(&args.config) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            error!("Failed to load config: {}", e);
            std::process::exit(1);
        }
    };

    let pac_engine = if let Some(path) = &config.proxy.pac_file {
        info!("Loading PAC file from {}", path);
        match PacEngine::new(path) {
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
