//! 186 CONTACT-COLLAB: a contact share in full. The contact's message arrives like a mesh
//! peer's (an unscoped `<message from=peer:…>`, steering into a running turn or opening one);
//! its A2A task completes when the message is consumed, marked `theuth.receipt`. The
//! receiving agent answers with `send_message(reply_to)` as often as it likes: each answer is a
//! new `SendMessage` to the sender's contact-only return route ([`RETURN_ROUTE`]) on the
//! conversation the sender minted (`contextId`), which the sender admits while that
//! conversation lives ([`THREAD_TTL`], sliding) and the share is still full. The sender's
//! answers go back to the export on the same conversation. Design doc 186.

use std::collections::{HashMap, HashSet};

use a2a_types::{A2AError, Message as A2aMessage, Part, Role, SendMessageRequest};
use a2a_types::{SendMessageResponse, Task};
use chrono::Utc;
use kameo::message::{Context, Message};
use runtime::SessionId;
use tokio_util::sync::CancellationToken;
use uuid::Uuid;

use super::super::{Generation, PeerMessageState, PeerRecord, ShareMode};
use super::{TaskRef, internal, snapshot};
use crate::store::communication::{A2aTask, A2aTaskStatus, AcceptExchange, EXTERNAL_ID_MAX};
use crate::supervisor::Agents;

/// 186 (D5): the contact-only return route (`/agents/~return/rpc`); never listed in a card,
/// never an export name (those are `[a-z0-9_-]`).
pub const RETURN_ROUTE: &str = "~return";
/// 186 (D5): a conversation's return grant lapses after this long without a message.
pub const THREAD_TTL: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
/// 186: the A2A `metadata` key of theuth's fields (`receipt`, `re`, `chain`).
pub const META: &str = "theuth";

/// 186 (D5, D9): what a full contact's message carries besides its text.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FullWire {
    /// `metadata.theuth.re`: the A2A `messageId` this message answers.
    pub re: Option<String>,
    /// `metadata.theuth.chain`: the sender's peer-chain count (bounded at `CHAIN_MAX`).
    pub chain: u32,
    /// `referenceTaskIds`: the task of the message this one answers (return route only).
    pub reference_task: Option<String>,
}

impl FullWire {
    /// The message's `metadata`.
    pub fn metadata(&self) -> HashMap<String, serde_json::Value> {
        let mut t = serde_json::Map::new();
        if let Some(re) = &self.re {
            t.insert("re".into(), re.clone().into());
        }
        t.insert("chain".into(), self.chain.into());
        HashMap::from([(META.to_owned(), serde_json::Value::Object(t))])
    }

    /// A received message's fields (absent ones empty; an unbounded `re` dropped).
    pub fn of(meta: Option<&HashMap<String, serde_json::Value>>) -> Self {
        let t = meta.and_then(|m| m.get(META)).and_then(|v| v.as_object());
        let get = |k: &str| t.and_then(|t| t.get(k));
        Self {
            re: get("re")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty() && s.len() <= EXTERNAL_ID_MAX)
                .map(str::to_owned),
            chain: get("chain")
                .and_then(serde_json::Value::as_u64)
                .map_or(0, |n| u32::try_from(n).unwrap_or(u32::MAX)),
            reference_task: None,
        }
    }
}

/// 186 (D4): `metadata` of a full contact's completed task (and of a return route's answer):
/// the message was taken in, nothing more; no reply is in it.
pub fn receipt_metadata() -> HashMap<String, serde_json::Value> {
    HashMap::from([(META.to_owned(), serde_json::json!({ "receipt": true }))])
}

/// 186 (D4): `metadata` says the message was received in full (a receipt, not a reply).
pub fn is_receipt(meta: Option<&HashMap<String, serde_json::Value>>) -> bool {
    meta.and_then(|m| m.get(META))
        .and_then(|t| t.get("receipt"))
        .and_then(serde_json::Value::as_bool)
        == Some(true)
}

