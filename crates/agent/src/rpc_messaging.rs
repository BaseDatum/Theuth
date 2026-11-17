use crate as agent;
use agent::rpc_forward as forward;
use agent::{Agents, AnswerPeerRequest, CommunicationState, GetCommunication, GetPeerMessage, ListPeerMessages, ListPeers, PeerMessagePage, PeerPage, PeerReceipt, PeerRecord, PeerScope, SetCommunication, SetPeerScope};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use runtime::SessionId;
use uuid::Uuid;
use rpc_core::{ask, Server};
use agent::rpc_support::RpcServerExt;
use agent::communication::files::{
    self, ContactFile, ContactFileThumbnail, NoticeAdmission, NoticeStatus, holder, notices,
};
use std::path::PathBuf;

/// Messaging RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait AgentMessagingApi {
    /// Effective communication state (50 §10): the global switch alone with
    /// `None`, else `session`'s inherited/opted-out view, scope and alias.
    /// Authoritative: render it, never guess it.
    #[method(name = "get_communication", with_extensions)]
    async fn get_communication(&self, session: Option<SessionId>) -> RpcResult<CommunicationState>;
    /// `/communication on|off` for `session` (memory-only opt-out), or the
    /// persisted global ceiling with `None`. A session cannot override an
    /// ancestor or global off; the returned state says what blocks it.
    #[method(name = "set_communication", with_extensions)]
    async fn set_communication(
        &self,
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<CommunicationState>;
    /// Owner-only root scope (`owner` | `isolated`) and alias (50 §2.3);
    /// `alias: None` keeps, `Some("")` clears. `Err` for a non-root.
    #[method(name = "set_peer_scope", with_extensions)]
    async fn set_peer_scope(
        &self,
        root: SessionId,
        scope: PeerScope,
        alias: Option<String>,
    ) -> RpcResult<CommunicationState>;
    /// The directory `session` may address (50 §5.1), the same permission
    /// filter the model's `discover_peers` sees; bounded fields only.
    #[method(name = "list_peers", with_extensions)]
    async fn list_peers(
        &self,
        session: SessionId,
        query: Option<String>,
        cursor: Option<String>,
        limit: Option<u16>,
    ) -> RpcResult<PeerPage>;
    /// `session`'s own record of exchange `id` (50 §5.2); `Err` when it is
    /// neither sender nor recipient.
    #[method(name = "peer_message", with_extensions)]
    async fn peer_message(&self, session: SessionId, id: Uuid) -> RpcResult<PeerRecord>;
    /// `session`'s sent and received exchanges, newest first, with the exact
    /// ids a human may resolve now (`awaiting_answer` / `awaiting_reply`);
    /// `pending_only` restricts the rows to those. The durable snapshot a UI
    /// reloads after `PeerMessageChanged` or a reconnect.
    #[method(name = "list_peer_messages", with_extensions)]
    async fn list_peer_messages(
        &self,
        session: SessionId,
        cursor: Option<String>,
        limit: Option<u16>,
        pending_only: bool,
    ) -> RpcResult<PeerMessagePage>;
    /// Resolve pending request `reply_to` on behalf of `session` (50 §5.3):
    /// as its recipient the answer travels to the sender as a peer message
    /// from `session`; as its blocked sender its own wait resolves. Either
    /// way `answered_by = owner:<session>`; never an arbitrary waiter. 199 D15:
    /// `files` (absolute paths on this machine) go with an answer to a contact,
    /// only on the machine that owns the chat (elsewhere: `attach on <machine>`).
    #[method(name = "answer_peer_request", with_extensions)]
    async fn answer_peer_request(
        &self,
        session: SessionId,
        reply_to: Uuid,
        text: String,
        files: Option<Vec<PathBuf>>,
    ) -> RpcResult<PeerReceipt>;
    /// A peer message from `session` to `to` (`parent`, `session:<uuid>`,
    /// `peer:<name>`, a child's name), routed like the model's `send_message`
    /// tool without a wait; the MCP back-channel's `send_message` (75 §9).
    /// `Err` is a refused admission and nothing was sent.
    #[method(name = "send_message", with_extensions)]
    async fn send_message(
        &self,
        session: SessionId,
        to: String,
        text: String,
    ) -> RpcResult<PeerReceipt>;
    /// 199: file `id` as its holder records it; carried to `holder` (an endpoint id)
    /// over `theuth/rpc/1`. Errors: `no such file on <machine>`, `<machine> is offline`.
    #[method(name = "contact_file", with_extensions)]
    async fn contact_file(&self, holder: String, id: String) -> RpcResult<ContactFile>;
    /// 199: an image file's thumbnail, fitted to 512 px by the holder's guarded decoder.
    /// Errors: `not an image`, `too large for a thumbnail`, `no such file on <machine>`.
    #[method(name = "contact_file_thumbnail", with_extensions)]
    async fn contact_file_thumbnail(
        &self,
        holder: String,
        id: String,
    ) -> RpcResult<ContactFileThumbnail>;
    /// 199: a no-clobber copy of a received file to `dest` on the holder's disk; the
    /// path written. Errors: `no such file on <machine>`, `<dest> exists`.
    #[method(name = "contact_file_export", with_extensions)]
    async fn contact_file_export(
        &self,
        holder: String,
        id: String,
        dest: PathBuf,
    ) -> RpcResult<PathBuf>;
    /// 199 P2: fetch a received file of `session` from its holder into this engine's
    /// cache, verified against the chat's own `received` notice; the cached path.
    /// Errors: `no notice for <id> in this chat`, `<machine> is offline`, `verification failed`.
    #[method(name = "contact_file_fetch", with_extensions)]
    async fn contact_file_fetch(
        &self,
        session: SessionId,
        holder: String,
        id: String,
    ) -> RpcResult<PathBuf>;
    /// 199 D17 (internal: this engine, or a paired machine with the front door's
    /// `Forwarded` marker): the holder delivers one file's notice to the chat's owner,
    /// as automatic input from `contact_id`'s share. `notice_id` is
    /// `files::notice_id(holder, file)`; a repeat answers `accepted` and adds nothing.
    #[method(name = "contact_file_notice", with_extensions)]
    async fn contact_file_notice(
        &self,
        session: SessionId,
        notice_id: Uuid,
        contact_id: String,
        notice: String,
    ) -> RpcResult<NoticeAdmission>;
    /// 199 D17: `committed` once the owner's journal (or `errors`) holds `notice_id`.
    #[method(name = "contact_file_notice_status", with_extensions)]
    async fn contact_file_notice_status(
        &self,
        session: SessionId,
        notice_id: Uuid,
    ) -> RpcResult<NoticeStatus>;
    /// 199-E3 (internal: a paired machine's forwarded call): whether the foreign file `id`
    /// held by `holder` still has the authority (grant) it arrived under; the chat's owner
    /// asks before it gives the file's notice to the agent. `false` for an unknown file.
    #[method(name = "contact_file_authority", with_extensions)]
    async fn contact_file_authority(&self, holder: String, id: String) -> RpcResult<bool>;
}

