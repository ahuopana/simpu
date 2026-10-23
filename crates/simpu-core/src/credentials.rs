//! Credential providers. Plugins (e.g. Bitwarden) implement [`CredentialProvider`].

use crate::model::SecretRef;
use std::collections::HashMap;
use thiserror::Error;
use zeroize::{Zeroize, ZeroizeOnDrop};

/// A secret that is wiped from memory when dropped and never printed.
#[derive(Clone, Zeroize, ZeroizeOnDrop)]
pub struct Secret(String);

impl Secret {
    pub fn new(s: impl Into<String>) -> Self {
        Secret(s.into())
    }
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(<redacted>)")
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum CredentialError {
    #[error("vault is locked")]
    Locked,
    #[error("item {0} not found")]
    NotFound(String),
    #[error("provider {0} not available")]
    NoProvider(String),
    #[error("provider error: {0}")]
    Other(String),
}

pub trait CredentialProvider: Send + Sync {
    fn id(&self) -> &str;
    fn get(&self, item: &str) -> Result<Secret, CredentialError>;
}

/// Routes [`SecretRef`]s to registered providers.
#[derive(Default)]
pub struct Credentials {
    providers: HashMap<String, Box<dyn CredentialProvider>>,
}

impl Credentials {
    pub fn register(&mut self, p: Box<dyn CredentialProvider>) {
        self.providers.insert(p.id().to_string(), p);
    }

    pub fn resolve(&self, r: &SecretRef) -> Result<Secret, CredentialError> {
        self.providers
            .get(&r.provider)
            .ok_or_else(|| CredentialError::NoProvider(r.provider.clone()))?
            .get(&r.item)
    }
}

/// In-memory provider, used by tests and as a fallback.
pub struct StaticProvider {
    items: HashMap<String, String>,
    locked: bool,
}

impl StaticProvider {
    pub fn new(items: &[(&str, &str)]) -> Self {
        Self {
            items: items
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
            locked: false,
        }
    }
    pub fn locked(mut self) -> Self {
        self.locked = true;
        self
    }
}

impl CredentialProvider for StaticProvider {
    fn id(&self) -> &str {
        "static"
    }
    fn get(&self, item: &str) -> Result<Secret, CredentialError> {
        if self.locked {
            return Err(CredentialError::Locked);
        }
        self.items
            .get(item)
            .map(|v| Secret::new(v.clone()))
            .ok_or_else(|| CredentialError::NotFound(item.into()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::SecretKind;

    fn r(provider: &str, item: &str) -> SecretRef {
        SecretRef {
            provider: provider.into(),
            item: item.into(),
            kind: SecretKind::Password,
        }
    }

    #[test]
    fn resolves_and_redacts() {
        let mut c = Credentials::default();
        c.register(Box::new(StaticProvider::new(&[("a", "hunter2")])));
        let s = c.resolve(&r("static", "a")).unwrap();
        assert_eq!(s.expose(), "hunter2");
        assert!(!format!("{s:?}").contains("hunter2"));
    }

    #[test]
    fn errors() {
        let mut c = Credentials::default();
        c.register(Box::new(StaticProvider::new(&[("a", "x")]).locked()));
        assert_eq!(
            c.resolve(&r("static", "a")).unwrap_err(),
            CredentialError::Locked
        );
        assert!(matches!(
            c.resolve(&r("nope", "a")),
            Err(CredentialError::NoProvider(_))
        ));
        let mut c = Credentials::default();
        c.register(Box::new(StaticProvider::new(&[])));
        assert!(matches!(
            c.resolve(&r("static", "zz")),
            Err(CredentialError::NotFound(_))
        ));
    }
}
