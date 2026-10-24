//! Export-time anonymization with consistent pseudonyms: the same original
//! value always maps to the same placeholder within one export.

use crate::CaptureError;
use regex::{Regex, RegexBuilder};
use simpu_core::ext::SessionMeta;

pub const MASK: &str = "******";

#[derive(Debug, Clone, Default)]
pub struct AnonymizeOptions {
    pub usernames: bool,
    /// Known hostnames (session host, extras) and all IP addresses.
    pub hostnames: bool,
    /// Built-in secret patterns: private keys, `password=...`, bearer tokens, AWS keys.
    pub secrets: bool,
    /// Extra usernames/hostnames to replace (e.g. from the connection tree).
    pub extra_usernames: Vec<String>,
    pub extra_hostnames: Vec<String>,
    /// User regexes, applied last; each distinct match becomes `<ruleN-K>`.
    pub custom: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct Anonymized {
    pub text: String,
    /// `(original, pseudonym)` in order of first use.
    pub mapping: Vec<(String, String)>,
}

#[derive(Default)]
struct Mapper {
    pairs: Vec<(String, String)>,
}

impl Mapper {
    fn get(&mut self, original: &str, make: impl FnOnce(usize) -> String, group: &str) -> String {
        if let Some((_, p)) = self.pairs.iter().find(|(o, _)| o == original) {
            return p.clone();
        }
        let n = self
            .pairs
            .iter()
            .filter(|(_, p)| p.starts_with(group))
            .count()
            + 1;
        let p = make(n);
        self.pairs.push((original.to_string(), p.clone()));
        p
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '-'
}

/// Replace whole-word occurrences of `needle` (case-sensitive).
fn replace_word(text: &str, needle: &str, repl: &str) -> String {
    if needle.is_empty() {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for (i, _) in text.match_indices(needle) {
        if i < last {
            continue;
        }
        let before = text[..i].chars().next_back();
        let after = text[i + needle.len()..].chars().next();
        if before.is_some_and(is_word) || after.is_some_and(is_word) {
            continue;
        }
        out.push_str(&text[last..i]);
        out.push_str(repl);
        last = i + needle.len();
    }
    out.push_str(&text[last..]);
    out
}

fn compile(pattern: &str) -> Result<Regex, CaptureError> {
    RegexBuilder::new(pattern)
        .size_limit(1 << 20)
        .build()
        .map_err(|e| CaptureError::BadRegex {
            pattern: pattern.into(),
            message: e.to_string(),
        })
}

/// Check a user regex without applying it (for live validation in the UI).
pub fn validate_regex(pattern: &str) -> Result<(), CaptureError> {
    compile(pattern).map(|_| ())
}

fn mask_secrets(text: &str) -> String {
    // Patterns are static and valid.
    let key =
        Regex::new(r"(?s)-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----")
            .expect("regex");
    let kv = Regex::new(
        r"(?i)\b(password|passwd|pwd|passphrase|secret|token|api[_-]?key)(\s*[=:]\s*)\S+",
    )
    .expect("regex");
    let bearer = Regex::new(r"(?i)(authorization:\s*(?:bearer|basic)\s+)\S+").expect("regex");
    let aws = Regex::new(r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b").expect("regex");
    let t = key.replace_all(text, MASK);
    let t = kv.replace_all(&t, format!("${{1}}${{2}}{MASK}"));
    let t = bearer.replace_all(&t, format!("${{1}}{MASK}"));
    aws.replace_all(&t, MASK).into_owned()
}

fn short_name(host: &str) -> Option<&str> {
    let s = host.split('.').next()?;
    (s.len() >= 3 && s != host && s.chars().any(|c| c.is_alphabetic())).then_some(s)
}

pub fn anonymize(
    text: &str,
    o: &AnonymizeOptions,
    meta: Option<&SessionMeta>,
) -> Result<Anonymized, CaptureError> {
    // Validate custom rules up front so nothing is half applied.
    let custom: Vec<Regex> = o
        .custom
        .iter()
        .map(|p| compile(p))
        .collect::<Result<_, _>>()?;
    let mut m = Mapper::default();
    let mut t = if o.secrets {
        mask_secrets(text)
    } else {
        text.to_string()
    };

    if o.usernames {
        let mut names: Vec<&str> = o.extra_usernames.iter().map(String::as_str).collect();
        if let Some(meta) = meta {
            names.insert(0, &meta.username);
        }
        for n in names.into_iter().filter(|n| n.len() >= 2) {
            let p = m.get(n, |k| format!("user{k}"), "user");
            t = replace_word(&t, n, &p);
        }
    }

    if o.hostnames {
        let mut hosts: Vec<&str> = o.extra_hostnames.iter().map(String::as_str).collect();
        if let Some(meta) = meta {
            hosts.insert(0, &meta.host);
        }
        hosts.sort_by_key(|h| std::cmp::Reverse(h.len())); // FQDN before its short form
        for h in hosts.into_iter().filter(|h| !h.is_empty()) {
            let p = m.get(h, |k| format!("host{k}"), "host");
            t = replace_word(&t, h, &p);
            if let Some(short) = short_name(h) {
                t = replace_word(&t, short, &p);
            }
        }
        let v4 = Regex::new(r"\b(?:\d{1,3}\.){3}\d{1,3}\b").expect("regex");
        t = v4
            .replace_all(&t, |c: &regex::Captures<'_>| {
                let ip = &c[0];
                if ip
                    .split('.')
                    .any(|o| o.parse::<u16>().map_or(true, |n| n > 255))
                {
                    return ip.to_string();
                }
                m.get(ip, |k| format!("10.0.0.{k}"), "10.0.0.")
            })
            .into_owned();
        // 8 full groups, or a compressed form containing "::" (so times like 10:00:01 are not matched).
        let v6 = Regex::new(
            r"(?i)\b(?:[0-9a-f]{1,4}:){7}[0-9a-f]{1,4}\b|\b[0-9a-f]{1,4}(?::[0-9a-f]{1,4}){0,6}::(?:[0-9a-f]{1,4}(?::[0-9a-f]{1,4}){0,6})?",
        )
        .expect("regex");
        t = v6
            .replace_all(&t, |c: &regex::Captures<'_>| {
                m.get(&c[0], |k| format!("fd00::{k}"), "fd00::")
            })
            .into_owned();
    }

    for (i, re) in custom.iter().enumerate() {
        let group = format!("<rule{}-", i + 1);
        t = re
            .replace_all(&t, |c: &regex::Captures<'_>| {
                m.get(&c[0], |k| format!("<rule{}-{k}>", i + 1), &group)
            })
            .into_owned();
    }
    Ok(Anonymized {
        text: t,
        mapping: m.pairs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> SessionMeta {
        SessionMeta {
            id: "s".into(),
            connection: "web".into(),
            host: "web1.example.com".into(),
            username: "alice".into(),
        }
    }

    fn all() -> AnonymizeOptions {
        AnonymizeOptions {
            usernames: true,
            hostnames: true,
            secrets: true,
            ..Default::default()
        }
    }

    #[test]
    fn user_and_host_including_short_name_and_prompt() {
        let r = anonymize(
            "alice@web1:~$ ssh web1.example.com as alice; alicex stays",
            &all(),
            Some(&meta()),
        )
        .unwrap();
        assert_eq!(r.text, "user1@host1:~$ ssh host1 as user1; alicex stays");
        assert_eq!(
            r.mapping,
            [
                ("alice".into(), "user1".into()),
                ("web1.example.com".into(), "host1".into())
            ]
        );
    }

    #[test]
    fn ips_are_consistent_and_invalid_octets_left_alone() {
        let r = anonymize(
            "a 10.1.2.3 b 192.168.0.9 c 10.1.2.3 d 999.1.1.1 e fe80::1:2 at 10:00:01",
            &all(),
            None,
        )
        .unwrap();
        assert_eq!(
            r.text,
            "a 10.0.0.1 b 10.0.0.2 c 10.0.0.1 d 999.1.1.1 e fd00::1 at 10:00:01"
        );
    }

    #[test]
    fn built_in_secret_patterns() {
        let src = "password=hunter2 TOKEN: abc\nAuthorization: Bearer xyz\nkey AKIAABCDEFGHIJKLMNOP\n-----BEGIN OPENSSH PRIVATE KEY-----\nAAAA\n-----END OPENSSH PRIVATE KEY-----\nplain";
        let r = anonymize(
            src,
            &AnonymizeOptions {
                secrets: true,
                ..Default::default()
            },
            None,
        )
        .unwrap();
        assert_eq!(r.text, "password=****** TOKEN: ******\nAuthorization: Bearer ******\nkey ******\n******\nplain");
        assert!(r.mapping.is_empty());
    }

    #[test]
    fn custom_regex_consistent_per_distinct_match() {
        let o = AnonymizeOptions {
            custom: vec![r"ACME-\d+".into()],
            ..Default::default()
        };
        let r = anonymize("ACME-1 ACME-2 ACME-1", &o, None).unwrap();
        assert_eq!(r.text, "<rule1-1> <rule1-2> <rule1-1>");
    }

    #[test]
    fn bad_regex_is_reported_and_nothing_applied() {
        let o = AnonymizeOptions {
            custom: vec!["(".into()],
            usernames: true,
            ..Default::default()
        };
        assert!(matches!(
            anonymize("alice", &o, Some(&meta())),
            Err(CaptureError::BadRegex { .. })
        ));
        assert!(validate_regex("a+").is_ok());
        assert!(validate_regex("[").is_err());
    }

    #[test]
    fn options_off_changes_nothing() {
        let r = anonymize(
            "alice 10.0.0.1",
            &AnonymizeOptions::default(),
            Some(&meta()),
        )
        .unwrap();
        assert_eq!(r.text, "alice 10.0.0.1");
    }

    #[test]
    fn extra_names_from_the_tree() {
        let o = AnonymizeOptions {
            usernames: true,
            hostnames: true,
            extra_usernames: vec!["deploy".into()],
            extra_hostnames: vec!["db.internal".into()],
            ..Default::default()
        };
        let r = anonymize("deploy on db.internal", &o, None).unwrap();
        assert_eq!(r.text, "user1 on host1");
    }
}
