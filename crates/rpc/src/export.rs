//! `just ts`: `cargo test -p rpc --features ts export_bindings` writes
//! `bindings/*.ts` for every wire type and `bindings/methods.json`, the sorted
//! list of every registered method name (25 §7).

#[cfg(feature = "ts")]
#[tokio::test]
async fn export_bindings() {
    use ts_rs::TS;
    let out = "../../bindings";
    let cfg = ts_rs::Config::new().with_out_dir(out);
    macro_rules! export {
        ($($t:ty),*) => {$( <$t>::export_all(&cfg).expect(stringify!($t)); )*};
    }
    export!(
        crate::api::Info,
        agent::AgentEvent,
        agent::SessionSummary,
        agent::HistoryEntry,
        agent::Loaded,
        agent::QueuedInput,
        agent::Usage,
        agent::ModelInfo,
        agent::SkillMeta,
        agent::SkillBody,
        agent::LoginInfo,
        agent::LoginFinished,
        agent::ProviderInfo,
        agent::AgentInfo,
        agent::AgentState,
        acp::EntryInfo,
        acp::registry::RegistryAgent,
        knowledge::Meta,
        knowledge::Concept,
        knowledge::Hit,
        knowledge::Put,
        knowledge::Source,
        knowledge::KnowledgeOp,
        knowledge::Sync,
        knowledge::HarvestResult,
        knowledge::Change,
        knowledge::History,
        knowledge::KnowledgeChanged,
        tools::ToolSpec,
        tools::Risk,
        tools::PolicyAction,
        tools::Gate,
        tools::McpServer,
        tools::McpToolInfo,
        tools::McpProgress,
        tools::ApprovalRequested,
        tools::ApprovalResolved,
        tools::ApprovalId,
        tools::Decision,
        tools::ApprovalsState,
        tools::RepoInfo,
        tools::JobEvent,
        tools::JobView,
        tools::JobId,
        tools::JobExit,
        tools::git::SyncResult,
        runtime::SessionId,
        runtime::TurnId,
        runtime::InputId,
        signal::SignalEvent,
        signal::SignalStatus,
        slack::SlackEvent,
        slack::SlackStatus
    );

    use crate::ApiServer;
    let rt = runtime::Runtime::new(figment::Figment::new(), std::env::temp_dir());
    let module = crate::api::Server::new(rt).into_rpc();
    let mut names: Vec<&str> = module.method_names().collect();
    names.sort_unstable();
    std::fs::write(
        format!("{out}/methods.json"),
        serde_json::to_string_pretty(&names).expect("methods.json"),
    )
    .expect("write methods.json");
}
