use log::error;

mod auth;
mod config;
mod launchd;
mod logger;
mod pac;
mod proxy;
mod ui;

#[cfg(test)]
mod tests;

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    if let Err(e) = logger::init() {
        eprintln!(
            "Warning: Failed to initialize file logger: {}. Continuing with stderr only.",
            e
        );
    }

    // Single-instance check via Unix socket.
    // If another UI instance is already running, connecting to its socket signals it
    // to bring itself to the front, then we exit.
    if std::os::unix::net::UnixStream::connect(launchd::UI_SOCKET_PATH).is_ok() {
        log::info!("Existing UI instance found. Signaling and exiting.");
        return Ok(());
    }

    // Run the UI
    match ui::run_ui() {
        Ok(_) => Ok(()),
        Err(e) => {
            error!("UI Error: {}", e);
            Err(Box::new(e))
        }
    }
}
