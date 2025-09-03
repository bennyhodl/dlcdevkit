mod console;
mod file;
mod multi;
mod tracing;

use ::tracing::Level;
use chrono::{DateTime, Utc};
use lightning::util::logger::Level as LdkLevel;
use lightning::util::logger::Record as LdkRecord;
pub use lightning::{
    log_bytes, log_debug, log_error, log_info, log_trace, log_warn,
    util::logger::Logger as WriteLog,
};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// Log levels supported by DDK logging system
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Trace,
    Debug,
    Info,
    Warn,
    Error,
}

impl From<String> for LogLevel {
    fn from(s: String) -> Self {
        match s.as_str() {
            "info" => LogLevel::Info,
            "debug" => LogLevel::Debug,
            "trace" => LogLevel::Trace,
            "warn" => LogLevel::Warn,
            "error" => LogLevel::Error,
            _ => LogLevel::Info,
        }
    }
}

/// Structured log record containing all log information
#[derive(Debug, Clone)]
pub struct LogRecord {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub target: String,
    pub module_path: Option<String>,
    pub file: Option<String>,
    pub line: Option<u32>,
    pub message: String,
    pub fields: HashMap<String, String>,
}

/// Trait for custom log writers that can be plugged into the logging system
pub trait LogWriter: Send + Sync + 'static {
    /// Write a log record to the output destination
    fn write_log(&self, record: &LogRecord);

    /// Flush any buffered log data
    fn flush(&self);
}

/// Central logger that coordinates all logging activities
pub struct Logger {
    writer: Arc<dyn LogWriter>,
    name: String,
}

impl lightning::util::logger::Logger for Logger {
    fn log(&self, record: LdkRecord) {
        let ddk_record: LogRecord = record.into();
        self.writer.write_log(&ddk_record);
    }
}

impl Logger {
    pub fn new(name: String, writer: Arc<dyn LogWriter>) -> Self {
        Self { name, writer }
    }

    /// Create a logger that outputs to the console
    pub fn console(name: String, max_level: LogLevel) -> Self {
        let writer = Arc::new(console::ConsoleLogWriter::new(max_level));
        Self::new(name, writer)
    }

    /// Create a logger that outputs to a file
    pub fn file(
        name: String,
        file_path: &Path,
        max_level: LogLevel,
    ) -> Result<Self, std::io::Error> {
        let writer = Arc::new(file::FileLogWriter::new(file_path, max_level)?);
        Ok(Self::new(name, writer))
    }

    /// Create a logger that integrates with tracing
    pub fn tracing(name: String, max_level: LogLevel) -> Self {
        let writer = Arc::new(tracing::TracingLogWriter::new(max_level));
        Self::new(name, writer)
    }

    /// Create a logger that discards all log messages (no-op)
    pub fn disabled(name: String) -> Self {
        struct NoOpWriter;
        impl LogWriter for NoOpWriter {
            fn write_log(&self, _record: &LogRecord) {}
            fn flush(&self) {}
        }
        let writer = Arc::new(NoOpWriter);
        Self::new(name, writer)
    }

    pub fn write_record(&self, record: LogRecord) {
        self.writer.write_log(&record);
    }

    pub fn trace(&self, target: &str, message: &str) {
        let record = LogRecord::new(
            LogLevel::Trace,
            target.to_string(),
            message.to_string(),
            None,
            None,
            None,
        );
        self.write_record(record);
    }

    pub fn debug(&self, target: &str, message: &str) {
        let record = LogRecord::new(
            LogLevel::Debug,
            target.to_string(),
            message.to_string(),
            None,
            None,
            None,
        );
        self.write_record(record);
    }

    pub fn info(&self, target: &str, message: &str) {
        let record = LogRecord::new(
            LogLevel::Info,
            target.to_string(),
            message.to_string(),
            None,
            None,
            None,
        );
        self.write_record(record);
    }

    pub fn warn(&self, target: &str, message: &str) {
        let record = LogRecord::new(
            LogLevel::Warn,
            target.to_string(),
            message.to_string(),
            None,
            None,
            None,
        );
        self.write_record(record);
    }

    pub fn error(&self, target: &str, message: &str) {
        let record = LogRecord::new(
            LogLevel::Error,
            target.to_string(),
            message.to_string(),
            None,
            None,
            None,
        );
        self.write_record(record);
    }

    pub fn flush(&self) {
        self.writer.flush();
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl From<LogLevel> for Level {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Trace => Level::TRACE,
            LogLevel::Debug => Level::DEBUG,
            LogLevel::Info => Level::INFO,
            LogLevel::Warn => Level::WARN,
            LogLevel::Error => Level::ERROR,
        }
    }
}

