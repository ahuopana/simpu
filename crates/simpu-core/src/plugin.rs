//! Host side of the plugin protocol: spawns a plugin executable and exposes it
//! as a [`CredentialProvider`].

use crate::credentials::{CredentialError, CredentialProvider, Secret};
use crate::model::SecretKind;
use simpu_plugin_api::*;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

struct Io {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: u64,
}

pub struct PluginProvider {
    info: Info,
    io: Mutex<Io>,
}

fn other(e: impl std::fmt::Display) -> CredentialError {
    CredentialError::Other(e.to_string())
}

impl Io {
    fn call(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<Response, CredentialError> {
        self.next_id += 1;
        let req = Request::new(self.next_id, method, params);
        writeln!(
            self.stdin,
            "{}",
            serde_json::to_string(&req).map_err(other)?
        )
        .map_err(other)?;
        self.stdin.flush().map_err(other)?;
        let mut line = String::new();
        if self.stdout.read_line(&mut line).map_err(other)? == 0 {
            return Err(CredentialError::Other("plugin exited".into()));
        }
        let resp: Response = serde_json::from_str(&line).map_err(other)?;
        if resp.id != req.id {
            return Err(CredentialError::Other("plugin response id mismatch".into()));
        }
        Ok(resp)
    }
}

impl PluginProvider {
    /// Start `exe` with extra environment variables (e.g. `BW_SESSION`) and handshake.
    pub fn spawn(exe: &Path, env: &[(&str, &str)]) -> Result<Self, CredentialError> {
        let mut child = Command::new(exe)
            .envs(env.iter().copied())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(other)?;
        let stdin = child.stdin.take().ok_or_else(|| other("no stdin"))?;
        let stdout = BufReader::new(child.stdout.take().ok_or_else(|| other("no stdout"))?);
        let mut io = Io {
            child,
            stdin,
            stdout,
            next_id: 0,
        };
        let resp = io.call(METHOD_INITIALIZE, serde_json::Value::Null)?;
        let info: Info =
            serde_json::from_value(resp.result.ok_or_else(|| other("initialize failed"))?)
                .map_err(other)?;
        if info.protocol != PROTOCOL_VERSION {
            return Err(other(format!(
                "plugin protocol {} unsupported",
                info.protocol
            )));
        }
        Ok(Self {
            info,
            io: Mutex::new(io),
        })
    }

    pub fn info(&self) -> &Info {
        &self.info
    }
}

impl CredentialProvider for PluginProvider {
    fn id(&self) -> &str {
        &self.info.id
    }

    fn get(&self, item: &str, kind: SecretKind) -> Result<Secret, CredentialError> {
        let kind = match kind {
            SecretKind::Password => "password",
            SecretKind::KeyFile => "keyfile",
        };
        let params = serde_json::to_value(GetParams {
            item: item.into(),
            kind: kind.into(),
        })
        .map_err(other)?;
        let resp = self
            .io
            .lock()
            .map_err(|_| other("plugin lock poisoned"))?
            .call(METHOD_CREDENTIAL_GET, params)?;
        if let Some(e) = resp.error {
            return Err(match e.code {
                ERR_LOCKED => CredentialError::Locked,
                ERR_NOT_FOUND => CredentialError::NotFound(item.into()),
                _ => CredentialError::Other(e.message),
            });
        }
        let r: GetResult =
            serde_json::from_value(resp.result.ok_or_else(|| other("empty result"))?)
                .map_err(other)?;
        Ok(Secret::new(r.secret))
    }
}

impl Drop for PluginProvider {
    fn drop(&mut self) {
        if let Ok(io) = self.io.get_mut() {
            let _ = io.child.kill();
            let _ = io.child.wait();
        }
    }
}
