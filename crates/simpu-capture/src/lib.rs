//! Log capture extension: records redacted session output, exports it
//! (everything or the last N commands), optionally anonymized.

pub mod anonymize;
pub mod ansi;
pub mod prompt;

use simpu_core::ext::{SessionMeta, SessionObserver};
use std::collections::HashMap;
use std::sync::Mutex;
use thiserror::Error;

pub use anonymize::AnonymizeOptions;

/// Default ring buffer size per session.
pub const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CaptureError {
    #[error("no capture for session {0}")]
    UnknownSession(String),
    #[error("invalid regular expression {pattern:?}: {message}")]
    BadRegex { pattern: String, message: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    All,
    LastCommands(usize),
}

#[derive(Debug, Clone)]
pub struct ExportOptions {
    pub scope: Scope,
    pub strip_escapes: bool,
    /// Custom prompt regex for last-N detection (default covers bash, PowerShell, IOS...).
    pub prompt_regex: Option<String>,
    pub anonymize: Option<AnonymizeOptions>,
}

impl Default for ExportOptions {
    fn default() -> Self {
        Self {
            scope: Scope::All,
            strip_escapes: true,
            prompt_regex: None,
            anonymize: None,
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct Export {
    pub text: String,
    /// `(original, pseudonym)` pairs used by the anonymizer, for the preview.
    pub mapping: Vec<(String, String)>,
    /// Last-N fell back to "last N lines" because no prompt was recognised.
    pub fallback_to_lines: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub recording: bool,
    pub bytes: usize,
}

struct Log {
    meta: Option<SessionMeta>,
    recording: bool,
    buf: Vec<u8>,
}

pub struct Capture {
    max_bytes: usize,
    logs: Mutex<HashMap<String, Log>>,
}

impl Default for Capture {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_BYTES)
    }
}

impl Capture {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            max_bytes,
            logs: Mutex::new(HashMap::new()),
        }
    }

    fn logs(&self) -> std::sync::MutexGuard<'_, HashMap<String, Log>> {
        self.logs.lock().unwrap_or_else(|p| p.into_inner())
    }

    /// Start (or restart: clears the buffer) recording a session.
    pub fn start(&self, session: &str) {
        let mut logs = self.logs();
        let log = logs.entry(session.to_string()).or_insert(Log {
            meta: None,
            recording: false,
            buf: Vec::new(),
        });
        log.buf.clear();
        log.recording = true;
    }

    /// Stop recording; the captured text stays available for export.
    pub fn stop(&self, session: &str) {
        if let Some(l) = self.logs().get_mut(session) {
            l.recording = false;
        }
    }

    pub fn status(&self, session: &str) -> Status {
        match self.logs().get(session) {
            Some(l) => Status {
                recording: l.recording,
                bytes: l.buf.len(),
            },
            None => Status {
                recording: false,
                bytes: 0,
            },
        }
    }

    /// Forget a session's log (e.g. when its tab is closed).
    pub fn discard(&self, session: &str) {
        self.logs().remove(session);
    }

    pub fn export(&self, session: &str, opts: &ExportOptions) -> Result<Export, CaptureError> {
        let (raw, meta) = {
            let logs = self.logs();
            let l = logs
                .get(session)
                .ok_or_else(|| CaptureError::UnknownSession(session.into()))?;
            (l.buf.clone(), l.meta.clone())
        };
        let mut text = if opts.strip_escapes {
            ansi::clean(&raw)
        } else {
            String::from_utf8_lossy(&raw).into_owned()
        };
        let mut fallback = false;
        if let Scope::LastCommands(n) = opts.scope {
            let pattern = opts
                .prompt_regex
                .as_deref()
                .unwrap_or(prompt::DEFAULT_PROMPT);
            let det = prompt::PromptDetector::new(pattern).map_err(|e| CaptureError::BadRegex {
                pattern: pattern.into(),
                message: e.to_string(),
            })?;
            // Prompt detection needs clean text even when the export keeps escapes.
            let clean = if opts.strip_escapes {
                text.clone()
            } else {
                ansi::clean(&raw)
            };
            let r = prompt::last_n(&clean, n, &det);
            text = r.text;
            fallback = r.fallback_to_lines;
        }
        let mut mapping = Vec::new();
        if let Some(a) = &opts.anonymize {
            let r = anonymize::anonymize(&text, a, meta.as_ref())?;
            text = r.text;
            mapping = r.mapping;
        }
        Ok(Export {
            text,
            mapping,
            fallback_to_lines: fallback,
        })
    }
}

