//! Test helper: captures the use cases' `tracing` events as JSON objects, so
//! tests can assert on the investigation events (OT-007 D7) and their fields.

use serde_json::Value;
use std::io::Write;
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
struct Buffer(Arc<Mutex<Vec<u8>>>);

impl Write for Buffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Buffer {
    type Writer = Self;
    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

/// Records every event on the current thread while it is alive.
/// `#[tokio::test]` runs on a current-thread runtime, so this sees everything
/// the use case logs.
pub struct LogCapture {
    buffer: Buffer,
    _guard: tracing::subscriber::DefaultGuard,
}

impl LogCapture {
    #[must_use]
    pub fn install() -> Self {
        let buffer = Buffer::default();
        let subscriber = tracing_subscriber::fmt()
            .json()
            .flatten_event(true)
            .with_max_level(tracing::Level::TRACE)
            .with_writer(buffer.clone())
            .finish();
        Self {
            _guard: tracing::subscriber::set_default(subscriber),
            buffer,
        }
    }

    /// Every captured event, one JSON object each.
    #[must_use]
    #[allow(
        clippy::missing_panics_doc,
        reason = "test-only helper; lock poison or invalid JSON panics the test"
    )]
    pub fn all(&self) -> Vec<Value> {
        let bytes = self.buffer.0.lock().unwrap().clone();
        String::from_utf8(bytes)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    /// The captured events whose `event` field is `name`.
    #[must_use]
    pub fn events(&self, name: &str) -> Vec<Value> {
        self.all()
            .into_iter()
            .filter(|e| e["event"] == name)
            .collect()
    }

    /// Every captured event that has an `event` field.
    #[must_use]
    pub fn named(&self) -> Vec<Value> {
        self.all()
            .into_iter()
            .filter(|e| e.get("event").is_some())
            .collect()
    }
}
