//! Host key verification against a known_hosts-style store.

use std::collections::HashMap;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Key matches the stored one.
    Trusted,
    /// Host never seen: ask the user, then call [`KnownHosts::add`].
    Unknown,
    /// Host seen with a different key: refuse and warn loudly.
    Changed { stored: String },
}

/// Maps `host:port` -> `algorithm base64key` (the OpenSSH public key line body).
#[derive(Debug, Default)]
pub struct KnownHosts {
    entries: HashMap<String, String>,
}

fn id(host: &str, port: u16) -> String {
    if port == 22 {
        host.to_string()
    } else {
        format!("[{host}]:{port}")
    }
}

impl KnownHosts {
    pub fn parse(text: &str) -> Self {
        let mut entries = HashMap::new();
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut it = line.splitn(2, char::is_whitespace);
            if let (Some(hosts), Some(key)) = (it.next(), it.next()) {
                for h in hosts.split(',') {
                    entries.insert(h.to_string(), key.trim().to_string());
                }
            }
        }
        Self { entries }
    }

    pub fn load(path: &Path) -> std::io::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(s) => Ok(Self::parse(&s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e),
        }
    }

    pub fn verify(&self, host: &str, port: u16, key: &str) -> Verdict {
        match self.entries.get(&id(host, port)) {
            None => Verdict::Unknown,
            Some(k) if k == key => Verdict::Trusted,
            Some(k) => Verdict::Changed { stored: k.clone() },
        }
    }

    pub fn add(&mut self, host: &str, port: u16, key: &str) {
        self.entries.insert(id(host, port), key.to_string());
    }

    pub fn serialize(&self) -> String {
        let mut lines: Vec<_> = self
            .entries
            .iter()
            .map(|(h, k)| format!("{h} {k}\n"))
            .collect();
        lines.sort();
        lines.concat()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.serialize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_then_trusted_then_changed() {
        let mut k = KnownHosts::default();
        assert_eq!(k.verify("h", 22, "ssh-ed25519 AAA"), Verdict::Unknown);
        k.add("h", 22, "ssh-ed25519 AAA");
        assert_eq!(k.verify("h", 22, "ssh-ed25519 AAA"), Verdict::Trusted);
        assert!(matches!(
            k.verify("h", 22, "ssh-ed25519 BBB"),
            Verdict::Changed { .. }
        ));
        // port is part of identity
        assert_eq!(k.verify("h", 2222, "ssh-ed25519 AAA"), Verdict::Unknown);
    }

    #[test]
    fn parse_openssh_format() {
        let k = KnownHosts::parse("# c\na,b ssh-rsa KEY\n[c]:2222 ssh-ed25519 K2\n");
        assert_eq!(k.verify("b", 22, "ssh-rsa KEY"), Verdict::Trusted);
        assert_eq!(k.verify("c", 2222, "ssh-ed25519 K2"), Verdict::Trusted);
    }

    #[test]
    fn save_and_load() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("kh");
        assert!(KnownHosts::load(&p).unwrap().entries.is_empty());
        let mut k = KnownHosts::default();
        k.add("h", 22, "ssh-ed25519 AAA");
        k.save(&p).unwrap();
        assert_eq!(
            KnownHosts::load(&p)
                .unwrap()
                .verify("h", 22, "ssh-ed25519 AAA"),
            Verdict::Trusted
        );
    }
}
