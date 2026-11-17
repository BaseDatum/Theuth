//! 199 D17, the typed file notice. The holder's outbox sends each notice through the
//! session front door ([`FrontDoor`]) to whichever machine owns the chat now; the owner
//! admits it as automatic input (never `External`), journaled `contact_file` under the
//! notice id, after an ownership check and a share re-check. One process-wide pending set
//! holds every admitted, not yet terminal notice: the admission dedup, and how the loop
//! knows an input is a notice ([`claim`]). Every terminal path clears it ([`clear`]).

use std::collections::HashMap;

use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;
use kameo::message::{Context, Message};
use runtime::SessionId;
use uuid::Uuid;

use super::{NoticeAdmission, NoticeStatus};
use crate::continuation::InputOrigin;
use crate::store::contacts::Owner;
use crate::supervisor::Agents;
use crate::{AgentEvent, Delivery, rpc_forward as forward};

/// A notice the owner admitted and has not settled yet.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pending {
    /// The notice id (the journal's `input_id`).
    pub id: Uuid,
    /// The chat it was admitted into.
    pub session: SessionId,
    /// The contact whose share carried the file (record id), re-checked at consumption.
    pub contact: String,
    /// Its exact text.
    pub text: String,
}

static PENDING: parking_lot::Mutex<Option<HashMap<Uuid, Pending>>> = parking_lot::const_mutex(None);

fn pending_has(id: Uuid) -> bool {
    PENDING.lock().as_ref().is_some_and(|m| m.contains_key(&id))
}

/// Hold `p` pending (admitted; the loop claims it by its text).
pub(crate) fn hold(p: Pending) {
    PENDING.lock().get_or_insert_default().insert(p.id, p);
}

/// A notice settled (committed, dropped as a duplicate, recorded as an ended share, or
/// refused): its id is no longer pending, so the holder's next re-send is admitted afresh.
pub fn clear(id: Uuid) {
    if let Some(m) = PENDING.lock().as_mut() {
        m.remove(&id);
    }
}

/// The loop's test of an input: the pending notice of `session` whose text this is.
pub fn claim(session: SessionId, text: &str) -> Option<Pending> {
    PENDING
        .lock()
        .as_ref()?
        .values()
        .find(|p| p.session == session && p.text == text)
        .cloned()
}

/// The commit-time dedup fired: the notice already has its row (`append_input`).
#[derive(Debug)]
pub struct NoticeCommitted;

impl std::fmt::Display for NoticeCommitted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("file notice already committed")
    }
}

impl std::error::Error for NoticeCommitted {}

/// The ended-share engine-state row for a notice text: its element, then the ended hint
/// ([`super::ended_notice`]'s text, from the element's own escaped `from`).
pub fn ended_text(notice: &str) -> String {
    let el = notice.lines().next().unwrap_or_default();
    let from = el
        .split_once("from=\"")
        .and_then(|(_, rest)| rest.split_once('"'))
        .map_or("", |(f, _)| f);
    format!(
        "{el}\nFile notice from theuth: the share with {from} has ended, so this was not given to the agent."
    )
}

/// How the holder reaches the chat's owner (the front door in production).
#[async_trait::async_trait]
pub trait NoticeDoor: Send + Sync {
    /// `contact_file_notice` on the chat's current owner.
    async fn notice(
        &self,
        session: SessionId,
        id: Uuid,
        contact: &str,
        text: &str,
    ) -> Result<NoticeAdmission, String>;
    /// `contact_file_notice_status` on the chat's current owner.
    async fn status(&self, session: SessionId, id: Uuid) -> Result<NoticeStatus, String>;
}

/// The session front door: here when the chat is ours, else `theuth/rpc/1` to its owner.
/// An old owner's method-not-found, `NOT_OWNER` and `OWNER_UNREACHABLE` are errors the
/// outbox retries; it never falls back to `send_input`.
pub struct FrontDoor;