/// `ask` → `Result<SendMessageResponse, A2AError>` (186 D5): a contact's `SendMessage` on
/// this engine's return route — an answer in a conversation this engine opened.
#[derive(Clone, Debug)]
pub struct A2aReturn {
    /// The contact's live binding (the `theuth/a2a/1` caller).
    pub binding: Uuid,
    /// Its grant epoch.
    pub route: CancellationToken,
    /// The request as decoded by the SDK.
    pub request: SendMessageRequest,
}

impl Message<A2aReturn> for Agents {
    type Reply = Result<SendMessageResponse, A2AError>;

    async fn handle(&mut self, m: A2aReturn, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.full_return(m).await
    }
}

/// 186 (D5): how one `send_message` to a contact travels when the share is full.
pub(super) struct FullShape {
    /// The route: the contact's export, or [`RETURN_ROUTE`].
    pub export: String,
    /// The conversation.
    pub context: String,
    /// Its metadata.
    pub wire: FullWire,
    /// An answer over the contact's return route (acknowledged at once, never followed).
    pub ret: bool,
}

/// The conversation with that contact ended: unknown, lapsed, or no longer shared in full.
fn gone() -> A2AError {
    A2AError::task_not_found("conversation")
}

/// 191 (D3, S4): [`super::NOT_OWNER_CODE`] "not owner": the session is not this engine's
/// (another machine owns it, its owner is empty on a mesh engine, its row is unknown or
/// unreadable here). The contact's client tries its next member (P3 forwards instead).
pub(crate) fn not_owner() -> A2AError {
    A2AError::new(super::NOT_OWNER_CODE, super::NOT_OWNER_TEXT)
}

impl Agents {
    /// 191 (D3, S4): the strict owner predicate of every contact path: the row exists and
    /// reads here and this engine owns it (never "unknown or empty reads as local"); else
    /// [`not_owner`].
    pub(crate) async fn contact_owned(&self, session: SessionId) -> Result<(), A2AError> {
        match self.store.owned_here(session).await {
            Ok(true) => return Ok(()),
            Ok(false) => {}
            Err(e) => tracing::warn!(%session, "a2a: session row unreadable: {e:#}"),
        }
        tracing::info!(%session, "191: contact path refused: not owner");
        Err(not_owner())
    }
}

impl Agents {
    /// 186 (D5): the open message `re` names, which `from` sent to `to`: the `reply_to` of an
    /// answer. `re` is that message's A2A `messageId` (its idempotency key). `None` when it
    /// names nothing of theirs, or a message already answered (the answer still travels on
    /// the conversation).
    pub(super) async fn answered(
        &self,
        re: Option<&str>,
        from: &str,
        to: &str,
    ) -> Result<Option<Uuid>, A2AError> {
        let Some(re) = re else { return Ok(None) };
        let Some(id) = self
            .store
            .exchange_by_request_id(from, re)
            .await
            .map_err(internal)?
        else {
            return Ok(None);
        };
        Ok(self
            .store
            .exchange_record(id)
            .await
            .map_err(internal)?
            .filter(|o| o.from == from && o.receipt.to == to && answerable(o.receipt.state))
            .map(|o| o.receipt.id))
    }