impl std::fmt::Display for LogLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LogLevel::Trace => write!(f, "TRACE"),
            LogLevel::Debug => write!(f, "DEBUG"),
            LogLevel::Info => write!(f, "INFO "),
            LogLevel::Warn => write!(f, "WARN "),
            LogLevel::Error => write!(f, "ERROR"),
        }
    }
}

impl std::fmt::Debug for Logger {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Logger")
            .field("name", &self.name)
            .field("writer", &"<log_writer>")
            .finish()
    }
}

impl LogRecord {
    pub fn new(
        level: LogLevel,
        target: String,
        message: String,
        module_path: Option<String>,
        file: Option<String>,
        line: Option<u32>,
    ) -> Self {
        Self {
            timestamp: Utc::now(),
            level,
            target,
            module_path,
            file,
            line,
            message,
            fields: HashMap::new(),
        }
    }

    pub fn add_field(&mut self, key: String, value: String) {
        self.fields.insert(key, value);
    }
}

// Convert LDK Level to DDK LogLevel
impl From<LdkLevel> for LogLevel {
    fn from(ldk_level: LdkLevel) -> Self {
        match ldk_level {
            LdkLevel::Gossip => LogLevel::Trace,
            LdkLevel::Trace => LogLevel::Trace,
            LdkLevel::Debug => LogLevel::Debug,
            LdkLevel::Info => LogLevel::Info,
            LdkLevel::Warn => LogLevel::Warn,
            LdkLevel::Error => LogLevel::Error,
        }
    }
}

// Convert LDK Record to DDK LogRecord
impl<'a> From<LdkRecord<'a>> for LogRecord {
    fn from(record: LdkRecord<'a>) -> Self {
        Self {
            timestamp: chrono::Utc::now(),
            level: record.level.into(),
            target: record.module_path.to_string(),
            module_path: Some(record.module_path.to_string()),
            file: None, // LDK Record doesn't include file info
            line: Some(record.line),
            message: record.args.to_string(),
            fields: HashMap::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Mutex;
    use std::thread;
    use std::time::Duration;

    /// Test utility logger that captures logs in memory for verification
    pub struct TestLogWriter {
        logs: Arc<Mutex<VecDeque<LogRecord>>>,
        max_level: LogLevel,
        max_logs: usize,
    }

    impl TestLogWriter {
        pub fn new(max_level: LogLevel) -> Self {
            Self {
                logs: Arc::new(Mutex::new(VecDeque::new())),
                max_level,
                max_logs: 1000, // Default maximum number of logs to keep in memory
            }
        }

        pub fn with_capacity(max_level: LogLevel, max_logs: usize) -> Self {
            Self {
                logs: Arc::new(Mutex::new(VecDeque::new())),
                max_level,
                max_logs,
            }
        }

        /// Retrieve all captured logs
        pub fn get_logs(&self) -> Vec<LogRecord> {
            self.logs.lock().unwrap().clone().into()
        }

        /// Get logs at a specific level
        pub fn get_logs_at_level(&self, level: LogLevel) -> Vec<LogRecord> {
            self.logs
                .lock()
                .unwrap()
                .iter()
                .filter(|log| log.level == level)
                .cloned()
                .collect()
        }

        /// Get logs containing specific text
        pub fn get_logs_containing(&self, text: &str) -> Vec<LogRecord> {
            self.logs
                .lock()
                .unwrap()
                .iter()
                .filter(|log| log.message.contains(text))
                .cloned()
                .collect()
        }

        /// Check if any log contains specific text
        pub fn has_log_containing(&self, text: &str) -> bool {
            self.logs
                .lock()
                .unwrap()
                .iter()
                .any(|log| log.message.contains(text))
        }

        /// Get logs from a specific target
        pub fn get_logs_from_target(&self, target: &str) -> Vec<LogRecord> {
            self.logs
                .lock()
                .unwrap()
                .iter()
                .filter(|log| log.target == target)
                .cloned()
                .collect()
        }
    }

    impl LogWriter for TestLogWriter {
        fn write_log(&self, record: &LogRecord) {
            if record.level >= self.max_level {
                let mut logs = self.logs.lock().unwrap();

                // Keep only the most recent logs if we exceed capacity
                if logs.len() >= self.max_logs {
                    logs.pop_front();
                }

                logs.push_back(record.clone());
            }
        }

        fn flush(&self) {
            // No-op for in-memory storage
        }
    }

    /// Convenience function to create a test logger with TestLogWriter
    pub fn create_test_logger(
        name: &str,
        max_level: LogLevel,
    ) -> (Arc<Logger>, Arc<TestLogWriter>) {
        let writer = Arc::new(TestLogWriter::new(max_level));
        let logger = Arc::new(Logger::new(name.to_string(), writer.clone()));
        (logger, writer)
    }

    #[test]
    fn test_test_log_writer_basic() {
        let writer = TestLogWriter::new(LogLevel::Debug);
        let record = LogRecord::new(
            LogLevel::Info,
            "test".to_string(),
            "test message".to_string(),
            Some("test::module".to_string()),
            Some("test.rs".to_string()),
            Some(42),
        );

        writer.write_log(&record);

        let logs = writer.get_logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "test message");
        assert_eq!(logs[0].level, LogLevel::Info);
    }

    #[test]
    fn test_log_level_filtering() {
        let writer = TestLogWriter::new(LogLevel::Warn);

        // This should be filtered out
        let debug_record = LogRecord::new(
            LogLevel::Debug,
            "test".to_string(),
            "debug message".to_string(),
            None,
            None,
            None,
        );

        // This should be included
        let error_record = LogRecord::new(
            LogLevel::Error,
            "test".to_string(),
            "error message".to_string(),
            None,
            None,
            None,
        );

        writer.write_log(&debug_record);
        writer.write_log(&error_record);

        let logs = writer.get_logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "error message");
    }

