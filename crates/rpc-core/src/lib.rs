//! Plugin-free RPC contracts shared by transports and method owners.

mod client;
pub mod compatibility;
mod discovery;
mod catalog;
pub use catalog::{Capabilities, DeclaredPlugin, ProtocolVersion, DiscoveryView, DiscoveryDocument, OpenRpcDocument, GUEST_METHODS, canonical_document, document_hash, freeze_discovery, discovery_document, discovery_view};
mod discovery_client;
pub use discovery_client::{DiscoveryCache, NegotiatedDiscovery};
pub use discovery::{Audience, DescribedModule, Descriptors, Discovery, MethodDescriptor, ParamDescriptor, SchemaFn, SubscriptionDescriptor, merge_described, schema_for};
pub use rpc_core_macros::rpc_descriptors;
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
