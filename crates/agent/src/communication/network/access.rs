//! 200 CONTACT-FULL-ACCESS, the engine side: session addressing (D2), the eligibility
//! predicate (D5, both halves), the listing's wire types, the member's page and filters
//! (D7, D8), the `full:` walk cursor, and the sender's card check, refusal texts and the
//! settled-card closure (D10).
//!
//! The hooks elsewhere: `network.rs` `admit_principal` / `admit_send` (D4's session branch),
//! the remote error mapping ([`map_refusal`]); `full.rs` `full_shape` (D10's session branches)
//! and `full_return` (`conversation_closed`); `supervisor.rs` `ListPeers` (previews, the walk,
//! `PeerPage::partial`), `full_access_live` and the member listing ([`FullAccessList`]);
//! `files/notices.rs` `admit_notice` (199 D17's full-access branch, `full_access_notice`).

use std::sync::Arc;

use a2a_types::A2AError;
use kameo::message::{Context, Message};
use runtime::SessionId;
use serde::{Deserialize, Serialize};

use super::super::{
    ALIAS_MAX, DirectoryKind, PeerActivity, PeerInfo, PeerKind, PeerRelationship, PeerState,
};
use crate::store::Store;
use crate::supervisor::Agents;

/// D2: the one spelling of a session target: `session:<uuid>` in an address suffix, a path
/// segment (`/agents/session:<uuid>/rpc`), an A2A scope export and a conversation's export.
pub const SESSION_PREFIX: &str = "session:";
/// D3: the one mounted handler `dispatch` rewrites a contact's `session:<uuid>` path to; a
/// path naming it directly is 404.
pub const SESSION_ROUTE: &str = "~session";
/// The `discover_peers` walk cursor: `full:<contact>:<uuid>`.
pub const CURSOR_PREFIX: &str = "full:";

/// D2: the session `s` names, exactly `session:<uuid>` with the UUID in its canonical
/// (lowercase, hyphenated) form; `None` for an export name, `~session` or any other spelling.
pub fn session_route(s: &str) -> Option<SessionId> {
    canonical(s.strip_prefix(SESSION_PREFIX)?)
}

/// A UUID in its one spelling (36 characters, lowercase, hyphenated).
fn canonical(id: &str) -> Option<SessionId> {
    if id.len() != 36 || id.bytes().any(|b| b.is_ascii_uppercase()) {
        return None;
    }
    id.parse().ok()
}

/// A contact alias as an address names it: 1 to [`ALIAS_MAX`] of `[a-z0-9_-]`.
fn contact_name(c: &str) -> bool {
    !c.is_empty()
        && c.len() <= ALIAS_MAX
        && c.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

/// Contract: the contact and session of a remote name `<contact>/session:<uuid>` (the
/// address `peer:<contact>/session:<uuid>` without `peer:`); the contact is checked alone
/// against [`ALIAS_MAX`], not the whole name.
pub fn contact_session(name: &str) -> Option<(&str, SessionId)> {
    let (contact, rest) = name.split_once('/')?;
    contact_name(contact)
        .then(|| session_route(rest))
        .flatten()
        .map(|s| (contact, s))
}

/// The address of a contact's session: `peer:<contact>/session:<uuid>`.
pub fn session_address(contact: &str, session: SessionId) -> String {
    format!("peer:{contact}/{SESSION_PREFIX}{session}")
}

/// Contract: the sender's text for `peer:<contact>/session:<x>` whose `x` is no session id
/// in its one spelling; `None` for any other address.
pub fn bad_session_id(address: &str) -> Option<String> {
    let name = address.strip_prefix("peer:")?;
    let (contact, rest) = name.split_once('/')?;
    (contact_name(contact) && rest.starts_with(SESSION_PREFIX) && session_route(rest).is_none())
        .then(|| format!("{address}: not a session id"))
}

/// The walk cursor after `last` (`PeerPage.cursor`): `full:<contact>:<uuid>`.
pub fn full_cursor(contact: &str, last: SessionId) -> String {
    format!("{CURSOR_PREFIX}{contact}:{last}")
}

/// The contact and the `after` key of a walk cursor; `None` for any other cursor.
pub fn parse_full_cursor(cursor: &str) -> Option<(&str, SessionId)> {
    let (contact, id) = cursor.strip_prefix(CURSOR_PREFIX)?.split_once(':')?;
    contact_name(contact)
        .then(|| canonical(id))
        .flatten()
        .map(|s| (contact, s))
}

/// D5: what a full-access contact may do with one session: `visible` (listed, task control,
/// D17's notice re-check), `sendable` (new messages; implies `visible`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Eligibility {
    /// Listed and reachable as an existing session.
    pub visible: bool,
    /// Takes a new message.
    pub sendable: bool,
}