#[async_trait::async_trait]
impl NoticeDoor for FrontDoor {
    async fn notice(
        &self,
        session: SessionId,
        id: Uuid,
        contact: &str,
        text: &str,
    ) -> Result<NoticeAdmission, String> {
        let ext = Extensions::new();
        let params = (session, id, contact, text);
        match forward::route(&ext, session, "contact_file_notice", params).await {
            Ok(Some(a)) => Ok(a),
            Ok(None) => admit(session, id, contact.to_owned(), text.to_owned())
                .await
                .map_err(|e| e.message().to_owned()),
            Err(e) => Err(format!("{} ({})", e.message(), e.code())),
        }
    }

    async fn status(&self, session: SessionId, id: Uuid) -> Result<NoticeStatus, String> {
        let ext = Extensions::new();
        match forward::route(&ext, session, "contact_file_notice_status", (session, id)).await {
            Ok(Some(s)) => Ok(s),
            Ok(None) => status(session, id)
                .await
                .map_err(|e| e.message().to_owned()),
            Err(e) => Err(format!("{} ({})", e.message(), e.code())),
        }
    }
}

/// The owner side of `contact_file_notice`, on this engine (the front door decided).
pub async fn admit(
    session: SessionId,
    id: Uuid,
    contact: String,
    text: String,
) -> RpcResult<NoticeAdmission> {
    let store = rpc_core::ask::<Agents, _>(crate::AGENT_ACTOR, crate::DurableStore).await?;
    if let Owner::Elsewhere(owner) = store
        .owner_strict(session)
        .await
        .map_err(|e| rpc_core::failed(format!("{e:#}")))?
    {
        return Err(forward::not_owner(owner));
    }
    rpc_core::ask::<Agents, _>(
        crate::AGENT_ACTOR,
        AdmitNotice {
            session,
            id,
            contact,
            text,
        },
    )
    .await
}

/// The owner side of `contact_file_notice_status`: the one committed predicate.
pub async fn status(session: SessionId, id: Uuid) -> RpcResult<NoticeStatus> {
    let store = rpc_core::ask::<Agents, _>(crate::AGENT_ACTOR, crate::DurableStore).await?;
    match store.notice_committed(session, id).await {
        Ok(true) => Ok(NoticeStatus::Committed),
        Ok(false) => Ok(NoticeStatus::Unknown),
        Err(e) => Err(rpc_core::failed(format!("{e:#}"))),
    }
}

/// `ask` → the admission (D17), on the chat's owner.
#[derive(Clone, Debug)]
pub struct AdmitNotice {
    /// The chat.
    pub session: SessionId,
    /// `files::notice_id(holder, file)`.
    pub id: Uuid,
    /// The contact's record id.
    pub contact: String,
    /// The exact notice text.
    pub text: String,
}

impl Message<AdmitNotice> for Agents {
    type Reply = Result<NoticeAdmission, String>;

    async fn handle(&mut self, m: AdmitNotice, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.admit_notice(m).await
    }
}

impl Agents {
    /// [`AdmitNotice`]: strict ownership, the dedup (pending or committed), the share
    /// re-check (ended: the engine-state row, no turn), then automatic input.
    pub(crate) async fn admit_notice(&mut self, m: AdmitNotice) -> Result<NoticeAdmission, String> {
        if !m.text.starts_with("<message_file ") {
            return Err("not a file notice".into());
        }
        match self.store.owner_strict(m.session).await {
            Ok(Owner::Here) => {}
            Ok(_) => return Err(crate::communication::network::NOT_OWNER_TEXT.into()),
            Err(e) => return Err(format!("{e:#}")),
        }
        // Admission dedup: pending here, or already committed (either form).
        if pending_has(m.id)
            || self
                .store
                .notice_committed(m.session, m.id)
                .await
                .map_err(|e| format!("{e:#}"))?
        {
            return Ok(NoticeAdmission::Accepted);
        }
        // SEC-200-1: the one authorisation predicate ([`Agents::notice_live`]); a paused
        // tree's notice is held by `input` (no actor, no turn, no row) until `ResumeTree`,
        // and judged again when its turn takes it ([`consumption_live`]).
        let live = self.notice_live(m.session, &m.contact, m.id).await?;
        if !live {
            ended(&self.store, &self.rt, m.session, None, m.id, &m.text).await?;
            return Ok(NoticeAdmission::Accepted);
        }
        // D17 (199-Z1 SEC-A1): the chain bound peer input has; refused (nothing pending, no
        // turn), the holder's outbox retries on its backoff and the notice is admitted once
        // an external input opened a fresh chain.
        if self.ledger.chain_exhausted(m.session) {
            return Err(format!(
                "chain limit: {} peer-triggered continuations since the last external input",
                crate::communication::CHAIN_MAX
            ));
        }
        let (id, session, text) = (m.id, m.session, m.text.clone());
        hold(Pending {
            id,
            session,
            contact: m.contact,
            text: m.text,
        });
        // Automatic: it continues the chain and never resets a budget; idle, it opens a
        // turn, busy, it waits for the next one.
        if let Err(e) = self
            .input(
                session,
                None,
                text,
                Vec::new(),
                None,
                InputOrigin::Automatic,
                Delivery::NextTurn,
            )
            .await
        {
            clear(id);
            return Err(e);
        }
        Ok(NoticeAdmission::Accepted)
    }

