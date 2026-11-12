//! Plugin-free RPC contracts shared by transports and method owners.

mod client;
mod follow;
mod helpers;
mod state;
mod peer;
mod stream;
pub use stream::{is_delta, Stream, Backlog, Next, BACKLOG};

#[cfg(test)]
pub mod stream_tests;

pub use client::{Client, ConnId};
pub use peer::{Guest, RpcPeer, RpcPeerGuard};
pub use follow::{Following, Unfollow};
pub use helpers::{ask, send, lookup, failed, peer, owner, dreaming_err, stub_err, ASK_FAILED, NOT_RUNNING, UNAUTHORIZED};
pub use state::{FocusMap, FollowMap, PrivateMap, SeenMap, Server};

/// Explicit aggregate factory, threaded through every transport and local client.
pub type ModuleFactory = fn(Server) -> jsonrpsee::RpcModule<Server>;

/// Merge owner modules, rejecting empty lists and duplicate wire method names.
/// Schema descriptors will travel with these same owner entries at step 9.
pub fn merge(
    modules: impl IntoIterator<Item = jsonrpsee::RpcModule<Server>>,
) -> jsonrpsee::RpcModule<Server> {
    let mut modules = modules.into_iter();
    let mut merged = modules.next().expect("RPC module list must not be empty");
    for module in modules {
        merged.merge(module).expect("duplicate RPC method name");
    }
    merged
}
