//! KeePassXC credential provider. Talks to `keepassxc-cli`.
//!
//! - Configuration (environment): `KEEPASSXC_DB` (database path, required),
//!   `KEEPASSXC_KEYFILE` (optional key file), `KEEPASSXC_CLI` (executable, default
//!   `keepassxc-cli`).
//! - The master password arrives through `vault.unlock`, is kept in memory only
//!   (zeroized on drop) and is given to the CLI on **stdin**, never on the
//!   command line or in the environment.
//! - Item syntax: `Group/Entry` (entry path). `password` secrets read the
//!   `Password` attribute. `keyfile` secrets read an attachment when the item is
//!   `Group/Entry#attachment-name`, otherwise the entry's `Notes`.

use simpu_plugin_api::*;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::Mutex;
use zeroize::Zeroizing;

/// What the plugin asks of the CLI. Faked in unit tests.
pub trait KpCli {
    /// `args` are CLI arguments after the database options; `password` goes to stdin.
    fn run(&self, args: &[&str], password: &str) -> Result<String, String>;
}

pub struct RealCli {
    pub bin: String,
}

impl RealCli {
    pub fn from_env() -> Self {
        Self {
            bin: std::env::var("KEEPASSXC_CLI").unwrap_or_else(|_| "keepassxc-cli".into()),
        }
    }
}

impl KpCli for RealCli {
    fn run(&self, args: &[&str], password: &str) -> Result<String, String> {
        let mut child = Command::new(&self.bin)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot run {}: {e}", self.bin))?;
        if let Some(mut stdin) = child.stdin.take() {
            let _ = writeln!(stdin, "{password}");
        }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout)
                .trim_end_matches(['\r', '\n'])
                .to_string())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            // keepassxc-cli prints the password prompt on stderr; keep only the real message.
            Err(err
                .lines()
                .filter(|l| !l.starts_with("Enter password"))
                .collect::<Vec<_>>()
                .join(" ")
                .trim()
                .to_string())
        }
    }
}

pub struct KeePassXc<C: KpCli> {
    pub cli: C,
    db: String,
    keyfile: Option<String>,
    password: Mutex<Option<Zeroizing<String>>>,
}

impl<C: KpCli> KeePassXc<C> {
    pub fn new(cli: C, db: String, keyfile: Option<String>) -> Self {
        Self {
            cli,
            db,
            keyfile,
            password: Mutex::new(None),
        }
    }

    pub fn from_env(cli: C) -> Self {
        Self::new(
            cli,
            std::env::var("KEEPASSXC_DB").unwrap_or_default(),
            std::env::var("KEEPASSXC_KEYFILE")
                .ok()
                .filter(|k| !k.is_empty()),
        )
    }

    fn run(&self, command: &[&str], tail: &[&str], password: &str) -> Result<String, String> {
        let mut args: Vec<&str> = command.to_vec();
        args.push("-q");
        if let Some(k) = &self.keyfile {
            args.extend(["-k", k]);
        }
        args.push(&self.db);
        args.extend_from_slice(tail);
        self.cli.run(&args, password)
    }
}

fn map_error(item: &str, e: String) -> RpcError {
    let l = e.to_lowercase();
    if l.contains("invalid credentials") || l.contains("wrong key") || l.contains("invalid key") {
        RpcError::new(ERR_BAD_PASSWORD, "Invalid master password")
    } else if l.contains("could not find") || l.contains("not found") {
        RpcError::new(ERR_NOT_FOUND, format!("entry {item} not found"))
    } else {
        RpcError::new(ERR_OTHER, e)
    }
}

impl<C: KpCli> Plugin for KeePassXc<C> {
    fn info(&self) -> Info {
        Info {
            id: "keepassxc".into(),
            name: "KeePassXC".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            protocol: PROTOCOL_VERSION,
            capabilities: vec!["credentials".into(), "unlock".into()],
        }
    }

    fn credential_get(&self, p: GetParams) -> Result<GetResult, RpcError> {
        if self.db.is_empty() {
            return Err(RpcError::new(ERR_OTHER, "KEEPASSXC_DB is not set"));
        }
        let guard = self.password.lock().unwrap_or_else(|e| e.into_inner());
        let Some(pw) = guard.as_ref() else {
            return Err(RpcError::new(ERR_LOCKED, "KeePassXC database is locked"));
        };
        let (entry, attachment) = match p.item.split_once('#') {
            Some((e, a)) => (e, Some(a)),
            None => (p.item.as_str(), None),
        };
        let result = match (p.kind.as_str(), attachment) {
            ("password", _) => self.run(&["show", "-s", "-a", "Password"], &[entry], pw),
            ("keyfile", Some(att)) => {
                self.run(&["attachment-export", "--stdout"], &[entry, att], pw)
            }
            ("keyfile", None) => self.run(&["show", "-s", "-a", "Notes"], &[entry], pw),
            (other, _) => {
                return Err(RpcError::new(
                    ERR_INVALID_PARAMS,
                    format!("unknown kind {other}"),
                ))
            }
        };
        match result {
            Ok(s) if s.is_empty() => Err(RpcError::new(
                ERR_NOT_FOUND,
                format!("entry {} has no such secret", p.item),
            )),
            Ok(secret) => Ok(GetResult { secret }),
            Err(e) => Err(map_error(&p.item, e)),
        }
    }

