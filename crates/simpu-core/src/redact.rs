//! Host-side masking of secrets the host itself knows (vault passwords, typed
//! passwords, unlock passwords). Applied to the byte stream *before* it reaches
//! any extension, so extensions never see these values (ADR 0007).

use crate::credentials::Secret;
use std::sync::{Arc, Mutex};

pub const TOKEN: &str = "⟦secret⟧";
/// Shorter secrets would mask ordinary text; they are not redacted.
const MIN_LEN: usize = 3;

/// Secrets known to the host. Cloneable handle; values are zeroized on drop.
#[derive(Clone, Default)]
pub struct SecretSet {
    inner: Arc<Mutex<Vec<Secret>>>,
}

impl SecretSet {
    pub fn add(&self, secret: &Secret) {
        if secret.expose().len() < MIN_LEN {
            return;
        }
        let mut v = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        if !v.iter().any(|s| s.expose() == secret.expose()) {
            v.push(secret.clone());
        }
    }

    /// Longest first, so a secret containing another one is masked as a whole.
    fn snapshot(&self) -> Vec<Vec<u8>> {
        let v = self.inner.lock().unwrap_or_else(|p| p.into_inner());
        let mut out: Vec<Vec<u8>> = v.iter().map(|s| s.expose().as_bytes().to_vec()).collect();
        out.sort_by_key(|s| std::cmp::Reverse(s.len()));
        out
    }
}

/// Per-session stream filter. Holds back a trailing partial match so a secret
/// split across chunks is still caught.
pub struct StreamRedactor {
    secrets: SecretSet,
    carry: Vec<u8>,
}

impl StreamRedactor {
    pub fn new(secrets: SecretSet) -> Self {
        Self {
            secrets,
            carry: Vec::new(),
        }
    }

    fn mask(buf: &[u8], secrets: &[Vec<u8>]) -> Vec<u8> {
        let mut out = Vec::with_capacity(buf.len());
        let mut i = 0;
        'outer: while i < buf.len() {
            for s in secrets {
                if buf[i..].starts_with(s) {
                    out.extend_from_slice(TOKEN.as_bytes());
                    i += s.len();
                    continue 'outer;
                }
            }
            out.push(buf[i]);
            i += 1;
        }
        out
    }

    /// Redacted bytes that are safe to hand out now.
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<u8> {
        let secrets = self.secrets.snapshot();
        let mut buf = std::mem::take(&mut self.carry);
        buf.extend_from_slice(chunk);
        // Longest suffix of `buf` that is a proper prefix of some secret must wait.
        let mut hold = 0;
        for s in &secrets {
            for len in (1..s.len().min(buf.len() + 1)).rev() {
                if len > hold && buf.ends_with(&s[..len]) {
                    hold = len;
                    break;
                }
            }
        }
        // Complete matches inside the buffer are masked first; the held tail cannot be one.
        let split = buf.len() - hold;
        let out = Self::mask(&buf[..split], &secrets);
        self.carry = buf[split..].to_vec();
        out
    }

    /// Emit whatever is still held back (call when the session ends).
    pub fn flush(&mut self) -> Vec<u8> {
        let secrets = self.secrets.snapshot();
        let buf = std::mem::take(&mut self.carry);
        Self::mask(&buf, &secrets)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn red(secrets: &[&str]) -> StreamRedactor {
        let set = SecretSet::default();
        for s in secrets {
            set.add(&Secret::new(*s));
        }
        StreamRedactor::new(set)
    }

    fn run(r: &mut StreamRedactor, chunks: &[&str]) -> String {
        let mut out = Vec::new();
        for c in chunks {
            out.extend(r.feed(c.as_bytes()));
        }
        out.extend(r.flush());
        String::from_utf8(out).unwrap()
    }

    #[test]
    fn masks_inside_a_chunk() {
        let mut r = red(&["hunter2"]);
        assert_eq!(
            run(&mut r, &["login hunter2 ok"]),
            format!("login {TOKEN} ok")
        );
    }

    #[test]
    fn masks_across_chunk_boundaries_at_every_split() {
        let text = "pw=hunter2!";
        for cut in 0..=text.len() {
            let mut r = red(&["hunter2"]);
            let (a, b) = text.split_at(cut);
            assert_eq!(
                run(&mut r, &[a, b]),
                format!("pw={TOKEN}!"),
                "split at {cut}"
            );
        }
        let mut r = red(&["hunter2"]);
        let chunks: Vec<String> = text.chars().map(String::from).collect();
        let refs: Vec<&str> = chunks.iter().map(String::as_str).collect();
        assert_eq!(run(&mut r, &refs), format!("pw={TOKEN}!"));
    }

    #[test]
    fn partial_prefix_is_held_then_released_if_not_a_match() {
        let mut r = red(&["hunter2"]);
        assert_eq!(r.feed(b"say hun"), b"say ");
        assert_eq!(run(&mut r, &["ting"]), "hunting");
    }

    #[test]
    fn longest_secret_wins_and_short_ones_are_ignored() {
        let mut r = red(&["abc", "abcdef", "xy"]);
        assert_eq!(
            run(&mut r, &["abcdef xy abc"]),
            format!("{TOKEN} xy {TOKEN}")
        );
    }

    #[test]
    fn secrets_added_later_apply_to_later_output() {
        let set = SecretSet::default();
        let mut r = StreamRedactor::new(set.clone());
        assert_eq!(run(&mut r, &["token abc123"]), "token abc123");
        set.add(&Secret::new("abc123"));
        assert_eq!(run(&mut r, &["token abc123"]), format!("token {TOKEN}"));
    }

    #[test]
    fn utf8_secrets_and_passthrough_of_binary() {
        let mut r = red(&["pässwörd"]);
        assert_eq!(run(&mut r, &["x pässwörd y"]), format!("x {TOKEN} y"));
        let mut r = red(&["secret"]);
        assert_eq!(r.feed(&[0xff, 0x00, 0x1b]), [0xff, 0x00, 0x1b]);
    }
}
