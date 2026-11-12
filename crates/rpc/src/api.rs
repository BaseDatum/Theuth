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

/// All domain client traits for typed methods on any jsonrpsee client.
pub mod clients {
    pub use super::s7_agent_artifacts::AgentArtifactsApiClient;
    pub use super::s7_artifact_artifacts::ArtifactArtifactsApiClient;
    pub use super::s7_app_artifacts::AppArtifactsApiClient;
    pub use super::s7_knowledge_concepts::KnowledgeConceptsApiClient;
    pub use super::s7_agent_concepts::AgentConceptsApiClient;
    pub use super::s7_document_documents::DocumentDocumentsApiClient;
    pub use super::s7_roster_documents::RosterDocumentsApiClient;
    pub use super::s7_agent_documents::AgentDocumentsApiClient;
    pub use super::s7_agent_notebooks::AgentNotebooksApiClient;
    pub use super::s7_document_notebooks::DocumentNotebooksApiClient;
    pub use super::s7_agent_sessions::AgentSessionsApiClient;
    pub use super::s7_sessionsync_sessions::SessionsyncSessionsApiClient;
    pub use super::s7_tools_tooling::ToolsToolingApiClient;
    pub use super::s7_agent_tooling::AgentToolingApiClient;
    pub use super::s7_agent_messaging::AgentMessagingApiClient;
    pub use super::s7_signal_messaging::SignalMessagingApiClient;
    pub use super::s7_slack_messaging::SlackMessagingApiClient;
    pub use super::s7_telegram_messaging::TelegramMessagingApiClient;
    pub use super::s7_a2a_messaging::A2aMessagingApiClient;
    pub use super::s7_rpc_compose_hub::RpcComposeHubApiClient;
    pub use super::s7_a2a_hub::A2aHubApiClient;
    pub use super::s7_agent_hub::AgentHubApiClient;
    pub use super::s7_sessionsync_hub::SessionsyncHubApiClient;
    pub use super::s7_tasks_hub::TasksHubApiClient;
    pub use super::s7_artifact_hub::ArtifactHubApiClient;
    pub use super::s7_link_hub::LinkHubApiClient;
    pub use super::s7_talk_hub::TalkHubApiClient;
    pub use super::s7_search_providers_hub::SearchProvidersHubApiClient;
    pub use super::s7_app_hub::AppHubApiClient;
    pub use super::s7_rpc_hub::RpcHubApiClient;
    pub use super::s7_agent_settings::AgentSettingsApiClient;
    pub use super::s7_tools_settings::ToolsSettingsApiClient;
    pub use super::s7_update_settings::UpdateSettingsApiClient;
    pub use super::s7_rpc_compose_settings::RpcComposeSettingsApiClient;
    pub use super::s7_web_settings::WebSettingsApiClient;
    pub use agent::rpc_jobs::JobsApiClient;
    pub use super::hub::CoreApiClient;
    pub use super::sessions::SessionsApiClient;
    pub use agent::rpc_turns::TurnsApiClient;
    pub use super::settings::SettingsApiClient;
    pub use super::tooling::ToolingApiClient;
    pub use agent::rpc_skills::SkillsApiClient;
    pub use computer::rpc::ScreensApiClient;
    pub use mcp_server::rpc::McpApiClient;
    pub use super::concepts::ConceptsApiClient;
    pub use super::documents::DocumentsApiClient;
    pub use super::notebooks::NotebooksApiClient;
    pub use ::codeindex::rpc::CodemapApiClient;
    pub use super::messaging::MessagingApiClient;
    pub use nodes::NetworkApiClient;
    pub use schedule::RoutinesApiClient;
    pub use roster::BoardsApiClient;
    pub use super::artifacts::ArtifactsApiClient;
    pub use secrets::CredentialsApiClient;
    pub use talk::SpeechApiClient;
    pub use meeting::MeetingsApiClient;
    pub use ::cells::rpc::KernelsApiClient;
    pub use dreaming::DreamsApiClient;
    pub use autoresearch::ResearchApiClient;
    pub use ::sandbox::rpc::SandboxesApiClient;
}

#[cfg(test)]
#[path = "api/s7_api_tests.rs"]
mod tests;
#[cfg(all(test, feature = "ts"))]
pub(crate) use tests::test_rpc_module;
