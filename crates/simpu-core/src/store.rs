//! Persisting the connection tree as TOML.

use crate::model::Tree;
use std::path::Path;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("serialize: {0}")]
    Serialize(#[from] toml::ser::Error),
}

pub fn load(path: &Path) -> Result<Tree, StoreError> {
    match std::fs::read_to_string(path) {
        Ok(s) => Ok(toml::from_str(&s)?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Tree::default()),
        Err(e) => Err(e.into()),
    }
}

/// Atomic save (write temp file, then rename) so a crash cannot corrupt the tree.
pub fn save(path: &Path, tree: &Tree) -> Result<(), StoreError> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, toml::to_string_pretty(tree)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Connection, Node, Protocol};

    #[test]
    fn missing_file_is_empty_tree() {
        let d = tempfile::tempdir().unwrap();
        assert_eq!(load(&d.path().join("x.toml")).unwrap(), Tree::default());
    }

    #[test]
    fn save_load_round_trip() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("tree.toml");
        let t = Tree {
            nodes: vec![Node::Connection(Connection::new(
                "a",
                Protocol::Ssh,
                "h",
                "u",
            ))],
        };
        save(&p, &t).unwrap();
        assert_eq!(load(&p).unwrap(), t);
        assert!(!p.with_extension("tmp").exists());
    }
}