    /// SEC-200-1: 199 D17's one authorisation predicate, at admission. A foreign file
    /// (199-E3) is judged by its own authority ([`super::foreign::share_live`]: never waits
    /// on another engine inside this mailbox); a contact's by
    /// [`Self::contact_notice_live`].
    pub(crate) async fn notice_live(
        &mut self,
        session: SessionId,
        contact: &str,
        id: Uuid,
    ) -> Result<bool, String> {
        if foreign_or_ended(contact) {
            return super::foreign::share_live(&self.store, &session.to_string(), contact, id)
                .await
                .map_err(|e| format!("{e:#}"));
        }
        self.contact_notice_live(session, contact).await
    }

    /// SEC-200-1, the contact half (admission and consumption alike): `contact` (a record
    /// id) still reaches `session` now by
    /// - an export of it still granted ([`Store::share_live`]), or
    /// - a conversation this chat opened toward the contact whose return rule still holds
    ///   ([`Store::notice_threads`], [`Self::conversation_open`]), or
    /// - full access given now (route and group rule) and `session` visible now
    ///   ([`Self::full_access_notice`]).
    ///
    /// A conversation the contact opened into the chat (an inbound `session:<uuid>` thread
    /// of a full-access send included) never authorises on its own.
    ///
    /// [`Store::share_live`]: crate::store::Store::share_live
    /// [`Store::notice_threads`]: crate::store::Store::notice_threads
    pub(crate) async fn contact_notice_live(
        &mut self,
        session: SessionId,
        contact: &str,
    ) -> Result<bool, String> {
        let s = session.to_string();
        let err = |e: anyhow::Error| format!("{e:#}");
        if self.store.share_live(&s, contact).await.map_err(err)? {
            return Ok(true);
        }
        let threads = self.store.notice_threads(&s, contact).await.map_err(err)?;
        if self.conversation_open(&threads) {
            return Ok(true);
        }
        Ok(self.full_access_notice(session, contact).await)
    }
}

/// A foreign peer's file (`peer:<binding>`) or one its holder marked ended (199-E3).
fn foreign_or_ended(contact: &str) -> bool {
    contact.starts_with(super::foreign::ENDED) || super::foreign::is_foreign(contact)
}

/// `ask` → the contact half of the predicate ([`Agents::contact_notice_live`]): how a
/// session loop judges a contact's notice when its turn takes it.
#[derive(Clone, Debug)]
pub struct ContactNoticeLive {
    /// The chat.
    pub session: SessionId,
    /// The contact's record id.
    pub contact: String,
}

impl Message<ContactNoticeLive> for Agents {
    type Reply = Result<bool, String>;

    async fn handle(
        &mut self,
        m: ContactNoticeLive,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.contact_notice_live(m.session, &m.contact).await
    }
}

/// SEC-200-1: how a session loop reaches its engine's supervisor for the contact half of
/// the predicate at consumption (paused and resumed notices included). Registered per
/// engine ([`set_authority`]); none registered fails closed.
#[async_trait::async_trait]
pub trait NoticeAuthority: Send + Sync {
    /// [`Agents::contact_notice_live`] for `session` and `contact`, now.
    async fn live(&self, session: SessionId, contact: &str) -> Result<bool, String>;
}

