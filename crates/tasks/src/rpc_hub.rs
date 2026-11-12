use crate as tasks;
use agent::Agents;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use runtime::SessionId;
use tools::{Answer, Approval, ApprovalId, AskId, Decision, Resolve};
use rpc_core::{ask, send, ASK_FAILED, Server};
use tasks::rpc_support::{route_question, tasks_actor};
use sessionsync::rpc_support::RpcSessionMoveExt;

/// Hub RPC methods owned by this plugin.
#[rpc(server, client)]
pub trait TasksHubApi {
    /// Answer a pending approval; `true` if it was still pending.
    #[method(name = "resolve_approval", with_extensions)]
    async fn resolve_approval(&self, id: ApprovalId, decision: Decision) -> RpcResult<bool>;
    /// Answer a pending `ask_user`: one answer per question, or `None` to
    /// dismiss the set, which also cancels the turn. `true` if it was still pending.
    #[method(name = "answer_ask", with_extensions)]
    async fn answer_ask(&self, id: AskId, answers: Option<Vec<Answer>>) -> RpcResult<bool>;
    /// 182 D10: the task lives on engine `to` (a mesh id) from now on — only its desk works it there.
    /// `to` must be online and have the task's agent (theuth is everywhere); a done or cancelled
    /// task does not move. A live work session of its checkout owned here moves first
    /// (`session_move`); that move's refusal refuses this one. Off an offline home the move
    /// lands here at once and replicates when that engine returns.
    #[method(name = "task_move_home", with_extensions)]
    async fn task_move_home(&self, id: String, to: String) -> RpcResult<tasks::Task>;
    /// 182 D9 "Move them to <this engine>": every task waiting on the offline engine `from`
    /// moves to this engine; the ones whose agent is not here are named in `refused`.
    #[method(name = "task_move_home_waiting", with_extensions)]
    async fn task_move_home_waiting(&self, from: String) -> RpcResult<tasks::MovedHome>;
}

#[async_trait]
impl TasksHubApiServer for Server {

    async fn resolve_approval(
        &self,
        ext: &Extensions,
        id: ApprovalId,
        decision: Decision,
    ) -> RpcResult<bool> {
        if let Some(r) = route_question(ext, &id.0, "resolve_approval", (id, decision)).await? {
            return Ok(r);
        }
        let pending = ask::<Approval, _>(tools::APPROVAL_ACTOR, Resolve { id, decision }).await?;
        // 130 E4: the receipt the companion reads back (`activity_list`).
        if pending {
            let what = match decision {
                Decision::Allow => "allowed",
                Decision::Deny => "denied",
            };
            tasks::activity::record(
                &self.rt,
                tasks::activity::ActivityKind::ApprovalResolved,
                &rpc_core::Client::actor(ext),
                &id.0.to_string(),
                format!("{what} approval {}", id.0),
            )
            .await;
        }
        Ok(pending)
    }

    async fn answer_ask(
        &self,
        ext: &Extensions,
        id: AskId,
        answers: Option<Vec<Answer>>,
    ) -> RpcResult<bool> {
        if let Some(r) = route_question(ext, &id.0, "answer_ask", (id, &answers)).await? {
            return Ok(r);
        }
        let what = if answers.is_some() {
            "answered"
        } else {
            "dismissed"
        };
        let pending = agent::answer_ask(id, answers)
            .await
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e.to_string(), None::<()>))?;
        if pending {
            tasks::activity::record(
                &self.rt,
                tasks::activity::ActivityKind::AskAnswered,
                &rpc_core::Client::actor(ext),
                &id.0.to_string(),
                format!("{what} question {}", id.0),
            )
            .await;
        }
        Ok(pending)
    }

    async fn task_move_home(
        &self,
        ext: &Extensions,
        id: String,
        to: String,
    ) -> RpcResult<tasks::Task> {
        let task = send(&tasks_actor()?, tasks::GetTask(tasks::TaskId(id.clone()))).await?;
        let actor = rpc_core::Client::actor(ext);
        let m = |check_only, session_moved| tasks::MoveHome {
            id: tasks::TaskId(id.clone()),
            to: to.clone(),
            actor: actor.clone(),
            session: None,
            check_only,
            session_moved,
        };
        // 182 S4: the move is checked before anything moves — a refused move
        // never leaves its session on the other machine.
        send(&tasks_actor()?, m(true, false)).await?;
        // 182 D10: a live work session of its checkout, owned here, moves first (181) so
        // it continues there; its refusal (a paused tree, the target refusing) refuses
        // the task's move. A desk or harness session stays: it is this engine's.
        let mut session_moved = false;
        if let Some(c) = task.checkout.as_ref().filter(|c| !c.orphaned)
            && let Ok(session) = c.session.parse::<SessionId>()
        {
            let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
            let me = store.engine().to_owned();
            let owned_here = to != me
                && matches!(store.fence(session).await, Ok(Some((_, owner))) if owner == me);
            let movable = owned_here
                && store
                    .session(session)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|s| s.desk.is_none() && s.agent.is_none());
            if movable {
                RpcSessionMoveExt::session_move(self, ext, session, to.clone()).await?;
                session_moved = true;
            }
        }
        send(&tasks_actor()?, m(false, session_moved)).await
    }

    async fn task_move_home_waiting(
        &self,
        ext: &Extensions,
        from: String,
    ) -> RpcResult<tasks::MovedHome> {
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        let m = tasks::MoveWaiting {
            from,
            to: store.engine().to_owned(),
            actor: rpc_core::Client::actor(ext),
        };
        send(&tasks_actor()?, m).await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    TasksHubApiServer::into_rpc(server)
}
