//! 200 CONTACT-FULL-ACCESS, the a2a side: the `full-access/v1` card extension (D11), the
//! trusted session header (D3), the contact-only session listing route (D7, [`serve`]) and
//! the asking side's fan-out and merge (D8, [`list`], [`merge`]).
//!
//! The other hooks: `reach.rs` `Route::Sessions`; `server.rs` `dispatch` (the
//! `session:<uuid>` branch: rewrite to `~session`, set [`SESSION_HEADER`]), `Live::build`
//! (mount `~session`), `Target::Session`, `Handler::admit_session`; `offers.rs`
//! `Ctx::contact_card` (advertise [`EXT_FULL_ACCESS`] per reader), the card refresh
//! (`Cached.full_access` from [`card_full_access`]), `offers::changed`; `forward.rs`
//! `carried` (a `session:<uuid>` segment, never `~session`); `client.rs`
//! `PeerSender::list_sessions`.

use std::sync::Arc;
use std::time::Duration;

use a2a_types::AgentCard;
use agent::communication::DirectoryKind;
use agent::communication::network::access::{
    self, ContactListing, FullAccessList, ListRequest, ListedRow, MemberListing,
};
use axum::http::HeaderMap;
use axum::response::{IntoResponse as _, Response};
use hyper::StatusCode;
use serde::Deserialize as _;
use serde::de::IntoDeserializer as _;

use crate::Ctx;

/// D11: the card extension a reader's own card carries, `params: {"granted": bool}`: present =
/// supported, `granted` = this reader has full access (`offers::Ctx::contact_card`).
pub const EXT_FULL_ACCESS: &str = "https://theuth.io/a2a/ext/full-access/v1";
/// D7: the contact-only listing route (beside Card and Notice; never carried).
pub const SESSIONS_PATH: &str = "/theuth/sessions";
/// D3: the trusted header `dispatch` sets to the session's UUID after rewriting a
/// `session:<uuid>` path; any inbound copy is removed first ([`strip_session_header`]).
pub const SESSION_HEADER: &str = "x-theuth-session";
/// D8: a member answers at most this many rows per request.
pub const PAGE_MAX: u16 = 200;
/// The contact preview in default discovery (= `CONTACT_ROWS` in the supervisor).
pub const PREVIEW: u16 = 8;
/// D8: how long the asking side waits for each member.
pub const LIST_DEADLINE: Duration = Duration::from_secs(2);
/// D6: a listed title's length in characters.
pub const TITLE_MAX: usize = 120;

/// D11: what a verified card says of full access for its reader: `None` without the
/// extension (an engine without 200), else its `granted` (anything but `true` reads false).
pub fn card_full_access(card: &AgentCard) -> Option<bool> {
    let e = card
        .capabilities
        .extensions
        .iter()
        .flatten()
        .find(|e| e.uri == EXT_FULL_ACCESS)?;
    Some(
        e.params
            .as_ref()
            .and_then(|p| p.get("granted"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false),
    )
}

/// D3: drop every inbound copy of [`SESSION_HEADER`]: only `dispatch` sets it, after it
/// resolved a contact's `session:<uuid>` path.
pub fn strip_session_header(headers: &mut HeaderMap) {
    headers.remove(SESSION_HEADER);
}

/// D7, one member: the request in `GET /theuth/sessions?limit=<1..200>[&after=<uuid>]
/// [&query=<text>][&kind=<k>][&active_only=1]`; `None` (`400 bad request`) for a missing or
/// malformed `limit`, a malformed `after`, `kind` or `active_only`. `after` is any UUID
/// (never looked up); unknown keys are ignored.
pub fn parse_request(query: Option<&str>) -> Option<ListRequest> {
    let mut req = ListRequest::default();
    let mut limit = None;
    for (k, v) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match &*k {
            "limit" => limit = Some(v.parse().ok().filter(|n| (1..=PAGE_MAX).contains(n))?),
            "after" => req.after = Some(v.parse().ok()?),
            "query" => req.query = Some(v.into_owned()).filter(|q| !q.is_empty()),
            "kind" => {
                let d: serde::de::value::StrDeserializer<'_, serde::de::value::Error> =
                    v.as_ref().into_deserializer();
                req.kind = Some(DirectoryKind::deserialize(d).ok()?);
            }
            "active_only" => {
                req.active_only = match &*v {
                    "1" => true,
                    "0" => false,
                    _ => return None,
                }
            }
            _ => {}
        }
    }
    req.limit = limit?;
    Some(req)
}

