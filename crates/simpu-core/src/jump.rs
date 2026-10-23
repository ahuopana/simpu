//! Jump host chain resolution.

use crate::model::{Connection, Tree};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum JumpError {
    #[error("jump host {0} not found")]
    NotFound(Uuid),
    #[error("jump chain contains a cycle at {0}")]
    Cycle(String),
    #[error("jump host {0} must use the ssh protocol")]
    NotSsh(String),
}

/// Resolve the full hop list for `target`: every jump host, expanded
/// recursively (a jump host may itself have jumps), ending with the target.
pub fn resolve_chain<'a>(
    tree: &'a Tree,
    target: &'a Connection,
) -> Result<Vec<&'a Connection>, JumpError> {
    fn go<'a>(
        tree: &'a Tree,
        c: &'a Connection,
        stack: &mut Vec<Uuid>,
        out: &mut Vec<&'a Connection>,
    ) -> Result<(), JumpError> {
        if stack.contains(&c.id) {
            return Err(JumpError::Cycle(c.name.clone()));
        }
        stack.push(c.id);
        for id in &c.jump {
            let j = tree.find(*id).ok_or(JumpError::NotFound(*id))?;
            if j.protocol != crate::Protocol::Ssh {
                return Err(JumpError::NotSsh(j.name.clone()));
            }
            go(tree, j, stack, out)?;
        }
        stack.pop();
        out.push(c);
        Ok(())
    }
    let mut out = Vec::new();
    go(tree, target, &mut Vec::new(), &mut out)?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Node, Protocol};

    fn c(name: &str) -> Connection {
        Connection::new(name, Protocol::Ssh, name, "u")
    }

    fn tree(cs: Vec<Connection>) -> Tree {
        Tree {
            nodes: cs.into_iter().map(Node::Connection).collect(),
        }
    }

    #[test]
    fn no_jump_is_just_target() {
        let t = c("t");
        let tr = tree(vec![t.clone()]);
        let chain = resolve_chain(&tr, tr.find(t.id).unwrap()).unwrap();
        assert_eq!(chain.len(), 1);
    }

    #[test]
    fn nested_jumps_expand_in_order() {
        let a = c("a");
        let mut b = c("b");
        b.jump = vec![a.id];
        let mut t = c("t");
        t.jump = vec![b.id];
        let tr = tree(vec![a, b, t.clone()]);
        let names: Vec<_> = resolve_chain(&tr, tr.find(t.id).unwrap())
            .unwrap()
            .iter()
            .map(|c| c.name.as_str())
            .collect();
        assert_eq!(names, ["a", "b", "t"]);
    }

    #[test]
    fn cycle_detected() {
        let mut a = c("a");
        let mut b = c("b");
        a.jump = vec![b.id];
        b.jump = vec![a.id];
        let tr = tree(vec![a.clone(), b]);
        assert!(matches!(
            resolve_chain(&tr, tr.find(a.id).unwrap()),
            Err(JumpError::Cycle(_))
        ));
    }

    #[test]
    fn missing_and_non_ssh_rejected() {
        let mut t = c("t");
        let ghost = Uuid::new_v4();
        t.jump = vec![ghost];
        let tr = tree(vec![t.clone()]);
        assert_eq!(
            resolve_chain(&tr, tr.find(t.id).unwrap()),
            Err(JumpError::NotFound(ghost))
        );

        let tel = Connection::new("tel", Protocol::Telnet, "h", "u");
        let mut t2 = c("t2");
        t2.jump = vec![tel.id];
        let tr = tree(vec![tel, t2.clone()]);
        assert!(matches!(
            resolve_chain(&tr, tr.find(t2.id).unwrap()),
            Err(JumpError::NotSsh(_))
        ));
    }
}
