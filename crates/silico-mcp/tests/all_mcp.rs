//! silico-mcp's tests, as one test program: each file a module, the client they share
//! loaded once.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/client.rs"]
mod client;

mod adversarial_server;
mod adversarial_sketch_edits;
mod adversarial_sketch_read;
mod adversarial_timeline_commands;
mod adversarial_undo_commands;
mod calls;
mod contract;
mod drag_commands;
mod errors;
mod exact;
mod handshake;
mod hostile;
mod lines;
mod sketch_edits;
mod timeline_commands;
mod undo_commands;
