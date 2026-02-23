use clap::Parser;
use log::{error, info};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

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
    #[arg(short, long, default_value = "config.toml")]
    config: String,

    /// Launch the configuration UI
    #[arg(long)]
    ui: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    logger::init().expect("Failed to initialize logger");
    let args = Args::parse();

    // Single Instance Check / IPC
    // Try to bind to the control port.
    let listener = match TcpListener::bind("127.0.0.1:3129") {
        Ok(l) => l,
        Err(_) => {
            // Port is busy, assume another instance is running.
            // If the user requested UI (or by default per new requirements), signal the existing instance.
            info!("Instance already running. Sending Show signal...");
            if let Ok(mut stream) = TcpStream::connect("127.0.0.1:3129") {
                let _ = stream.write_all(b"S");
            } else {
                error!("Failed to connect to existing instance.");
            }
            return Ok(());
        }
    };

    // We are the main instance.
    // Create a channel for IPC messages.
    let (tx, rx) = tokio::sync::mpsc::channel(32);

    // Spawn a thread to handle IPC connections.
    // We use a standard thread because the Iced runtime might not be accessible yet,
    // and we want this listener to be independent of the UI loop's state (mostly).
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(mut stream) => {
                    let mut buf = [0u8; 1];
                    // Read the command byte
                    if stream.read_exact(&mut buf).is_ok() {
                        if buf[0] == b'S' {
                            // "Show" command
                            if let Err(e) = tx.blocking_send(ui::ExternalCmd::Show) {
                                error!("Failed to send IPC command to UI: {}", e);
                                break; // Channel closed, UI probably exited.
                            }
                        }
                    }
                }
                Err(e) => error!("IPC Connection failed: {}", e),
            }
        }
    });

    // Run the UI. The UI now handles the Proxy lifecycle internally.
    // We pass the config path and the IPC receiver.
    match ui::run_ui(args.config, rx) {
        Ok(_) => Ok(()),
        Err(e) => {
            error!("UI Error: {}", e);
            Err(Box::new(e))
        }
    }
}
