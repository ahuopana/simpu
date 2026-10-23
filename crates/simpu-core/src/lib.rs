//! Simpu core: platform-independent logic for the Simpu SSH client.
//!
//! Everything OS-specific lives behind traits (see [`platform`]) so the whole
//! crate can be tested on Linux CI.

pub mod credentials;
pub mod hostkeys;
pub mod jump;
pub mod model;
pub mod platform;
pub mod ssh;
pub mod store;

pub use model::{Connection, Folder, Node, Protocol, Tree};