/// Production: the engine's supervisor.
pub struct Supervisor(pub kameo::actor::WeakActorRef<Agents>);

#[async_trait::async_trait]
impl NoticeAuthority for Supervisor {
    async fn live(&self, session: SessionId, contact: &str) -> Result<bool, String> {
        let agents = self.0.upgrade().ok_or("the supervisor stopped")?;
        let contact = contact.to_owned();
        agents
            .ask(ContactNoticeLive { session, contact })
            .await
            .map_err(|e| e.to_string())
    }
}

/// Register `rt`'s engine's [`NoticeAuthority`] (the supervisor, once spawned).
pub fn set_authority(rt: &runtime::Runtime, authority: std::sync::Arc<dyn NoticeAuthority>) {
    super::Slot::of(rt).set(authority);
}

/// SEC-200-1: 199 D17's one authorisation predicate when a session loop CONSUMES notice
/// `id` (text `text`) for `session`: a foreign file through
/// [`super::foreign::consumption_live`] (its holder asked now, off every mailbox), a
/// contact's through the supervisor ([`NoticeAuthority`], the same
/// [`Agents::contact_notice_live`] admission ran). Any error, or no supervisor, ends it.
pub async fn consumption_live(
    store: &crate::store::Store,
    rt: &runtime::Runtime,
    session: SessionId,
    contact: &str,
    id: Uuid,
    text: &str,
) -> bool {
    let live = match foreign_or_ended(contact) {
        true => {
            super::foreign::consumption_live(store, rt, &session.to_string(), contact, id, text)
                .await
                .map_err(|e| format!("{e:#}"))
        }
        false => match super::Slot::<dyn NoticeAuthority>::of(rt).get() {
            Some(authority) => authority.live(session, contact).await,
            None => Err("no supervisor".into()),
        },
    };
    live.inspect_err(|e| tracing::info!(%session, notice = %id, "199: notice re-check: {e}"))
        .unwrap_or(false)
}

/// Tests without a spawned supervisor: a stand-in [`NoticeAuthority`] answering the export
/// half only ([`crate::store::Store::share_live`]).
#[cfg(test)]
pub(crate) struct ShareOnly(pub crate::store::Store);

