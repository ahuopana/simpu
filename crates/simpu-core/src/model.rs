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
}

impl Connection {
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
    fn toml_round_trip() {
        let t = sample();
        let s = toml::to_string_pretty(&t).unwrap();
        let back: Tree = toml::from_str(&s).unwrap();
        assert_eq!(t, back);
    }
}
