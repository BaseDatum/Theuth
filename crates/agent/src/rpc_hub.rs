use crate as agent;
use agent::rpc_forward as forward;
use std::path::PathBuf;
use agent::{Agents, ListAgents};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use runtime::SessionId;
use tools::{ApprovalsState, GetApprovals, JOBS_ACTOR, Jobs, ToolRegistry, ListTerminals, TerminalSummary};
use crate::pathac::PathCompletion;
use rpc_core::{ask, send, failed, peer, owner, Server};
use mesh::rpc_support::mesh_actor;
use agent::rpc_document::{doc_forward, machine_name, named, to_doc_server};
use document::rpc_support::{doc_send, documents, nb_result};
use crate::pathac;
use agent::rpc_support::RpcServerExt;

/// Hub RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait AgentHubApi {
    /// PTY jobs of `session` (and, with `descendants: true`, its subtree)
    /// whose terminal is still retained (51 §7.3).
    #[method(name = "list_terminals", with_extensions)]
    async fn list_terminals(
        &self,
        session: SessionId,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<TerminalSummary>>;
    /// PATH-AC: candidates for the path token under `cursor` in `input`,
    /// listed by the engine running `session` (`None`: this engine, its
    /// cwd). Relative paths resolve against the session's cwd; `~`/`$VAR`
    /// expand for the lookup only. Empty when nothing triggers, the
    /// directory is missing or the owner is unreachable: never an error.
    /// 183 W-12: `engine` (mesh id) lists on that engine when `session` is
    /// `None` (New chat on a machine); a missing trailing param is `None`.
    #[method(name = "path_complete", with_extensions)]
    async fn path_complete(
        &self,
        session: Option<SessionId>,
        input: String,
        cursor: usize,
        engine: Option<String>,
    ) -> RpcResult<PathCompletion>;
    /// 183 §4.5 (W-5): the approvals switches as they stand, for `session`
    /// (answered by its owner) or the global default with `None`.
    #[method(name = "get_approvals", with_extensions)]
    async fn get_approvals(&self, session: Option<SessionId>) -> RpcResult<ApprovalsState>;
    /// Add a source to a notebook (144 §4): pre-flight, the `new` row and card at once, then the ingest job
    /// (`SourceEvent` on `subscribe_events`). Exactly one origin field. Hub only.
    #[method(name = "source_add", with_extensions)]
    async fn source_add(
        &self,
        args: document::ingest::SourceAddArgs,
    ) -> RpcResult<document::Source>;
    /// Re-run the ingest from the origin or the blob (144 §4.2); refused while queued or running. Hub only.
    #[method(name = "source_refresh", with_extensions)]
    async fn source_refresh(&self, id: document::DocId) -> RpcResult<document::Source>;
    /// The three builtins, then the skills whose front matter has `notebook_insight: true`.
    #[method(name = "insight_skills")]
    async fn insight_skills(&self) -> RpcResult<Vec<String>>;
    /// 184 §4.8: "Move to ▸ machine" (owner only): the note's server writes `home = to` and hands the
    /// live copy to `to`. Notes only; boards, meeting notes, task threads and unhomed kinds are refused.
    #[method(name = "doc_move_home", with_extensions)]
    async fn doc_move_home(
        &self,
        doc: document::DocId,
        to: String,
    ) -> RpcResult<document::DocSummary>;
    /// Stop the whole process. Logged with the calling connection (79 smoke bug 7).
    #[method(name = "shutdown", with_extensions)]
    async fn shutdown(&self) -> RpcResult<()>;
}

#[async_trait]
impl AgentHubApiServer for Server {