/// D8, the asking side: every answering member's page merged by session id descending, each
/// id once (an ownership move can make two answers overlap: the first answer keeps it), the
/// first `limit` emitted. The cursor is the last emitted id, `None` only when every answer
/// held fewer than `limit` rows and nothing was trimmed. `partial` (the members that did not
/// answer in time) passes through: reporting only.
pub fn merge(answers: Vec<MemberListing>, partial: Vec<String>, limit: u16) -> ContactListing {
    let limit = usize::from(limit);
    let full = answers.iter().any(|a| a.sessions.len() >= limit);
    let mut rows: Vec<ListedRow> = answers
        .into_iter()
        .flat_map(|a| {
            let machine = a.machine;
            a.sessions.into_iter().map(move |session| ListedRow {
                machine: machine.clone(),
                session,
            })
        })
        .collect();
    rows.sort_by_key(|r| std::cmp::Reverse(r.session.session.0));
    rows.dedup_by_key(|r| r.session.session);
    let trimmed = rows.len() > limit;
    rows.truncate(limit);
    let cursor = (full || trimmed)
        .then(|| rows.last().map(|r| r.session.session))
        .flatten();
    ContactListing {
        rows,
        partial,
        cursor,
    }
}

/// The query string of `req` as [`parse_request`] reads it.
fn query_of(req: &ListRequest) -> String {
    let mut q = url::form_urlencoded::Serializer::new(String::new());
    q.append_pair("limit", &req.limit.to_string());
    if let Some(a) = req.after {
        q.append_pair("after", &a.to_string());
    }
    if let Some(t) = &req.query {
        q.append_pair("query", t);
    }
    if let Some(k) = req.kind
        && let Ok(serde_json::Value::String(k)) = serde_json::to_value(k)
    {
        q.append_pair("kind", &k);
    }
    if req.active_only {
        q.append_pair("active_only", "1");
    }
    q.finish()
}

/// Longest machine name kept from a member's answer (the card's text limit).
const MACHINE_MAX: usize = 64;

