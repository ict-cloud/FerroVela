use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

pub const SERVICE_LABEL: &str = "com.ictcloud.ferrovela";

/// Returns the path to the UI IPC socket.
///
/// On macOS, `$TMPDIR` is a per-user, per-session directory managed by launchd
/// (e.g. `/var/folders/…/T/`).  It is not world-writable, so other users cannot
/// even reach the socket.  The socket file itself is additionally created with
/// mode `0600` by the UI process.
pub fn ui_socket_path() -> std::path::PathBuf {
    let dir = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            // Fallback for environments where TMPDIR is unset.
            let p = home()
                .join("Library")
                .join("Application Support")
                .join(SERVICE_LABEL);
            let _ = std::fs::create_dir_all(&p);
            p
        });
    dir.join(format!("{SERVICE_LABEL}.sock"))
}

fn home() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("/tmp"))
}

pub fn plist_path() -> PathBuf {
    home()
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{SERVICE_LABEL}.plist"))
}

pub fn log_path() -> PathBuf {
    home().join("Library").join("Logs").join("ferrovela.log")
}

/// Finds the `ferrovela` proxy binary in the same directory as the running executable.
fn proxy_exe() -> Result<PathBuf> {
    let path = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("ferrovela")))
        .filter(|p| p.exists())
        .context("proxy binary not found next to the running executable")?;
    Ok(path)
}

fn uid() -> u32 {
    // SAFETY: getuid(2) is always safe to call.
    unsafe { libc::getuid() }
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn generate_plist() -> Result<String> {
    let exe = proxy_exe()?;
    let log = log_path();
    Ok(format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
	<key>Label</key>
	<string>{label}</string>
	<key>ProgramArguments</key>
	<array>
		<string>{exe}</string>
	</array>
	<key>RunAtLoad</key>
	<false/>
	<key>KeepAlive</key>
	<false/>
	<key>StandardOutPath</key>
	<string>{log}</string>
	<key>StandardErrorPath</key>
	<string>{log}</string>
</dict>
</plist>
"#,
        label = SERVICE_LABEL,
        exe = xml_escape(&exe.to_string_lossy()),
        log = xml_escape(&log.to_string_lossy()),
    ))
}

fn install() -> Result<()> {
    let plist = generate_plist()?;
    let path = plist_path();
    std::fs::create_dir_all(path.parent().unwrap()).context("creating LaunchAgents directory")?;
    std::fs::write(&path, plist).context("writing plist file")?;
    Ok(())
}

pub fn start() -> Result<()> {
    install()?;
    let uid = uid();
    let plist = plist_path();
    let out = Command::new("launchctl")
        .args([
            "bootstrap",
            &format!("gui/{uid}"),
            plist.to_str().unwrap_or(""),
        ])
        .output()
        .context("running launchctl bootstrap")?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(anyhow::anyhow!(
            "launchctl bootstrap failed: {}",
            stderr.trim()
        ))
    }
}

pub fn stop() -> Result<()> {
    let uid = uid();
    let out = Command::new("launchctl")
        .args(["bootout", &format!("gui/{uid}/{SERVICE_LABEL}")])
        .output()
        .context("running launchctl bootout")?;
    if out.status.success() {
        Ok(())
    } else {
        let stderr = String::from_utf8_lossy(&out.stderr);
        Err(anyhow::anyhow!(
            "launchctl bootout failed: {}",
            stderr.trim()
        ))
    }
}

/// Returns `true` when the launchd service is loaded (i.e. bootstrapped).
///
/// A loaded service is either running or in the process of starting.
/// We intentionally do **not** require `pid > 0` because launchd may take a
/// moment to fork the process after bootstrap, and checking only for a live
/// PID would race with the UI poll and flip the toggle back to "Stopped".
pub fn is_running() -> bool {
    let uid = uid();
    Command::new("launchctl")
        .args(["print", &format!("gui/{uid}/{SERVICE_LABEL}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Returns the PID of the proxy process, or `None` when the service is not
/// loaded or has not yet been assigned a process.
pub fn pid() -> Option<u32> {
    let uid = uid();
    let output = Command::new("launchctl")
        .args(["print", &format!("gui/{uid}/{SERVICE_LABEL}")])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    stdout.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix("pid = ")
            .and_then(|v| v.parse::<u32>().ok())
            .filter(|&pid| pid > 0)
    })
}
