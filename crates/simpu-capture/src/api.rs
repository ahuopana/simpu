//! Wire types for the UI (camelCase JSON) and helpers used by the app shell.

use crate::{AnonymizeOptions, Capture, CaptureError, ExportOptions, Scope};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AnonymizeRequest {
    pub usernames: bool,
    pub hostnames: bool,
    pub secrets: bool,
    /// One regular expression per entry.
    pub custom: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ExportRequest {
    /// `all` or `last`.
    pub scope: String,
    pub last_n: usize,
    pub strip_escapes: bool,
    pub prompt_regex: Option<String>,
    pub anonymize: Option<AnonymizeRequest>,
}

impl Default for ExportRequest {
    fn default() -> Self {
        Self {
            scope: "all".into(),
            last_n: 1,
            strip_escapes: true,
            prompt_regex: None,
            anonymize: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ExportResponse {
    pub text: String,
    pub mapping: Vec<(String, String)>,
    pub fallback_to_lines: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StatusResponse {
    pub recording: bool,
    pub bytes: usize,
}

/// Names known from the connection tree, so they are anonymized even if the
/// session itself never printed them as its own user/host.
#[derive(Debug, Clone, Default)]
pub struct KnownNames {
    pub usernames: Vec<String>,
    pub hostnames: Vec<String>,
}

impl ExportRequest {
    pub fn into_options(self, known: &KnownNames) -> ExportOptions {
        ExportOptions {
            scope: if self.scope == "last" {
                Scope::LastCommands(self.last_n.max(1))
            } else {
                Scope::All
            },
            strip_escapes: self.strip_escapes,
            prompt_regex: self.prompt_regex.filter(|p| !p.trim().is_empty()),
            anonymize: self.anonymize.map(|a| AnonymizeOptions {
                usernames: a.usernames,
                hostnames: a.hostnames,
                secrets: a.secrets,
                extra_usernames: known.usernames.clone(),
                extra_hostnames: known.hostnames.clone(),
                custom: a
                    .custom
                    .into_iter()
                    .filter(|c| !c.trim().is_empty())
                    .collect(),
            }),
        }
    }
}

impl Capture {
    pub fn export_request(
        &self,
        session: &str,
        req: ExportRequest,
        known: &KnownNames,
    ) -> Result<ExportResponse, CaptureError> {
        let e = self.export(session, &req.into_options(known))?;
        Ok(ExportResponse {
            text: e.text,
            mapping: e.mapping,
            fallback_to_lines: e.fallback_to_lines,
        })
    }

    pub fn status_response(&self, session: &str) -> StatusResponse {
        let s = self.status(session);
        StatusResponse {
            recording: s.recording,
            bytes: s.bytes,
        }
    }
}

/// Write via a temp file and rename so a crash never leaves a half-written export.
pub fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = std::path::PathBuf::from(tmp);
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// `None` if the pattern compiles, otherwise the error message.
pub fn regex_error(pattern: &str) -> Option<String> {
    crate::anonymize::validate_regex(pattern)
        .err()
        .map(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use simpu_core::ext::{SessionMeta, SessionObserver};

    #[test]
    fn request_json_maps_to_options_and_runs() {
        let req: ExportRequest = serde_json::from_str(
            r#"{"scope":"last","lastN":1,"anonymize":{"usernames":true,"custom":["", "ACME-\\d+"]}}"#,
        )
        .unwrap();
        assert!(req.strip_escapes, "default");
        let known = KnownNames {
            usernames: vec!["bob".into()],
            hostnames: vec![],
        };
        let o = req.clone().into_options(&known);
        assert_eq!(o.scope, Scope::LastCommands(1));
        let a = o.anonymize.unwrap();
        assert_eq!(
            (a.custom.len(), a.extra_usernames.clone()),
            (1, vec!["bob".to_string()])
        );

        let c = Capture::default();
        c.start("s");
        let m = SessionMeta {
            id: "s".into(),
            connection: "c".into(),
            host: "h".into(),
            username: "alice".into(),
        };
        c.output(&m, b"bob@h:~$ echo ACME-7\nACME-7\nbob@h:~$ ");
        let r = c.export_request("s", req, &known).unwrap();
        assert_eq!(r.text, "user2@h:~$ echo <rule1-1>\n<rule1-1>");
        assert!(serde_json::to_string(&r)
            .unwrap()
            .contains("fallbackToLines"));
    }

    #[test]
    fn defaults_and_helpers() {
        let r: ExportRequest = serde_json::from_str("{}").unwrap();
        assert_eq!((r.scope.as_str(), r.last_n), ("all", 1));
        assert!(regex_error("a+").is_none());
        assert!(regex_error("(").is_some());
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("out.log");
        write_atomic(&p, "hello").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "hello");
        assert!(!d.path().join("out.log.tmp").exists());
    }
}
