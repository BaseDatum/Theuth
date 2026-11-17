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
        // 199-E3 (SEC-P3): a foreign file's own authority, not only its binding.
        let live =
            super::foreign::share_live(&self.store, &m.session.to_string(), &m.contact, m.id)
                .await
                .map_err(|e| format!("{e:#}"))?;
        if !live {
            ended(&self.store, &self.rt, m.session, None, m.id, &m.text).await?;
            return Ok(NoticeAdmission::Accepted);
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
