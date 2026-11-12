use agent::rpc_forward as forward;
use std::path::Path;
use agent::{Agents, CreateSession, CreateSessionOptions, SessionOpen};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use runtime::SessionId;
use sessionsync::types::{SessionOrphan, SessionOwner};
use tools::Retention;
use rpc_core::{ask, failed, peer, Server};
use sessionsync::rpc_support::RpcSessionMoveExt;
use agent::rpc_support::RpcServerExt;
use agent::rpc_session_state::RpcSessionTrackExt;
use sessionsync::rpc_session_state::RpcSessionWaitExt;

#[rpc(server, client)]
pub trait SessionsyncHubApi {
    /// 181: send `session`, owned here, to the paired engine `to` (its mesh
    /// id): `to` takes it (`session_take(moved)` over the mesh), this engine
    /// releases it, a running turn resumes there. Errors in plain words
    /// (`xeus is offline`, `this session is not owned here`, `xeus refused: …`).
    #[method(name = "session_move", with_extensions)]
    async fn session_move(&self, session: SessionId, to: String) -> RpcResult<SessionOwner>;
    /// Who owns `session` and whether that is this engine (68 §5).
    #[method(name = "session_owner", with_extensions)]
    async fn session_owner(&self, session: SessionId) -> RpcResult<SessionOwner>;
    /// Turns this engine wrote as a superseded owner and never delivered
    /// (68 §4.3), one entry per epoch; empty when none.
    #[method(name = "session_orphans", with_extensions)]
    async fn session_orphans(&self, session: SessionId) -> RpcResult<Vec<SessionOrphan>>;
    /// Open a root with explicit storage (51 §4.1): `retention: durable`
    /// (default) is `new_session` plus `project`; `private` needs `view` and
    /// binds the tree to the calling connection, returning the lease secret
    /// it presents on `resume_private` / `leave_session`. Always returns
    /// `SessionOpen`.
    #[method(name = "create_session", with_extensions)]
    async fn create_session(&self, options: CreateSessionOptions) -> RpcResult<SessionOpen>;
}

#[async_trait]
impl SessionsyncHubApiServer for Server {

    async fn session_move(
        &self,
        ext: &Extensions,
        session: SessionId,
        to: String,
    ) -> RpcResult<SessionOwner> {
        RpcSessionMoveExt::session_move(self, ext, session, to).await
    }

    async fn session_owner(&self, ext: &Extensions, session: SessionId) -> RpcResult<SessionOwner> {
        self.authorize(ext, session).await?;
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        sessionsync::owner(&store, session)
            .await
            .map_err(|e| failed(format!("{e:#}")))?
            .ok_or_else(|| failed(format!("{session}: no such session")))
    }

    async fn session_orphans(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Vec<SessionOrphan>> {
        self.authorize(ext, session).await?;
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        sessionsync::orphans(&store, session)
            .await
            .map_err(|e| failed(format!("{e:#}")))
    }

    async fn create_session(
        &self,
        ext: &Extensions,
        options: CreateSessionOptions,
    ) -> RpcResult<SessionOpen> {
        let peer = peer(ext)?;
        // 183 §5.6 (W-10): New chat on another machine: that engine creates and
        // owns the row from epoch 1. Until the row replicates here, calls for it
        // are forwarded there by id (and its reads while it is in use). A
        // private tree belongs to this window's connection and never
        // replicates: it cannot start elsewhere.
        if let Some(engine) = options.engine.clone() {
            if options.retention == Retention::Private && !engine.is_empty() {
                let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
                if engine != store.engine() {
                    return Err(failed(
                        "a private chat runs on this machine; it cannot start on another",
                    ));
                }
            }
            let mut there = options.clone();
            there.engine = None;
            if let Some(open) =
                forward::to_engine::<SessionOpen>(ext, Some(&engine), "create_session", (there,))
                    .await?
            {
                sessionsync::owner::expect(open.session, replica::PeerId(engine.clone()));
                self.wait_for_row(open.session, &engine).await;
                return Ok(open);
            }
        }
        // A forwarded create names a folder of this machine (§13 row 12).
        if ext.get::<forward::Forwarded>().is_some()
            && let Some(cwd) = options.cwd.as_deref()
            && !Path::new(cwd).is_dir()
        {
            return Err(failed(format!(
                "{cwd} does not exist on {}",
                platform::process::machine().name
            )));
        }
        let open = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            CreateSession {
                options,
                peer: Some((peer.id(), peer.closed_token())),
            },
        )
        .await?;
        self.track(&open, peer.id());
        Ok(open)
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    SessionsyncHubApiServer::into_rpc(server)
}