    /// 186 (D5): the full shape of a send from `from` to the contact `alias` (route `binding`),
    /// `orig` the message it answers; `None` = today's path. A fresh message goes full when
    /// the contact's card shares the export with us in full (`peer:<contact>` = its first);
    /// an answer goes on the conversation the answered message belongs to: back over the
    /// contact's return route when it opened it, to its export when we did.
    pub(super) async fn full_shape(
        &self,
        from: SessionId,
        alias: &str,
        binding: Uuid,
        orig: Option<&PeerRecord>,
    ) -> Result<Option<FullShape>, String> {
        let chain = self.chain_of(from);
        let err = |e: anyhow::Error| format!("{e:#}");
        if let Some(orig) = orig {
            let Some((context, re)) = self
                .store
                .exchange_thread(orig.receipt.id)
                .await
                .map_err(err)?
            else {
                return Ok(None);
            };
            let Some(t) = self
                .store
                .contact_thread(binding, &context)
                .await
                .map_err(err)?
            else {
                // 186 review 2: a full message whose conversation was collected (quiet past the
                // TTL, or its contact re-paired): not a task to answer on.
                return Err(format!(
                    "the conversation with `{alias}` ended (quiet for 7 days, or its contact was paired again); send a new message without reply_to"
                ));
            };
            if t.outbound {
                return Ok(Some(FullShape {
                    export: t.export,
                    context,
                    wire: FullWire {
                        re: Some(re),
                        chain,
                        reference_task: None,
                    },
                    ret: false,
                }));
            }
            // D12: an export no longer shared with the contact ends its conversations.
            if !self
                .network
                .peers
                .get(&binding)
                .is_some_and(|p| p.full.contains(&t.export))
            {
                return Err(format!(
                    "peer `{alias}`: {} is no longer shared; nothing was sent",
                    t.export
                ));
            }
            return Ok(Some(FullShape {
                export: RETURN_ROUTE.to_owned(),
                context,
                wire: FullWire {
                    re: Some(re),
                    chain,
                    reference_task: orig.receipt.task_id.clone(),
                },
                ret: true,
            }));
        }
        let (name, written) = match alias.split_once('/') {
            Some((n, e)) => (n, Some(e)),
            None => (alias, None),
        };
        let Some(offers) = self.network.offers.get(name) else {
            return Ok(None);
        };
        // The contact's gate maps a bare `peer:<contact>` to the first export it grants us.
        let Some(offer) = (match written {
            Some(e) => offers.iter().find(|o| o.name == e),
            None => offers.iter().min_by(|a, b| a.name.cmp(&b.name)),
        }) else {
            return Ok(None);
        };
        if offer.mode != ShareMode::Full {
            return Ok(None);
        }
        // 186 review 1: lapsed conversations (and revoked bindings') go first; a send after
        // a lapse opens a new one.
        let stale = Utc::now().timestamp_millis() - THREAD_TTL.as_millis() as i64;
        if let Err(e) = self.store.gc_contact_threads(stale).await {
            tracing::warn!("a2a: conversations not collected: {e:#}");
        }
        let context = match self
            .store
            .outbound_contact_thread(binding, from, &offer.name)
            .await
            .map_err(err)?
        {
            Some(c) => c,
            None => a2a_types::new_context_id(),
        };
        self.store
            .open_contact_thread(binding, &context, from, &offer.name, true)
            .await
            .map_err(err)?;
        Ok(Some(FullShape {
            export: offer.name.clone(),
            context,
            wire: FullWire {
                re: None,
                chain,
                reference_task: None,
            },
            ret: false,
        }))
    }