/// D5, the row half, on any machine from its replicated store: a durable row, not an
/// advisor, no `contact_inbox` marker or opt-out on its ancestry, an `Owner` root. An
/// unreadable row reads not visible (fail closed).
pub async fn row_eligibility(store: &Store, session: SessionId) -> bool {
    store
        .full_access_row(session)
        .await
        .inspect_err(|e| tracing::warn!(%session, "200: session row unreadable: {e:#}"))
        .unwrap_or(false)
}

impl Agents {
    /// D5: the one predicate for the listing, D4's admission and 199's D17 notice re-check.
    /// The row half ([`row_eligibility`]) and, on the owner, the live half: `owner_strict`,
    /// lineage mirrored as `directory_from` does, `Policy::reachable_from_owner`; `sendable`
    /// adds ordinary unscoped intake (not observed, not in a paused tree).
    pub async fn full_access_eligibility(&mut self, session: SessionId) -> Eligibility {
        use crate::store::contacts::Owner;
        if !row_eligibility(&self.store, session).await
            || !matches!(self.store.owner_strict(session).await, Ok(Owner::Here))
        {
            return Eligibility::default();
        }
        self.mirror(session).await;
        match self.store.session(session).await {
            Ok(Some(row)) => self.full_access_live(&row),
            _ => Eligibility::default(),
        }
    }

    /// 199 D17's full-access branch (design 200, "Interplay with 199"): `contact` (a record
    /// id) is a live contact we give full access now (its live route's grant and D1's group
    /// rule on the records), and `session` is visible to it now (D5, both halves).
    pub(crate) async fn full_access_notice(&mut self, session: SessionId, contact: &str) -> bool {
        let mut ours = false;
        for b in self.network.full_access_bindings() {
            let of = self.store.contact_of_binding(&format!("peer:{b}")).await;
            if let Ok(Some((id, _))) = of
                && self
                    .store
                    .share_root_eq(&id, contact)
                    .await
                    .unwrap_or(false)
            {
                ours = true;
                break;
            }
        }
        ours && self
            .store
            .full_access_granted(contact)
            .await
            .unwrap_or(false)
            && self.full_access_eligibility(session).await.visible
    }

    /// SEC-200-1, the conversation half of 199 D17: one of `threads` (`(binding, export)`,
    /// conversations the chat opened toward the contact, [`Store::notice_threads`]) whose
    /// return rule still holds as `full_return` applies it to the answer that carried the
    /// file: the contact's route live, and the conversation not closed by the settled
    /// cards (D10 for `session:<uuid>`, 186 D12 for an export).
    pub(crate) fn conversation_open(&self, threads: &[(String, String)]) -> bool {
        threads.iter().any(|(binding, export)| {
            binding
                .parse::<uuid::Uuid>()
                .ok()
                .and_then(|b| self.network.peers.get(&b))
                .filter(|p| p.contact && !p.generation.is_cancelled())
                .is_some_and(|p| !self.network.conversation_closed(&p.name, export))
        })
    }
}

/// `ask` → `Result<(), A2AError>` (D4 (3)): the row half on the machine that received a
/// contact's `session:<uuid>` request, before the strict owner resolve: not visible is the
/// plain `not owner` (never forwarded), so every hidden session answers alike everywhere.
#[derive(Clone, Copy, Debug)]
pub struct FullAccessRow(pub SessionId);

impl Message<FullAccessRow> for Agents {
    type Reply = Result<(), A2AError>;

    async fn handle(
        &mut self,
        m: FullAccessRow,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if row_eligibility(&self.store, m.0).await {
            Ok(())
        } else {
            tracing::info!(session = %m.0, "200: full access: not visible here");
            Err(super::full::not_owner())
        }
    }
}

