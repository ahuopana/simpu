//! Bitwarden credential provider. Talks to the official `bw` CLI.
//!
//! - Session: either `BW_SESSION` is inherited from the environment, or the
//!   host sends `vault.unlock` with the master password, which is passed to
//!   `bw unlock` through an environment variable and never stored. Only the
//!   resulting session token is kept in memory.
//! - `password` secrets: `bw get password <item>`.
//! - `keyfile` secrets: private key stored in the item's notes:
//!   `bw get notes <item>`.
//! - `BW_BIN` overrides the executable (used by tests).

use simpu_plugin_api::*;
use std::process::Command;

/// Runs `bw` with arguments; returns stdout on success or stderr on failure.
pub trait BwRunner {
    /// `session` is the `BW_SESSION` token to use, if any.
    fn run(&self, args: &[&str], session: Option<&str>) -> Result<String, String>;
    /// `bw unlock --raw` with the master password; returns the session token.
    fn unlock(&self, password: &str) -> Result<String, String>;
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

fn finish(out: std::process::Output) -> Result<String, String> {
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout)
            .trim_end_matches(['\r', '\n'])
            .to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

impl BwRunner for CliRunner {
    fn run(&self, args: &[&str], session: Option<&str>) -> Result<String, String> {
        let mut cmd = Command::new(&self.bin);
        cmd.args(args);
        if let Some(s) = session {
            cmd.env("BW_SESSION", s);
        }
        finish(
            cmd.output()
                .map_err(|e| format!("cannot run {}: {e}", self.bin))?,
        )
    }

    fn unlock(&self, password: &str) -> Result<String, String> {
        // The master password goes through the environment, never the command line.
        let out = Command::new(&self.bin)
            .args(["unlock", "--passwordenv", "SIMPU_BW_PASSWORD", "--raw"])
            .env("SIMPU_BW_PASSWORD", password)
            .output()
            .map_err(|e| format!("cannot run {}: {e}", self.bin))?;
        finish(out)
    }
}

pub struct Bitwarden<R: BwRunner> {
    pub runner: R,
    /// Session token from `bw unlock`, or the inherited `BW_SESSION`.
    session: std::sync::Mutex<Option<String>>,
}

impl<R: BwRunner> Bitwarden<R> {
    pub fn new(runner: R) -> Self {
        Self {
            runner,
            session: std::sync::Mutex::new(
                std::env::var("BW_SESSION").ok().filter(|s| !s.is_empty()),
            ),
        }
    }

    fn session(&self) -> Option<String> {
        self.session
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn is_unlocked(&self) -> Result<bool, RpcError> {
        let status = self
            .runner
            .run(&["status"], self.session().as_deref())
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
            capabilities: vec!["credentials".into(), "unlock".into()],
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
        match self
            .runner
            .run(&["get", field, &p.item], self.session().as_deref())
        {
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

    fn unlock(&self, password: &str) -> Result<(), RpcError> {
        match self.runner.unlock(password) {
            Ok(token) if !token.is_empty() => {
                *self.session.lock().unwrap_or_else(|p| p.into_inner()) = Some(token);
                Ok(())
            }
            Ok(_) => Err(RpcError::new(ERR_OTHER, "bw returned no session token")),
            Err(e) if e.to_lowercase().contains("invalid master password") => {
                Err(RpcError::new(ERR_BAD_PASSWORD, "Invalid master password"))
            }
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
        fn unlock(&self, password: &str) -> Result<String, String> {
            if password == "master" {
                Ok("tok".into())
            } else {
                Err("Invalid master password.".into())
            }
        }
        fn run(&self, args: &[&str], _session: Option<&str>) -> Result<String, String> {
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
        Bitwarden::new(Fake {
            status,
            calls: RefCell::new(vec![]),
        })
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

    #[test]
    fn unlock_success_and_wrong_password() {
        let b = bw("locked");
        assert_eq!(b.unlock("nope").unwrap_err().code, ERR_BAD_PASSWORD);
        assert!(b.session().is_none());
        b.unlock("master").unwrap();
        assert_eq!(b.session().as_deref(), Some("tok"));
    }
}