#[async_trait]
impl AgentMessagingApiServer for Server {

    async fn get_communication(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
    ) -> RpcResult<CommunicationState> {
        self.authorize_opt_fwd(ext, session).await?;
        // 183 W-5: the owner's `agent.communication` decides.
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "get_communication", (session,)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, GetCommunication { session }).await
    }

    async fn set_communication(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        enabled: bool,
    ) -> RpcResult<CommunicationState> {
        self.authorize_opt_fwd(ext, session).await?;
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "set_communication", (session, enabled)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, SetCommunication { session, enabled }).await
    }

    async fn set_peer_scope(
        &self,
        ext: &Extensions,
        root: SessionId,
        scope: PeerScope,
        alias: Option<String>,
    ) -> RpcResult<CommunicationState> {
        self.authorize_fwd(ext, root).await?;
        if let Some(r) = forward::route(ext, root, "set_peer_scope", (root, &scope, &alias)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, SetPeerScope { root, scope, alias }).await
    }

    async fn list_peers(
        &self,
        ext: &Extensions,
        session: SessionId,
        query: Option<String>,
        cursor: Option<String>,
        limit: Option<u16>,
    ) -> RpcResult<PeerPage> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ListPeers {
                session,
                query,
                cursor,
                limit,
                active_only: false,
                kind: None,
            },
        )
        .await
    }

    async fn peer_message(
        &self,
        ext: &Extensions,
        session: SessionId,
        id: Uuid,
    ) -> RpcResult<PeerRecord> {
        local_first(
            async {
                self.authorize(ext, session).await?;
                ask::<Agents, _>(agent::AGENT_ACTOR, GetPeerMessage { session, id }).await
            },
            async {
                self.authorize_fwd(ext, session).await?;
                if let Some(r) =
                    forward::route(ext, session, "peer_message", (session, id)).await?
                {
                    return Ok(r);
                }
                ask::<Agents, _>(agent::AGENT_ACTOR, GetPeerMessage { session, id }).await
            },
        )
        .await
    }

    async fn list_peer_messages(
        &self,
        ext: &Extensions,
        session: SessionId,
        cursor: Option<String>,
        limit: Option<u16>,
        pending_only: bool,
    ) -> RpcResult<PeerMessagePage> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(
            ext,
            session,
            "list_peer_messages",
            (session, &cursor, limit, pending_only),
        )
        .await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            ListPeerMessages {
                session,
                cursor,
                limit,
                pending_only,
            },
        )
        .await
    }

    async fn answer_peer_request(
        &self,
        ext: &Extensions,
        session: SessionId,
        reply_to: Uuid,
        text: String,
        files: Option<Vec<PathBuf>>,
    ) -> RpcResult<PeerReceipt> {
        self.authorize_fwd(ext, session).await?;
        if let Some(files) = files.filter(|f| !f.is_empty()) {
            return answer_with_files(ext, session, reply_to, text, files).await;
        }
        if let Some(r) = forward::route(
            ext,
            session,
            "answer_peer_request",
            (session, reply_to, &text),
        )
        .await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            AnswerPeerRequest {
                session,
                reply_to,
                text,
            },
        )
        .await
    }

    async fn send_message(
        &self,
        ext: &Extensions,
        session: SessionId,
        to: String,
        text: String,
    ) -> RpcResult<PeerReceipt> {
        self.authorize(ext, session).await?;
        let d = ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::supervisor::Deliver {
                from: session,
                to,
                text,
                await_reply: false,
                reply_to: None,
                request_id: None,
                admitted: tokio_util::sync::CancellationToken::new(),
            },
        )
        .await?;
        Ok(d.receipt)
    }

    async fn contact_file(
        &self,
        ext: &Extensions,
        holder: String,
        id: String,
    ) -> RpcResult<ContactFile> {
        if let Some(r) = to_holder(ext, &holder, "contact_file", (&holder, &id)).await? {
            return Ok(r);
        }
        let (store, r) = here().await?;
        holder::card(&store, &r, &id)
            .await
            .map_err(rpc_core::failed)
    }

    async fn contact_file_thumbnail(
        &self,
        ext: &Extensions,
        holder: String,
        id: String,
    ) -> RpcResult<ContactFileThumbnail> {
        if let Some(r) = to_holder(ext, &holder, "contact_file_thumbnail", (&holder, &id)).await? {
            return Ok(r);
        }
        let (store, r) = here().await?;
        holder::thumbnail(&store, &r, &id)
            .await
            .map_err(rpc_core::failed)
    }

    async fn contact_file_export(
        &self,
        ext: &Extensions,
        holder: String,
        id: String,
        dest: PathBuf,
    ) -> RpcResult<PathBuf> {
        if let Some(r) =
            to_holder(ext, &holder, "contact_file_export", (&holder, &id, &dest)).await?
        {
            return Ok(r);
        }
        let (store, r) = here().await?;
        holder::export(&store, &r, &id, dest)
            .await
            .map_err(rpc_core::failed)
    }

    // 199 P2 (D11): expected values from this engine's own copy of the notice, bytes from
    // the holder named, whoever owns the chat now.
    async fn contact_file_fetch(
        &self,
        ext: &Extensions,
        session: SessionId,
        holder: String,
        id: String,
    ) -> RpcResult<PathBuf> {
        self.authorize(ext, session).await?;
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        let notice = files::received_notice(&store, session, &holder, &id)
            .await
            .map_err(|e| rpc_core::failed(format!("{e:#}")))?
            .ok_or_else(|| rpc_core::failed(format!("no notice for {id} in this chat")))?;
        let missing = || rpc_core::failed(format!("no such file on {}", notice.machine));
        if mesh::self_id().as_deref() == Some(holder.as_str()) {
            let held = ask::<Agents, _>(agent::AGENT_ACTOR, files::HeldContactFile { id }).await?;
            return held.map(|h| h.path).ok_or_else(missing);
        }
        let offline = || rpc_core::failed(format!("{} is offline", notice.machine));
        let t = match replica::actor() {
            Some(r) => r.ask(replica::GetTransport).await.ok().flatten(),
            None => None,
        }
        .ok_or_else(offline)?;
        let holder = replica::PeerId(holder);
        if !t.peers().contains(&holder) {
            return Err(offline());
        }
        (forward::hooks().fetch_contact_file)(t.as_ref(), &self.rt.data_dir, &holder, &id, &notice)
            .await
            .map_err(|e| rpc_core::failed(format!("{e:#}")))?
            .ok_or_else(missing)
    }

    // 199 D17: internal only. The holder's own outbox calls `notices::admit` directly; over
    // RPC only a paired machine's forwarded call (the front door's `Forwarded` marker) is
    // taken, for a session this engine owns (`NOT_OWNER` otherwise).
    async fn contact_file_notice(
        &self,
        ext: &Extensions,
        session: SessionId,
        notice_id: Uuid,
        contact_id: String,
        notice: String,
    ) -> RpcResult<NoticeAdmission> {
        if ext.get::<forward::Forwarded>().is_none() {
            return Err(rpc_core::failed(
                "contact_file_notice is internal: only a paired machine's front door calls it",
            ));
        }
        self.authorize_fwd(ext, session).await?;
        let params = (session, notice_id, &contact_id, &notice);
        if let Some(r) = forward::route(ext, session, "contact_file_notice", params).await? {
            return Ok(r);
        }
        notices::admit(session, notice_id, contact_id, notice).await
    }

    // 199 D17: through the same front door; the one committed predicate.
    async fn contact_file_notice_status(
        &self,
        ext: &Extensions,
        session: SessionId,
        notice_id: Uuid,
    ) -> RpcResult<NoticeStatus> {
        self.authorize_fwd(ext, session).await?;
        let params = (session, notice_id);
        if let Some(r) = forward::route(ext, session, "contact_file_notice_status", params).await? {
            return Ok(r);
        }
        notices::status(session, notice_id).await
    }

    // 199-E3 (SEC-P3): internal only, answered by the holder from its own authority.
    async fn contact_file_authority(
        &self,
        ext: &Extensions,
        holder: String,
        id: String,
    ) -> RpcResult<bool> {
        if ext.get::<forward::Forwarded>().is_none() {
            return Err(rpc_core::failed(
                "contact_file_authority is internal: only a paired machine calls it",
            ));
        }
        let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
        if holder != store.engine() {
            return Err(rpc_core::failed(format!("not the holder of {id}")));
        }
        Ok(files::foreign::held_live(&store, &id))
    }
}