/// `ask` → `Result<Vec<ListedSession>, String>` (D7): this machine's answer to a contact's
/// `GET /theuth/sessions`: the visible sessions it owns, filtered by `req` (`machine` is
/// this machine's name, for the query), its page ([`member_page`]).
#[derive(Clone, Debug)]
pub struct FullAccessList {
    /// The member request.
    pub req: ListRequest,
    /// This machine's name as the contact shows it.
    pub machine: String,
}

/// D8: one listing request, the same for every member: up to `limit` rows below `after`,
/// filtered by the member (`query`, `kind`, `active_only`), so the caller never trims.
/// `PeerSender::list_sessions` takes it, and the member route parses it from its query.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListRequest {
    /// Rows with a smaller session id only; any UUID is a key (never looked up).
    pub after: Option<SessionId>,
    /// 1 to `PAGE_MAX` rows.
    pub limit: u16,
    /// Lowercase substring of the title, `session:<uuid>`, the backend kind or the machine.
    pub query: Option<String>,
    /// `DirectoryKind::admits` on the row as the caller renders it (kind `a2a`, tag `contact`).
    pub kind: Option<DirectoryKind>,
    /// Rows whose activity is active only.
    pub active_only: bool,
}

/// D6: one visible session as its member answers it: discover_peers' own-session fields plus
/// `send`/`reply`, nothing else (the engine is the member's `machine`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedSession {
    /// The session id (its address is `peer:<contact>/session:<uuid>`).
    pub session: SessionId,
    /// Its title: control and bidi characters removed, cut to `TITLE_MAX`.
    #[serde(default)]
    pub title: Option<String>,
    /// Backend.
    pub kind: PeerKind,
    /// Coarse liveness.
    pub state: PeerState,
    /// Takes a new message (D5 `sendable`).
    pub send: bool,
    /// Can answer with `reply_to`.
    pub reply: bool,
    /// Live state.
    #[serde(default)]
    pub activity: Option<PeerActivity>,
}

/// D7: one member's answer to `GET /theuth/sessions`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberListing {
    /// The member's machine name.
    pub machine: String,
    /// Its page: id descending, at most `limit`, all below `after`.
    pub sessions: Vec<ListedSession>,
}

/// One merged row: a member's session and its machine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedRow {
    /// The member that answered it.
    pub machine: String,
    /// The session.
    #[serde(flatten)]
    pub session: ListedSession,
}

/// D8: the merged listing on the asking side (`PeerSender::list_sessions`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContactListing {
    /// At most `limit` rows, session id descending, each id once.
    pub rows: Vec<ListedRow>,
    /// The members that did not answer in time, by machine name; reporting only.
    #[serde(default)]
    pub partial: Vec<String>,
    /// The last emitted id; `None` only when every answering member returned fewer than
    /// `limit` rows and nothing was trimmed.
    pub cursor: Option<SessionId>,
}

/// D8, the member's page: of its visible, filtered `rows`, those below `after`, session id
/// descending (UUIDv7: creation order, never `last_active`), the first `limit`.
pub fn member_page(
    mut rows: Vec<ListedSession>,
    after: Option<SessionId>,
    limit: u16,
) -> Vec<ListedSession> {
    rows.retain(|r| after.is_none_or(|a| r.session.0 < a.0));
    rows.sort_unstable_by_key(|r| std::cmp::Reverse(r.session.0));
    rows.truncate(limit.into());
    rows
}

/// D6: an untrusted title as a listing shows it: control and bidi characters removed, cut to
/// `max` characters; empty is none.
pub fn clean_title(title: &str, max: usize) -> Option<String> {
    let t: String = title
        .chars()
        .filter(|c| {
            !c.is_control()
                && !matches!(c, '\u{200e}' | '\u{200f}' | '\u{061c}')
                && !('\u{202a}'..='\u{202e}').contains(c)
                && !('\u{2066}'..='\u{2069}').contains(c)
        })
        .take(max)
        .collect();
    let t = t.trim();
    (!t.is_empty()).then(|| t.to_owned())
}

/// A backend kind's wire name (`native`, `acp`, …): what the member's query matches and the
/// third tag of a listed row.
pub fn kind_name(k: PeerKind) -> &'static str {
    match k {
        PeerKind::Native => "native",
        PeerKind::Acp => "acp",
        PeerKind::Node => "node",
        PeerKind::A2a => "a2a",
        PeerKind::Parent => "parent",
    }
}

