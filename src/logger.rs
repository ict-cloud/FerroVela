use log::{Level, Metadata, Record};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::sync::Mutex;

struct SimpleLogger {
    file: Mutex<File>,
}

impl log::Log for SimpleLogger {
    fn enabled(&self, metadata: &Metadata) -> bool {
        metadata.level() <= Level::Info
    }

    fn log(&self, record: &Record) {
        if self.enabled(record.metadata()) {
            // More efficient formatting - single string allocation
            eprintln!("{} - {}", record.level(), record.args());
            if let Ok(mut file) = self.file.lock() {
                // Use writeln! for better efficiency
                let _ = writeln!(file, "{} - {}", record.level(), record.args());
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut file) = self.file.lock() {
            let _ = file.flush();
        }
    }
}

pub fn init() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open("service.log")?;

    let logger = SimpleLogger {
        file: Mutex::new(file),
    };

    let boxed_logger = Box::new(logger);
    let static_logger: &'static SimpleLogger = Box::leak(boxed_logger);

    log::set_logger(static_logger).map_err(|e| {
        Box::new(std::io::Error::other(e.to_string())) as Box<dyn std::error::Error + Send + Sync>
    })?;

    log::set_max_level(log::LevelFilter::Info);
    Ok(())
}