    async fn list_terminals(
        &self,
        ext: &Extensions,
        session: SessionId,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<TerminalSummary>> {
        self.authorize(ext, session).await?;
        let mut sessions = vec![session];
        if descendants == Some(true) {
            let agents = ask::<Agents, _>(agent::AGENT_ACTOR, ListAgents { session }).await?;
            sessions.extend(agents.into_iter().map(|a| a.id));
        }
        ask::<Jobs, _>(JOBS_ACTOR, ListTerminals { sessions }).await
    }

    async fn path_complete(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        input: String,
        cursor: usize,
        engine: Option<String>,
    ) -> RpcResult<PathCompletion> {
        peer(ext)?;
        self.authorize_opt(ext, session).await?;
        // 183 W-12: "Browse xeus…" with no session yet completes on that engine
        // (nothing when it cannot answer, never this machine's paths).
        if session.is_none() {
            match forward::to_engine::<PathCompletion>(
                ext,
                engine.as_deref(),
                "path_complete",
                (None::<SessionId>, &input, cursor),
            )
            .await
            {
                Ok(None) => {}
                Ok(Some(r)) => return Ok(r),
                Err(_) => return Ok(PathCompletion::default()),
            }
        }
        let mut cwd = None;
        if let Some(session) = session {
            if let Some(owner) = forward::owner_peer(session).await {
                let forwarded: RpcResult<Option<PathCompletion>> = forward::to_peer(
                    ext,
                    &owner,
                    "path_complete",
                    (Some(session), &input, cursor),
                )
                .await;
                return Ok(forwarded.ok().flatten().unwrap_or_default());
            }
            let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
            cwd = store
                .session(session)
                .await
                .ok()
                .flatten()
                .and_then(|s| s.cwd)
                .map(PathBuf::from);
        }
        let cwd = cwd
            .or_else(|| std::env::current_dir().ok())
            .or_else(std::env::home_dir)
            .unwrap_or_default();
        Ok(pathac::complete(&cwd, &input, cursor).await)
    }

    async fn get_approvals(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<ApprovalsState> {
        self.authorize_opt_fwd(ext, session).await?;
        // 183 W-5: the effective value mixes the row's pin with the owner's
        // `tools.approvals`; owner offline → `-32010` (183-notes deviation 7).
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "get_approvals", (session,)).await?
        {
            return Ok(r);
        }
        // The row is the pin's truth (D4); the registry holds it only once the
        // session's actor spawned here, so a session idle since a restart or a
        // move would read "inherit". ponytail: a setter racing this read can be
        // overwritten by the row it replaced; both are user clicks.
        if let Some(s) = session
            && let Ok(store) = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await
            && let Ok(Some((pin, _))) = store.session_settings(s).await
        {
            let _ = ask::<ToolRegistry, _>(
                tools::REGISTRY_ACTOR,
                tools::RestoreApprovals { session: s, pin },
            )
            .await;
        }
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, GetApprovals { session }).await
    }

    async fn source_add(
        &self,
        ext: &Extensions,
        args: document::ingest::SourceAddArgs,
    ) -> RpcResult<document::Source> {
        owner(ext)?;
        // 16 §5 (2026-11-11): the authenticated owner may add a local-network URL.
        let r = nb_result(
            tools::egress::scope(
                true,
                document::ingest::source_add(
                    &documents()?,
                    args.clone(),
                    document::Principal::Owner { device: None },
                ),
            )
            .await,
        );
        doc_forward(ext, "source_add", (&args,), r).await
    }

    async fn source_refresh(
        &self,
        ext: &Extensions,
        id: document::DocId,
    ) -> RpcResult<document::Source> {
        owner(ext)?;
        // 16 §5 (2026-11-11): the authenticated owner refreshes under the owner's grant.
        let r = nb_result(
            tools::egress::scope(
                true,
                document::ingest::source_refresh(
                    &documents()?,
                    id.clone(),
                    document::Principal::Owner { device: None },
                ),
            )
            .await,
        );
        doc_forward(ext, "source_refresh", (&id,), r).await
    }

    async fn insight_skills(&self) -> RpcResult<Vec<String>> {
        let mut out = document::ingest::insights::builtin_names();
        out.extend(agent::notebook_context::insight_skills(
            &agent::skills::dir(&self.rt.data_dir),
        ));
        Ok(out)
    }

    async fn doc_move_home(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        to: String,
    ) -> RpcResult<document::DocSummary> {
        owner(ext)?;
        // §4.8: the note's server runs the checks and hands the live copy over.
        let docs = documents()?;
        if let Some(v) = to_doc_server(ext, &docs, &doc, "doc_move_home", (&doc, &to)).await? {
            return Ok(v);
        }
        let r = doc_send(
            &docs,
            document::MoveHome {
                doc,
                to: mesh::EngineId(to.clone()),
                checks: true,
            },
        )
        .await;
        // §12.4: the actor's copy names engine ids; the user reads machine names.
        match r {
            Err(e) if e.message().contains(&to) => {
                let st = match mesh_actor() {
                    Ok(m) => send(&m, mesh::Status).await.ok(),
                    Err(_) => None,
                };
                let name = st.as_ref().and_then(|st| machine_name(st, &to));
                Err(failed(named(e.message(), &to, name)))
            }
            r => r,
        }
    }

    async fn shutdown(&self, ext: &Extensions) -> RpcResult<()> {
        match ext.get::<rpc_core::RpcPeer>() {
            Some(p) => tracing::warn!(
                peer = p.id(),
                remote = p.remote(),
                "shutdown requested over rpc"
            ),
            None => tracing::warn!("shutdown requested over rpc (unauthenticated connection)"),
        }
        // 109 §9.2: the drain runs before the token (every turn's cancel is
        // its child); off the request so the caller's reply is immediate.
        let rt = self.rt.clone();
        tokio::spawn(async move {
            agent::supervisor::drain_engine(&rt, "shutdown").await;
            rt.shutdown.cancel();
        });
        Ok(())
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(AgentHubApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}
