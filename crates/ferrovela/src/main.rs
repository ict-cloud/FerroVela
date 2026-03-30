use log::error;
use std::sync::Arc;

use ferrovela_lib::{config, launchd, logger, pac::PacEngine, proxy::Proxy};

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

    let cfg = Arc::new(config::load_config());

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
