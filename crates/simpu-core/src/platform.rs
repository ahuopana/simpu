//! Traits for OS-specific functionality. Windows implementations live in the
//! app shell; tests use the fakes here (see ADR 0004).

use std::path::PathBuf;

/// Where per-user config lives.
pub trait Dirs: Send + Sync {
    fn config_dir(&self) -> PathBuf;
}

/// Source of ssh-agent connections (Windows OpenSSH pipe, Pageant, Unix socket).
pub trait AgentLocator: Send + Sync {
    /// Human-readable description of the agent endpoint, or `None` if no agent runs.
    fn endpoint(&self) -> Option<String>;
}

/// Fixed-directory implementation for tests.
pub struct FixedDirs(pub PathBuf);

impl Dirs for FixedDirs {
    fn config_dir(&self) -> PathBuf {
        self.0.clone()
    }
}

/// Agent locator that reports no agent.
pub struct NoAgent;

impl AgentLocator for NoAgent {
    fn endpoint(&self) -> Option<String> {
        None
    }
}