    /// 186 (D4): a full contact's task: it waits aside (`working`, the session's slot stays
    /// free), its message goes in like a mesh peer's, and it completes on consumption.
    pub(super) async fn deliver_full(&mut self, task: A2aTask, generation: Generation) -> A2aTask {
        // 191 (D3, S4): never delivered into a session that is not strictly this engine's.
        if let Err(e) = self.contact_owned(task.session).await {
            return self.fail_task(&task, &e.message).await.unwrap_or(task);
        }
        let Some(exchange) = task.dedup_exchange else {
            return self
                .fail_task(&task, "task has no initiating exchange")
                .await
                .unwrap_or(task);
        };
        let Some(origin) = self
            .network
            .contact_scopes
            .get(&task.request_scope)
            .cloned()
        else {
            return self
                .fail_task(&task, "contact origin lost")
                .await
                .unwrap_or(task);
        };
        let mut record = match self.store.exchange_record(exchange).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                return self
                    .fail_task(&task, "initiating exchange missing")
                    .await
                    .unwrap_or(task);
            }
            Err(e) => {
                tracing::error!("a2a: exchange record: {e:#}");
                return task;
            }
        };
        let snap = snapshot(&task);
        let chain = FullWire::of(
            snap.history
                .as_ref()
                .and_then(|h| h.first())
                .and_then(|m| m.metadata.as_ref()),
        )
        .chain;
        let task = match self
            .transition_with(&task, snap, A2aTaskStatus::Working, None, None, None, true)
            .await
        {
            Ok(t) => t,
            Err(e) => {
                tracing::error!(task = %task.id, "a2a: full working transition: {e:#}");
                return task;
            }
        };
        let binding = task
            .principal
            .strip_prefix("peer:")
            .and_then(|b| b.parse::<Uuid>().ok());
        let bound = async {
            let binding = binding.ok_or_else(|| anyhow::anyhow!("malformed principal"))?;
            self.store
                .set_exchange_contact(exchange, ShareMode::Full, Some(&task.context_id))
                .await?;
            self.store.bind_exchange_task(exchange, &task.id).await?;
            self.store
                .open_contact_thread(binding, &task.context_id, task.session, &task.export, false)
                .await
        };
        if let Err(e) = bound.await {
            let why = format!("conversation not recorded: {e:#}");
            return self.fail_task(&task, &why).await.unwrap_or(task);
        }
        record.contact_mode = Some(ShareMode::Full);
        self.network
            .full_inbound
            .insert(exchange, TaskRef::of(&task));
        match self
            .full_intake(
                record,
                format!("peer:{}", origin.contact),
                generation,
                chain,
            )
            .await
        {
            Ok(false) => task,
            Ok(true) => self.full_consumed(exchange).await.unwrap_or(task),
            Err(e) => {
                self.network.full_inbound.remove(&exchange);
                self.fail_task(&task, &format!("intake refused: {e}"))
                    .await
                    .unwrap_or(task)
            }
        }
    }

    /// 186 (D4, D5): one full message into its local recipient as a mesh peer's (unscoped,
    /// shown as `from`). An answer to a message whose `send_message` awaits it completes that
    /// call instead (`Ok(true)`: consumed directly, no turn).
    pub(super) async fn full_intake(
        &mut self,
        mut record: PeerRecord,
        from: String,
        generation: Generation,
        chain: u32,
    ) -> Result<bool, String> {
        // 191 (D3, S4): a contact's message never reaches a session that is not strictly this
        // engine's (another's, an empty owner, an unknown row); an unreadable owner refuses too.
        self.contact_owned(record.session)
            .await
            .map_err(|e| e.message)?;
        if let Some(q) = record.receipt.reply_to
            && let Ok(Some(mut orig)) = self.store.exchange_receipt(q).await
            && orig.state == PeerMessageState::Replied
        {
            self.peer_changed(&orig).await;
            orig.to = from.clone();
            if self.settle_wait(&orig) {
                let revision = generation.revision() as i64;
                match self
                    .store
                    .consume_exchange_direct(record.receipt.id, record.session, revision)
                    .await
                {
                    Ok(Some(r)) => self.peer_changed(&r).await,
                    Ok(None) => {}
                    Err(e) => {
                        tracing::warn!(exchange = %record.receipt.id, "a2a: direct consume: {e:#}")
                    }
                }
                return Ok(true);
            }
        }
        record.from = from;
        // Unscoped: it joins a running turn as a steer, or opens one, like a mesh peer's.
        record.request_scope = None;
        self.enqueue_peer_chained(record, generation, chain).await?;
        Ok(false)
    }

    /// 186 (D4): a full contact's message was consumed (by a turn, or by the `send_message`
    /// awaiting it): its task completes with `theuth.receipt`.
    pub(crate) async fn full_consumed(&mut self, exchange: Uuid) -> Option<A2aTask> {
        let r = self.network.full_inbound.remove(&exchange)?;
        let task = match r.load(&self.store).await {
            Ok(t) => t,
            Err(e) => {
                tracing::warn!(%exchange, "a2a: full task: {}", e.message);
                return None;
            }
        };
        if task.status.is_terminal() {
            return Some(task);
        }
        let mut snap = snapshot(&task);
        snap.metadata = Some(receipt_metadata());
        match self
            .transition_with(
                &task,
                snap,
                A2aTaskStatus::Completed,
                None,
                None,
                None,
                false,
            )
            .await
        {
            Ok(t) => Some(t),
            Err(e) => {
                tracing::warn!(task = %task.id, "a2a: full receipt: {e:#}");
                None
            }
        }
    }

    /// 186: a full contact's message never entered a turn: its task fails with the reason.
    pub(crate) async fn full_intake_failed(&mut self, exchange: Uuid, reason: &str) {
        let Some(r) = self.network.full_inbound.remove(&exchange) else {
            return;
        };
        if let Ok(task) = r.load(&self.store).await
            && !task.status.is_terminal()
            && let Err(e) = self
                .fail_task(&task, &format!("intake refused: {reason}"))
                .await
        {
            tracing::warn!(task = %task.id, "a2a: full intake failure: {e:#}");
        }
    }

    /// 186 review 2: the exported agent `agent` (bare roster name) as the roster reads it now.
    pub(super) async fn live_persona(&self, agent: &str) -> Live {
        match &self.identity {
            None => Live::Unknown,
            Some(source) => match source.inbox(agent).await {
                Some(p) => Live::Known(p),
                None => Live::Missing,
            },
        }
    }

    /// 186 (D6), review 2: run the full inbox `id` under `ceiling` (no contact limits), or
    /// capped as a contact's inbox no admission has limited yet when it is `None` (no known
    /// ceiling).
    pub(super) async fn run_full_inbox(
        &mut self,
        id: SessionId,
        ceiling: Option<Option<tools::Risk>>,
    ) -> Result<(), String> {
        match ceiling {
            Some(c) => self.clear_contact_limits(id, c).await,
            None => {
                self.install_contact_limits(id, super::INBOX_FAIL_CLOSED, Vec::new())
                    .await
            }
        }
    }

    /// 186 review 2: the contact `binding` is gone (blocked, unpaired, re-pinned): none of its
    /// inboxes runs in full any more, and each is capped as a contact's no admission limited.
    pub(super) async fn cap_inboxes_of(&mut self, binding: Uuid) {
        let rows = match self.store.a2a_inboxes_of(&format!("peer:{binding}")).await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!(%binding, "a2a: inboxes: {e:#}");
                return;
            }
        };
        for (_, inbox, full) in rows {
            if full && let Err(e) = self.store.unmark_a2a_inbox_full(inbox).await {
                tracing::error!(session = %inbox, "a2a: inbox mode not reset: {e:#}");
            }
            if let Err(e) = self
                .install_contact_limits(inbox, super::INBOX_FAIL_CLOSED, Vec::new())
                .await
            {
                tracing::error!(session = %inbox, "{e}");
            }
        }
    }

    /// 186 review 3: a reload's route set is whole: a binding the store still holds live
    /// outside it (its contact removed or blocked while the engine was down, so in no
    /// in-memory route either) is revoked and its inboxes capped.
    pub(super) async fn drop_unrouted(&mut self, keep: &HashSet<Uuid>) {
        let keep: Vec<Uuid> = keep.iter().copied().collect();
        match self.store.revoke_peer_bindings_except(&keep).await {
            Ok(gone) => {
                for id in gone {
                    self.cap_inboxes_of(id).await;
                }
            }
            Err(e) => tracing::error!("a2a: unrouted bindings not revoked: {e:#}"),
        }
    }

    /// `tell` handler body ([`InboxPersonasChanged`]): every full inbox of a routed contact
    /// takes its agent's ceiling as the roster reads it now (a tightening applies to a running
    /// inbox at once; an agent gone caps it). One whose contact is in no route (before the
    /// first reload, or gone) is left to its actor start and the reload.
    pub(crate) async fn refresh_full_inboxes(&mut self) {
        // 191 (D7): no cap is lifted before `contacts_ready` (the a2a plugin tells
        // `InboxPersonasChanged` right after the flip).
        if self.identity.is_none()
            || !self
                .contacts_ready
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            return;
        }
        let rows = match self.store.full_a2a_inboxes().await {
            Ok(r) => r,
            Err(e) => {
                tracing::error!("a2a: full inboxes: {e:#}");
                return;
            }
        };
        for row in rows {
            let routed = row
                .principal
                .strip_prefix("peer:")
                .and_then(|b| b.parse::<Uuid>().ok())
                .is_some_and(|b| self.network.peers.contains_key(&b));
            if !routed {
                continue;
            }
            let ceiling = match row.agent.as_deref() {
                Some(agent) => match self.live_persona(agent).await {
                    Live::Known(p) => {
                        if let Err(e) = self.store.set_a2a_inbox_ceiling(row.session, p.tools).await
                        {
                            tracing::error!(session = %row.session, "a2a: inbox ceiling: {e:#}");
                        }
                        let ceiling = Some(p.tools);
                        self.network.inbox_personas.insert(row.session, p);
                        ceiling
                    }
                    Live::Missing | Live::Unknown => None,
                },
                None => None,
            };
            if let Err(e) = self.run_full_inbox(row.session, ceiling).await {
                tracing::error!(session = %row.session, "{e}");
            }
        }
    }
}