#[cfg(test)]
#[async_trait::async_trait]
impl NoticeAuthority for ShareOnly {
    async fn live(&self, session: SessionId, contact: &str) -> Result<bool, String> {
        self.0
            .share_live(&session.to_string(), contact)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// D17 ended share: the notice as a terminal engine-state row (keyed by its id, so a
/// repeat adds nothing) and the live `AgentEvent::Error`; no turn.
pub async fn ended(
    store: &crate::store::Store,
    rt: &runtime::Runtime,
    session: SessionId,
    turn: Option<runtime::TurnId>,
    id: Uuid,
    notice: &str,
) -> Result<(), String> {
    let text = ended_text(notice);
    store
        .record_error_once(session, id, &text)
        .await
        .map_err(|e| format!("{e:#}"))?;
    clear(id);
    rt.publish(AgentEvent::Error {
        session,
        turn,
        message: text,
    })
    .await;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ended row is the notice's element and the fixed hint, with the element's own
    /// (escaped) sender.
    #[test]
    fn ended_text_keeps_the_element_and_its_sender() {
        let n = "<message_file from=\"peer:a&amp;b\" message=\"m\" name=\"x\" type=\"t\" state=\"failed\" reason=\"r\" holder=\"h\" file=\"f\"/>\nFile notice from theuth, not from peer:a&amp;b: no reply to the contact is needed.";
        assert_eq!(
            ended_text(n),
            "<message_file from=\"peer:a&amp;b\" message=\"m\" name=\"x\" type=\"t\" state=\"failed\" reason=\"r\" holder=\"h\" file=\"f\"/>\nFile notice from theuth: the share with peer:a&amp;b has ended, so this was not given to the agent."
        );
    }

    /// A claimed notice is found by its session and exact text only; clearing ends it.
    #[test]
    fn pending_notices_are_claimed_by_session_and_text() {
        let (s, other, id) = (SessionId::new(), SessionId::new(), Uuid::new_v4());
        hold(Pending {
            id,
            session: s,
            contact: "c".into(),
            text: "<message_file a/>".into(),
        });
        assert!(pending_has(id));
        assert_eq!(claim(s, "<message_file a/>").map(|p| p.id), Some(id));
        assert!(claim(other, "<message_file a/>").is_none());
        assert!(claim(s, "<message_file b/>").is_none());
        clear(id);
        assert!(claim(s, "<message_file a/>").is_none());
    }

    const TEXT: &str = "<message_file from=\"peer:maya\" message=\"m\" name=\"a.pdf\" type=\"application/pdf\" state=\"failed\" reason=\"r\" holder=\"h\" file=\"f\"/>\nFile notice from theuth, not from peer:maya: no reply to the contact is needed.";

    fn notice(session: SessionId, id: Uuid) -> AdmitNotice {
        AdmitNotice {
            session,
            id,
            contact: "rec".into(),
            text: TEXT.into(),
        }
    }

    /// The owner's admission: a share that ended is recorded once as the engine-state row
    /// (no turn, nothing pending), and a repeat is `accepted` and adds nothing; a notice
    /// already pending is `accepted` without a second input; a chat owned by another
    /// machine is refused (`NOT_OWNER`).
    #[tokio::test]
    async fn admission_dedups_records_ended_shares_and_checks_the_owner() {
        let d = crate::TestDir::new("notice");
        let mut a = crate::supervisor::tests::agents_on(d.0.clone(), Some("A"), true).await;
        let s = SessionId::new();
        a.store
            .create_session(s, "m", None, None, None)
            .await
            .unwrap();
        let id = Uuid::new_v4();
        for _ in 0..2 {
            assert_eq!(
                a.admit_notice(notice(s, id)).await,
                Ok(NoticeAdmission::Accepted)
            );
        }
        let errors = a.store.errors(s).await.unwrap();
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].1, ended_text(TEXT));
        assert!(!pending_has(id));
        assert!(a.store.notice_committed(s, id).await.unwrap());
        // Pending already: accepted, nothing written.
        let held = Uuid::new_v4();
        hold(Pending {
            id: held,
            session: s,
            contact: "rec".into(),
            text: TEXT.into(),
        });
        assert_eq!(
            a.admit_notice(notice(s, held)).await,
            Ok(NoticeAdmission::Accepted)
        );
        assert_eq!(a.store.errors(s).await.unwrap().len(), 1);
        clear(held);
        // A chat the other machine owns.
        let b = crate::store::Store::open(
            &d.0.join("sessions.sqlite"),
            Some(mesh::EngineId("B".into())),
        )
        .await
        .unwrap();
        let theirs = SessionId::new();
        b.create_session(theirs, "m", None, None, None)
            .await
            .unwrap();
        assert!(
            a.admit_notice(notice(theirs, Uuid::new_v4()))
                .await
                .is_err()
        );
        assert!(a.store.errors(theirs).await.unwrap().is_empty());
    }

