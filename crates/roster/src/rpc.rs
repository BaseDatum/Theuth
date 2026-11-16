use crate as roster;
use agent::ModelInfo;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use roster::rpc_support::{attention_actor, mask_alerts, roster_actor};
use rpc_core::{Server, lookup, send};
use runtime::SessionId;
use tasks::rpc_support::tasks_actor;

#[rpc_core_macros::rpc_descriptors(owner = "roster", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait BoardsApi {
    /// The built-in theuth agent's model (`roster.theuth_model`), if set (148 E3 / R3):
    /// its desk and the work sessions the desk starts run on it. `None` = same as new sessions.
    #[method(name = "theuth_agent_model")]
    async fn theuth_agent_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `roster.theuth_model`; `None` clears it. An unknown or un-keyed id is refused
    /// like `set_subagent_model`. An open theuth desk switches at once.
    #[method(name = "set_theuth_agent_model")]
    async fn set_theuth_agent_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// The built-in theuth agent's effort (`roster.theuth_effort`), if set (194 rows 4–5).
    /// `None` = same as new sessions.
    #[method(name = "theuth_agent_effort")]
    async fn theuth_agent_effort(&self) -> RpcResult<Option<String>>;
    /// Persist `roster.theuth_effort`; `None` clears it. Refused when theuth's effective model
    /// does not take it. Its open desk and the desk's new work sessions start on it. Returns
    /// the stored value.
    #[method(name = "set_theuth_agent_effort")]
    async fn set_theuth_agent_effort(&self, effort: Option<String>) -> RpcResult<Option<String>>;
    /// Unresolved desktop hand-offs (79 §6.6).
    #[method(name = "handoffs_list")]
    async fn handoffs_list(&self) -> RpcResult<Vec<roster::attention::Handoff>>;
    /// Resolve a hand-off: `outcome` is `done` | `cancelled`.
    #[method(name = "handoff_resolve")]
    async fn handoff_resolve(&self, id: String, outcome: String) -> RpcResult<()>;
    /// Every roster agent with status, desk, open tasks and month spend (67 §12).
    #[method(name = "agents_list")]
    async fn agents_list(&self) -> RpcResult<Vec<roster::AgentSummary>>;
    /// One agent file (67 §6.1). `Err` when unknown.
    #[method(name = "agent_get")]
    async fn agent_get(&self, name: String) -> RpcResult<roster::AgentFile>;
    /// The agent's desk session id, opening the desk when it is not (the Desks row's click). `Err` when the agent is unknown, paused, a proposal, or this engine is not the hub.
    #[method(name = "desk_open")]
    async fn desk_open(&self, name: String) -> RpcResult<SessionId>;
    /// Write `agents/<name>.md` from `file` (create or replace) — the file editor's save (67 §12).
    #[method(name = "agent_put")]
    async fn agent_put(&self, file: roster::AgentFile) -> RpcResult<()>;
    /// Flip an agent's `paused:` line; paused agents receive no wakes (67 §6.4).
    #[method(name = "agent_pause")]
    async fn agent_pause(&self, name: String, paused: bool) -> RpcResult<()>;
    /// Approve a proposed agent: clears `proposed_by`, unpauses (67 §6.5).
    #[method(name = "agent_approve", with_extensions)]
    async fn agent_approve(&self, name: String) -> RpcResult<()>;
    /// Delete `agents/<name>.md`; a pending proposal is refused with `note` on its hire task (67 §6.5). `theuth` is refused.
    #[method(name = "agent_remove")]
    async fn agent_remove(&self, name: String, note: Option<String>) -> RpcResult<()>;
    /// Tasks matching `filter`, oldest first (67 §12 board).
    #[method(name = "tasks_list")]
    async fn tasks_list(&self, filter: tasks::TaskFilter) -> RpcResult<Vec<tasks::Task>>;
    /// One task (67 §7.1). `Err` when unknown.
    #[method(name = "task_get")]
    async fn task_get(&self, id: String) -> RpcResult<tasks::Task>;
    /// Create a task as the user (`created_by: user`); the owner is woken (67 §7).
    #[method(name = "task_create")]
    async fn task_create(&self, input: tasks::TaskCreate) -> RpcResult<tasks::Task>;
    /// Patch a task as the user: assign, set state, blockers, placement (67 §7.2, §12).
    #[method(name = "task_update")]
    async fn task_update(&self, id: String, patch: tasks::TaskPatch) -> RpcResult<tasks::Task>;
    /// Move a task on the user's intent (67 §7.2): backlog ↔ todo, → cancelled,
    /// review → done | todo, blocked → todo. `Err` names the allowed targets.
    /// `next` (132 D3): with `done` on a review that offered next moves, the
    /// index of the pick — `**next** (n) <label>` in the thread, the owner woken `answer`.
    #[method(name = "task_move", with_extensions)]
    async fn task_move(
        &self,
        id: String,
        state: tasks::TaskState,
        note: Option<String>,
        next: Option<u32>,
    ) -> RpcResult<tasks::Task>;
    /// Append a comment to a task's thread as the user; wakes the owner (67 §7.3).
    #[method(name = "task_comment")]
    async fn task_comment(&self, id: String, text: String) -> RpcResult<()>;
    /// The task's field-level event log, oldest first (67 §7.2).
    #[method(name = "task_events")]
    async fn task_events(&self, id: String) -> RpcResult<Vec<tasks::TaskEvent>>;
    /// Tasks whose `blockers` name `id` (the inverse link, computed; 67 §7.1).
    #[method(name = "task_blocks")]
    async fn task_blocks(&self, id: String) -> RpcResult<Vec<tasks::TaskId>>;
    /// 182 D8: every engine's agents (the replicated per-engine roster, this engine's rows
    /// included), by name then engine — the Agents page rows and the Move to checks.
    #[method(name = "engine_agents_list")]
    async fn engine_agents_list(&self) -> RpcResult<Vec<roster::EngineAgent>>;
    /// The activity feed, newest first: `since_ms`, `kinds`, `limit` (default 50, max 500) (70 HUD).
    #[method(name = "activity_list")]
    async fn activity_list(
        &self,
        filter: tasks::activity::ActivityFilter,
    ) -> RpcResult<Vec<tasks::activity::ActivityEntry>>;
    /// The attention inbox (70 HUD): approvals, decisions, hires, blocked / review tasks,
    /// budgets, unreachable nodes — undismissed and unsnoozed, newest first. Badge = length.
    /// 155 §7.3 / §7.5: per connection — `alert` is blanked for items whose
    /// session this connection focuses (`set_event_focus`) and for `finished`
    /// alerts it has already seen.
    #[method(name = "attention_list", with_extensions)]
    async fn attention_list(&self) -> RpcResult<Vec<roster::attention::AttentionItem>>;
    /// Hide an attention item until `until_ms` (ms since epoch).
    #[method(name = "attention_snooze")]
    async fn attention_snooze(&self, id: String, until_ms: u64) -> RpcResult<()>;
    /// Hide an attention item for good.
    #[method(name = "attention_dismiss")]
    async fn attention_dismiss(&self, id: String) -> RpcResult<()>;
    /// The packet behind a decision / review attention item (101 §3): `decision:<id>` | `review:<task>`;
    /// `null` when none was raised.
    #[method(name = "packet_get")]
    async fn packet_get(&self, key: String) -> RpcResult<Option<tasks::packet::Packet>>;
    /// Answer the `decision:<id>` a task is blocked on, as the user (67 §11); the owner is re-woken.
    #[method(name = "task_decide", with_extensions)]
    async fn task_decide(&self, decision: String, answer: String) -> RpcResult<()>;
}

