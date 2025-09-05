use super::{LogLevel, LogRecord, LogWriter};
use std::io::Write;

/// Console log writer that writes logs to stdout/stderr
pub struct ConsoleLogWriter {
    max_level: LogLevel,
}

impl ConsoleLogWriter {
    pub fn new(max_level: LogLevel) -> Self {
        Self { max_level }
    }

    fn format_log_record(&self, record: &LogRecord) -> String {
        let timestamp = record.timestamp.format("%H:%M:%S%.3f");
        let location = match (&record.module_path, record.line) {
            (Some(module), Some(line)) => format!(" [{module}:{line}]"),
            (Some(module), None) => format!(" [{module}]"),
            _ => String::new(),
        };

        format!(
            "{} {}{} {}",
            timestamp, record.level, location, record.message
        )
    }
}

impl LogWriter for ConsoleLogWriter {
    fn write_log(&self, record: &LogRecord) {
        if record.level >= self.max_level {
            let formatted = self.format_log_record(record);
            match record.level {
                LogLevel::Error => eprintln!("{formatted}"),
                _ => println!("{formatted}"),
            }
        }
    }

    fn flush(&self) {
        use std::io::{stderr, stdout};
        let _ = stdout().flush();
        let _ = stderr().flush();
    }
}