/// D7, one member: `GET` [`SESSIONS_PATH`] from the contact pinned to `remote` (its permit
/// held by `contact_route`): `503` before `contacts_ready`, `403 full access is off`, `400
/// bad request`, else this machine's [`MemberListing`]. The answer races the grant epoch it
/// was admitted under: full access turned off (any grant change) mid-listing is `401`, and no
/// title leaves after it.
pub(crate) async fn serve(c: &Arc<Ctx>, remote: &str, query: Option<&str>) -> Response {
    if !c.ready.load(std::sync::atomic::Ordering::SeqCst) {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let Some((epoch, on)) = c.shared.read().full_access_of(remote) else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !on {
        return (StatusCode::FORBIDDEN, access::OFF).into_response();
    }
    let Some(req) = parse_request(query) else {
        return (StatusCode::BAD_REQUEST, "bad request").into_response();
    };
    let Ok(agents) = c.agents() else {
        return StatusCode::SERVICE_UNAVAILABLE.into_response();
    };
    let machine = c.my_names().1;
    let listed = agents.ask(FullAccessList {
        req,
        machine: machine.clone(),
    });
    let answer = tokio::select! {
        biased;
        () = epoch.cancelled() => {
            tracing::info!(%remote, "200: listing ended: the grant changed");
            return StatusCode::UNAUTHORIZED.into_response();
        }
        r = listed => r,
    };
    match answer {
        Ok(mut sessions) => {
            for s in &mut sessions {
                s.title = s
                    .title
                    .as_deref()
                    .and_then(|t| access::clean_title(t, TITLE_MAX));
            }
            if epoch.is_cancelled() {
                return StatusCode::UNAUTHORIZED.into_response();
            }
            axum::Json(MemberListing { machine, sessions }).into_response()
        }
        Err(e) => {
            tracing::warn!(%remote, "200: listing failed: {e}");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

/// D8, the asking side: `req` to every member of `contact` (`eids`) at once over
/// `theuth/a2a/1`; answers within [`LIST_DEADLINE`] merged ([`merge`]), the rest named in
/// `partial` by `name_of`. A member's page is re-cut to the request (below `after`, at most
/// `limit`, titles cleaned): an answer never moves the cursor. `Err` only when every member
/// that answered refused (`403`) and none was missing.
pub(crate) async fn list(
    net: &mesh::net::Net,
    contact: &str,
    eids: &[String],
    name_of: &(dyn Fn(&str) -> String + Sync),
    req: &ListRequest,
) -> Result<ContactListing, String> {
    let path = format!("{SESSIONS_PATH}?{}", query_of(req));
    let answers = futures::future::join_all(eids.iter().map(|eid| {
        let path = path.as_str();
        async move {
            let r = tokio::time::timeout(
                LIST_DEADLINE,
                super::offers::call_with(net, eid, hyper::Method::GET, path, None),
            )
            .await;
            (eid, r)
        }
    }))
    .await;
    let (mut got, mut partial, mut refused) = (Vec::new(), Vec::new(), 0usize);
    for (eid, r) in answers {
        match r {
            Ok(Ok((s, body))) if s == StatusCode::OK => {
                match serde_json::from_slice::<MemberListing>(&body) {
                    Ok(mut m) => {
                        m.sessions = access::member_page(m.sessions, req.after, req.limit);
                        for s in &mut m.sessions {
                            s.title = s
                                .title
                                .as_deref()
                                .and_then(|t| access::clean_title(t, TITLE_MAX));
                        }
                        m.machine = access::clean_title(&m.machine, MACHINE_MAX)
                            .unwrap_or_else(|| name_of(eid));
                        got.push(m);
                    }
                    Err(e) => {
                        tracing::info!(%eid, "200: listing answer not read: {e}");
                        partial.push(name_of(eid));
                    }
                }
            }
            Ok(Ok((s, _))) if s == StatusCode::FORBIDDEN => refused += 1,
            Ok(Ok((s, _))) => {
                tracing::info!(%eid, "200: listing answered HTTP {s}");
                partial.push(name_of(eid));
            }
            Ok(Err(e)) => {
                tracing::info!(%eid, "200: listing: {e}");
                partial.push(name_of(eid));
            }
            Err(_) => partial.push(name_of(eid)),
        }
    }
    if got.is_empty() && partial.is_empty() && refused > 0 {
        return Err(format!("{contact} has not given you full access"));
    }
    Ok(merge(got, partial, req.limit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent::communication::network::access::{ListedSession, member_page};
    use agent::communication::{PeerKind, PeerState};
    use runtime::SessionId;

    fn sid(n: u128) -> SessionId {
        SessionId(uuid::Uuid::from_u128(n))
    }

    fn row(n: u128) -> ListedSession {
        ListedSession {
            session: sid(n),
            title: None,
            kind: PeerKind::Native,
            state: PeerState::Idle,
            send: true,
            reply: true,
            activity: None,
        }
    }

    /// A member holding `ids`, answering `after` / `limit` as the route will.
    fn answer(machine: &str, ids: &[u128], after: Option<SessionId>, limit: u16) -> MemberListing {
        MemberListing {
            machine: machine.to_owned(),
            sessions: member_page(ids.iter().map(|n| row(*n)).collect(), after, limit),
        }
    }

    fn ids(l: &ContactListing) -> Vec<u128> {
        l.rows
            .iter()
            .map(|r| r.session.session.0.as_u128())
            .collect()
    }

    #[test]
    fn the_card_extension_reads_absent_granted_and_refused() {
        let facts = crate::contacts::CardFacts {
            name: "Maya".into(),
            description: String::new(),
            machine: "studio".into(),
            os_name: "macOS".into(),
            endpoint: "e".into(),
            public_url: None,
            skills: vec![],
        };
        let mut card = crate::contacts::card(&facts);
        assert_eq!(
            card_full_access(&card),
            None,
            "the common card: not advertised"
        );
        let with = |params: Option<serde_json::Value>| {
            let mut c = card.clone();
            c.capabilities
                .extensions
                .get_or_insert_default()
                .push(a2a_types::AgentExtension {
                    uri: EXT_FULL_ACCESS.to_owned(),
                    description: None,
                    required: Some(false),
                    params: params.map(|p| serde_json::from_value(p).unwrap()),
                });
            card_full_access(&c)
        };
        assert_eq!(with(Some(serde_json::json!({"granted": true}))), Some(true));
        assert_eq!(
            with(Some(serde_json::json!({"granted": false}))),
            Some(false)
        );
        assert_eq!(
            with(Some(serde_json::json!({"granted": "yes"}))),
            Some(false)
        );
        assert_eq!(with(None), Some(false));
        card.capabilities.extensions = None;
        assert_eq!(card_full_access(&card), None);
    }

    #[test]
    fn every_inbound_session_header_is_stripped() {
        let mut h = HeaderMap::new();
        h.append(
            SESSION_HEADER,
            "0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f".parse().unwrap(),
        );
        let mixed = axum::http::HeaderName::from_bytes(b"X-Theuth-Session").unwrap();
        h.append(mixed, "forged".parse().unwrap());
        h.append("content-type", "application/json".parse().unwrap());
        strip_session_header(&mut h);
        assert!(h.get(SESSION_HEADER).is_none());
        assert_eq!(h.len(), 1, "other headers stay");
    }

    #[test]
    fn the_member_request_parses_any_uuid_and_refuses_garbage() {
        let id = "0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f";
        let r = parse_request(Some(&format!(
            "limit=25&after={id}&query=port%20layout&kind=contact&active_only=1&x=y"
        )))
        .unwrap();
        assert_eq!(
            r,
            ListRequest {
                after: Some(id.parse().unwrap()),
                limit: 25,
                query: Some("port layout".into()),
                kind: Some(DirectoryKind::Contact),
                active_only: true,
            }
        );
        // Any UUID is a key, ours or not, any version.
        let nil = parse_request(Some("limit=1&after=00000000-0000-0000-0000-000000000000"));
        assert_eq!(nil.unwrap().after, Some(sid(0)));
        assert_eq!(
            parse_request(Some("limit=200")).unwrap(),
            ListRequest {
                limit: 200,
                ..ListRequest::default()
            }
        );
        for bad in [
            None,
            Some(""),
            Some("after=0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f"),
            Some("limit=0"),
            Some("limit=201"),
            Some("limit=-1"),
            Some("limit=ten"),
            Some("limit=5&after=nope"),
            Some("limit=5&after="),
            Some("limit=5&kind=chat"),
            Some("limit=5&kind=Contact"),
            Some("limit=5&active_only=yes"),
        ] {
            assert_eq!(parse_request(bad), None, "{bad:?}");
        }
    }

    /// Review round 2's later-page timeout: A holds [100, 80], B [90, 70], `limit = 2`.
    #[test]
    fn the_merge_walks_a_timeout_on_a_later_page() {
        let (a, b) = ([100, 80], [90, 70]);
        let page1 = merge(
            vec![answer("A", &a, None, 2), answer("B", &b, None, 2)],
            vec![],
            2,
        );
        assert_eq!((ids(&page1), page1.cursor), (vec![100, 90], Some(sid(90))));
        // Page 2, B times out: A's 80 alone, and the cursor is null (nothing trimmed).
        let after = page1.cursor;
        let page2 = merge(vec![answer("A", &a, after, 2)], vec!["B".into()], 2);
        assert_eq!((ids(&page2), page2.cursor), (vec![80], None));
        assert_eq!(page2.partial, ["B"]);
        // Repeating page 2 with B answering retries it.
        let again = merge(
            vec![answer("A", &a, after, 2), answer("B", &b, after, 2)],
            vec![],
            2,
        );
        assert_eq!((ids(&again), again.cursor), (vec![80, 70], None));
        assert!(again.partial.is_empty());
        // An empty page with a timed-out member still names it.
        let empty = merge(vec![answer("A", &a, Some(sid(70)), 2)], vec!["B".into()], 2);
        assert_eq!((ids(&empty), empty.cursor), (vec![], None));
        assert_eq!(empty.partial, ["B"]);
    }

    /// More rows answered than remain in the budget: 5 left across two members, `limit = 3`.
    #[test]
    fn a_trimmed_page_has_a_cursor_and_the_last_page_none() {
        let (a, b) = ([5, 3, 1], [4, 2]);
        let p1 = merge(
            vec![answer("A", &a, None, 3), answer("B", &b, None, 3)],
            vec![],
            3,
        );
        assert_eq!((ids(&p1), p1.cursor), (vec![5, 4, 3], Some(sid(3))));
        let p2 = merge(
            vec![answer("A", &a, p1.cursor, 3), answer("B", &b, p1.cursor, 3)],
            vec![],
            3,
        );
        assert_eq!((ids(&p2), p2.cursor), (vec![2, 1], None));
        // A member answering exactly `limit` rows keeps the cursor, even with nothing after.
        let exact = merge(vec![answer("A", &a, None, 3)], vec![], 3);
        assert_eq!(exact.cursor, Some(sid(1)));
        let end = merge(vec![answer("A", &a, exact.cursor, 3)], vec![], 3);
        assert_eq!((ids(&end), end.cursor), (vec![], None));
    }

    /// An ownership move: two members answer one id; it is emitted once, before the limit.
    #[test]
    fn a_duplicate_id_is_emitted_once() {
        let l = merge(
            vec![
                answer("A", &[10, 9], None, 3),
                answer("B", &[9, 8], None, 3),
            ],
            vec![],
            3,
        );
        assert_eq!((ids(&l), l.cursor), (vec![10, 9, 8], None));
        assert_eq!(l.rows[1].machine, "A", "the first answer keeps it");
    }

    /// 300 members, one offline: every page names it, the cursor is one id, and the walk lists
    /// every other member's rows exactly once, id descending.
    #[test]
    fn three_hundred_members_with_one_offline() {
        const OFFLINE: u128 = 17;
        let held = |m: u128| [m, m + 300, m + 600];
        let mut seen = Vec::new();
        let mut after = None;
        let mut pages = 0;
        loop {
            let answers = (0..300u128)
                .filter(|m| *m != OFFLINE)
                .map(|m| answer(&format!("m{m}"), &held(m), after, 100))
                .collect();
            let page = merge(answers, vec![format!("m{OFFLINE}")], 100);
            assert_eq!(page.partial, [format!("m{OFFLINE}")]);
            assert!(page.rows.len() <= 100);
            seen.extend(ids(&page));
            pages += 1;
            match page.cursor {
                Some(c) => after = Some(c),
                None => break,
            }
        }
        let mut want: Vec<u128> = (0..900).filter(|n| n % 300 != OFFLINE).collect();
        want.reverse();
        assert_eq!(seen, want);
        assert_eq!(pages, 9);
    }
}