/// This engine's store and card identity.
async fn here() -> RpcResult<(agent::store::Store, agent::store::contact_files::Render)> {
    let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
    let r = holder::render(&store).await;
    Ok((store, r))
}

/// 199: a `(holder, id)` call carried to `holder` over `theuth/rpc/1` (`Ok(None)`: it is
/// this engine, or the call was forwarded here already). An unreachable holder is
/// `<machine> is offline`.
async fn to_holder<T: serde::de::DeserializeOwned>(
    ext: &Extensions,
    holder: &str,
    method: &str,
    params: impl serde::Serialize,
) -> RpcResult<Option<T>> {
    match forward::to_engine(ext, Some(holder), method, params).await {
        Err(e) if e.code() == forward::OWNER_UNREACHABLE => Err(rpc_core::failed(format!(
            "{} is offline",
            agent::communication::mesh::machine_name_now(holder).await
        ))),
        r => r,
    }
}

/// 199 D15: a human answer with files, on the chat's own machine only: the same gate,
/// offers and send path as the model's `send_message(reply_to, files)`, to the contact
/// whose request it answers; the answer keeps the human's provenance.
async fn answer_with_files(
    ext: &Extensions,
    session: SessionId,
    reply_to: Uuid,
    text: String,
    files: Vec<PathBuf>,
) -> RpcResult<PeerReceipt> {
    let store = ask::<Agents, _>(agent::AGENT_ACTOR, agent::DurableStore).await?;
    let owner = match ext.get::<forward::Forwarded>() {
        Some(_) => Some(store.engine().to_owned()),
        None => store
            .owner(session)
            .await
            .map_err(|e| rpc_core::failed(format!("{e:#}")))?
            .map(|(o, _)| o)
            .filter(|o| !o.is_empty() && o != store.engine()),
    };
    if let Some(owner) = owner {
        let machine = agent::communication::mesh::machine_name_now(&owner).await;
        return Err(rpc_core::failed(format!("attach on {machine}")));
    }
    let request = ask::<Agents, _>(
        agent::AGENT_ACTOR,
        GetPeerMessage {
            session,
            id: reply_to,
        },
    )
    .await?;
    let binding = request
        .from
        .strip_prefix("peer:")
        .and_then(|b| b.parse::<Uuid>().ok());
    let alias = match binding {
        Some(b) => store
            .peer_binding_by_id(b)
            .await
            .map_err(|e| rpc_core::failed(format!("{e:#}")))?
            .map(|b| b.alias),
        None => None,
    }
    .ok_or_else(|| {
        rpc_core::failed(format!(
            "files can go to contacts only; {} is not a contact",
            request.from
        ))
    })?;
    send_files(session, reply_to, text, files, store, alias).await
}

