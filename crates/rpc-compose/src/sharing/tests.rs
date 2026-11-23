use super::*;
use document::{DocId, Member, Members, ShareLink};
use jsonrpsee::types::error::INVALID_PARAMS_CODE;
use runtime::Runtime;

const DAY: i64 = 24 * 3600 * 1000;
const NOW: i64 = 1_000 * DAY;

fn link(id: &str, expires_ms: Option<i64>, revoked_ms: Option<i64>) -> ShareLink {
    ShareLink {
        id: id.into(),
        role: Role::Editor,
        secret_hash_sealed: String::new(),
        name: None,
        created_ms: NOW - 10 * DAY,
        expires_ms,
        revoked_ms,
    }
}

fn member(pub_: &str, via: Option<&str>, last_seen_ms: i64) -> Member {
    Member {
        pub_: pub_.into(),
        name: format!("m-{pub_}"),
        color: String::new(),
        role: Role::Viewer,
        added_ms: NOW - DAY,
        via: via.map(Into::into),
        last_seen_ms,
    }
}

fn contact(name: &str, exports: &[&str], answers: &[(&str, &str)], blocked: bool) -> Contact {
    serde_json::from_value(serde_json::json!({
        "name": name, "machine": "m", "endpoint": "e", "fingerprint": "f", "path": "lan",
        "exports": exports,
        "first_contact": answers.iter().map(|(x, a)| (x.to_string(), a.to_string())).collect::<HashMap<_, _>>(),
        "blocked": blocked, "paired_ms": 7, "shared": [], "full_capable": false,
    }))
    .unwrap()
}

/// Expired and revoked links get their state; a link revoked over 30 days ago is omitted; a
/// link's `count` and `last_access_ms` come from the members who joined through it.
#[test]
fn document_rows_carry_link_states_and_members_via_link() {
    let doc = DocId::new();
    let d = SharedDoc {
        doc: doc.clone(),
        title: "Plan".into(),
        sharing: Members {
            members: vec![
                member("a", Some("lnk_live"), NOW - 5),
                member("b", Some("lnk_live"), NOW - 3),
                member("c", Some("lnk_revoked"), 0),
            ],
            links: vec![
                link("lnk_live", Some(NOW + DAY), None),
                link("lnk_expired", Some(NOW - 1), None),
                link("lnk_revoked", None, Some(NOW - DAY)),
                link("lnk_old", None, Some(NOW - 31 * DAY)),
            ],
        },
    };
    let rows = doc_rows(&d, NOW, true, true);
    let by_id = |id: &str| {
        rows.iter()
            .find(|r| r.id == format!("{doc}|{id}"))
            .unwrap_or_else(|| panic!("{id} in {rows:#?}"))
    };
    let live = by_id("lnk_live");
    assert_eq!(
        (live.state.as_str(), live.count, live.last_access_ms),
        ("active", Some(2), Some(NOW - 3))
    );
    assert_eq!(live.audience, ANYONE);
    assert_eq!(live.url, None);
    assert_eq!(live.parent.as_deref(), Some(doc.as_str()));
    let expired = by_id("lnk_expired");
    assert_eq!(
        (expired.state.as_str(), expired.revocable),
        ("expired", true)
    );
    let revoked = by_id("lnk_revoked");
    assert_eq!(
        (revoked.state.as_str(), revoked.count),
        ("revoked", Some(1))
    );
    assert!(revoked.revocable, "its member can still be swept");
    assert!(
        !rows.iter().any(|r| r.id.ends_with("lnk_old")),
        "revoked > 30 days: omitted"
    );
    let m = by_id("c");
    assert_eq!(
        (m.kind, m.permission.as_deref(), m.last_access_ms),
        (SharingKind::DocumentMember, Some("viewer"), None)
    );

    let only_links = doc_rows(&d, NOW, false, true);
    assert!(
        only_links
            .iter()
            .all(|r| r.kind == SharingKind::DocumentLink)
    );
    assert_eq!(only_links.len(), 3);
}

/// A revoked link nobody joined through has nothing left to revoke.
#[test]
fn revoked_link_without_members_is_not_revocable() {
    let d = SharedDoc {
        doc: DocId::new(),
        title: "t".into(),
        sharing: Members {
            members: vec![],
            links: vec![link("lnk_x", None, Some(NOW - DAY))],
        },
    };
    assert!(!doc_rows(&d, NOW, true, true)[0].revocable);
}

#[test]
fn export_and_contact_rows_join_grants() {
    let exports: Vec<Export> = serde_json::from_value(serde_json::json!([
        {"name": "helper", "target": {"kind": "agent", "agent": "theuth"}, "description": ""},
        {"name": "chat", "target": {"kind": "chat", "session": runtime::SessionId::new()}, "description": "", "paused": "chat archived"},
    ]))
    .unwrap();
    let contacts = vec![
        contact("ann", &["helper"], &[("helper", "allow")], false),
        contact("bob", &["helper", "chat"], &[], true),
    ];
    let rows = export_rows(&exports, &contacts);
    assert_eq!(rows[0].audience, "ann, bob");
    assert_eq!(
        (rows[0].count, rows[0].permission.as_deref()),
        (Some(2), Some("mixed"))
    );
    assert_eq!(rows[0].open.as_deref(), Some("theuth"));
    assert_eq!(
        (
            rows[1].state.as_str(),
            rows[1].count,
            rows[1].permission.as_deref()
        ),
        ("paused", Some(1), Some("ask"))
    );
    let rows = contact_rows(&contacts);
    assert_eq!(
        (
            rows[0].state.as_str(),
            rows[0].count,
            rows[0].permission.as_deref()
        ),
        ("active", Some(1), Some("allow"))
    );
    assert_eq!(
        (rows[1].state.as_str(), rows[1].count),
        ("blocked", Some(2))
    );
}

