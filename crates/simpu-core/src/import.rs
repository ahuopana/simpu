//! Import connections from an OpenSSH client config (`~/.ssh/config`).

use crate::model::{Connection, Protocol, SecretKind, SecretRef};
use std::collections::HashMap;

#[derive(Default, Debug)]
struct Block {
    patterns: Vec<String>,
    options: Vec<(String, String)>,
}

fn is_wildcard(p: &str) -> bool {
    p.contains(['*', '?', '!'])
}

fn parse_blocks(text: &str) -> Vec<Block> {
    let mut blocks = Vec::new();
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        // "Key value" or "Key=value"
        let (key, value) = match line.split_once(|c: char| c.is_whitespace() || c == '=') {
            Some((k, v)) => (
                k.trim(),
                v.trim().trim_start_matches('=').trim().trim_matches('"'),
            ),
            None => continue,
        };
        if key.eq_ignore_ascii_case("Host") {
            blocks.push(Block {
                patterns: value.split_whitespace().map(String::from).collect(),
                options: vec![],
            });
        } else if key.eq_ignore_ascii_case("Match") {
            blocks.push(Block::default()); // unsupported: ignore its options
        } else if let Some(b) = blocks.last_mut() {
            b.options
                .push((key.to_ascii_lowercase(), value.to_string()));
        }
    }
    blocks
}

fn opt<'a>(b: &'a Block, key: &str) -> Option<&'a str> {
    b.options
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

/// `[user@]host[:port]` as used by ProxyJump.
fn parse_jump_spec(spec: &str) -> (Option<String>, String, Option<u16>) {
    let (user, rest) = match spec.split_once('@') {
        Some((u, r)) => (Some(u.to_string()), r),
        None => (None, spec),
    };
    match rest.rsplit_once(':') {
        Some((h, p)) if p.parse::<u16>().is_ok() => (user, h.to_string(), p.parse().ok()),
        _ => (user, rest.to_string(), None),
    }
}

/// Convert an ssh config into connections. Wildcard hosts are ignored.
/// `IdentityFile` becomes a `file` provider key secret; `ProxyJump` aliases
/// are linked to the imported hosts (unknown ones become new connections).
pub fn parse_ssh_config(text: &str, default_user: &str) -> Vec<Connection> {
    let blocks = parse_blocks(text);
    let mut conns: Vec<Connection> = Vec::new();
    let mut by_alias: HashMap<String, usize> = HashMap::new();

    for b in &blocks {
        for alias in b.patterns.iter().filter(|p| !is_wildcard(p)) {
            let host = opt(b, "hostname").unwrap_or(alias);
            let user = opt(b, "user").unwrap_or(default_user);
            let mut c = Connection::new(alias, Protocol::Ssh, host, user);
            c.port = opt(b, "port").and_then(|p| p.parse().ok());
            if let Some(f) = opt(b, "identityfile") {
                c.secret = Some(SecretRef {
                    provider: "file".into(),
                    item: f.to_string(),
                    kind: SecretKind::KeyFile,
                });
            }
            by_alias.insert(alias.clone(), conns.len());
            conns.push(c);
        }
    }

    // Second pass: ProxyJump.
    for b in &blocks {
        let Some(spec) = opt(b, "proxyjump") else {
            continue;
        };
        if spec.eq_ignore_ascii_case("none") {
            continue;
        }
        let mut chain = Vec::new();
        for hop in spec.split(',').map(str::trim).filter(|h| !h.is_empty()) {
            let (user, host, port) = parse_jump_spec(hop);
            let idx = match by_alias.get(&host) {
                Some(&i) => i,
                None => {
                    let mut c = Connection::new(
                        &host,
                        Protocol::Ssh,
                        &host,
                        user.as_deref().unwrap_or(default_user),
                    );
                    c.port = port;
                    by_alias.insert(host.clone(), conns.len());
                    conns.push(c);
                    conns.len() - 1
                }
            };
            chain.push(conns[idx].id);
        }
        for alias in b.patterns.iter().filter(|p| !is_wildcard(p)) {
            if let Some(&i) = by_alias.get(alias) {
                let own = conns[i].id;
                conns[i].jump = chain.iter().copied().filter(|j| *j != own).collect();
            }
        }
    }
    conns
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
# comment
Host *
    ServerAliveInterval 30

Host bastion
    HostName bastion.example.com
    User ops
    Port 2222
    IdentityFile ~/.ssh/id_ed25519

Host db app
    HostName 10.0.0.5
    ProxyJump bastion

Host far
    HostName far.example.com
    ProxyJump jumpy@gw.example.com:2200,bastion
"#;

    #[test]
    fn parses_hosts_options_and_skips_wildcards() {
        let c = parse_ssh_config(SAMPLE, "me");
        let names: Vec<_> = c.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["bastion", "db", "app", "far", "gw.example.com"]);
        let b = &c[0];
        assert_eq!(
            (b.host.as_str(), b.username.as_str(), b.port),
            ("bastion.example.com", "ops", Some(2222))
        );
        let s = b.secret.as_ref().unwrap();
        assert_eq!(
            (s.provider.as_str(), s.item.as_str(), s.kind),
            ("file", "~/.ssh/id_ed25519", SecretKind::KeyFile)
        );
        assert_eq!(c[1].username, "me", "default user applies");
    }

    #[test]
    fn proxy_jump_links_known_hosts_and_creates_unknown_ones() {
        let c = parse_ssh_config(SAMPLE, "me");
        let id = |n: &str| c.iter().find(|c| c.name == n).unwrap().id;
        assert_eq!(c[1].jump, [id("bastion")]);
        assert_eq!(c[2].jump, [id("bastion")]);
        let far = c.iter().find(|c| c.name == "far").unwrap();
        assert_eq!(far.jump, [id("gw.example.com"), id("bastion")]);
        let gw = c.iter().find(|c| c.name == "gw.example.com").unwrap();
        assert_eq!((gw.username.as_str(), gw.port), ("jumpy", Some(2200)));
    }

    #[test]
    fn tolerates_equals_quotes_and_junk() {
        let c = parse_ssh_config(
            "Host x\n  HostName=\"x.example.com\"\n  Bogus\n  ProxyJump none\n",
            "me",
        );
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].host, "x.example.com");
        assert!(c[0].jump.is_empty());
        assert!(parse_ssh_config("", "me").is_empty());
    }
}
