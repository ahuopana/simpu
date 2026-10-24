//! Simpu core: platform-independent logic for the Simpu SSH client.
//!
//! Everything OS-specific lives behind traits (see [`platform`]) so the whole
//! crate can be tested on Linux CI.

pub mod credentials;
pub mod ext;
pub mod forward;
pub mod hostkeys;
pub mod import;
pub mod jump;
pub mod manager;
pub mod model;
pub mod platform;
pub mod plugin;
pub mod redact;
pub mod sftp;
pub mod socks5;
pub mod ssh;
pub mod store;
pub mod telnet;

pub use model::{Connection, Folder, Node, Protocol, Tree};
