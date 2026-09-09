//! `just ts`: `cargo test -p rpc --features ts export_bindings` writes
//! `bindings/*.ts` for every wire type.

#[cfg(feature = "ts")]
#[test]
fn export_bindings() {
    use ts_rs::TS;
    let cfg = ts_rs::Config::new().with_out_dir("../../bindings");
    macro_rules! export {
        ($($t:ty),*) => {$( <$t>::export_all(&cfg).expect(stringify!($t)); )*};
    }
    export!(
        agent::AgentEvent,
        agent::SessionSummary,
        agent::HistoryEntry,
        agent::Loaded,
        agent::Usage,
        agent::ModelInfo,
        agent::SkillMeta,
        agent::SkillBody,
        tools::ToolSpec,
        tools::Risk,
        tools::PolicyAction,
        tools::McpServer,
        tools::McpToolInfo,
        tools::McpProgress,
        tools::ApprovalRequested,
        tools::ApprovalResolved,
        tools::ApprovalId,
        tools::Decision,
        tools::ApprovalsState,
        tools::RepoInfo,
        tools::git::SyncResult,
        runtime::SessionId,
        runtime::TurnId
    );
}