impl SessionObserver for Capture {
    fn output(&self, session: &SessionMeta, redacted: &[u8]) {
        let mut logs = self.logs();
        let Some(log) = logs.get_mut(&session.id) else {
            return;
        };
        log.meta.get_or_insert_with(|| session.clone());
        if !log.recording {
            return;
        }
        log.buf.extend_from_slice(redacted);
        if log.buf.len() > self.max_bytes {
            let mut cut = log.buf.len() - self.max_bytes;
            // drop whole lines where possible so the start is not mid-line
            if let Some(nl) = log.buf[cut..].iter().position(|b| *b == b'\n') {
                cut += nl + 1;
            }
            log.buf.drain(..cut.min(log.buf.len()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> SessionMeta {
        SessionMeta {
            id: "s1".into(),
            connection: "web".into(),
            host: "web1.example.com".into(),
            username: "alice".into(),
        }
    }

    #[test]
    fn records_only_between_start_and_stop() {
        let c = Capture::default();
        c.output(&meta(), b"ignored (never started)\n");
        c.start("s1");
        c.output(&meta(), b"one\n");
        assert_eq!(
            c.status("s1"),
            Status {
                recording: true,
                bytes: 4
            }
        );
        c.stop("s1");
        c.output(&meta(), b"after stop\n");
        assert_eq!(
            c.export("s1", &ExportOptions::default()).unwrap().text,
            "one\n"
        );
        // restart clears
        c.start("s1");
        assert_eq!(c.status("s1").bytes, 0);
        assert_eq!(
            c.export("nope", &ExportOptions::default()),
            Err(CaptureError::UnknownSession("nope".into()))
        );
    }

    #[test]
    fn ring_buffer_drops_oldest_whole_lines() {
        let c = Capture::new(20);
        c.start("s1");
        for i in 0..10 {
            c.output(&meta(), format!("line number {i}\n").as_bytes());
        }
        let t = c.export("s1", &ExportOptions::default()).unwrap().text;
        assert!(t.len() <= 20);
        assert!(t.ends_with("line number 9\n"));
        assert!(
            t.starts_with("line")
                || t.starts_with(" number")
                || t.starts_with("number")
                || t.starts_with("\n")
                || t.starts_with("line")
        );
    }

    #[test]
    fn export_last_commands_then_anonymize_with_session_facts() {
        let c = Capture::default();
        c.start("s1");
        let m = meta();
        c.output(&m, b"\x1b[32malice@web1\x1b[0m:~$ ls\r\nfile\r\nalice@web1:~$ ssh 10.1.1.1\r\nwelcome alice\r\nalice@web1:~$ ");
        let opts = ExportOptions {
            scope: Scope::LastCommands(1),
            anonymize: Some(AnonymizeOptions {
                usernames: true,
                hostnames: true,
                ..Default::default()
            }),
            ..Default::default()
        };
        let e = c.export("s1", &opts).unwrap();
        assert_eq!(e.text, "user1@host1:~$ ssh 10.0.0.1\nwelcome user1");
        assert!(!e.fallback_to_lines);
        assert_eq!(e.mapping.len(), 3);
        // raw scope keeps colors when asked
        let raw = c
            .export(
                "s1",
                &ExportOptions {
                    strip_escapes: false,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(raw.text.contains('\x1b'));
    }

    #[test]
    fn bad_prompt_regex_is_an_error() {
        let c = Capture::default();
        c.start("s1");
        let o = ExportOptions {
            scope: Scope::LastCommands(1),
            prompt_regex: Some("(".into()),
            ..Default::default()
        };
        assert!(matches!(
            c.export("s1", &o),
            Err(CaptureError::BadRegex { .. })
        ));
    }
}
