//! Extension hooks. Extensions observe session output that the host has
//! already redacted (see [`crate::redact`]); they never see raw output.

use std::sync::{Arc, Mutex};

/// Non-secret facts about a session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionMeta {
    pub id: String,
    pub connection: String,
    pub host: String,
    pub username: String,
}

pub trait SessionObserver: Send + Sync {
    /// Redacted output bytes of a session (stdout/stderr merged), in order.
    fn output(&self, session: &SessionMeta, redacted: &[u8]);
    /// The session ended.
    fn closed(&self, _session: &SessionMeta) {}
}

#[derive(Default)]
pub struct ExtensionHost {
    observers: Mutex<Vec<Arc<dyn SessionObserver>>>,
}

impl ExtensionHost {
    pub fn register(&self, o: Arc<dyn SessionObserver>) {
        self.observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(o);
    }

    fn all(&self) -> Vec<Arc<dyn SessionObserver>> {
        self.observers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn output(&self, meta: &SessionMeta, redacted: &[u8]) {
        if redacted.is_empty() {
            return;
        }
        for o in self.all() {
            o.output(meta, redacted);
        }
    }

    pub fn closed(&self, meta: &SessionMeta) {
        for o in self.all() {
            o.closed(meta);
        }
    }
}