#[async_trait]
impl BoardsApiServer for Server {
    async fn theuth_agent_model(&self) -> RpcResult<Option<ModelInfo>> {
        send(&roster_actor()?, roster::TheuthModel).await
    }

    async fn set_theuth_agent_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        send(&roster_actor()?, roster::SetTheuthModel { model }).await
    }

    async fn theuth_agent_effort(&self) -> RpcResult<Option<String>> {
        send(&roster_actor()?, roster::TheuthEffort).await
    }

    async fn set_theuth_agent_effort(&self, effort: Option<String>) -> RpcResult<Option<String>> {
        send(&roster_actor()?, roster::SetTheuthEffort { effort }).await
    }

    // 79 §6.6 (H fills these; stubs so the method list is complete).

    async fn handoffs_list(&self) -> RpcResult<Vec<roster::attention::Handoff>> {
        send(&attention_actor()?, roster::attention::ListHandoffs).await
    }

    async fn handoff_resolve(&self, id: String, outcome: String) -> RpcResult<()> {
        send(
            &attention_actor()?,
            roster::attention::ResolveHandoff { id, outcome },
        )
        .await
    }

    async fn agents_list(&self) -> RpcResult<Vec<roster::AgentSummary>> {
        send(&roster_actor()?, roster::ListAgents).await
    }

    async fn agent_get(&self, name: String) -> RpcResult<roster::AgentFile> {
        send(&roster_actor()?, roster::GetAgent(name)).await
    }

    async fn desk_open(&self, name: String) -> RpcResult<SessionId> {
        send(&roster_actor()?, roster::OpenDesk(name)).await
    }

    async fn agent_put(&self, file: roster::AgentFile) -> RpcResult<()> {
        roster_actor()?
            .ask(roster::PutAgent(file))
            .await
            .map_err(|e| match e {
                kameo::error::SendError::HandlerError(message) => {
                    jsonrpsee::types::ErrorObjectOwned::owned(-32602, message, None::<()>)
                }
                e => rpc_core::failed(e.to_string()),
            })
    }

    async fn agent_pause(&self, name: String, paused: bool) -> RpcResult<()> {
        send(&roster_actor()?, roster::PauseAgent { name, paused }).await
    }

    async fn agent_approve(&self, ext: &Extensions, name: String) -> RpcResult<()> {
        let m = roster::ApproveAgent {
            name,
            actor: rpc_core::Client::actor(ext),
        };
        send(&roster_actor()?, m).await
    }

    async fn agent_remove(&self, name: String, note: Option<String>) -> RpcResult<()> {
        send(&roster_actor()?, roster::RemoveAgent { name, note }).await
    }

    async fn tasks_list(&self, filter: tasks::TaskFilter) -> RpcResult<Vec<tasks::Task>> {
        send(&tasks_actor()?, tasks::ListTasks(filter)).await
    }

    async fn task_get(&self, id: String) -> RpcResult<tasks::Task> {
        send(&tasks_actor()?, tasks::GetTask(tasks::TaskId(id))).await
    }

    async fn task_create(&self, input: tasks::TaskCreate) -> RpcResult<tasks::Task> {
        let m = tasks::CreateTask {
            input,
            created_by: "user".into(),
            session: None,
        };
        send(&tasks_actor()?, m).await
    }

    async fn task_update(&self, id: String, patch: tasks::TaskPatch) -> RpcResult<tasks::Task> {
        let m = tasks::UpdateTask {
            id: tasks::TaskId(id),
            patch,
            actor: "user".into(),
            session: None,
        };
        send(&tasks_actor()?, m).await
    }

    async fn task_move(
        &self,
        ext: &Extensions,
        id: String,
        state: tasks::TaskState,
        note: Option<String>,
        next: Option<u32>,
    ) -> RpcResult<tasks::Task> {
        let m = tasks::MoveTask {
            id: tasks::TaskId(id),
            state,
            note,
            done_because: None,
            next,
            actor: rpc_core::Client::actor(ext),
            session: None,
        };
        send(&tasks_actor()?, m).await
    }

    async fn task_comment(&self, id: String, text: String) -> RpcResult<()> {
        let m = tasks::CommentTask {
            id: tasks::TaskId(id),
            text,
            progress: false,
            actor: "user".into(),
            session: None,
        };
        send(&tasks_actor()?, m).await
    }

    async fn task_events(&self, id: String) -> RpcResult<Vec<tasks::TaskEvent>> {
        send(&tasks_actor()?, tasks::TaskEvents(tasks::TaskId(id))).await
    }

    async fn task_blocks(&self, id: String) -> RpcResult<Vec<tasks::TaskId>> {
        send(&tasks_actor()?, tasks::TaskBlocks(tasks::TaskId(id))).await
    }

    async fn engine_agents_list(&self) -> RpcResult<Vec<roster::EngineAgent>> {
        send(&roster_actor()?, roster::ListEngineAgents).await
    }

    async fn activity_list(
        &self,
        filter: tasks::activity::ActivityFilter,
    ) -> RpcResult<Vec<tasks::activity::ActivityEntry>> {
        let actor = lookup::<tasks::activity::Activity>(tasks::activity::ACTIVITY_ACTOR)?;
        send(&actor, tasks::activity::ListActivity(filter)).await
    }

    async fn attention_list(
        &self,
        ext: &Extensions,
    ) -> RpcResult<Vec<roster::attention::AttentionItem>> {
        let mut items: Vec<roster::attention::AttentionItem> =
            send(&attention_actor()?, roster::attention::ListAttention).await?;
        if let Some(peer) = ext.get::<rpc_core::RpcPeer>() {
            let focus = self.focus.read().get(&peer.id()).cloned();
            let mut seen = self.seen.write();
            let fresh = !seen.contains_key(&peer.id());
            mask_alerts(
                &mut items,
                focus.as_deref(),
                seen.entry(peer.id()).or_default(),
            );
            drop(seen);
            if fresh {
                self.forget_on_close(peer.clone());
            }
        }
        Ok(items)
    }

    async fn attention_snooze(&self, id: String, until_ms: u64) -> RpcResult<()> {
        send(
            &attention_actor()?,
            roster::attention::SnoozeAttention { id, until_ms },
        )
        .await
    }

    async fn attention_dismiss(&self, id: String) -> RpcResult<()> {
        send(&attention_actor()?, roster::attention::DismissAttention(id)).await
    }

    async fn packet_get(&self, key: String) -> RpcResult<Option<tasks::packet::Packet>> {
        send(&tasks_actor()?, tasks::GetPacket(key)).await
    }

    async fn task_decide(
        &self,
        ext: &Extensions,
        decision: String,
        answer: String,
    ) -> RpcResult<()> {
        let m = tasks::AnswerDecision {
            decision,
            answer,
            actor: rpc_core::Client::actor(ext),
            session: None,
        };
        send(&tasks_actor()?, m).await
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(BoardsApiServer::into_rpc(server), rpc_descriptors())
        .expect("roster RPC descriptors")
}
