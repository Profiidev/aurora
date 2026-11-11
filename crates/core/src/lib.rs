//! Pairing, device store, config, IPC and the engine tying them to the
//! transport. Used by `aurorad`, the CLI, the desktop GUI and the mobile app.

pub mod config;
pub mod engine;
pub mod ipc;
pub mod pairing;
pub mod store;
pub mod types;