/// 186 review 2: what the roster says of an exported agent right now.
pub(super) enum Live {
    /// Its persona (its `tools:` ceiling with it).
    Known(crate::InboxPersona),
    /// No such agent (deleted, or its file is invalid).
    Missing,
    /// The roster has not joined yet.
    Unknown,
}

/// 186 review 2: the ceiling a full inbox runs under at an actor start: the stricter of the
/// stored (last real persona) and the live one; with no live persona, the stored one; an agent
/// gone, or neither known: `None` (capped).
pub(super) fn full_ceiling(
    live: &Live,
    stored: Option<Option<tools::Risk>>,
) -> Option<Option<tools::Risk>> {
    let stricter = |a: Option<tools::Risk>, b: Option<tools::Risk>| match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (x, None) | (None, x) => x,
    };
    match live {
        Live::Missing => None,
        Live::Known(p) => Some(stored.map_or(p.tools, |s| stricter(p.tools, s))),
        Live::Unknown => stored,
    }
}

/// `tell` (186 review 2): the roster re-read its agents (or joined): every full inbox takes
/// its agent's ceiling as it reads now.
#[derive(Clone, Copy, Debug, Default)]
pub struct InboxPersonasChanged;

impl Message<InboxPersonasChanged> for Agents {
    type Reply = ();

