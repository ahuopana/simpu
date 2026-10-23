//! End to end: the real plugin binary, spawned by the real host code, talking
//! to a fake `bw` executable.

use simpu_core::credentials::{CredentialError, Credentials};
use simpu_core::model::{SecretKind, SecretRef};
use simpu_core::plugin::PluginProvider;
use std::path::Path;

fn provider(status: &str) -> PluginProvider {
    PluginProvider::spawn(
        Path::new(env!("CARGO_BIN_EXE_simpu-plugin-bitwarden")),
        &[
            ("BW_BIN", env!("CARGO_BIN_EXE_fake-bw")),
            ("FAKE_BW_STATUS", status),
            ("FAKE_BW_ITEMS", "jump=jump-pw;target=target-pw"),
        ],
    )
    .expect("spawn plugin")
}

fn r(item: &str) -> SecretRef {
    SecretRef {
        provider: "bitwarden".into(),
        item: item.into(),
        kind: SecretKind::Password,
    }
}

#[test]
fn resolves_through_plugin_process() {
    let p = provider("unlocked");
    assert_eq!(p.info().id, "bitwarden");
    let mut c = Credentials::default();
    c.register(Box::new(p));
    assert_eq!(c.resolve(&r("jump")).unwrap().expose(), "jump-pw");
    assert_eq!(c.resolve(&r("target")).unwrap().expose(), "target-pw");
    assert!(matches!(
        c.resolve(&r("missing")),
        Err(CredentialError::NotFound(_))
    ));
}

#[test]
fn locked_vault_is_reported() {
    let mut c = Credentials::default();
    c.register(Box::new(provider("locked")));
    assert_eq!(c.resolve(&r("jump")).unwrap_err(), CredentialError::Locked);
}
