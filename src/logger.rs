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
            let msg = format!("{} - {}\n", record.level(), record.args());
            println!("{}", msg.trim()); // Print to stdout
            if let Ok(mut file) = self.file.lock() {
                let _ = file.write_all(msg.as_bytes());
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
        .write(true)
        .append(true)
        .open("service.log")
        .expect("Failed to open service.log");

    let logger = SimpleLogger {
        file: Mutex::new(file),
    };

    log::set_boxed_logger(Box::new(logger))
        .map(|()| log::set_max_level(log::LevelFilter::Info))
        .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)
}
