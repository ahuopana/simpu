//! Bitwarden credential provider. Talks to the official `bw` CLI.
//!
//! - Session: `BW_SESSION` must be set in the environment (the user runs
//!   `bw unlock` and Simpu passes the token on; the master password is never
//!   handled by this plugin).
//! - `password` secrets: `bw get password <item>`.
//! - `keyfile` secrets: private key stored in the item's notes:
//!   `bw get notes <item>`.
//! - `BW_BIN` overrides the executable (used by tests).

use simpu_plugin_api::*;
use std::process::Command;

/// Runs `bw` with arguments; returns stdout on success or stderr on failure.
pub trait BwRunner {
    fn run(&self, args: &[&str]) -> Result<String, String>;
}

pub struct CliRunner {
    pub bin: String,
}

impl CliRunner {
    pub fn from_env() -> Self {
        Self {
            bin: std::env::var("BW_BIN").unwrap_or_else(|_| "bw".into()),
        }
    }
}

impl BwRunner for CliRunner {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new(&self.bin)
            .args(args)
            .output()
            .map_err(|e| format!("cannot run {}: {e}", self.bin))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout)
                .trim_end_matches(['\r', '\n'])
                .to_string())
        } else {
            Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
        }
    }
}

pub struct Bitwarden<R: BwRunner> {
    pub runner: R,
}

impl<R: BwRunner> Bitwarden<R> {
    fn is_unlocked(&self) -> Result<bool, RpcError> {
        let status = self
            .runner
            .run(&["status"])
            .map_err(|e| RpcError::new(ERR_OTHER, e))?;
        Ok(status.contains("\"status\":\"unlocked\""))
    }
}

impl<R: BwRunner> Plugin for Bitwarden<R> {
    fn info(&self) -> Info {
        Info {
            id: "bitwarden".into(),
            name: "Bitwarden".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: PROTOCOL_VERSION,
            capabilities: vec!["credentials".into()],
        }
    }

    fn credential_get(&self, p: GetParams) -> Result<GetResult, RpcError> {
        if !self.is_unlocked()? {
            return Err(RpcError::new(ERR_LOCKED, "Bitwarden vault is locked"));
        }
        let field = match p.kind.as_str() {
            "password" => "password",
            "keyfile" => "notes",
            other => {
                return Err(RpcError::new(
                    ERR_INVALID_PARAMS,
                    format!("unknown kind {other}"),
                ))
            }
        };
        match self.runner.run(&["get", field, &p.item]) {
            Ok(s) if s.is_empty() => Err(RpcError::new(
                ERR_NOT_FOUND,
                format!("item {} has no {field}", p.item),
            )),
            Ok(secret) => Ok(GetResult { secret }),
            Err(e) if e.to_lowercase().contains("not found") => Err(RpcError::new(
                ERR_NOT_FOUND,
                format!("item {} not found", p.item),
            )),
            Err(e) => Err(RpcError::new(ERR_OTHER, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        status: &'static str,
        calls: RefCell<Vec<String>>,
    }
    impl BwRunner for Fake {
        fn run(&self, args: &[&str]) -> Result<String, String> {
            self.calls.borrow_mut().push(args.join(" "));
            match args {
                ["status"] => Ok(format!("{{\"status\":\"{}\"}}", self.status)),
                ["get", "password", "pw1"] => Ok("s3cret".into()),
                ["get", "notes", "key1"] => Ok("-----BEGIN KEY-----".into()),
                ["get", "password", "empty"] => Ok(String::new()),
                _ => Err("Not found.".into()),
            }
        }
    }

    fn bw(status: &'static str) -> Bitwarden<Fake> {
        Bitwarden {
            runner: Fake {
                status,
                calls: RefCell::new(vec![]),
            },
        }
    }
    fn get(item: &str, kind: &str) -> GetParams {
        GetParams {
            item: item.into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn password_and_keyfile() {
        let b = bw("unlocked");
        assert_eq!(
            b.credential_get(get("pw1", "password")).unwrap().secret,
            "s3cret"
        );
        assert_eq!(
            b.credential_get(get("key1", "keyfile")).unwrap().secret,
            "-----BEGIN KEY-----"
        );
    }

    #[test]
    fn locked_vault_never_fetches() {
        let b = bw("locked");
        assert_eq!(
            b.credential_get(get("pw1", "password")).unwrap_err().code,
            ERR_LOCKED
        );
        assert_eq!(b.runner.calls.borrow().as_slice(), ["status"]);
    }

    #[test]
    fn missing_empty_and_bad_kind() {
        let b = bw("unlocked");
        assert_eq!(
            b.credential_get(get("zzz", "password")).unwrap_err().code,
            ERR_NOT_FOUND
        );
        assert_eq!(
            b.credential_get(get("empty", "password")).unwrap_err().code,
            ERR_NOT_FOUND
        );
        assert_eq!(
            b.credential_get(get("pw1", "totp")).unwrap_err().code,
            ERR_INVALID_PARAMS
        );
    }
}
