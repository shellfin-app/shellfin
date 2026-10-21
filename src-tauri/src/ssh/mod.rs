//! SSH client (§5): target resolution, dialing and hop chaining, host-key verification,
//! authentication, agent access, prompts and shell quoting.
#![deny(clippy::indexing_slicing)]

pub mod agent;
pub mod auth;
pub mod connect;
pub mod handler;
pub mod identity;
pub mod keys;
pub mod prompt;
pub mod shellquote;
pub mod target;
