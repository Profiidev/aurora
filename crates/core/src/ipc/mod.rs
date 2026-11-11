//! Local-socket IPC between `aurorad` and its clients (CLI, GUI): newline
//! delimited JSON, one request per connection, trait per command.

pub mod client;
pub mod command;
pub mod commands;
pub mod server;
pub mod util;

pub use client::{send, subscribe};
pub use command::{IpcCommand, IpcStream};
pub use server::{IpcRegistry, IpcServer};