/// Longest `name` of a listed row.
const NAME_MAX: usize = 40;

/// Contract: one listed session of `contact` as a `discover_peers` row.
pub fn session_row(contact: &str, r: &ListedRow) -> PeerInfo {
    let s = &r.session;
    let label = s
        .title
        .clone()
        .unwrap_or_else(|| s.session.to_string().chars().take(8).collect());
    PeerInfo {
        address: session_address(contact, s.session),
        name: format!("{contact}/{label}")
            .chars()
            .take(NAME_MAX)
            .collect(),
        kind: PeerKind::A2a,
        relationship: PeerRelationship::Remote,
        state: s.state,
        send: s.send,
        reply: s.reply,
        stream: false,
        activity: s.activity.clone(),
        title: s.title.clone(),
        tags: vec![
            "contact".to_owned(),
            "session".to_owned(),
            kind_name(s.kind).to_owned(),
        ],
        engine: Some(r.machine.clone()),
        reachable: None,
    }
}

/// A listed row (`tags[1] == "session"`): a contact's session, not one of its exports.
pub fn is_session_row(p: &PeerInfo) -> bool {
    super::super::is_contact_row(p) && p.tags.get(1).is_some_and(|t| t == "session")
}

/// Contract, the member's filters: `kind` with `DirectoryKind::admits` on the row as the
/// caller renders it, `active_only` on its activity, `query` (lowercase substring) on the
/// title, `session:<uuid>`, the backend kind and `machine`.
pub fn member_admits(req: &ListRequest, s: &ListedSession, machine: &str) -> bool {
    if req.active_only && !s.activity.as_ref().is_some_and(PeerActivity::active) {
        return false;
    }
    if let Some(k) = req.kind {
        let row = ListedRow {
            machine: machine.to_owned(),
            session: s.clone(),
        };
        if !k.admits(&session_row("", &row)) {
            return false;
        }
    }
    let Some(q) = req.query.as_deref().map(str::to_lowercase) else {
        return true;
    };
    s.title
        .as_deref()
        .is_some_and(|t| t.to_lowercase().contains(&q))
        || format!("{SESSION_PREFIX}{}", s.session).contains(&q)
        || kind_name(s.kind).contains(&q)
        || machine.to_lowercase().contains(&q)
}

/// Contract: the contact a `discover_peers` query names, and what the members are asked:
/// `<alias>` / `peer:<alias>` → no query, `<alias>/<rest>` / `peer:<alias>/<rest>` → `rest`.
/// `None` when it names no contact of `granting`.
pub fn names_contact<'a>(query: &'a str, granting: &[&str]) -> Option<(String, Option<&'a str>)> {
    let q = query.strip_prefix("peer:").unwrap_or(query);
    let (alias, rest) = match q.split_once('/') {
        Some((a, r)) => (a, Some(r).filter(|r| !r.is_empty())),
        None => (q, None),
    };
    granting
        .iter()
        .find(|g| **g == alias)
        .map(|g| ((*g).to_owned(), rest))
}

/// What the sender's cards say of full access from one contact (D10).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardSays {
    /// No verified card of it this run: the message leaves, the card is asked for.
    Unknown,
    /// Its card lacks the extension.
    Unsupported,
    /// `granted: false`.
    NotGranted,
    /// `granted: true`.
    Granted,
}

/// The text a sender reads for `peer:<contact>/session:<uuid>` (`name` without `peer:`).
pub fn not_granted(name: &str, contact: &str) -> String {
    format!(
        "peer:{name}: {contact} has not given you full access; message a chat {contact} shares instead (discover_peers kind=\"contact\")"
    )
}

/// D10, the sender's pre-check: `Err` with the sender's text when the latest verified card
/// refuses (`Unsupported`, `NotGranted`).
pub fn pre_check(says: CardSays, name: &str, contact: &str) -> Result<(), String> {
    match says {
        CardSays::Unsupported => Err(format!(
            "peer:{name}: {contact}'s theuth does not support full access yet"
        )),
        CardSays::NotGranted => Err(not_granted(name, contact)),
        CardSays::Unknown | CardSays::Granted => Ok(()),
    }
}