    /// 199-Z1 T-1 + SEC-A1: an admitted notice is `InputOrigin::Automatic`: consumed, it
    /// continues the communication chain (one link) and keeps the reminder budget. After
    /// `CHAIN_MAX` consumed notices the next one is refused (nothing pending, no row, no
    /// turn); once an external input opened a fresh chain, the holder's re-send is admitted.
    #[tokio::test]
    async fn notices_are_automatic_and_stop_at_the_chain_limit() {
        use crate::communication::CHAIN_MAX;
        use crate::continuation::InputConsumed;
        let (_d, mut a) = crate::supervisor::tests::agents_with(true).await;
        let (s, t) = crate::supervisor::tests::in_turn(&mut a).await;
        // No spawned supervisor here: its export half judges consumption.
        set_authority(&a.rt, std::sync::Arc::new(ShareOnly(a.store.clone())));
        for sql in [
            "INSERT INTO contacts (id, alias, binding) VALUES ('rec', 'maya', 'peer:b1')".to_owned(),
            format!("INSERT INTO a2a_exports (id, name, session) VALUES ('x1', 'ops', '{s}')"),
            "INSERT INTO contact_grants (id, contact, export, granted) VALUES ('rec|x1', 'rec', 'x1', 1)"
                .to_owned(),
        ] {
            a.store.exec_for_test(&sql).await;
        }
        a.ledger.request_for_test(s, Some((2, 0)));
        let consume = |a: &mut crate::supervisor::Agents| {
            let pending = a.ledger.pending_for_test(s);
            assert_eq!(pending.len(), 1, "{pending:?}");
            assert_eq!(pending[0].1, InputOrigin::Automatic);
            a.ledger.consumed(InputConsumed {
                session: s,
                id: pending[0].0,
                turn: t,
            });
        };
        for n in 1..=CHAIN_MAX {
            let id = Uuid::new_v4();
            assert_eq!(
                a.admit_notice(notice(s, id)).await,
                Ok(NoticeAdmission::Accepted),
                "notice {n}"
            );
            consume(&mut a);
            clear(id);
            assert_eq!(
                a.ledger.request_for_test(s, None),
                (2, n),
                "reminders kept, chain +1"
            );
        }
        let over = Uuid::new_v4();
        let e = a.admit_notice(notice(s, over)).await.unwrap_err();
        assert!(e.starts_with("chain limit"), "{e}");
        assert!(!pending_has(over));
        assert!(a.ledger.pending_for_test(s).is_empty(), "no input, no turn");
        assert!(!a.store.notice_committed(s, over).await.unwrap());
        assert!(a.store.errors(s).await.unwrap().is_empty());
        let opener = a.ledger.admit(s, InputOrigin::External, None);
        a.ledger.consumed(InputConsumed {
            session: s,
            id: opener.id,
            turn: t,
        });
        assert_eq!(
            a.admit_notice(notice(s, over)).await,
            Ok(NoticeAdmission::Accepted)
        );
        consume(&mut a);
        clear(over);
        assert_eq!(a.ledger.request_for_test(s, None), (0, 1));
    }

    /// SEC-P3 (rounds 1–2): a foreign file's notice needs the authority it arrived under, not
    /// only a live binding. Ended where this engine holds it (an export removed, the binding
    /// kept), or marked ended by the holder (another engine, or after its restart): the
    /// engine-state row, no input. An unmarked notice whose authority another engine of the
    /// mesh holds is judged by that holder: only the binding is re-checked here.
    #[tokio::test]
    async fn a_foreign_notice_whose_authority_ended_starts_no_turn() {
        use super::super::foreign;
        let d = crate::TestDir::new("notice-foreign");
        let mut a = crate::supervisor::tests::agents_on(d.0.clone(), Some("A"), true).await;
        let s = SessionId::new();
        a.store
            .create_session(s, "m", None, None, None)
            .await
            .unwrap();
        let binding = a
            .store
            .bind_peer("lab", "https://lab.example/")
            .await
            .unwrap();
        let contact = format!("peer:{}", binding.id);
        let ended = tokio_util::sync::CancellationToken::new();
        let (revoked, marked, elsewhere) = (Uuid::new_v4(), Uuid::new_v4(), Uuid::new_v4());
        foreign::authorize(revoked, ended.clone());
        ended.cancel();
        // The holder's outbox marks a notice whose authority it no longer holds live.
        let mark = foreign::notice_contact(&contact, marked);
        assert_eq!(mark, format!("{}{contact}", foreign::ENDED));
        let session = s.to_string();
        assert!(
            foreign::share_live(&a.store, &session, &contact, elsewhere)
                .await
                .unwrap(),
            "another holder's unmarked notice: its binding decides"
        );
        for (id, contact) in [(revoked, contact.clone()), (marked, mark)] {
            let m = AdmitNotice {
                session: s,
                id,
                contact,
                text: TEXT.into(),
            };
            assert_eq!(a.admit_notice(m).await, Ok(NoticeAdmission::Accepted));
            assert!(!pending_has(id), "no input opened");
            assert!(a.store.notice_committed(s, id).await.unwrap(), "recorded");
        }
        assert_eq!(a.store.errors(s).await.unwrap().len(), 2);
    }
}