#[cfg(unix)]
async fn send_files(
    session: SessionId,
    reply_to: Uuid,
    text: String,
    files: Vec<PathBuf>,
    store: agent::store::Store,
    alias: String,
) -> RpcResult<PeerReceipt> {
    let agents = rpc_core::lookup::<Agents>(agent::AGENT_ACTOR)?;
    let to = format!("peer:{alias}");
    let staged = agent::communication::files::send::stage(
        &agents,
        session,
        std::slice::from_ref(&to),
        &files,
        None,
        &text,
    )
    .await
    .map_err(rpc_core::failed)?;
    let sent = ask::<Agents, _>(
        agent::AGENT_ACTOR,
        agent::supervisor::Deliver {
            from: session,
            to,
            text: text.clone(),
            await_reply: false,
            reply_to: Some(reply_to),
            request_id: Some(staged.request_ids[0].clone()),
            admitted: tokio_util::sync::CancellationToken::new(),
        },
    )
    .await;
    staged.finish().await;
    let receipt = sent?.receipt;
    // Provenance: the human answered for this session.
    let _ = store.owner_answer(reply_to, session, &text).await;
    Ok(receipt)
}

#[cfg(not(unix))]
async fn send_files(
    _: SessionId,
    _: Uuid,
    _: String,
    _: Vec<PathBuf>,
    _: agent::store::Store,
    _: String,
) -> RpcResult<PeerReceipt> {
    Err(rpc_core::failed(files::NOT_YET))
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(AgentMessagingApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}

/// `peer_message` on a mesh: this engine's record when it holds one, else the
/// owner's. Exchange records are engine-local, so an old owner keeps answering
/// for its own records whether the new owner is reachable or not. Not always
/// current: for a request this session sent before its ownership moved, the
/// reply lands on the new owner, and the old owner's copy keeps reading
/// "awaiting reply". `owner` is not polled on a local hit.
async fn local_first<T>(
    local: impl Future<Output = RpcResult<T>>,
    owner: impl Future<Output = RpcResult<T>>,
) -> RpcResult<T> {
    match local.await {
        Ok(r) => Ok(r),
        Err(_) => owner.await,
    }
}

#[cfg(test)]
mod tests {
    use super::local_first;
    use jsonrpsee::types::ErrorObjectOwned;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn err(code: i32) -> ErrorObjectOwned {
        ErrorObjectOwned::owned(code, "x", None::<()>)
    }

    /// A local record answers even while the owner is unreachable (-32010),
    /// and the owner is never asked then; a local miss is the owner's answer
    /// (or its error) verbatim.
    #[tokio::test]
    async fn a_local_record_answers_before_the_owner_is_asked() {
        let asked = AtomicBool::new(false);
        let offline = async {
            asked.store(true, Ordering::SeqCst);
            Err::<u8, _>(err(-32010))
        };
        assert_eq!(local_first(async { Ok(7) }, offline).await.unwrap(), 7);
        assert!(!asked.load(Ordering::SeqCst), "the owner was asked");
        assert_eq!(
            local_first(async { Err(err(-32000)) }, async { Ok(9u8) })
                .await
                .unwrap(),
            9
        );
        let e = local_first(async { Err::<u8, _>(err(-32000)) }, async { Err(err(-32010)) })
            .await
            .unwrap_err();
        assert_eq!(e.code(), -32010);
    }

    /// 199 D17: `contact_file_notice` is internal: a call without the front door's
    /// `Forwarded` marker (a desktop, a phone, this engine's own clients) is refused
    /// before anything is looked at.
    #[tokio::test]
    async fn a_file_notice_without_the_forwarded_marker_is_refused() {
        use super::AgentMessagingApiServer as _;
        let d = crate::TestDir::new("rpcnotice");
        let server =
            rpc_core::Server::new(runtime::Runtime::new(figment::Figment::new(), d.0.clone()));
        let mut ext = jsonrpsee::Extensions::new();
        ext.insert(rpc_core::RpcPeer::internal());
        let e = server
            .contact_file_notice(
                &ext,
                runtime::SessionId::new(),
                uuid::Uuid::new_v4(),
                "rec".into(),
                "<message_file/>".into(),
            )
            .await
            .unwrap_err();
        assert!(e.message().contains("internal"), "{}", e.message());
    }
}
