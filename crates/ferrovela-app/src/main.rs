//! ferrovela-app: A thin entry point for the FerroVela application bundle.
//!
//! This binary is intended to be the main executable for the macOS app bundle.
//! It ensures the background proxy daemon is running via launchd and then
//! executes the UI binary.

use anyhow::{Context, Result};
use std::env;
use std::os::unix::process::CommandExt;
use std::process::Command;

fn main() -> Result<()> {
    // 1. Locate the ferrovela-ui binary.
    // In a standard cargo-bundle setup, it should be in the same folder as this executable
    // (e.g., FerroVela.app/Contents/MacOS/ferrovela-app).
    let current_exe = env::current_exe().context("failed to get current executable path")?;
    let bin_dir = current_exe
        .parent()
        .context("failed to get binary directory")?;
    let ui_bin = bin_dir.join("ferrovela-ui");

    if !ui_bin.exists() {
        anyhow::bail!(
            "ferrovela-ui binary not found at {:?}. Expected it to be next to {:?}",
            ui_bin,
            current_exe
        );
    }

    // 3. Exec into the ferrovela-ui binary.
    // This replaces the current process with the UI process, passing any arguments.
    let err = Command::new(ui_bin).args(env::args().skip(1)).exec();

    // If exec() returns, it means it failed.
    Err(anyhow::Error::from(err).context("failed to execute ferrovela-ui"))
}
