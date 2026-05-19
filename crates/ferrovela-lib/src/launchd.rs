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

/// Removes the service from the launchd domain.
/// Returns `Ok(())` both on success and when the service was not registered
/// (ESRCH / error 3 — "No such process").
fn bootout_service(target: &str) -> Result<()> {
    let out = Command::new("launchctl")
        .args(["bootout", target])
        .output()
        .context("running launchctl bootout")?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // Error 3 = ESRCH — service not registered; treat as already stopped.
    if stderr.contains("3:") || stderr.contains("No such process") {
        return Ok(());
    }
    Err(anyhow::anyhow!(
        "launchctl bootout failed: {}",
        stderr.trim()
    ))
}

/// Polls `pid()` up to `attempts` times with `interval_ms` between each try.
fn wait_for_pid(attempts: u32, interval_ms: u64) -> Option<u32> {
    for _ in 0..attempts {
        if let Some(p) = pid() {
            return Some(p);
        }
        std::thread::sleep(std::time::Duration::from_millis(interval_ms));
    }
    None
}

pub fn start() -> Result<()> {
    install()?;
    let uid = uid();
    let plist = plist_path();
    let target = format!("gui/{uid}");
    let service = format!("gui/{uid}/{SERVICE_LABEL}");

    // ── Phase A: bootstrap ────────────────────────────────────────────────
    // If the service is already registered (error 37), the launchd domain
    // still holds the old in-memory configuration. Evict it first so the
    // freshly-written plist is picked up on re-bootstrap.
    let out = Command::new("launchctl")
        .args(["bootstrap", &target, plist.to_str().unwrap_or("")])
        .output()
        .context("running launchctl bootstrap")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        if stderr.contains("37:") {
            bootout_service(&service)
                .map_err(|e| anyhow::anyhow!("failed to clear stale service registration: {e}"))?;
            let out2 = Command::new("launchctl")
                .args(["bootstrap", &target, plist.to_str().unwrap_or("")])
                .output()
                .context("running launchctl bootstrap (after stale cleanup)")?;
            if !out2.status.success() {
                let stderr2 = String::from_utf8_lossy(&out2.stderr);
                return Err(anyhow::anyhow!(
                    "re-bootstrap failed after clearing stale registration: {}",
                    stderr2.trim()
                ));
            }
        } else {
            return Err(anyhow::anyhow!(
                "launchctl bootstrap failed: {}",
                stderr.trim()
            ));
        }
    }

    // ── Phase B: kickstart ────────────────────────────────────────────────
    // RunAtLoad=false means bootstrap alone does not spawn the process.
    let out = Command::new("launchctl")
        .args(["kickstart", &service])
        .output()
        .context("running launchctl kickstart")?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(anyhow::anyhow!(
            "launchctl kickstart failed: {}",
            stderr.trim()
        ));
    }

    // ── Phase C: verify the process is actually alive ─────────────────────
    // kickstart returns 0 once the process is spawned, not once it stays
    // alive. Poll for up to 500 ms (10 × 50 ms) to catch immediate crashes.
    if wait_for_pid(10, 50).is_none() {
        return Err(anyhow::anyhow!(
            "service process exited immediately after launch — check {} for details",
            log_path().display()
        ));
    }

    Ok(())
}

pub fn stop() -> Result<()> {
    let uid = uid();
    bootout_service(&format!("gui/{uid}/{SERVICE_LABEL}"))
}

/// Returns `true` when the proxy process is actually running (has a live PID).
///
/// A service can be bootstrapped into the launchd domain without the proxy
/// binary being active — e.g. after a reboot when `RunAtLoad` is `false`.
/// macOS automatically bootstraps every plist in `~/Library/LaunchAgents/` at
/// login, so checking the `launchctl print` exit code alone would wrongly
/// report "Running" when the process was never spawned.
///
/// `start()` verifies that a PID appears before returning `Ok(())`, so the
/// 3-second poll exists only to catch crashes that happen after the initial check.
pub fn is_running() -> bool {
    pid().is_some()
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
