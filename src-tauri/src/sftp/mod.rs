//! SFTP & transfers (§6): client (bounded listing, file ops), text codec, atomic saves, transfers,
//! OS drag & drop registry. Server-controlled names and paths are hostile (§9.4/§9.5).
#![deny(clippy::indexing_slicing)]

pub mod client;
pub mod drops;
pub mod names;
pub mod path;
pub mod save;
pub mod text;
pub mod throttle;
pub mod transfer;
pub mod tree;
