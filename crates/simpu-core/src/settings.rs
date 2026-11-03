//! User settings (`settings.toml` in the config directory). Every field has a
//! default so the file may be missing or partial.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Settings {
    pub keepassxc: KeePassXcSettings,
    pub bitwarden: BitwardenSettings,
    pub network: NetworkSettings,
    pub reconnect: ReconnectSettings,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct KeePassXcSettings {
    /// Path of the `.kdbx` database; empty = KeePassXC support off.
    pub database: String,
    /// Optional key file path.
    pub keyfile: String,
    /// `keepassxc-cli` executable; empty = look it up in `PATH`.
    pub cli: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BitwardenSettings {
    /// `bw` executable; empty = look it up in `PATH`.
    pub cli: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkSettings {
    /// Seconds between SSH keepalives; 0 disables them.
    pub keepalive_secs: u64,
    /// Unanswered keepalives before the connection is considered dead.
    pub keepalive_max: usize,
    pub connect_timeout_secs: u64,
}

impl Default for NetworkSettings {
    fn default() -> Self {
        Self {
            keepalive_secs: 30,
            keepalive_max: 3,
            connect_timeout_secs: 20,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ReconnectSettings {
    /// First retry delay (doubles up to `max_secs`).
    pub base_secs: u64,
    pub max_secs: u64,
    /// Give up automatic retries after this many attempts; 0 = never.
    pub max_attempts: u32,
}

impl Default for ReconnectSettings {
    fn default() -> Self {
        Self {
            base_secs: 1,
            max_secs: 60,
            max_attempts: 0,
        }
    }
}

impl Settings {
    /// Human-readable problems; empty when the settings are usable.
    pub fn validate(&self) -> Vec<String> {
        let mut e = Vec::new();
        let n = &self.network;
        if n.keepalive_secs != 0 && !(5..=3600).contains(&n.keepalive_secs) {
            e.push("keepalive interval must be 0 (off) or 5-3600 seconds".to_string());
        }
        if !(1..=20).contains(&n.keepalive_max) {
            e.push("keepalive misses must be 1-20".into());
        }
        if !(3..=300).contains(&n.connect_timeout_secs) {
            e.push("connect timeout must be 3-300 seconds".into());
        }
        let r = &self.reconnect;
        if !(1..=600).contains(&r.base_secs) {
            e.push("first retry delay must be 1-600 seconds".into());
        }
        if r.max_secs < r.base_secs || r.max_secs > 3600 {
            e.push("longest retry delay must be between the first delay and 3600 seconds".into());
        }
        if r.max_attempts > 1000 {
            e.push("retry attempts must be 0 (unlimited) to 1000".into());
        }
        e
    }

    pub fn connect_options(&self) -> crate::ssh::ConnectOptions {
        let n = &self.network;
        crate::ssh::ConnectOptions {
            keepalive_interval: (n.keepalive_secs > 0)
                .then(|| std::time::Duration::from_secs(n.keepalive_secs)),
            keepalive_max: n.keepalive_max,
            connect_timeout: std::time::Duration::from_secs(n.connect_timeout_secs),
        }
    }

    pub fn reconnect_config(&self) -> crate::manager::ReconnectConfig {
        let r = &self.reconnect;
        crate::manager::ReconnectConfig {
            base: std::time::Duration::from_secs(r.base_secs),
            max: std::time::Duration::from_secs(r.max_secs),
            max_attempts: (r.max_attempts > 0).then_some(r.max_attempts),
        }
    }
}

pub fn load(path: &Path) -> Result<Settings, String> {
    match std::fs::read_to_string(path) {
        Ok(s) => toml::from_str(&s).map_err(|e| format!("{}: {e}", path.display())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// Atomic save (temp file + rename).
pub fn save(path: &Path, s: &Settings) -> Result<(), String> {
    let tmp = path.with_extension("tmp");
    let text = toml::to_string_pretty(s).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, text).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_valid_and_match_the_built_in_behaviour() {
        let s = Settings::default();
        assert!(s.validate().is_empty());
        let o = s.connect_options();
        assert_eq!(
            (o.keepalive_interval, o.keepalive_max),
            (Some(std::time::Duration::from_secs(30)), 3)
        );
        let r = s.reconnect_config();
        assert_eq!(
            (r.base.as_secs(), r.max.as_secs(), r.max_attempts),
            (1, 60, None)
        );
    }

    #[test]
    fn partial_files_fall_back_to_defaults_per_field() {
        let s: Settings = toml::from_str(
            "[keepassxc]\ndatabase = \"C:/vault.kdbx\"\n[network]\nkeepalive_secs = 0\n",
        )
        .unwrap();
        assert_eq!(s.keepassxc.database, "C:/vault.kdbx");
        assert_eq!(s.network.keepalive_max, 3, "untouched fields keep defaults");
        assert_eq!(
            s.connect_options().keepalive_interval,
            None,
            "0 turns keepalives off"
        );
        assert_eq!(s.reconnect, ReconnectSettings::default());
    }

    #[test]
    fn validation_reports_every_problem() {
        let mut s = Settings::default();
        s.network.keepalive_secs = 2;
        s.network.keepalive_max = 0;
        s.network.connect_timeout_secs = 1;
        s.reconnect.base_secs = 0;
        s.reconnect.max_secs = 5000;
        assert_eq!(s.validate().len(), 5);
        s = Settings::default();
        s.reconnect.base_secs = 10;
        s.reconnect.max_secs = 5;
        assert_eq!(s.validate().len(), 1);
    }

    #[test]
    fn save_load_round_trip_and_missing_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("settings.toml");
        assert_eq!(load(&p).unwrap(), Settings::default());
        let mut s = Settings::default();
        s.keepassxc.database = "/db.kdbx".into();
        s.reconnect.max_attempts = 7;
        save(&p, &s).unwrap();
        assert_eq!(load(&p).unwrap(), s);
        assert!(!p.with_extension("tmp").exists());
        std::fs::write(&p, "not = [valid").unwrap();
        assert!(load(&p).is_err());
    }
}
