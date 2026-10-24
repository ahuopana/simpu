//! Connection tree: folders and connections.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Ssh,
    Telnet,
    Sftp,
    Scp,
}

impl Protocol {
    pub fn default_port(self) -> u16 {
        match self {
            Protocol::Ssh | Protocol::Sftp | Protocol::Scp => 22,
            Protocol::Telnet => 23,
        }
    }

    /// Telnet sends everything, including passwords, in clear text.
    pub fn is_plaintext(self) -> bool {
        matches!(self, Protocol::Telnet)
    }
}

/// Reference to a secret held by a credential provider (never the secret itself).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretRef {
    /// Provider id, e.g. `bitwarden` or `static`.
    pub provider: String,
    /// Provider-specific item id.
    pub item: String,
    #[serde(default)]
    pub kind: SecretKind,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SecretKind {
    #[default]
    Password,
    /// Private key (PEM/OpenSSH); the provider may also supply a passphrase.
    KeyFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ForwardKind {
    /// `-L`: listen locally, connect to `dest` through the server.
    Local,
    /// `-R`: listen on the server, connect to `dest` from here.
    Remote,
    /// `-D`: local SOCKS5 proxy.
    Dynamic,
}

fn loopback() -> String {
    "127.0.0.1".into()
}

fn is_loopback(s: &String) -> bool {
    s == "127.0.0.1"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Forward {
    pub kind: ForwardKind,
    #[serde(default = "loopback", skip_serializing_if = "is_loopback")]
    pub bind_host: String,
    pub bind_port: u16,
    /// Not used for `dynamic`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dest_port: Option<u16>,
}

impl Forward {
    pub fn local(bind_port: u16, dest_host: &str, dest_port: u16) -> Self {
        Self {
            kind: ForwardKind::Local,
            bind_host: loopback(),
            bind_port,
            dest_host: Some(dest_host.into()),
            dest_port: Some(dest_port),
        }
    }
    pub fn remote(bind_port: u16, dest_host: &str, dest_port: u16) -> Self {
        Self {
            kind: ForwardKind::Remote,
            bind_host: loopback(),
            bind_port,
            dest_host: Some(dest_host.into()),
            dest_port: Some(dest_port),
        }
    }
    pub fn dynamic(bind_port: u16) -> Self {
        Self {
            kind: ForwardKind::Dynamic,
            bind_host: loopback(),
            bind_port,
            dest_host: None,
            dest_port: None,
        }
    }

    /// Anything but loopback exposes the forward to other machines.
    pub fn is_exposed(&self) -> bool {
        !matches!(self.bind_host.as_str(), "127.0.0.1" | "localhost" | "::1")
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.bind_port == 0 && self.kind != ForwardKind::Remote {
            return Err("bind port must be 1-65535".into());
        }
        if self.kind != ForwardKind::Dynamic {
            if self
                .dest_host
                .as_deref()
                .is_none_or(|h| h.trim().is_empty())
            {
                return Err("destination host is required".into());
            }
            if self.dest_port.is_none_or(|p| p == 0) {
                return Err("destination port must be 1-65535".into());
            }
        }
        Ok(())
    }
}

/// What to do when a session's connection drops.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReconnectPolicy {
    /// Retry with backoff.
    Auto,
    /// Wait for the user to press Reconnect.
    Ask,
    /// Close the session.
    Off,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Connection {
    pub id: Uuid,
    pub name: String,
    pub protocol: Protocol,
    pub host: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    pub username: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secret: Option<SecretRef>,
    /// Ordered jump hosts (first hop first), by connection id.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jump: Vec<Uuid>,
    /// Port forwards started with the session.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forwards: Vec<Forward>,
    /// `None` = default: `auto` if the connection has forwards, otherwise `ask`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reconnect: Option<ReconnectPolicy>,
    /// Tunnel only: no interactive shell (like `ssh -N`).
    #[serde(default, skip_serializing_if = "is_false")]
    pub no_shell: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

impl Connection {
    pub fn effective_reconnect(&self) -> ReconnectPolicy {
        self.reconnect.unwrap_or(if self.forwards.is_empty() {
            ReconnectPolicy::Ask
        } else {
            ReconnectPolicy::Auto
        })
    }

    pub fn new(name: &str, protocol: Protocol, host: &str, username: &str) -> Self {
        Self {
            id: Uuid::new_v4(),
            name: name.into(),
            protocol,
            host: host.into(),
            port: None,
            username: username.into(),
            secret: None,
            jump: Vec::new(),
            forwards: Vec::new(),
            reconnect: None,
            no_shell: false,
        }
    }

    pub fn effective_port(&self) -> u16 {
        self.port.unwrap_or_else(|| self.protocol.default_port())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub name: String,
    #[serde(default)]
    pub children: Vec<Node>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Node {
    Folder(Folder),
    Connection(Connection),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tree {
    #[serde(default)]
    pub nodes: Vec<Node>,
}

impl Tree {
    /// All connections, depth-first.
    pub fn connections(&self) -> Vec<&Connection> {
        fn walk<'a>(nodes: &'a [Node], out: &mut Vec<&'a Connection>) {
            for n in nodes {
                match n {
                    Node::Connection(c) => out.push(c),
                    Node::Folder(f) => walk(&f.children, out),
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.nodes, &mut out);
        out
    }

    pub fn find(&self, id: Uuid) -> Option<&Connection> {
        self.connections().into_iter().find(|c| c.id == id)
    }

    /// Case-insensitive search over name, host and username.
    pub fn search(&self, query: &str) -> Vec<&Connection> {
        let q = query.to_lowercase();
        self.connections()
            .into_iter()
            .filter(|c| {
                c.name.to_lowercase().contains(&q)
                    || c.host.to_lowercase().contains(&q)
                    || c.username.to_lowercase().contains(&q)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Tree {
        Tree {
            nodes: vec![
                Node::Folder(Folder {
                    name: "prod".into(),
                    children: vec![Node::Connection(Connection::new(
                        "db",
                        Protocol::Ssh,
                        "db.example.com",
                        "root",
                    ))],
                }),
                Node::Connection(Connection::new("sw", Protocol::Telnet, "10.0.0.1", "admin")),
            ],
        }
    }

    #[test]
    fn default_ports() {
        assert_eq!(Protocol::Ssh.default_port(), 22);
        assert_eq!(Protocol::Telnet.default_port(), 23);
        assert!(Protocol::Telnet.is_plaintext());
        assert!(!Protocol::Sftp.is_plaintext());
    }

    #[test]
    fn walk_and_search() {
        let t = sample();
        assert_eq!(t.connections().len(), 2);
        assert_eq!(t.search("EXAMPLE").len(), 1);
        assert_eq!(t.search("admin")[0].name, "sw");
        assert!(t.search("nothing").is_empty());
    }

    #[test]
    fn old_files_without_new_fields_still_load_and_defaults_apply() {
        let old = r#"
[[nodes]]
type = "connection"
id = "11111111-1111-1111-1111-111111111111"
name = "legacy"
protocol = "ssh"
host = "h"
username = "u"
"#;
        let t: Tree = toml::from_str(old).unwrap();
        let c = t.connections()[0];
        assert!(c.forwards.is_empty() && !c.no_shell && c.reconnect.is_none());
        assert_eq!(c.effective_reconnect(), ReconnectPolicy::Ask);
        // serializing an unchanged connection does not add the new keys
        let s = toml::to_string_pretty(&t).unwrap();
        assert!(
            !s.contains("forwards") && !s.contains("no_shell") && !s.contains("reconnect"),
            "{s}"
        );
    }

    #[test]
    fn forwards_round_trip_and_default_policy() {
        let mut c = Connection::new("db", Protocol::Ssh, "h", "u");
        c.forwards = vec![
            Forward::local(5433, "db.internal", 5432),
            Forward::remote(8080, "127.0.0.1", 80),
            Forward::dynamic(1080),
        ];
        assert_eq!(c.effective_reconnect(), ReconnectPolicy::Auto);
        c.reconnect = Some(ReconnectPolicy::Off);
        c.no_shell = true;
        let t = Tree {
            nodes: vec![Node::Connection(c.clone())],
        };
        let s = toml::to_string_pretty(&t).unwrap();
        let back: Tree = toml::from_str(&s).unwrap();
        assert_eq!(back, t);
        let json = serde_json::to_string(&c).unwrap();
        assert!(json.contains(r#""kind":"dynamic""#) && json.contains(r#""reconnect":"off""#));
    }

    #[test]
    fn forward_validation_and_exposure() {
        assert!(Forward::local(1, "h", 2).validate().is_ok());
        assert!(Forward::dynamic(1080).validate().is_ok());
        assert!(Forward::local(0, "h", 2).validate().is_err());
        assert!(Forward::local(1, " ", 2).validate().is_err());
        assert!(Forward {
            dest_port: None,
            ..Forward::local(1, "h", 2)
        }
        .validate()
        .is_err());
        assert!(!Forward::dynamic(1).is_exposed());
        assert!(Forward {
            bind_host: "0.0.0.0".into(),
            ..Forward::dynamic(1)
        }
        .is_exposed());
    }

    #[test]
    fn toml_round_trip() {
        let t = sample();
        let s = toml::to_string_pretty(&t).unwrap();
        let back: Tree = toml::from_str(&s).unwrap();
        assert_eq!(t, back);
    }
}