/// D4's owner refusals as the sender of `peer:<name>` reads them (the contact's answer is
/// the cause); `None` keeps `why` as it is. `name` must be `<contact>/session:<uuid>`.
pub fn map_refusal(name: &str, why: &str) -> Option<String> {
    let (contact, _) = contact_session(name)?;
    let text = if why.contains(OFF) {
        not_granted(name, contact)
    } else if why.contains(NO_MESSAGES) {
        format!("peer:{name}: {NO_MESSAGES}")
    } else if why.contains(super::NOT_OWNER_TEXT) {
        format!("peer:{name}: no such session")
    } else if why.contains("grants no export") {
        format!("peer:{name}: the machine that holds this session needs updating")
    } else {
        return None;
    };
    Some(text)
}

/// D4 (2): what the owner answers a contact without full access.
pub const OFF: &str = "full access is off";
/// D4 (5): a visible session that takes no new message.
pub const NO_MESSAGES: &str = "this session does not take messages";

impl super::NetworkState {
    /// D10: what the latest verified card of `contact` says of full access for us.
    pub(crate) fn card_says(&self, contact: &str) -> CardSays {
        if !self.offers.contains_key(contact) {
            return CardSays::Unknown;
        }
        match self.full_access.get(contact) {
            None => CardSays::Unsupported,
            Some(false) => CardSays::NotGranted,
            Some(true) => CardSays::Granted,
        }
    }

    /// D10's settled-card closure, `full_return`'s consistency check: a `session:<uuid>`
    /// conversation closes exactly when the contact is settled (every machine of its roster
    /// served a verified card at its last refresh) and none grants us full access; any other
    /// export as 186 D12 ([`Self::share_closed`]).
    pub(crate) fn conversation_closed(&self, alias: &str, export: &str) -> bool {
        match session_route(export) {
            Some(_) => self.settled.get(alias).is_some_and(|s| !s.full_access),
            None => self.share_closed(alias, export),
        }
    }

