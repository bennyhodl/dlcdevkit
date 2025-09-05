#![allow(dead_code)]
use super::{LogRecord, LogWriter};
use std::sync::Arc;

/// Multi-writer that can send logs to multiple destinations
pub struct MultiLogWriter {
    writers: Vec<Arc<dyn LogWriter>>,
}

impl MultiLogWriter {
    pub fn new() -> Self {
        Self {
            writers: Vec::new(),
        }
    }

    pub fn add_writer(&mut self, writer: Arc<dyn LogWriter>) {
        self.writers.push(writer);
    }
}

impl LogWriter for MultiLogWriter {
    fn write_log(&self, record: &LogRecord) {
        for writer in &self.writers {
            writer.write_log(record);
        }
    }

    fn flush(&self) {
        for writer in &self.writers {
            writer.flush();
        }
    }
}
