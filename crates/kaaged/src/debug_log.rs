use kaage_proto::{DebugLogEntry, DebugLogLevel};
use std::{
    collections::VecDeque,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::sync::broadcast;
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::{Layer, layer::Context};

const MAX_DEBUG_ENTRIES: usize = 2000;

pub struct DebugLogHub {
    entries: Mutex<VecDeque<DebugLogEntry>>,
    tx: broadcast::Sender<DebugLogEntry>,
    next_seq: AtomicU64,
}

impl DebugLogHub {
    pub fn new() -> Arc<Self> {
        let (tx, _) = broadcast::channel(512);
        Arc::new(Self {
            entries: Mutex::new(VecDeque::with_capacity(MAX_DEBUG_ENTRIES)),
            tx,
            next_seq: AtomicU64::new(1),
        })
    }

    pub fn push(&self, level: DebugLogLevel, target: &str, message: String) -> DebugLogEntry {
        let seq = self.next_seq.fetch_add(1, Ordering::Relaxed);
        let ts_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;
        let module = normalize_module_name(target);
        let entry = DebugLogEntry {
            seq,
            ts_ms,
            level,
            module,
            target: target.to_string(),
            message,
        };
        {
            let mut guard = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if guard.len() >= MAX_DEBUG_ENTRIES {
                guard.pop_front();
            }
            guard.push_back(entry.clone());
        }
        let _ = self.tx.send(entry.clone());
        entry
    }

    pub fn snapshot(&self) -> Vec<DebugLogEntry> {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned()
            .collect()
    }

    pub fn clear(&self) {
        self.entries
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }

    pub fn subscribe(&self) -> broadcast::Receiver<DebugLogEntry> {
        self.tx.subscribe()
    }
}

pub fn normalize_module_name(target: &str) -> String {
    let trimmed = target.trim();
    if let Some(rest) = trimmed.strip_prefix("kaaged::") {
        return rest
            .split("::")
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("kaaged")
            .to_string();
    }
    if trimmed == "kaaged" {
        return "kaaged".to_string();
    }
    if trimmed.starts_with("pi_durable") {
        return "pi_durable".to_string();
    }
    if trimmed.starts_with("tower_http") || trimmed.starts_with("axum") {
        return "http".to_string();
    }
    trimmed
        .split("::")
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("system")
        .to_string()
}

pub struct DebugLogLayer {
    hub: Arc<DebugLogHub>,
}

impl DebugLogLayer {
    pub fn new(hub: Arc<DebugLogHub>) -> Self {
        Self { hub }
    }
}

struct MessageVisitor {
    message: Option<String>,
    fields: Vec<String>,
}

impl tracing::field::Visit for MessageVisitor {
    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            self.message = Some(format!("{value:?}"));
        } else {
            self.fields.push(format!("{}={value:?}", field.name()));
        }
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        if field.name() == "message" {
            self.message = Some(value.to_string());
        } else {
            self.fields.push(format!("{}={value}", field.name()));
        }
    }
}

impl<S: Subscriber> Layer<S> for DebugLogLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let meta = event.metadata();
        let level = match *meta.level() {
            Level::ERROR => DebugLogLevel::Error,
            Level::WARN => DebugLogLevel::Warn,
            Level::INFO => DebugLogLevel::Info,
            Level::DEBUG => DebugLogLevel::Debug,
            Level::TRACE => DebugLogLevel::Trace,
        };
        let mut visitor = MessageVisitor {
            message: None,
            fields: Vec::new(),
        };
        event.record(&mut visitor);
        let msg = match (visitor.message, visitor.fields.is_empty()) {
            (Some(m), true) => m,
            (Some(m), false) => format!("{m} ({})", visitor.fields.join(", ")),
            (None, false) => visitor.fields.join(", "),
            (None, true) => meta.name().to_string(),
        };
        self.hub.push(level, meta.target(), msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_kaage_module_names_and_ring_buffer() {
        assert_eq!(normalize_module_name("kaaged::copilot"), "copilot");
        assert_eq!(normalize_module_name("kaaged::vitals::psi"), "vitals");
        assert_eq!(normalize_module_name("pi_durable::ai::http"), "pi_durable");
        assert_eq!(normalize_module_name("tower_http::trace"), "http");
        assert_eq!(normalize_module_name("kaaged"), "kaaged");

        let hub = DebugLogHub::new();
        let mut rx = hub.subscribe();
        let e = hub.push(DebugLogLevel::Warn, "kaaged::copilot", "test warn".into());
        assert_eq!(e.module, "copilot");
        assert_eq!(hub.snapshot().len(), 1);
        let received = rx.try_recv().expect("broadcast entry");
        assert_eq!(received.message, "test warn");
        hub.clear();
        assert!(hub.snapshot().is_empty());
    }
}
