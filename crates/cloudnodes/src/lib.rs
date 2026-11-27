//! Cloud nodes (design doc 224): an always-on theuth engine on the user's own provider account —
//! DigitalOcean (native install under systemd), Fly.io (the engine image; beta), Kubernetes
//! (`sandbox::k8s`; desktop only), AWS Lightsail (listed, coming soon) — created from the app,
//! joined to the owner's mesh with a key-less single-use ticket, and managed (status, restart,
//! resize, update, terminal, logs, destroy) over the mesh. No SSH and no inbound port anywhere.
//!
//! - [`types`]: the `cloud_*` wire types and the `cloudnodes.events` topic.
//! - [`providers`]: one client per provider behind [`providers::Provider`], credential
//!   discovery, the size/region/price catalog and the VM user-data template.
//! - [`engine`]: the service — credentials ([`creds`]), node records ([`records`]), the create
//!   job, list/status merged with the mesh ([`mesh_view`]), restart/resize/update/destroy.
//! - [`actor`]: the `cloudnodes` plugin and actor; [`rpc`]: the `cloud_*` methods.

pub mod actor;
pub mod creds;
pub mod engine;
pub mod mesh_view;
pub mod providers;
pub mod records;
pub mod rpc;
pub mod rpc_support;
pub mod types;

pub use actor::CloudPlugin;
pub use engine::TICKET_TTL_MS;
pub use rpc::rpc_module;
