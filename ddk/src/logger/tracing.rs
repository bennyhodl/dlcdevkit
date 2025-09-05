use super::{LogLevel, LogRecord, LogWriter};

/// Tracing-compatible log writer that bridges to the standard Rust `log` crate
pub struct TracingLogWriter {
    max_level: LogLevel,
}

impl TracingLogWriter {
    pub fn new(max_level: LogLevel) -> Self {
        Self { max_level }
    }
}

impl LogWriter for TracingLogWriter {
    fn write_log(&self, record: &LogRecord) {
        if record.level >= self.max_level {
            let module_path = record.module_path.as_deref().unwrap_or(&record.target);
            // Use tracing macros with module information as fields
            match record.level {
                LogLevel::Trace => tracing::trace!(
                    message = record.message,
                    module = module_path,
                    timestamp = record.timestamp.to_utc().timestamp_millis(),
                ),
                LogLevel::Debug => tracing::debug!(
                    message = record.message,
                    module = module_path,
                    timestamp = record.timestamp.to_utc().timestamp_millis(),
                ),
                LogLevel::Info => tracing::info!(
                    message = record.message,
                    module = module_path,
                    timestamp = record.timestamp.to_utc().timestamp_millis(),
                ),
                LogLevel::Warn => tracing::warn!(
                    message = %record.message,
                    module = module_path,
                    timestamp = record.timestamp.to_utc().timestamp_millis(),
                ),
                LogLevel::Error => tracing::error!(
                    message = record.message,
                    module = module_path,
                    timestamp = record.timestamp.to_utc().timestamp_millis(),
                ),
            };
        };
    }

    fn flush(&self) {
        // Tracing handles flushing internally
    }
}

// Initializes the global tracing subscriber with DDK logger integration
// pub fn init_tracing_subscriber(config: TracingConfig) -> Result<(), Box<dyn std::error::Error>> {
//     let filter = EnvFilter::try_from_default_env().or_else(|_| {
//         let level_filter = match config.max_level {
//             LogLevel::Trace => "trace",
//             LogLevel::Debug => "debug",
//             LogLevel::Info => "info",
//             LogLevel::Warn => "warn",
//             LogLevel::Error => "error",
//         };
//         EnvFilter::try_new(level_filter)
//     })?;

//     let subscriber = tracing_subscriber::Registry::default().with(filter).with(
//         fmt::layer()
//             .with_target(true)
//             .with_thread_ids(false)
//             .with_thread_names(false)
//             .with_file(true)
//             .with_line_number(true),
//     );

//     tracing::subscriber::set_global_default(subscriber)?;
//     Ok(())
// }
