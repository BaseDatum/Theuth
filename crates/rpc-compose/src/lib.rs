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

pub use bootstrap::{RpcPlugin, new_server, methods, serve};
pub use search::SearchToolsPlugin;
pub use acp::AcpPlugin;
pub use api::clients;

#[cfg(test)]
use bootstrap::install_forward_hooks;
#[cfg(test)]
mod forward_tests;
#[cfg(test)]
mod export;
