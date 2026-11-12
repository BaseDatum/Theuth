//! Cross-plugin wire methods, contracts and typed clients.

mod rpc_hub;
mod rpc_settings;
mod contracts;
pub use contracts::{Info, PendingPrompts, StreamControl, Lagged, ConfigFlagKey, ConfigFlag};

include!("api/clients.rs");

#[cfg(test)]
use contracts::{admitted, wanted};
#[cfg(test)]
#[path = "api/tests.rs"]
mod tests;
#[cfg(all(test, feature = "ts"))]
pub(crate) use tests::test_rpc_module;
