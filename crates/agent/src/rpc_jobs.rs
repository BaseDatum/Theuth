// `subscribe_pty` has five parameters: the generated server method (plus `&self`, the pending
// sink and `ext`) exceeds clippy's limit, and the macro output takes no item attribute.
#![allow(clippy::too_many_arguments)]

mod history;

use crate::rpc_engine;
use crate::rpc_forward as forward;
use crate::rpc_support::{RpcServerExt, attach_pty};
pub use history::{JobCursor, JobLogPage};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::PendingSubscriptionSink;
use kameo::error::SendError;
use rpc_core::{Server, ask, failed, lookup};
use runtime::SessionId;
use tools::{
    CancelJob, CancelOrigin, JOBS_ACTOR, JobId, JobView, Jobs, Key, ListJobs, LiveCounts,
    LiveCountsRow, PtyChunk, PtyInput, PtyResize, PtyScreen, PtyScreenReply, PtyViewport,
};

#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait JobsApi {
    /// Background jobs of a session (14 §2): id, state, seconds, log path, tail.
    #[method(name = "list_jobs", with_extensions)]
    async fn list_jobs(&self, session: SessionId) -> RpcResult<Vec<JobView>>;
    /// The session's complete job journal merged with live jobs, newest first.
    #[method(name = "job_history", with_extensions)]
    async fn job_history(
        &self,
        session: SessionId,
        before: Option<JobCursor>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<JobView>>;
    /// Scrubbed log bytes; without an offset, returns the last page.
    /// PTYs use the existing terminal viewer instead of raw escape logs.
    #[method(name = "job_log", with_extensions)]
    async fn job_log(
        &self,
        session: SessionId,
        id: JobId,
        offset: Option<u64>,
        limit: Option<u32>,
    ) -> RpcResult<JobLogPage>;
    /// Every session with a running job, a live PTY or a pending reminder
    /// (ids only): the desktop's sidebar snapshot after connect / resync.
    #[method(name = "list_live_counts")]
    async fn list_live_counts(&self) -> RpcResult<Vec<LiveCountsRow>>;
    /// Kill a running job; `Err` when unknown or already finished.
    #[method(name = "cancel_job", with_extensions)]
    async fn cancel_job(&self, session: SessionId, id: JobId) -> RpcResult<()>;
    /// One page of a PTY job's retained terminal (51 §7): `Unchanged` when
    /// `if_revision` still matches, `Unavailable` when the job's terminal is
    /// gone. An error `terminal busy` means every render slot is taken; retry.
    /// `engine` (mesh id, owner only): the job runs on that engine.
    #[method(name = "pty_screen", with_extensions)]
    async fn pty_screen(
        &self,
        session: SessionId,
        job: JobId,
        viewport: PtyViewport,
        if_revision: Option<u64>,
        engine: Option<String>,
    ) -> RpcResult<PtyScreenReply>;
    /// A human types into a running PTY job of `session` (55 §2.1): `text`
    /// verbatim, then `keys`, encoded for the program's current modes. Returns
    /// once written; the viewer keeps polling `pty_screen`. Errors when the
    /// job is unknown, finished, or the write fails. `engine` (mesh id, owner
    /// only): the job runs on that engine.
    #[method(name = "pty_input", with_extensions)]
    async fn pty_input(
        &self,
        session: SessionId,
        job: JobId,
        text: Option<String>,
        keys: Option<Vec<Key>>,
        engine: Option<String>,
    ) -> RpcResult<()>;
    /// Resize a running PTY job's grid (the program gets SIGWINCH). Rows
    /// 2–200, columns 10–400. Errors when the job is unknown or finished.
    /// `engine` (mesh id, owner only): the job runs on that engine.
    #[method(name = "pty_resize", with_extensions)]
    async fn pty_resize(
        &self,
        session: SessionId,
        job: JobId,
        rows: u16,
        cols: u16,
        engine: Option<String>,
    ) -> RpcResult<()>;
    /// Attach a terminal emulator to a PTY job: first a `reset` item with
    /// the retained buffer as escape sequences (base64), then every byte the
    /// program writes as `data` items, in order, and `exit` once it ends.
    /// `rows`/`cols` resize the pty before the snapshot. A client that falls
    /// behind gets a fresh `reset` instead of a gap. `engine` (mesh id, owner
    /// only): the job runs on that engine; its items are relayed over the
    /// mesh (`theuth/pty/1`).
    #[subscription(name = "subscribe_pty", unsubscribe = "unsubscribe_pty", item = PtyChunk, with_extensions)]
    async fn subscribe_pty(
        &self,
        session: SessionId,
        job: JobId,
        rows: Option<u16>,
        cols: Option<u16>,
        engine: Option<String>,
    ) -> SubscriptionResult;

    /// Pending reminder details, read from the session's owner.
    #[method(name = "list_reminders", with_extensions)]
    async fn list_reminders(
        &self,
        session: SessionId,
    ) -> RpcResult<Vec<tools::reminder::PendingReminderView>>;

    /// Cancel a pending reminder without delivering it.
    #[method(name = "cancel_reminder", with_extensions)]
    async fn cancel_reminder(&self, session: SessionId, id: String) -> RpcResult<()>;
}