#[test]
fn share_rows_name_their_machine_and_stop_only_when_running() {
    let share: Share = serde_json::from_value(serde_json::json!({
        "id": "s1", "engine": "eid", "edge": "edge", "domain": "tun.zip", "host": "x.tun.zip",
        "path": "/app1", "url": "https://x.tun.zip/app1/", "source": {"port": 8080},
        "access": "password", "interstitial": true, "created_ms": 5, "live_ms": 6,
        "expires_ms": 9, "created_by": "settings",
        "approved_by": {"approval_id": "a", "surface": "settings"}, "state": "live",
        "stopped_ms": null, "stopped_by": null,
    }))
    .unwrap();
    let machines = HashMap::from([("eid".to_owned(), "laptop".to_owned())]);
    let r = share_row(&share, &machines);
    assert_eq!(
        (r.title.as_str(), r.audience.as_str(), r.state.as_str()),
        ("x.tun.zip/app1", "password", "live")
    );
    assert_eq!(
        (r.engine.as_deref(), r.url.as_deref(), r.expires_ms),
        (Some("laptop"), Some("https://x.tun.zip/app1/"), Some(9))
    );
    assert!(r.revocable);
    let stopped = Share {
        state: ShareState::Stopped,
        ..share
    };
    assert!(!share_row(&stopped, &HashMap::new()).revocable);
}

/// 213 §3 dispatch table, and the refusals of malformed ids and subs.
#[test]
fn revoke_dispatch_table() {
    let doc = DocId::new();
    let d = doc.as_str();
    assert_eq!(
        plan(SharingKind::DocumentMember, &format!("{d}|pubkey"), None).unwrap(),
        Revoke::DocMember {
            doc: doc.clone(),
            member: "pubkey".into()
        }
    );
    assert_eq!(
        plan(SharingKind::DocumentLink, &format!("{d}|lnk_1"), None).unwrap(),
        Revoke::DocLink {
            doc: doc.clone(),
            link: "lnk_1".into(),
            members: false
        }
    );
    assert_eq!(
        plan(
            SharingKind::DocumentLink,
            &format!("{d}|lnk_1"),
            Some("members")
        )
        .unwrap(),
        Revoke::DocLink {
            doc: doc.clone(),
            link: "lnk_1".into(),
            members: true
        }
    );
    assert_eq!(
        plan(SharingKind::PublicShare, "s1", None).unwrap(),
        Revoke::ShareStop("s1".into())
    );
    assert_eq!(
        plan(SharingKind::Export, "helper", None).unwrap(),
        Revoke::ExportRemove("helper".into())
    );
    assert_eq!(
        plan(SharingKind::Export, "helper", Some("ann")).unwrap(),
        Revoke::Ungrant {
            contact: "ann".into(),
            export: "helper".into()
        }
    );
    assert_eq!(
        plan(SharingKind::Contact, "ann", None).unwrap(),
        Revoke::ContactRemove("ann".into())
    );

    for (kind, id, sub) in [
        (SharingKind::DocumentMember, "no-pipe".to_owned(), None),
        (SharingKind::DocumentMember, "notadoc|x".to_owned(), None),
        (SharingKind::DocumentMember, format!("{d}|"), None),
        (
            SharingKind::DocumentMember,
            format!("{d}|x"),
            Some("members"),
        ),
        (SharingKind::DocumentLink, format!("{d}|lnk_1"), Some("all")),
        (SharingKind::PublicShare, "all".to_owned(), None),
        (SharingKind::PublicShare, "s1".to_owned(), Some("x")),
        (SharingKind::Contact, "ann".to_owned(), Some("x")),
        (SharingKind::Contact, String::new(), None),
    ] {
        let e = plan(kind, &id, sub).unwrap_err();
        assert_eq!(e.code(), INVALID_PARAMS_CODE, "{kind:?} {id} {sub:?}");
    }
}

/// Plugins not running here: every kind is `partial` with its reason, the call succeeds; a2a off
/// answers the contacts `OFF` text. A guest connection is refused outright.
#[tokio::test]
async fn list_reports_partial_for_plugins_not_running_and_refuses_guests() {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    let server = crate::new_server(rt);
    let mut ext = Extensions::new();
    ext.insert(rpc_core::RpcPeer::internal());
    let l = list(&server, &ext, None).await.unwrap();
    assert!(l.rows.is_empty());
    let kinds: Vec<_> = l.partial.iter().map(|p| p.kind).collect();
    assert_eq!(kinds, ALL.to_vec());
    let a2a = l
        .partial
        .iter()
        .find(|p| p.kind == SharingKind::Export)
        .unwrap();
    assert_eq!(a2a.reason, a2a::contacts::OFF);

    let only = list(&server, &ext, Some(vec![SharingKind::Contact]))
        .await
        .unwrap();
    assert_eq!(
        only.partial.iter().map(|p| p.kind).collect::<Vec<_>>(),
        [SharingKind::Contact]
    );

    let (guest, _g) = rpc_core::RpcPeer::open_guest([1; 32], [2; 32]);
    let mut gext = Extensions::new();
    gext.insert(guest);
    assert_eq!(
        list(&server, &gext, None).await.unwrap_err().code(),
        rpc_core::UNAUTHORIZED
    );
    assert_eq!(
        revoke(&server, &gext, SharingKind::Contact, "ann".into(), None)
            .await
            .unwrap_err()
            .code(),
        rpc_core::UNAUTHORIZED
    );
}