    /// Contract: the live contacts whose latest verified card grants us full access, with
    /// their senders, by alias; none for a contact's inbox root (it sees no contact).
    pub(crate) fn granting(&self, root: SessionId) -> Vec<(String, Arc<dyn super::PeerSender>)> {
        if self.inboxes.contains(&root) {
            return Vec::new();
        }
        let mut out: Vec<(String, Arc<dyn super::PeerSender>)> = self
            .peers
            .values()
            .filter(|p| p.contact && !p.generation.is_cancelled())
            .filter(|p| self.card_says(&p.name) == CardSays::Granted)
            .filter_map(|p| Some((p.name.clone(), p.sender.clone()?)))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// D10: we gave the contact on `binding` full access (my grant, its `Peer`).
    pub(crate) fn grants_full_access(&self, binding: uuid::Uuid) -> bool {
        self.peers.get(&binding).is_some_and(|p| p.full_access)
    }

    /// 199 D17's full-access branch: the live contact routes we gave full access, by binding.
    pub(crate) fn full_access_bindings(&self) -> Vec<uuid::Uuid> {
        self.peers
            .iter()
            .filter(|(_, p)| p.contact && p.full_access && !p.generation.is_cancelled())
            .map(|(b, _)| *b)
            .collect()
    }

    /// D5: `s` is a contact's inbox this run (resolved, or its limits installed): another
    /// contact's conversation, hidden like a private chat.
    pub(crate) fn contact_inbox(&self, s: SessionId) -> bool {
        self.inboxes.contains(&s) || self.inbox_limits.contains_key(&s)
    }

    /// Tests: a live contact route `name` on `binding` we gave full access (no sender).
    #[cfg(test)]
    pub(crate) fn insert_full_access_for_test(&mut self, binding: uuid::Uuid, name: &str) {
        self.insert_route_for_test(binding, name, true);
    }

    /// Tests: a live contact route `name` on `binding` (no sender), our grant
    /// `full_access`; it replaces (and cancels) the route on that binding, as a reconcile.
    #[cfg(test)]
    pub(crate) fn insert_route_for_test(
        &mut self,
        binding: uuid::Uuid,
        name: &str,
        full_access: bool,
    ) {
        let old = self.peers.insert(
            binding,
            super::RemotePeer {
                name: name.into(),
                roots: Vec::new(),
                sender: None,
                generation: tokio_util::sync::CancellationToken::new(),
                contact: true,
                full: Vec::new(),
                full_access,
            },
        );
        if let Some(old) = old {
            old.generation.cancel();
        }
    }

    /// Tests: `alias`'s settled cards (every machine verified).
    #[cfg(test)]
    pub(crate) fn settle_for_test(&mut self, alias: &str, cards: super::SettledCards) {
        self.settled.insert(alias.into(), cards);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ID: &str = "0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f";

    #[test]
    fn session_route_takes_only_a_canonical_session_uuid() {
        let id: SessionId = ID.parse().unwrap();
        assert_eq!(session_route(&format!("session:{ID}")), Some(id));
        for bad in [
            "research",
            "~session",
            "~return",
            ID,
            "session:",
            "session:not-a-uuid",
            "session:0193F0A2-6B1C-7D3E-9F40-1A2B3C4D5E6F",
            "session:0193f0a26b1c7d3e9f401a2b3c4d5e6f",
            "session:{0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f}",
            "session:urn:uuid:0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f",
        ] {
            assert_eq!(session_route(bad), None, "{bad}");
        }
    }

    #[test]
    fn a_contact_session_name_bounds_the_contact_alone() {
        let id: SessionId = ID.parse().unwrap();
        let max = "m".repeat(ALIAS_MAX);
        let name = format!("{max}/session:{ID}");
        assert_eq!(contact_session(&name), Some((max.as_str(), id)));
        assert_eq!(
            session_address(&max, id),
            format!("peer:{name}"),
            "format and parse agree"
        );
        for bad in [
            format!("{max}m/session:{ID}"),
            format!("/session:{ID}"),
            format!("Maya/session:{ID}"),
            format!("maya/session:{}", &ID[1..]),
            "maya/research".to_owned(),
            "maya/~session".to_owned(),
            format!("maya/x/session:{ID}"),
        ] {
            assert_eq!(contact_session(&bad), None, "{bad}");
        }
    }

    #[test]
    fn the_walk_cursor_round_trips_and_refuses_other_cursors() {
        let id: SessionId = ID.parse().unwrap();
        let c = full_cursor("maya", id);
        assert_eq!(c, format!("full:maya:{ID}"));
        assert_eq!(parse_full_cursor(&c), Some(("maya", id)));
        for bad in [
            format!("session:{ID}"),
            format!("full:maya:session:{ID}"),
            format!("full::{ID}"),
            format!("full:Maya:{ID}"),
            "full:maya:".to_owned(),
            "full:maya".to_owned(),
            format!("peer:maya/session:{ID}"),
        ] {
            assert_eq!(parse_full_cursor(&bad), None, "{bad}");
        }
    }

    #[test]
    fn a_member_page_is_below_after_id_descending_and_limited() {
        let row = |n: u128| ListedSession {
            session: SessionId(uuid::Uuid::from_u128(n)),
            title: None,
            kind: PeerKind::Native,
            state: PeerState::Idle,
            send: true,
            reply: true,
            activity: None,
        };
        let ids = |rows: Vec<ListedSession>| -> Vec<u128> {
            rows.iter().map(|r| r.session.0.as_u128()).collect()
        };
        let all = || vec![row(70), row(100), row(80), row(90)];
        assert_eq!(ids(member_page(all(), None, 2)), [100, 90]);
        let after = Some(SessionId(uuid::Uuid::from_u128(90)));
        assert_eq!(ids(member_page(all(), after, 5)), [80, 70]);
        // Any UUID is a key: one between two ids pages exactly like an id.
        let between = Some(SessionId(uuid::Uuid::from_u128(85)));
        assert_eq!(ids(member_page(all(), between, 5)), [80, 70]);
    }

    /// Contract (refusal and error texts): the owner's refusals in the sender's words, the
    /// card pre-check, a bad id; any other address or text is left as it is.
    #[test]
    fn a_sender_reads_the_contracts_texts() {
        let name = format!("maya/session:{ID}");
        let at = |t: &str| format!("peer:{name}: {t}");
        let not_granted = at(
            "maya has not given you full access; message a chat maya shares instead (discover_peers kind=\"contact\")",
        );
        assert_eq!(
            map_refusal(&name, "full access is off").as_deref(),
            Some(not_granted.as_str())
        );
        assert_eq!(
            map_refusal(&name, "peer maya refused: not owner on every machine of it"),
            Some(at("no such session"))
        );
        assert_eq!(map_refusal(&name, NO_MESSAGES), Some(at(NO_MESSAGES)));
        assert_eq!(
            map_refusal(&name, "the contact grants no export to this engine"),
            Some(at("the machine that holds this session needs updating"))
        );
        assert_eq!(map_refusal(&name, "busy: 8 tasks already wait here"), None);
        assert_eq!(map_refusal("maya/proj", "not owner"), None, "a share");
        assert_eq!(
            pre_check(CardSays::Unsupported, &name, "maya").unwrap_err(),
            at("maya's theuth does not support full access yet")
        );
        assert_eq!(
            pre_check(CardSays::NotGranted, &name, "maya").unwrap_err(),
            not_granted
        );
        assert!(pre_check(CardSays::Unknown, &name, "maya").is_ok());
        assert!(pre_check(CardSays::Granted, &name, "maya").is_ok());
        assert_eq!(
            bad_session_id("peer:maya/session:x").as_deref(),
            Some("peer:maya/session:x: not a session id")
        );
        assert_eq!(bad_session_id(&format!("peer:maya/session:{ID}")), None);
        assert_eq!(bad_session_id("peer:maya/proj"), None);
    }

    /// Contract: the query names a granting contact (`maya`, `peer:maya`, `maya/<rest>`);
    /// the member filters on the title, the id, the backend kind and the machine, `kind` on
    /// the row as rendered (`a2a`, `contact` keep it), `active_only` on its activity.
    #[test]
    fn the_query_names_the_contact_and_the_member_filters() {
        let granting = ["maya", "sky"];
        assert_eq!(
            names_contact("maya", &granting),
            Some(("maya".to_owned(), None))
        );
        assert_eq!(
            names_contact("peer:maya/port", &granting),
            Some(("maya".to_owned(), Some("port")))
        );
        assert_eq!(
            names_contact("maya/", &granting),
            Some(("maya".to_owned(), None))
        );
        assert_eq!(names_contact("bob", &granting), None);
        assert_eq!(names_contact("port", &granting), None);
        let s = ListedSession {
            session: ID.parse().unwrap(),
            title: Some("Port layout review".into()),
            kind: PeerKind::Acp,
            state: PeerState::Idle,
            send: true,
            reply: false,
            activity: None,
        };
        let req = |query: Option<&str>, kind, active_only| ListRequest {
            after: None,
            limit: 10,
            query: query.map(str::to_owned),
            kind,
            active_only,
        };
        for q in ["port", "session:0193f0a2", "acp", "macbook"] {
            assert!(
                member_admits(&req(Some(q), None, false), &s, "Sky's MacBook"),
                "{q}"
            );
        }
        assert!(!member_admits(
            &req(Some("deploy"), None, false),
            &s,
            "Sky's MacBook"
        ));
        for (k, keeps) in [
            (DirectoryKind::Contact, true),
            (DirectoryKind::A2a, true),
            (DirectoryKind::Native, false),
            (DirectoryKind::Acp, false),
        ] {
            assert_eq!(
                member_admits(&req(None, Some(k), false), &s, "m"),
                keeps,
                "{k:?}"
            );
        }
        assert!(!member_admits(&req(None, None, true), &s, "m"), "idle");
        assert_eq!(
            clean_title("a\u{202e}b\u{7}c\u{2066}", 120).as_deref(),
            Some("abc")
        );
        assert_eq!(
            clean_title(&"x".repeat(200), 120).map(|t| t.len()),
            Some(120)
        );
        assert_eq!(clean_title("\u{200f}", 120), None);
    }

    /// D10: a `session:` conversation closes exactly when the contact is settled and no card
    /// grants full access; unsettled (a machine unverified, a failed refresh) admits; a share
    /// keeps 186's rule.
    #[test]
    fn a_session_conversation_closes_on_the_settled_cards() {
        let mut n = super::super::NetworkState::default();
        let session = format!("session:{ID}");
        assert!(!n.conversation_closed("maya", &session), "unsettled");
        n.settled.insert(
            "maya".into(),
            super::super::SettledCards {
                full: ["proj".to_owned()].into(),
                full_access: false,
            },
        );
        assert!(n.conversation_closed("maya", &session));
        assert!(!n.conversation_closed("maya", "proj"), "a share stays");
        n.settled.get_mut("maya").unwrap().full_access = true;
        assert!(!n.conversation_closed("maya", &session));
    }
}
