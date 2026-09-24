use std::collections::VecDeque;
use std::fmt;
use std::sync::{Arc, Mutex, OnceLock};

use chrono::Local;
use serde::Serialize;
use tauri::{AppHandle, Emitter};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();

/// Call once Tauri is ready so the layer can emit events to the frontend.
pub fn set_app_handle(handle: AppHandle) {
    let _ = APP_HANDLE.set(handle);
}

#[derive(Clone, Serialize)]
pub struct LogLine {
    pub timestamp: String,
    pub level: String,
    pub target: String,
    pub message: String,
}

#[derive(Clone)]
pub struct LogBuffer {
    lines: Arc<Mutex<VecDeque<LogLine>>>,
    max_lines: usize,
}

impl LogBuffer {
    pub fn new(max_lines: usize) -> Self {
        Self {
            lines: Arc::new(Mutex::new(VecDeque::with_capacity(max_lines))),
            max_lines,
        }
    }

    fn push(&self, line: LogLine) {
        let mut lines = self.lines.lock().unwrap();
        if lines.len() >= self.max_lines {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// Return all lines starting from the given cursor, plus the new cursor.
    pub fn read_from(&self, cursor: usize) -> (Vec<LogLine>, usize) {
        let lines = self.lines.lock().unwrap();
        let total = lines.len();
        if cursor >= total {
            return (vec![], total);
        }
        (lines.iter().skip(cursor).cloned().collect(), total)
    }
}

pub struct CaptureLayer {
    buffer: LogBuffer,
}

impl CaptureLayer {
    pub fn new(buffer: LogBuffer) -> Self {
        Self { buffer }
    }
}

struct MessageVisitor {
    message: String,
    fields: Vec<(String, String)>,
}

impl MessageVisitor {
    fn new() -> Self {
        Self {
            message: String::new(),
            fields: vec![],
        }
    }

    fn formatted(&self) -> String {
        let mut out = self.message.clone();
        for (k, v) in &self.fields {
            out.push_str(&format!(" {k}={v}"));
        }
        out
    }
}

impl Visit for MessageVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = format!("{value:?}");
        } else {
            self.fields
                .push((field.name().to_string(), format!("{value:?}")));
        }
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message = value.to_string();
        } else {
            self.fields
                .push((field.name().to_string(), value.to_string()));
        }
    }
}

impl<S: Subscriber> Layer<S> for CaptureLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        let level = *metadata.level();
        let target = metadata.target();

        let mut visitor = MessageVisitor::new();
        event.record(&mut visitor);

        let line = LogLine {
            timestamp: Local::now().format("%H:%M:%S%.3f").to_string(),
            level: match level {
                Level::ERROR => "ERROR".into(),
                Level::WARN => "WARN".into(),
                Level::INFO => "INFO".into(),
                Level::DEBUG => "DEBUG".into(),
                Level::TRACE => "TRACE".into(),
            },
            target: target.to_string(),
            message: visitor.formatted(),
        };

        // Emit to frontend if available
        if let Some(handle) = APP_HANDLE.get() {
            let _ = handle.emit("log-line", &line);
        }

        self.buffer.push(line);
    }
}
