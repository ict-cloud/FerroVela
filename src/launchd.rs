use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

pub const SERVICE_LABEL: &str = "com.ictcloud.ferrovela";
pub const UI_SOCKET_PATH: &str = "/tmp/ferrovela-ui.sock";

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

fn uid() -> String {
    Command::new("id")
        .arg("-u")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "501".to_string())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

fn generate_plist(config_path: &str) -> Result<String> {
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
		<string>--config</string>
		<string>{config}</string>
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
        config = xml_escape(config_path),
        log = xml_escape(&log.to_string_lossy()),
    ))
}

fn install(config_path: &str) -> Result<()> {
    let plist = generate_plist(config_path)?;
    let path = plist_path();
    std::fs::create_dir_all(path.parent().unwrap()).context("creating LaunchAgents directory")?;
    std::fs::write(&path, plist).context("writing plist file")?;
    Ok(())
}

pub fn start(config_path: &str) -> Result<()> {
    install(config_path)?;
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

/// Returns `true` when the launchd service is loaded and has a running PID.
pub fn is_running() -> bool {
    let uid = uid();
    Command::new("launchctl")
        .args(["print", &format!("gui/{uid}/{SERVICE_LABEL}")])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}