    fn unlock(&self, password: &str) -> Result<(), RpcError> {
        if self.db.is_empty() {
            return Err(RpcError::new(ERR_OTHER, "KEEPASSXC_DB is not set"));
        }
        // `ls` opens the database, which validates the password (and key file).
        self.run(&["ls"], &[], password)
            .map_err(|e| map_error("database", e))?;
        *self.password.lock().unwrap_or_else(|e| e.into_inner()) =
            Some(Zeroizing::new(password.to_string()));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    struct Fake {
        calls: RefCell<Vec<(Vec<String>, String)>>,
    }

    impl KpCli for Fake {
        fn run(&self, args: &[&str], password: &str) -> Result<String, String> {
            self.calls.borrow_mut().push((
                args.iter().map(|s| s.to_string()).collect(),
                password.to_string(),
            ));
            if password != "master" {
                return Err(
                    "Error while reading the database: Invalid credentials were provided".into(),
                );
            }
            match args {
                ["ls", ..] => Ok(String::new()),
                ["show", "-s", "-a", "Password", "-q", .., "Servers/web"] => Ok("pw-web".into()),
                ["show", "-s", "-a", "Notes", "-q", .., "Servers/key"] => Ok("PEM".into()),
                ["attachment-export", "--stdout", "-q", .., "Servers/key", "id_rsa"] => {
                    Ok("ATTACHED".into())
                }
                ["show", "-s", "-a", "Password", "-q", .., "Empty"] => Ok(String::new()),
                _ => Err("Could not find entry with path nope.".into()),
            }
        }
    }

    fn kp() -> KeePassXc<Fake> {
        KeePassXc::new(
            Fake {
                calls: RefCell::new(vec![]),
            },
            "/db.kdbx".into(),
            Some("/k.key".into()),
        )
    }

    fn get(item: &str, kind: &str) -> GetParams {
        GetParams {
            item: item.into(),
            kind: kind.into(),
        }
    }

    #[test]
    fn locked_until_unlocked_then_returns_secrets() {
        let k = kp();
        assert_eq!(
            k.credential_get(get("Servers/web", "password"))
                .unwrap_err()
                .code,
            ERR_LOCKED
        );
        assert!(k.cli.calls.borrow().is_empty(), "no CLI call while locked");
        k.unlock("master").unwrap();
        assert_eq!(
            k.credential_get(get("Servers/web", "password"))
                .unwrap()
                .secret,
            "pw-web"
        );
        assert_eq!(
            k.credential_get(get("Servers/key", "keyfile"))
                .unwrap()
                .secret,
            "PEM"
        );
        assert_eq!(
            k.credential_get(get("Servers/key#id_rsa", "keyfile"))
                .unwrap()
                .secret,
            "ATTACHED"
        );
    }

    #[test]
    fn password_goes_to_stdin_and_key_file_is_passed() {
        let k = kp();
        k.unlock("master").unwrap();
        let calls = k.cli.calls.borrow();
        let (args, stdin) = &calls[0];
        assert_eq!(stdin, "master");
        assert_eq!(args.join(" "), "ls -q -k /k.key /db.kdbx");
        assert!(
            !args.iter().any(|a| a.contains("master")),
            "never on the command line"
        );
    }

    #[test]
    fn wrong_password_missing_entry_empty_secret_and_bad_kind() {
        let k = kp();
        assert_eq!(k.unlock("nope").unwrap_err().code, ERR_BAD_PASSWORD);
        assert_eq!(
            k.credential_get(get("Servers/web", "password"))
                .unwrap_err()
                .code,
            ERR_LOCKED,
            "failed unlock keeps it locked"
        );
        k.unlock("master").unwrap();
        assert_eq!(
            k.credential_get(get("nope", "password")).unwrap_err().code,
            ERR_NOT_FOUND
        );
        assert_eq!(
            k.credential_get(get("Empty", "password")).unwrap_err().code,
            ERR_NOT_FOUND
        );
        assert_eq!(
            k.credential_get(get("Servers/web", "totp"))
                .unwrap_err()
                .code,
            ERR_INVALID_PARAMS
        );
    }

    #[test]
    fn missing_database_setting_is_reported() {
        let k = KeePassXc::new(
            Fake {
                calls: RefCell::new(vec![]),
            },
            String::new(),
            None,
        );
        assert_eq!(k.unlock("master").unwrap_err().code, ERR_OTHER);
    }
}
