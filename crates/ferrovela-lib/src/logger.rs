use log::{Level, Metadata, Record};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

/// Maximum size of a single log file before rotation is triggered (10 MiB).
const MAX_LOG_BYTES: u64 = 10 * 1024 * 1024;

/// Number of rotated backup files to keep (`.1` … `.N`).
const MAX_LOG_BACKUPS: u32 = 3;

struct RotatingFile {
    file: File,
    /// Bytes written to the current file since it was opened/rotated.
    bytes_written: u64,
}

struct RotatingLogger {
    path: PathBuf,
    inner: Mutex<RotatingFile>,
}

impl RotatingLogger {
    fn open(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        // Seed the counter from the current file size so a restart does not
        // reset the threshold (the file might already be nearly full).
        let bytes_written = file.metadata().map(|m| m.len()).unwrap_or(0);
        Ok(Self {
            path: path.to_owned(),
            inner: Mutex::new(RotatingFile {
                file,
                bytes_written,
            }),
        })
    }

    /// Rotate log files:
    ///
    /// ```text
    /// ferrovela.log.3  →  deleted
    /// ferrovela.log.2  →  ferrovela.log.3
    /// ferrovela.log.1  →  ferrovela.log.2
    /// ferrovela.log    →  ferrovela.log.1
    ///                      (new empty ferrovela.log opened)
    /// ```
    fn rotate(path: &Path) -> std::io::Result<File> {
        // Shift existing backups down, removing the oldest.
        for n in (1..MAX_LOG_BACKUPS).rev() {
            let from = backup_path(path, n);
            let to = backup_path(path, n + 1);
            if from.exists() {
                let _ = std::fs::rename(&from, &to);
            }
        }
        // Move the current log to .1
        if path.exists() {
            let _ = std::fs::rename(path, backup_path(path, 1));
        }
        // Open a fresh log file.
        OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path)
    }
}

fn format_timestamp() -> String {
    let secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let (year, month, day, hour, min, sec) = epoch_to_datetime(secs);
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{min:02}:{sec:02}Z")
}

/// Converts Unix epoch seconds to (year, month, day, hour, min, sec) UTC.
fn epoch_to_datetime(secs: u64) -> (u64, u8, u8, u8, u8, u8) {
    let sec = (secs % 60) as u8;
    let mins = secs / 60;
    let min = (mins % 60) as u8;
    let hours = mins / 60;
    let hour = (hours % 24) as u8;
    let days = hours / 24;

    // Compute year/month/day from days since epoch (1970-01-01).
    let mut year = 1970u64;
    let mut remaining = days;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if remaining < days_in_year {
            break;
        }
        remaining -= days_in_year;
        year += 1;
    }
    let months = [
        31u64,
        if is_leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1u8;
    for &m in &months {
        if remaining < m {
            break;
        }
        remaining -= m;
        month += 1;
    }
    let day = (remaining + 1) as u8;
    (year, month, day, hour, min, sec)
}

fn is_leap(year: u64) -> bool {
    (year.is_multiple_of(4) && !year.is_multiple_of(100)) || year.is_multiple_of(400)
}

fn backup_path(path: &Path, n: u32) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(format!(".{n}"));
    PathBuf::from(s)
}

impl log::Log for RotatingLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        if let Ok(mut inner) = self.inner.lock() {
            // Rotate before writing if the file has reached the size limit.
            if inner.bytes_written >= MAX_LOG_BYTES {
                match RotatingLogger::rotate(&self.path) {
                    Ok(new_file) => {
                        inner.file = new_file;
                        inner.bytes_written = 0;
                    }
                    Err(e) => {
                        // Rotation failed — keep writing to the existing file
                        // rather than silently dropping log lines.
                        let _ = writeln!(inner.file, "ERROR - log rotation failed: {e}");
                    }
                }
            }
            let timestamp = format_timestamp();
            let msg = format!("{} {} - {}\n", timestamp, record.level(), record.args());
            if inner.file.write_all(msg.as_bytes()).is_ok() {
                inner.bytes_written += msg.len() as u64;
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut inner) = self.inner.lock() {
            let _ = inner.file.flush();
        }
    }
}

pub fn init_to(path: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let logger = RotatingLogger::open(path)?;

    let static_logger: &'static RotatingLogger = Box::leak(Box::new(logger));
    log::set_logger(static_logger).map_err(|e| {
        Box::new(std::io::Error::other(e.to_string())) as Box<dyn std::error::Error + Send + Sync>
    })?;
    log::set_max_level(log::LevelFilter::Info);
    Ok(())
}

pub fn init() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    init_to(Path::new("service.log"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use log::Log;

    #[test]
    fn test_rotation_creates_backup_and_fresh_file() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join("test.log");

        // Pre-fill the log file so it is already at the size limit.
        let big = "x".repeat(MAX_LOG_BYTES as usize);
        std::fs::write(&log, &big).unwrap();

        let logger = RotatingLogger::open(&log).unwrap();
        // Write one line — this must trigger rotation first.
        use log::Record;
        logger.log(
            &Record::builder()
                .level(log::Level::Info)
                .args(format_args!("hello after rotation"))
                .build(),
        );

        // The backup must exist and contain the original content.
        let backup = backup_path(&log, 1);
        assert!(backup.exists(), "backup .1 must exist after rotation");
        let backup_content = std::fs::read_to_string(&backup).unwrap();
        assert_eq!(backup_content.len(), MAX_LOG_BYTES as usize);

        // The new log file must be small (just the one line written after rotation).
        let new_content = std::fs::read_to_string(&log).unwrap();
        assert!(new_content.contains("hello after rotation"));
        assert!(new_content.len() < MAX_LOG_BYTES as usize);
    }

    #[test]
    fn test_rotation_shifts_existing_backups() {
        let dir = tempfile::TempDir::new().unwrap();
        let log = dir.path().join("test.log");

        // Seed existing backups .1 and .2.
        std::fs::write(backup_path(&log, 1), "backup1").unwrap();
        std::fs::write(backup_path(&log, 2), "backup2").unwrap();

        // Fill the primary log to trigger rotation.
        let big = "x".repeat(MAX_LOG_BYTES as usize);
        std::fs::write(&log, &big).unwrap();

        let logger = RotatingLogger::open(&log).unwrap();
        use log::Record;
        logger.log(
            &Record::builder()
                .level(log::Level::Info)
                .args(format_args!("trigger"))
                .build(),
        );

        // Old .2 → .3, old .1 → .2, primary → .1.
        assert_eq!(
            std::fs::read_to_string(backup_path(&log, 3)).unwrap(),
            "backup2"
        );
        assert_eq!(
            std::fs::read_to_string(backup_path(&log, 2)).unwrap(),
            "backup1"
        );
        assert_eq!(
            std::fs::read_to_string(backup_path(&log, 1)).unwrap().len(),
            MAX_LOG_BYTES as usize
        );
    }
}
