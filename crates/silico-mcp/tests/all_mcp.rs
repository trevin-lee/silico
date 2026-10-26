//! silico-mcp's tests, as one test program: each file a module, the client they share
//! loaded once.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[path = "common/client.rs"]
mod client;

mod adversarial_server;
mod calls;
mod contract;
mod errors;
mod exact;
mod handshake;
mod hostile;
mod lines;