    #[test]
    fn test_capacity_limit() {
        let writer = TestLogWriter::with_capacity(LogLevel::Debug, 3);

        for i in 0..5 {
            let record = LogRecord::new(
                LogLevel::Info,
                "test".to_string(),
                format!("message {}", i),
                None,
                None,
                None,
            );
            writer.write_log(&record);
        }

        let logs = writer.get_logs();
        assert_eq!(logs.len(), 3);
        // Should keep the most recent logs
        assert_eq!(logs[0].message, "message 2");
        assert_eq!(logs[1].message, "message 3");
        assert_eq!(logs[2].message, "message 4");
    }

    #[test]
    fn test_log_search_functionality() {
        let writer = TestLogWriter::new(LogLevel::Debug);

        let records = vec![
            LogRecord::new(
                LogLevel::Info,
                "ddk::wallet".to_string(),
                "wallet synchronized".to_string(),
                None,
                None,
                None,
            ),
            LogRecord::new(
                LogLevel::Error,
                "ddk::transport".to_string(),
                "connection failed".to_string(),
                None,
                None,
                None,
            ),
            LogRecord::new(
                LogLevel::Warn,
                "ddk::wallet".to_string(),
                "low balance warning".to_string(),
                None,
                None,
                None,
            ),
        ];

        for record in &records {
            writer.write_log(record);
        }

        // Test filtering by level
        let error_logs = writer.get_logs_at_level(LogLevel::Error);
        assert_eq!(error_logs.len(), 1);
        assert!(error_logs[0].message.contains("connection failed"));

        // Test filtering by text content
        let wallet_logs = writer.get_logs_containing("wallet");
        assert_eq!(wallet_logs.len(), 1);

        // Test filtering by target
        let wallet_target_logs = writer.get_logs_from_target("ddk::wallet");
        assert_eq!(wallet_target_logs.len(), 2);

        // Test existence check
        assert!(writer.has_log_containing("synchronized"));
        assert!(!writer.has_log_containing("nonexistent"));
    }

    #[test]
    fn test_create_test_logger() {
        let (logger, writer) = create_test_logger("test_logger", LogLevel::Debug);

        logger.info("ddk::test", "test message");

        let logs = writer.get_logs();
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "test message");
        assert_eq!(logs[0].target, "ddk::test");
        assert_eq!(logger.name(), "test_logger");
    }

    #[test]
    fn test_thread_safety() {
        let writer = Arc::new(TestLogWriter::new(LogLevel::Debug));
        let mut handles = vec![];

        for i in 0..10 {
            let writer_clone = writer.clone();
            let handle = thread::spawn(move || {
                let record = LogRecord::new(
                    LogLevel::Info,
                    "test".to_string(),
                    format!("message from thread {}", i),
                    None,
                    None,
                    None,
                );
                writer_clone.write_log(&record);
                thread::sleep(Duration::from_millis(10));
            });
            handles.push(handle);
        }

        for handle in handles {
            handle.join().unwrap();
        }

        let logs = writer.get_logs();
        assert_eq!(logs.len(), 10);
    }
}
