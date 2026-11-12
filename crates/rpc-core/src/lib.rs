//! Plugin-free RPC contracts shared by transports and method owners.

mod client;
mod peer;

pub use client::{Client, ConnId};
pub use peer::{Guest, RpcPeer, RpcPeerGuard};
