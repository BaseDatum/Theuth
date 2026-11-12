//! The JSON-RPC surface. Every method is a lazy registry lookup plus one
//! `ask`; event subscriptions drain the owner-declared `wire` feed.

#[cfg(test)]
use s7_composition_support::{admitted, wanted};

mod s7_agent_artifacts;
mod s7_artifact_artifacts;
mod s7_app_artifacts;
mod s7_knowledge_concepts;
mod s7_agent_concepts;
mod s7_document_documents;
mod s7_roster_documents;
mod s7_agent_documents;
mod s7_agent_notebooks;
mod s7_document_notebooks;
mod s7_agent_sessions;
mod s7_sessionsync_sessions;
mod s7_tools_tooling;
mod s7_agent_tooling;
mod s7_agent_messaging;
mod s7_signal_messaging;
mod s7_slack_messaging;
mod s7_telegram_messaging;
mod s7_a2a_messaging;
mod s7_rpc_compose_hub;
mod s7_a2a_hub;
mod s7_agent_hub;
mod s7_sessionsync_hub;
mod s7_tasks_hub;
mod s7_artifact_hub;
mod s7_link_hub;
mod s7_talk_hub;
mod s7_search_providers_hub;
mod s7_app_hub;
mod s7_rpc_hub;
mod s7_agent_settings;
mod s7_tools_settings;
mod s7_update_settings;
mod s7_rpc_compose_settings;
mod s7_web_settings;
mod s7_composition_support;
pub use s7_composition_support::{Info, PendingPrompts, StreamControl, Lagged, ConfigFlagKey, ConfigFlag};

/// The wire API is split into domain server and client traits.
/// Each domain exports its own server module; [`clients`] exports the typed client methods
/// for the in-process [`crate::client::Local`] or a WebSocket (63 §2.3).
pub mod hub;
/// Sessions RPC server and client traits.
pub mod sessions;
/// Settings RPC server and client traits.
pub mod settings;
/// Tooling RPC server and client traits.
pub mod tooling;

/// Concepts RPC server and client traits.
pub mod concepts;
/// Documents RPC server and client traits.
pub mod documents;
/// Notebooks RPC server and client traits.
pub mod notebooks;
/// Messaging RPC server and client traits.
pub mod messaging;
/// Artifacts RPC server and client traits.
pub mod artifacts;

pub use hub::{CoreApiClient, CoreApiServer};
pub use sessions::{SessionsApiClient, SessionsApiServer};
pub use settings::{SettingsApiClient, SettingsApiServer};
pub use tooling::{SESSION_TOOLS, ToolingApiClient, ToolingApiServer};

pub use concepts::{ConceptsApiClient, ConceptsApiServer};
pub use documents::{DocumentsApiClient, DocumentsApiServer};
pub use notebooks::{NotebooksApiClient, NotebooksApiServer};
pub use messaging::{MessagingApiClient, MessagingApiServer};
pub use artifacts::{ArtifactsApiClient, ArtifactsApiServer};


#[cfg(test)]
#[path = "api/s7_api_tests.rs"]
mod tests;
#[cfg(all(test, feature = "ts"))]
pub(crate) use tests::test_rpc_module;
