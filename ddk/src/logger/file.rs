use super::{LogLevel, LogRecord, LogWriter};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::{Arc, Mutex};

/// File-based log writer that writes logs to a file with timestamps
pub struct FileLogWriter {
    writer: Arc<Mutex<BufWriter<File>>>,
    max_level: LogLevel,
}

impl FileLogWriter {
    pub fn new(file_path: &Path, max_level: LogLevel) -> Result<Self, std::io::Error> {
        // Create parent directory if it doesn't exist
        if let Some(parent) = file_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(file_path)?;

        let writer = Arc::new(Mutex::new(BufWriter::new(file)));

        Ok(Self { writer, max_level })
    }

    fn format_log_record(&self, record: &LogRecord) -> String {
        let timestamp = record.timestamp.format("%Y-%m-%d %H:%M:%S%.3f");
        let location = match (&record.module_path, record.line) {
            (Some(module), Some(line)) => format!(" [{module}:{line}]"),
            (Some(module), None) => format!(" [{module}]"),
            _ => String::new(),
        };

        format!(
            "{} {}{} {}\n",
            timestamp, record.level, location, record.message
        )
    }
}

impl LogWriter for FileLogWriter {
    fn write_log(&self, record: &LogRecord) {
        if record.level >= self.max_level {
            if let Ok(mut writer) = self.writer.lock() {
                let formatted = self.format_log_record(record);
                let _ = writer.write_all(formatted.as_bytes());
            }
        }
    }

    fn flush(&self) {
        if let Ok(mut writer) = self.writer.lock() {
            let _ = writer.flush();
        }
    }
}
