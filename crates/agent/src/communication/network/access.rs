//! 200 CONTACT-FULL-ACCESS, the engine side: session addressing (D2), the eligibility
//! predicate (D5), the listing's wire types and the member's page (D7, D8), the `full:` walk
//! cursor.
//!
//! P0 (200-C) freezes the shapes and the pure functions; nothing calls the predicate yet and
//! a send to `peer:<contact>/session:<uuid>` answers [`NOT_YET`]. The engine hooks still to
//! build, each marked `200-E` where it lands:
//! - `network.rs` `network_send` (the [`NOT_YET`] refusal gives way to D10's session branch in
//!   `full.rs` `full_shape`), `admit_principal` (D4's session branch), the remote error
//!   mapping, `remote_peers` (the `peer:<contact>` title and preview rows);
//! - `full.rs` `full_return`: its `share_closed` call becomes a `conversation_closed` here
//!   (D10's settled-card closure, `SettledCards::full_access`);
//! - `supervisor.rs` `ListPeers` / `PeerRead::fetch` / `directory_from` (previews, the walk,
//!   the `full:` cursor, `PeerPage::partial`) and `arrange` (the full-access title);
//! - `rpc_messaging.rs` `contact_file_notice` (199-E1b owns its body): D17's share re-check
//!   gains "or full access on for that contact and `full_access_eligibility(session).visible`",
//!   with the paused-tree hold (design 200, "Interplay with 199").

use runtime::SessionId;
use serde::{Deserialize, Serialize};

use super::super::{ALIAS_MAX, DirectoryKind, PeerActivity, PeerKind, PeerState};
use crate::store::Store;
use crate::supervisor::Agents;

/// D2: the one spelling of a session target: `session:<uuid>` in an address suffix, a path
/// segment (`/agents/session:<uuid>/rpc`), an A2A scope export and a conversation's export.
pub const SESSION_PREFIX: &str = "session:";
/// D3: the one mounted handler `dispatch` rewrites a contact's `session:<uuid>` path to; a
/// path naming it directly is 404.
pub const SESSION_ROUTE: &str = "~session";
/// P0: what every full-access path answers until 200-E lands.
pub const NOT_YET: &str = "full access is not available yet";
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
// 200-E: the row half (`Store::lineage`, `sessions.communication`, the root scope).
pub async fn row_eligibility(_store: &Store, _session: SessionId) -> bool {
    false
}

impl Agents {
    /// D5: the one predicate for the listing, D4's admission and 199's D17 notice re-check.
    /// The row half ([`row_eligibility`]) and, on the owner, the live half: `owner_strict`,
    /// lineage mirrored as `directory_from` does, `Policy::reachable_from_owner`; `sendable`
    /// adds ordinary unscoped intake (not observed, not in a paused tree).
    // 200-E: both halves; answers not visible until then.
    pub async fn full_access_eligibility(&self, _session: SessionId) -> Eligibility {
        Eligibility::default()
    }
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
}