#[async_trait]
impl JobsApiServer for Server {
    async fn list_jobs(&self, ext: &Extensions, session: SessionId) -> RpcResult<Vec<JobView>> {
        self.authorize(ext, session).await?;
        ask::<Jobs, _>(JOBS_ACTOR, ListJobs(session)).await
    }
    async fn job_history(
        &self,
        ext: &Extensions,
        session: SessionId,
        before: Option<JobCursor>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<JobView>> {
        self.authorize_fwd(ext, session).await?;
        if let Some(reply) =
            forward::route(ext, session, "job_history", (session, &before, limit)).await?
        {
            return Ok(reply);
        }
        let store = ask::<crate::Agents, _>(crate::AGENT_ACTOR, crate::DurableStore).await?;
        let live = ask::<Jobs, _>(JOBS_ACTOR, ListJobs(session)).await?;
        history::history(&store, session, live, before, limit)
            .await
            .map_err(failed)
    }

    async fn job_log(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: JobId,
        offset: Option<u64>,
        limit: Option<u32>,
    ) -> RpcResult<JobLogPage> {
        self.authorize_fwd(ext, session).await?;
        if let Some(reply) =
            forward::route(ext, session, "job_log", (session, &id, offset, limit)).await?
        {
            return Ok(reply);
        }
        let store = ask::<crate::Agents, _>(crate::AGENT_ACTOR, crate::DurableStore).await?;
        let live = ask::<Jobs, _>(JOBS_ACTOR, ListJobs(session)).await?;
        history::log(&store, session, &id, live, offset, limit)
            .await
            .map_err(failed)
    }

    async fn list_live_counts(&self) -> RpcResult<Vec<LiveCountsRow>> {
        ask::<Jobs, _>(JOBS_ACTOR, LiveCounts).await
    }

    /// Every RPC caller is a human surface (TUI `/jobs kill`, the desktop
    /// row): the model learns of the kill by a one-line `<job_result>`.
    async fn cancel_job(&self, ext: &Extensions, session: SessionId, id: JobId) -> RpcResult<()> {
        self.authorize(ext, session).await?;
        ask::<Jobs, _>(
            JOBS_ACTOR,
            CancelJob {
                session,
                id,
                origin: CancelOrigin::Ui,
            },
        )
        .await
    }

    async fn pty_screen(
        &self,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        viewport: PtyViewport,
        if_revision: Option<u64>,
        engine: Option<String>,
    ) -> RpcResult<PtyScreenReply> {
        if let Some(peer) = rpc_engine::pty_peer(ext, engine.as_deref()).await? {
            let params = (session, &job, &viewport, if_revision);
            return forward::call_peer(ext, &peer, "pty_screen", params).await;
        }
        self.authorize(ext, session).await?;
        lookup::<Jobs>(JOBS_ACTOR)?
            .ask(PtyScreen {
                session,
                id: job,
                viewport,
                if_revision,
            })
            .await
            .map_err(|e| match e {
                SendError::HandlerError(busy) => failed(format!("terminal busy: {busy}")),
                other => failed(other.to_string()),
            })
    }

    async fn pty_input(
        &self,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        text: Option<String>,
        keys: Option<Vec<Key>>,
        engine: Option<String>,
    ) -> RpcResult<()> {
        if let Some(peer) = rpc_engine::pty_peer(ext, engine.as_deref()).await? {
            let params = (session, &job, &text, &keys);
            return forward::call_peer(ext, &peer, "pty_input", params).await;
        }
        self.authorize(ext, session).await?;
        lookup::<Jobs>(JOBS_ACTOR)?
            .ask(PtyInput {
                session,
                id: job,
                text,
                keys: keys.unwrap_or_default(),
            })
            .await
            .map_err(|e| match e {
                SendError::HandlerError(msg) => failed(msg),
                other => failed(other.to_string()),
            })
    }

    async fn pty_resize(
        &self,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        rows: u16,
        cols: u16,
        engine: Option<String>,
    ) -> RpcResult<()> {
        if let Some(peer) = rpc_engine::pty_peer(ext, engine.as_deref()).await? {
            let params = (session, &job, rows, cols);
            return forward::call_peer(ext, &peer, "pty_resize", params).await;
        }
        self.authorize(ext, session).await?;
        lookup::<Jobs>(JOBS_ACTOR)?
            .ask(PtyResize {
                session,
                id: job,
                rows,
                cols,
            })
            .await
            .map_err(|e| match e {
                SendError::HandlerError(msg) => failed(msg),
                other => failed(other.to_string()),
            })
    }

    async fn subscribe_pty(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        session: SessionId,
        job: JobId,
        rows: Option<u16>,
        cols: Option<u16>,
        engine: Option<String>,
    ) -> SubscriptionResult {
        let remote = async {
            let Some(peer) = rpc_engine::pty_peer(ext, engine.as_deref()).await? else {
                return Ok(None);
            };
            let open = rpc_engine::PtyOpen { session, job: job.clone(), rows, cols };
            rpc_engine::open(&peer, open).await.map(Some)
        };
        let remote = match remote.await {
            Ok(r) => r,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        if let Some((read, write)) = remote {
            let sink = pending.accept().await?;
            tokio::spawn(rpc_engine::relay(read, write, sink));
            return Ok(());
        }
        let admit = async {
            self.authorize(ext, session).await?;
            attach_pty(session, &job, rows.zip(cols)).await
        };
        let at = match admit.await {
            Ok(a) => a,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        let reattach = move || {
            let job = job.clone();
            async move { attach_pty(session, &job, None).await }
        };
        tokio::spawn(rpc_engine::pump(at, reattach, sink));
        Ok(())
    }

    async fn list_reminders(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Vec<tools::reminder::PendingReminderView>> {
        self.authorize_fwd(ext, session).await?;
        if let Some(rows) =
            crate::rpc_forward::route(ext, session, "list_reminders", (session,)).await?
        {
            return Ok(rows);
        }
        ask::<Jobs, _>(JOBS_ACTOR, tools::reminder::ListReminders(session)).await
    }

    async fn cancel_reminder(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: String,
    ) -> RpcResult<()> {
        self.authorize_fwd(ext, session).await?;
        if let Some(reply) =
            crate::rpc_forward::route(ext, session, "cancel_reminder", (session, &id)).await?
        {
            return Ok(reply);
        }
        ask::<Jobs, _>(JOBS_ACTOR, tools::CancelReminder { session, id }).await
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(JobsApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}
