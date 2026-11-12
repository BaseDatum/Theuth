//! Cross-plugin RPC composition and application adapters.

/// Cross-plugin wire methods and client contracts.
pub mod api;
mod bootstrap;
/// Legacy remote-event decoding and local republishing.
pub mod client_mirror;
/// Agent Client Protocol application adapter.
pub mod acp;
/// Unified owner search and its tool adapter.
pub mod search;
/// Owner-aware local topic intake and stream controls.
pub mod topics;

pub use bootstrap::{RpcPlugin, new_server, methods, serve, prepare};
pub use search::SearchToolsPlugin;
pub use acp::AcpPlugin;
pub use api::clients;

/// Assemble the cross-plugin RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    api::CompositionApiServer::into_rpc(server)
}

#[cfg(test)]
use bootstrap::install_forward_hooks;
#[cfg(test)]
mod forward_tests;
#[cfg(test)]
mod export;