    async fn handle(&mut self, _: InboxPersonasChanged, _: &mut Context<Self, Self::Reply>) {
        self.refresh_full_inboxes().await;
    }
}

impl Agents {
    /// 186 (D4): the contact read our full message `exchange` (its task completed with a
    /// receipt): `consumed`; an awaiting `send_message` keeps waiting for the answer.
    pub(super) async fn full_read(&mut self, exchange: Uuid) {
        self.network.outbound.remove(&exchange);
        match self.store.read_exchange(exchange).await {
            Ok(r) => self.peer_changed(&r).await,
            Err(e) => tracing::warn!(%exchange, "a2a: full receipt: {e:#}"),
        }
    }

    /// [`A2aReturn`]: an answer in a conversation this engine opened with the contact, into
    /// the session that opened it, as a mesh peer's message (`peer:<contact>/<export>`), or
    /// into the awaiting `send_message`. `TASK_NOT_FOUND` when the conversation is unknown,
    /// lapsed ([`THREAD_TTL`]) or its share is no longer full (or the contact gone).
    pub(crate) async fn full_return(
        &mut self,
        m: A2aReturn,
    ) -> Result<SendMessageResponse, A2AError> {
        if m.route.is_cancelled() {
            return Err(gone());
        }
        let alias = self
            .network
            .peers
            .get(&m.binding)
            .filter(|p| p.contact && !p.generation.is_cancelled())
            .map(|p| p.name.clone())
            .ok_or_else(gone)?;
        let msg = &m.request.message;
        let text = super::inbound_text(msg, |t| self.redact(t).into_owned())?;
        let context = msg.context_id.clone().ok_or_else(gone)?;
        let now = Utc::now().timestamp_millis();
        let thread = self
            .store
            .contact_thread(m.binding, &context)
            .await
            .map_err(internal)?
            .filter(|t| t.outbound && now - t.last_ms <= THREAD_TTL.as_millis() as i64)
            .ok_or_else(gone)?;
        // 191 (D3, S4): `~return` into a session that is not strictly this engine's is
        // `NOT_OWNER`; the contact tries the next member.
        self.contact_owned(thread.session).await?;
        let full = self.network.offers.get(&alias).is_some_and(|o| {
            o.iter()
                .any(|o| o.name == thread.export && o.mode == ShareMode::Full)
        });
        if !full {
            return Err(gone());
        }
        self.store
            .open_contact_thread(m.binding, &context, thread.session, &thread.export, true)
            .await
            .map_err(internal)?;
        let generation = self
            .remote_generation(thread.session, true)
            .map_err(|e| A2AError::unsupported_operation(e.to_string()))?;
        let wire = FullWire::of(msg.metadata.as_ref());
        let (peer, session) = (
            format!("peer:{}", m.binding),
            format!("session:{}", thread.session),
        );
        // 186 review 1 (D9): at most CONTACT_QUEUED of a contact's answers wait to be taken in;
        // review 2: a retry of one already accepted is its dedup (the same `reply_to` as the
        // first, though that message reads answered now), never `busy`.
        let retry = self
            .store
            .exchange_by_request_id(&peer, &msg.message_id)
            .await
            .map_err(internal)?;
        let (reply_to, waiting) = match retry {
            Some(id) => (
                self.store
                    .exchange_record(id)
                    .await
                    .map_err(internal)?
                    .and_then(|r| r.receipt.reply_to),
                0,
            ),
            None => (
                self.answered(wire.re.as_deref(), &session, &peer).await?,
                self.store
                    .answers_waiting(m.binding, thread.session)
                    .await
                    .map_err(internal)?,
            ),
        };
        if waiting >= super::CONTACT_QUEUED as i64 {
            return Err(A2AError::unsupported_operation(format!(
                "busy: {waiting} answers already wait here; retry later"
            )));
        }
        let a = AcceptExchange {
            sender: &peer,
            recipient: &session,
            sender_session: None,
            recipient_session: Some(thread.session),
            request_id: Some(&msg.message_id),
            reply_to,
            request_scope: None,
            generation: generation.revision() as i64,
            body: &text,
        };
        let receipt = self
            .store
            .accept_exchange(&a)
            .await
            .map_err(|e| A2AError::unsupported_operation(format!("refused: {e:#}")))?;
        if receipt.state == PeerMessageState::Accepted && self.claim_enqueued(receipt.id) {
            let from = format!("peer:{alias}/{}", thread.export);
            let intake = async {
                self.store
                    .set_exchange_contact(receipt.id, ShareMode::Full, Some(&context))
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                let mut record = self
                    .store
                    .exchange_record(receipt.id)
                    .await
                    .map_err(|e| format!("{e:#}"))?
                    .ok_or("accepted exchange missing")?;
                // 186 (D8): an answer to ours is no contact's turn (it is a peer's, journaled
                // `peer`); the row keeps its `full` provenance in the store.
                record.contact_mode = None;
                self.full_intake(record, from, generation, wire.chain).await
            };
            if let Err(e) = intake.await {
                self.unclaim_enqueued(receipt.id);
                if let Ok(r) = self.store.fail_exchange(receipt.id, &e).await {
                    self.peer_changed(&r).await;
                }
                return Err(A2AError::unsupported_operation(format!("refused: {e}")));
            }
        }
        let mut ack = A2aMessage::new(Role::Agent, vec![Part::text("received")]);
        ack.context_id = Some(context);
        ack.metadata = Some(receipt_metadata());
        Ok(SendMessageResponse::Message(ack))
    }
}

/// A message an answer may name as its `reply_to` (the store refuses one already answered).
pub(super) fn answerable(s: PeerMessageState) -> bool {
    matches!(
        s,
        PeerMessageState::Accepted | PeerMessageState::Consumed | PeerMessageState::Held
    )
}

/// 186 (D4): a remote task snapshot is a full contact's receipt.
pub(super) fn task_is_receipt(t: &Task) -> bool {
    is_receipt(t.metadata.as_ref())
}
