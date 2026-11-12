use super::*;

#[rpc(server, client)]
pub trait MessagingApi {
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
    /// way `answered_by = owner:<session>`; never an arbitrary waiter.
    #[method(name = "answer_peer_request", with_extensions)]
    async fn answer_peer_request(
        &self,
        session: SessionId,
        reply_to: Uuid,
        text: String,
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
    /// Start Signal linking (if not registered); returns the provisioning URL
    /// to render as a QR. `SignalEvent::LinkUrl`/`Linked` follow on the stream.
    #[method(name = "signal_link")]
    async fn signal_link(&self) -> RpcResult<String>;
    /// Signal link/connection state, trust lists and phone threads.
    #[method(name = "signal_status")]
    async fn signal_status(&self) -> RpcResult<SignalStatus>;
    /// Add an E.164 to `signal.allowed` (live and in the config file).
    #[method(name = "signal_allow")]
    async fn signal_allow(&self, number: String) -> RpcResult<()>;
    /// Remove an E.164 from `signal.allowed`.
    #[method(name = "signal_remove_allowed")]
    async fn signal_remove_allowed(&self, number: String) -> RpcResult<()>;
    /// Drop the Signal registration (`presage.db`) and return to linking; threads are kept.
    #[method(name = "signal_unlink")]
    async fn signal_unlink(&self) -> RpcResult<()>;
    /// The Slack app manifest this engine's Slack code expects (`slack::MANIFEST`,
    /// `crates/slack/manifest.yaml`) verbatim, for Settings › Channels › Slack ›
    /// App manifest; answers whether or not Slack is configured or enabled.
    #[method(name = "slack_manifest")]
    async fn slack_manifest(&self) -> RpcResult<String>;
    /// Slack connection state, trust lists, channels and bound threads.
    #[method(name = "slack_status")]
    async fn slack_status(&self) -> RpcResult<SlackStatus>;
    /// Add a user id (`U…`) to `slack.allowed` (live and in the config file).
    #[method(name = "slack_allow")]
    async fn slack_allow(&self, user: String) -> RpcResult<()>;
    /// Remove a user id from `slack.allowed`.
    #[method(name = "slack_remove_allowed")]
    async fn slack_remove_allowed(&self, user: String) -> RpcResult<()>;
    /// Add a channel id (`C…`) to `slack.channels` (live and in the config file).
    #[method(name = "slack_allow_channel")]
    async fn slack_allow_channel(&self, channel: String) -> RpcResult<()>;
    /// Remove a channel id from `slack.channels`.
    #[method(name = "slack_remove_channel")]
    async fn slack_remove_channel(&self, channel: String) -> RpcResult<()>;
    /// Telegram bot identity, poller state, trust lists, groups and bound threads (98 §13).
    #[method(name = "telegram_status")]
    async fn telegram_status(&self) -> RpcResult<TelegramStatus>;
    /// Pair the owner (98 §10.4): with `owner`, set `telegram.owner` live and in the
    /// config file and answer `None`; without, mint a single-use code and answer the
    /// `t.me/<bot>?start=<code>` deep link (needs the daemon).
    #[method(name = "telegram_link")]
    async fn telegram_link(&self, owner: Option<u64>) -> RpcResult<Option<PairingLink>>;
    /// Add a user id to `telegram.allowed` (live and in the config file).
    #[method(name = "telegram_allow")]
    async fn telegram_allow(&self, user: u64) -> RpcResult<()>;
    /// Remove a user id from `telegram.allowed`.
    #[method(name = "telegram_remove_allowed")]
    async fn telegram_remove_allowed(&self, user: u64) -> RpcResult<()>;
    /// Add a chat id (`-100…`) to `telegram.groups` (live and in the config file).
    #[method(name = "telegram_allow_group")]
    async fn telegram_allow_group(&self, chat: i64) -> RpcResult<()>;
    /// Remove a chat id from `telegram.groups`.
    #[method(name = "telegram_remove_group")]
    async fn telegram_remove_group(&self, chat: i64) -> RpcResult<()>;
    /// A2A network status (50 §10): listener, configured peers and exports,
    /// and the config/listener/reload error when any. `a2a.enabled: false`
    /// answers the disabled state; enabled without a live actor is an error.
    #[method(name = "a2a_status")]
    async fn a2a_status(&self) -> RpcResult<A2aStatus>;
    /// Re-read `peers.yaml` and the `a2a` section, validate and swap
    /// atomically (50 §6.1); a failed reload keeps the old configuration and
    /// is the `Err`. `A2aChanged` follows on the stream.
    #[method(name = "reload_a2a")]
    async fn reload_a2a(&self) -> RpcResult<A2aStatus>;
    /// 183 §7.4: open (`true`, the Add contact sheet; returns the invite ticket) or close the
    /// Add contact window.
    #[method(name = "contact_window")]
    async fn contact_window(&self, open: bool) -> RpcResult<Option<a2a::contacts::ContactWindow>>;
    /// 183 §7.1: A2A agents the browse sees while the window is open, newest first, ≤ 32.
    #[method(name = "contact_nearby")]
    async fn contact_nearby(&self) -> RpcResult<Vec<a2a::contacts::NearbyAgent>>;
    /// 183 §7.4: start the ceremony with a Nearby agent, a ticket or a card URL.
    #[method(name = "contact_pair")]
    async fn contact_pair(
        &self,
        target: a2a::contacts::ContactTarget,
    ) -> RpcResult<mesh::PairCeremony>;
    /// 183 §7.4: this side's Accept (`accept`) or Reject for a contact ceremony.
    #[method(name = "contact_answer")]
    async fn contact_answer(&self, ceremony: uuid::Uuid, accept: bool) -> RpcResult<()>;
    /// 183 §12.7: this machine's contacts.
    #[method(name = "contact_list")]
    async fn contact_list(&self) -> RpcResult<Vec<a2a::contacts::Contact>>;
    /// 183 §12.7: rename, cap, grant exports, block or unblock a contact.
    #[method(name = "contact_update")]
    async fn contact_update(
        &self,
        name: String,
        patch: a2a::contacts::ContactPatch,
    ) -> RpcResult<a2a::contacts::Contact>;
    /// 183 §8.4: Remove (`block: false`) or Block a contact: local and immediate.
    #[method(name = "contact_remove")]
    async fn contact_remove(&self, name: String, block: bool) -> RpcResult<()>;
    /// 183 §7.6, D9: this machine's exports (agents and chats), with `paused`.
    #[method(name = "a2a_exports")]
    async fn a2a_exports(&self) -> RpcResult<Vec<a2a::contacts::Export>>;
    /// 183 §7.6: create or replace an export by name.
    #[method(name = "a2a_export_put")]
    async fn a2a_export_put(&self, export: a2a::contacts::Export) -> RpcResult<()>;
    /// 183 §12.10: remove an export and its grants (Stop sharing).
    #[method(name = "a2a_export_remove")]
    async fn a2a_export_remove(&self, name: String) -> RpcResult<()>;
    /// 183 §7.6 (W-27): the `contact_first` Attention item's Allow once · Always · Block.
    #[method(name = "contact_first_answer")]
    async fn contact_first_answer(
        &self,
        contact: String,
        export: String,
        answer: a2a::config::FirstContact,
    ) -> RpcResult<()>;
    /// 183 F3, F11: every (contact, export) whose first message waits for the owner's
    /// answer — the source of the `contact_first` Attention item; refetch on `A2aChanged`. 186
    /// review 1: then every contact message queued behind its chat's running turn (`queued`).
    #[method(name = "contact_first_waiting")]
    async fn contact_first_waiting(&self) -> RpcResult<Vec<a2a::contacts::FirstWaiting>>;
}

#[async_trait]
impl MessagingApiServer for Server {

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
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, GetPeerMessage { session, id }).await
    }

    async fn list_peer_messages(
        &self,
        ext: &Extensions,
        session: SessionId,
        cursor: Option<String>,
        limit: Option<u16>,
        pending_only: bool,
    ) -> RpcResult<PeerMessagePage> {
        self.authorize(ext, session).await?;
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
    ) -> RpcResult<PeerReceipt> {
        self.authorize_fwd(ext, session).await?;
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

    async fn signal_link(&self) -> RpcResult<String> {
        ask::<Signal, _>(SIGNAL_ACTOR, Link).await
    }

    async fn signal_status(&self) -> RpcResult<SignalStatus> {
        ask::<Signal, _>(SIGNAL_ACTOR, Status).await
    }

    async fn signal_allow(&self, number: String) -> RpcResult<()> {
        ask::<Signal, _>(SIGNAL_ACTOR, Allow(number)).await
    }

    async fn signal_remove_allowed(&self, number: String) -> RpcResult<()> {
        ask::<Signal, _>(SIGNAL_ACTOR, RemoveAllowed(number)).await
    }

    async fn signal_unlink(&self) -> RpcResult<()> {
        ask::<Signal, _>(SIGNAL_ACTOR, Unlink).await
    }

    async fn slack_manifest(&self) -> RpcResult<String> {
        Ok(slack::MANIFEST.to_owned())
    }

    async fn slack_status(&self) -> RpcResult<SlackStatus> {
        ask::<Slack, _>(SLACK_ACTOR, slack::Status).await
    }

    async fn slack_allow(&self, user: String) -> RpcResult<()> {
        ask::<Slack, _>(SLACK_ACTOR, slack::Allow(user)).await
    }

    async fn slack_remove_allowed(&self, user: String) -> RpcResult<()> {
        ask::<Slack, _>(SLACK_ACTOR, slack::RemoveAllowed(user)).await
    }

    async fn slack_allow_channel(&self, channel: String) -> RpcResult<()> {
        ask::<Slack, _>(SLACK_ACTOR, slack::AllowChannel(channel)).await
    }

    async fn slack_remove_channel(&self, channel: String) -> RpcResult<()> {
        ask::<Slack, _>(SLACK_ACTOR, slack::RemoveChannel(channel)).await
    }

    async fn telegram_status(&self) -> RpcResult<TelegramStatus> {
        send(&telegram()?, telegram::Status).await
    }

    async fn telegram_link(&self, owner: Option<u64>) -> RpcResult<Option<PairingLink>> {
        send(&telegram()?, telegram::Link { owner }).await
    }

    async fn telegram_allow(&self, user: u64) -> RpcResult<()> {
        send(&telegram()?, telegram::Allow(user)).await
    }

    async fn telegram_remove_allowed(&self, user: u64) -> RpcResult<()> {
        send(&telegram()?, telegram::RemoveAllowed(user)).await
    }

    async fn telegram_allow_group(&self, chat: i64) -> RpcResult<()> {
        send(&telegram()?, telegram::AllowGroup(chat)).await
    }

    async fn telegram_remove_group(&self, chat: i64) -> RpcResult<()> {
        send(&telegram()?, telegram::RemoveGroup(chat)).await
    }

    async fn a2a_status(&self) -> RpcResult<A2aStatus> {
        match a2a(&self.rt)? {
            Some(actor) => send(&actor, a2a::GetStatus).await,
            None => Ok(A2aStatus::disabled()),
        }
    }

    async fn reload_a2a(&self) -> RpcResult<A2aStatus> {
        match a2a(&self.rt)? {
            Some(actor) => send(&actor, a2a::Reload).await,
            None => Ok(A2aStatus::disabled()),
        }
    }

    // 183 Phase 3 (§10.1): each asks the A2A actor, which runs whatever `a2a.enabled` says
    // (W-26). Only a process without the a2a plugin answers the `off` values.

    async fn contact_window(&self, open: bool) -> RpcResult<Option<a2a::contacts::ContactWindow>> {
        let off = if open {
            Err(failed(a2a::contacts::OFF))
        } else {
            Ok(None)
        };
        contacts(&self.rt, a2a::contacts::ContactWindowOpen(open), off).await
    }

    async fn contact_nearby(&self) -> RpcResult<Vec<a2a::contacts::NearbyAgent>> {
        contacts(&self.rt, a2a::contacts::ContactsNearby, Ok(Vec::new())).await
    }

    async fn contact_pair(
        &self,
        target: a2a::contacts::ContactTarget,
    ) -> RpcResult<mesh::PairCeremony> {
        let off = Err(failed(a2a::contacts::OFF));
        contacts(&self.rt, a2a::contacts::ContactPair(target), off).await
    }

    async fn contact_answer(&self, ceremony: uuid::Uuid, accept: bool) -> RpcResult<()> {
        let msg = a2a::contacts::ContactAnswer { ceremony, accept };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn contact_list(&self) -> RpcResult<Vec<a2a::contacts::Contact>> {
        let mut out = contacts(&self.rt, a2a::contacts::ContactsList, Ok(Vec::new())).await?;
        name_paired_on(&mut out).await;
        Ok(out)
    }

    async fn contact_update(
        &self,
        name: String,
        patch: a2a::contacts::ContactPatch,
    ) -> RpcResult<a2a::contacts::Contact> {
        let msg = a2a::contacts::ContactUpdate { name, patch };
        let mut c = contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await?;
        name_paired_on(std::slice::from_mut(&mut c)).await;
        Ok(c)
    }

    async fn contact_remove(&self, name: String, block: bool) -> RpcResult<()> {
        let msg = a2a::contacts::ContactRemove { name, block };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn a2a_exports(&self) -> RpcResult<Vec<a2a::contacts::Export>> {
        contacts(&self.rt, a2a::contacts::ExportsList, Ok(Vec::new())).await
    }

    async fn a2a_export_put(&self, export: a2a::contacts::Export) -> RpcResult<()> {
        let msg = a2a::contacts::ExportPut(export);
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn a2a_export_remove(&self, name: String) -> RpcResult<()> {
        let msg = a2a::contacts::ExportRemove { name };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn contact_first_answer(
        &self,
        contact: String,
        export: String,
        answer: a2a::config::FirstContact,
    ) -> RpcResult<()> {
        let msg = a2a::contacts::ContactFirstAnswer {
            contact,
            export,
            answer,
        };
        contacts(&self.rt, msg, Err(failed(a2a::contacts::OFF))).await
    }

    async fn contact_first_waiting(&self) -> RpcResult<Vec<a2a::contacts::FirstWaiting>> {
        contacts(&self.rt, a2a::contacts::ContactsWaiting, Ok(Vec::new())).await
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    MessagingApiServer::into_rpc(server)
}
