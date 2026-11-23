//! 213 §3: the Settings › Sharing hub over every kind of thing this owner shares: document members
//! and links, public shares, A2A exports and contacts. One read aggregate (`sharing_list`) and one
//! revoke dispatcher (`sharing_revoke`) that calls the per-kind handler bodies (not the wire), so
//! forwarding and consent stay as they are; revoke is never consent-gated.

use std::collections::HashMap;

use a2a::config::FirstContact;
use a2a::contacts::{Contact, Export, ExportTarget};
use a2a::rpc_messaging::A2aMessagingApiServer as _;
use agent::rpc_documents::AgentDocumentsApiServer as _;
use document::rpc_support::{doc_send, documents};
use document::{Role, SharedDoc, SharedDocs};
use jsonrpsee::Extensions;
use jsonrpsee::core::RpcResult;
use jsonrpsee::types::ErrorObjectOwned;
use jsonrpsee::types::error::INVALID_PARAMS_CODE;
use link::rpc_hub::LinkHubApiServer as _;
use rpc_core::{Server, owner, send};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use share::Share;
use share::model::ShareState;
use share::rpc_share::ShareApiServer as _;

/// What a [`SharingRow`] is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SharingKind {
    /// Someone who joined a document (through a link). Id `<doc>|<member pub>`.
    DocumentMember,
    /// A document's share link. Id `<doc>|<link id>`.
    DocumentLink,
    /// A public share on the edge (210). Id: the share id.
    PublicShare,
    /// A chat or agent exported to contacts (183). Id: the export name.
    Export,
    /// A contact (183). Id: the contact name.
    Contact,
}

const ALL: [SharingKind; 5] = [
    SharingKind::DocumentMember,
    SharingKind::DocumentLink,
    SharingKind::PublicShare,
    SharingKind::Export,
    SharingKind::Contact,
];

/// One shared thing as the hub lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharingRow {
    /// Kind.
    pub kind: SharingKind,
    /// Id, per kind: `<doc>|<member pub>`, `<doc>|<link id>`, share id, export name, contact name.
    pub id: String,
    /// Doc title | host+path | export name | contact name.
    pub title: String,
    /// Human text: member name | "anyone with the link" | "public" | "password" | contact names.
    pub audience: String,
    /// Doc role | share access | export first-contact policy | contact first-contact.
    pub permission: Option<String>,
    /// `live|pending|offline|stopped` (share), `active|expired|revoked` (doc), `paused` (export),
    /// `blocked` (contact).
    pub state: String,
    /// Created (or joined / paired), Unix ms.
    pub created_ms: Option<i64>,
    /// Expiry, Unix ms.
    pub expires_ms: Option<i64>,
    /// Best effort; `None` when unknown.
    pub last_access_ms: Option<i64>,
    /// Members joined via a link | visitors | contacts granted an export | exports a contact may reach.
    pub count: Option<u64>,
    /// Public share URL; `None` for doc links (the secret is shown only at creation).
    pub url: Option<String>,
    /// Hosting machine name when known.
    pub engine: Option<String>,
    /// Doc id for `document_*` rows.
    pub parent: Option<String>,
    /// `sharing_revoke` has something to do for it.
    pub revocable: bool,
    /// What the UI opens: doc id | session id | agent name | contact name.
    pub open: Option<String>,
}

/// A kind the list could not read: its plugin is off or not running, or it failed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharingPartial {
    /// Kind.
    pub kind: SharingKind,
    /// Plugin off / not running / error text.
    pub reason: String,
}

/// `sharing_list` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharingList {
    /// Rows, grouped by kind (documents, public shares, exports, contacts).
    pub rows: Vec<SharingRow>,
    /// Kinds asked for that could not be read.
    pub partial: Vec<SharingPartial>,
}

/// `sharing_revoke` reply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SharingRevoked {
    /// Kind.
    pub kind: SharingKind,
    /// Id.
    pub id: String,
    /// The edge has not confirmed yet (`share_stop`'s `edge_pending`).
    pub pending: bool,
}

/// Revoked links stay listed this long, then are omitted (213 §3).
const REVOKED_KEEP_MS: i64 = 30 * 24 * 3600 * 1000;

const ANYONE: &str = "anyone with the link";

fn role(r: Role) -> &'static str {
    match r {
        Role::Editor => "editor",
        Role::Viewer => "viewer",
    }
}

fn first_contact(f: FirstContact) -> &'static str {
    match f {
        FirstContact::Ask => "ask",
        FirstContact::Allow => "allow",
        FirstContact::Block => "block",
    }
}

/// The one answer every entry shares, `mixed` otherwise, `None` for none.
fn uniform(mut it: impl Iterator<Item = FirstContact>) -> Option<String> {
    let first = it.next()?;
    Some(if it.all(|f| f == first) {
        first_contact(first).to_owned()
    } else {
        "mixed".to_owned()
    })
}

fn nonzero(ms: i64) -> Option<i64> {
    (ms > 0).then_some(ms)
}

/// A document's member and link rows (as `kinds` selects); revoked links older than 30 days omitted.
pub(crate) fn doc_rows(d: &SharedDoc, now: i64, members: bool, links: bool) -> Vec<SharingRow> {
    let doc = d.doc.as_str();
    let mut out = Vec::new();
    if links {
        for l in &d.sharing.links {
            let state = match (l.revoked_ms, l.expires_ms) {
                (Some(r), _) if now - r > REVOKED_KEEP_MS => continue,
                (Some(_), _) => "revoked",
                (None, Some(e)) if e <= now => "expired",
                _ => "active",
            };
            let via = || {
                d.sharing
                    .members
                    .iter()
                    .filter(|m| m.via.as_deref() == Some(l.id.as_str()))
            };
            let joined = via().count() as u64;
            out.push(SharingRow {
                kind: SharingKind::DocumentLink,
                id: format!("{doc}|{}", l.id),
                title: d.title.clone(),
                audience: l.name.clone().unwrap_or_else(|| ANYONE.to_owned()),
                permission: Some(role(l.role).to_owned()),
                state: state.to_owned(),
                created_ms: Some(l.created_ms),
                expires_ms: l.expires_ms,
                last_access_ms: via().filter_map(|m| nonzero(m.last_seen_ms)).max(),
                count: Some(joined),
                url: None,
                engine: None,
                parent: Some(doc.to_owned()),
                // A revoked link still sweeps the members who came through it.
                revocable: l.revoked_ms.is_none() || joined > 0,
                open: Some(doc.to_owned()),
            });
        }
    }
    if members {
        for m in &d.sharing.members {
            out.push(SharingRow {
                kind: SharingKind::DocumentMember,
                id: format!("{doc}|{}", m.pub_),
                title: d.title.clone(),
                audience: m.name.clone(),
                permission: Some(role(m.role).to_owned()),
                state: "active".to_owned(),
                created_ms: Some(m.added_ms),
                expires_ms: None,
                last_access_ms: nonzero(m.last_seen_ms),
                count: None,
                url: None,
                engine: None,
                parent: Some(doc.to_owned()),
                revocable: true,
                open: Some(doc.to_owned()),
            });
        }
    }
    out
}

/// A public share's row; `machines` maps engine ids to machine names.
pub(crate) fn share_row(s: &Share, machines: &HashMap<String, String>) -> SharingRow {
    let title = if s.path == "/" {
        s.host.clone()
    } else {
        format!("{}{}", s.host, s.path)
    };
    SharingRow {
        kind: SharingKind::PublicShare,
        id: s.id.clone(),
        title,
        audience: s.access.as_str().to_owned(),
        permission: Some(s.access.as_str().to_owned()),
        state: s.state.as_str().to_owned(),
        created_ms: Some(s.created_ms as i64),
        expires_ms: s.expires_ms.map(|e| e as i64),
        // `share.traffic` has no producer yet (210 M3): no visitors, no last access.
        last_access_ms: None,
        count: None,
        url: Some(s.url.clone()),
        engine: machines.get(&s.engine).cloned(),
        parent: None,
        revocable: s.state != ShareState::Stopped,
        open: None,
    }
}

/// Export rows: `count` = contacts granted, `audience` their names.
pub(crate) fn export_rows(exports: &[Export], contacts: &[Contact]) -> Vec<SharingRow> {
    exports
        .iter()
        .map(|e| {
            let granted: Vec<&Contact> = contacts
                .iter()
                .filter(|c| c.exports.contains(&e.name))
                .collect();
            SharingRow {
                kind: SharingKind::Export,
                id: e.name.clone(),
                title: e.name.clone(),
                audience: granted
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
                permission: uniform(
                    granted
                        .iter()
                        .map(|c| c.first_contact.get(&e.name).copied().unwrap_or_default()),
                ),
                state: if e.paused.is_some() {
                    "paused"
                } else {
                    "active"
                }
                .to_owned(),
                created_ms: None,
                expires_ms: None,
                last_access_ms: None,
                count: Some(granted.len() as u64),
                url: None,
                engine: None,
                parent: None,
                revocable: true,
                open: Some(match &e.target {
                    ExportTarget::Agent { agent } => agent.clone(),
                    ExportTarget::Chat { session } => session.to_string(),
                }),
            }
        })
        .collect()
}

/// Contact rows: `count` = exports it may reach, `state` blocked/active.
pub(crate) fn contact_rows(contacts: &[Contact]) -> Vec<SharingRow> {
    contacts
        .iter()
        .map(|c| SharingRow {
            kind: SharingKind::Contact,
            id: c.name.clone(),
            title: c.name.clone(),
            audience: c.name.clone(),
            permission: uniform(
                c.exports
                    .iter()
                    .map(|x| c.first_contact.get(x).copied().unwrap_or_default()),
            ),
            state: if c.blocked { "blocked" } else { "active" }.to_owned(),
            created_ms: nonzero(c.paired_ms as i64),
            expires_ms: None,
            last_access_ms: None,
            count: Some(c.exports.len() as u64),
            url: None,
            engine: None,
            parent: None,
            revocable: true,
            open: Some(c.name.clone()),
        })
        .collect()
}

/// What `sharing_revoke` does for one row (213 §3 dispatch table).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Revoke {
    /// `doc_member_revoke(doc, member)`.
    DocMember {
        doc: document::DocId,
        member: String,
    },
    /// `doc_link_revoke(doc, link)`, then with `members` `doc_member_revoke` for each member via it.
    DocLink {
        doc: document::DocId,
        link: String,
        members: bool,
    },
    /// `share_stop(id)`.
    ShareStop(String),
    /// `a2a_export_remove(name)`.
    ExportRemove(String),
    /// `contact_update(contact, revoke: [export])`.
    Ungrant { contact: String, export: String },
    /// `contact_remove(name, block = false)`.
    ContactRemove(String),
}

fn bad(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(INVALID_PARAMS_CODE, msg.into(), None::<()>)
}

fn doc_pair(id: &str) -> RpcResult<(document::DocId, String)> {
    let (doc, rest) = id
        .split_once('|')
        .filter(|(_, r)| !r.is_empty())
        .ok_or_else(|| bad(format!("expected `<doc>|<id>`, got `{id}`")))?;
    let doc =
        document::DocId::parse(doc).ok_or_else(|| bad(format!("not a document id: `{doc}`")))?;
    Ok((doc, rest.to_owned()))
}

/// Parse one revoke request into its action; a malformed id or an unknown `sub` is `INVALID_PARAMS`.
pub(crate) fn plan(kind: SharingKind, id: &str, sub: Option<&str>) -> RpcResult<Revoke> {
    if id.is_empty() {
        return Err(bad("empty id"));
    }
    let no_sub = |r: Revoke| match sub {
        None => Ok(r),
        Some(s) => Err(bad(format!("{kind:?} takes no sub, got `{s}`"))),
    };
    match kind {
        SharingKind::DocumentMember => {
            let (doc, member) = doc_pair(id)?;
            no_sub(Revoke::DocMember { doc, member })
        }
        SharingKind::DocumentLink => {
            let (doc, link) = doc_pair(id)?;
            let members = match sub {
                None => false,
                Some("members") => true,
                Some(s) => return Err(bad(format!("document_link sub is `members`, got `{s}`"))),
            };
            Ok(Revoke::DocLink { doc, link, members })
        }
        SharingKind::PublicShare if id == "all" => {
            Err(bad("one share id; Stop all is `share_stop`"))
        }
        SharingKind::PublicShare => no_sub(Revoke::ShareStop(id.to_owned())),
        SharingKind::Export => Ok(match sub {
            None => Revoke::ExportRemove(id.to_owned()),
            Some(contact) => Revoke::Ungrant {
                contact: contact.to_owned(),
                export: id.to_owned(),
            },
        }),
        SharingKind::Contact => no_sub(Revoke::ContactRemove(id.to_owned())),
    }
}

/// `sharing_revoke`: owner only, never gated; each branch is the existing handler body.
pub(crate) async fn revoke(
    server: &Server,
    ext: &Extensions,
    kind: SharingKind,
    id: String,
    sub: Option<String>,
) -> RpcResult<SharingRevoked> {
    owner(ext)?;
    let mut pending = false;
    match plan(kind, &id, sub.as_deref())? {
        Revoke::DocMember { doc, member } => server.doc_member_revoke(ext, doc, member).await?,
        Revoke::DocLink { doc, link, members } => {
            server
                .doc_link_revoke(ext, doc.clone(), link.clone())
                .await?;
            if members {
                let joined = server.doc_members(ext, doc.clone()).await?.members;
                for m in joined
                    .into_iter()
                    .filter(|m| m.via.as_deref() == Some(link.as_str()))
                {
                    server.doc_member_revoke(ext, doc.clone(), m.pub_).await?;
                }
            }
        }
        Revoke::ShareStop(id) => pending = server.share_stop(ext, id).await?.edge_pending.is_some(),
        Revoke::ExportRemove(name) => server.a2a_export_remove(name).await?,
        Revoke::Ungrant { contact, export } => {
            let patch = a2a::contacts::ContactPatch {
                revoke: Some(vec![export]),
                ..Default::default()
            };
            server.contact_update(contact, patch).await?;
        }
        Revoke::ContactRemove(name) => server.contact_remove(name, false).await?,
    }
    Ok(SharingRevoked { kind, id, pending })
}

/// `sharing_list`: owner only; a kind whose plugin is off or not running is `partial`.
pub(crate) async fn list(
    server: &Server,
    ext: &Extensions,
    kinds: Option<Vec<SharingKind>>,
) -> RpcResult<SharingList> {
    owner(ext)?;
    let kinds = kinds.unwrap_or_else(|| ALL.to_vec());
    let want = |k| kinds.contains(&k);
    let mut out = SharingList::default();
    let partial = |out: &mut SharingList, ks: &[SharingKind], reason: String| {
        for &kind in ks.iter().filter(|k| want(**k)) {
            out.partial.push(SharingPartial {
                kind,
                reason: reason.clone(),
            });
        }
    };

    let (members, links) = (
        want(SharingKind::DocumentMember),
        want(SharingKind::DocumentLink),
    );
    if members || links {
        let docs = match documents() {
            Ok(d) => doc_send(&d, SharedDocs).await,
            Err(e) => Err(e),
        };
        match docs {
            Ok(docs) => {
                let now = platform::time::now_ms_i64();
                out.rows
                    .extend(docs.iter().flat_map(|d| doc_rows(d, now, members, links)));
            }
            Err(e) => partial(
                &mut out,
                &[SharingKind::DocumentMember, SharingKind::DocumentLink],
                e.message().to_owned(),
            ),
        }
    }

    if want(SharingKind::PublicShare) {
        match server.share_list().await {
            Ok(shares) => {
                let machines = machine_names().await;
                out.rows
                    .extend(shares.iter().map(|s| share_row(s, &machines)));
            }
            Err(e) => partial(
                &mut out,
                &[SharingKind::PublicShare],
                e.message().to_owned(),
            ),
        }
    }

    let (exports, contacts) = (want(SharingKind::Export), want(SharingKind::Contact));
    if exports || contacts {
        let a2a_kinds = [SharingKind::Export, SharingKind::Contact];
        match a2a::rpc_support::a2a(&server.rt) {
            Ok(Some(actor)) => {
                let listed = match send(&actor, a2a::contacts::ContactsList).await {
                    Ok(cs) if exports => send(&actor, a2a::contacts::ExportsList)
                        .await
                        .map(|xs| (cs, xs)),
                    Ok(cs) => Ok((cs, Vec::new())),
                    Err(e) => Err(e),
                };
                match listed {
                    Ok((cs, xs)) => {
                        if exports {
                            out.rows.extend(export_rows(&xs, &cs));
                        }
                        if contacts {
                            out.rows.extend(contact_rows(&cs));
                        }
                    }
                    Err(e) => partial(&mut out, &a2a_kinds, e.message().to_owned()),
                }
            }
            Ok(None) => partial(&mut out, &a2a_kinds, a2a::contacts::OFF.to_owned()),
            Err(e) => partial(&mut out, &a2a_kinds, e.message().to_owned()),
        }
    }
    Ok(out)
}

/// Engine id → machine name from the mesh's status; empty when the mesh cannot answer.
async fn machine_names() -> HashMap<String, String> {
    let Ok(mesh) = mesh::rpc_support::mesh_actor() else {
        return HashMap::new();
    };
    let Ok(s) = mesh.ask(mesh::Status).await else {
        return HashMap::new();
    };
    std::iter::once((s.self_id.0, s.self_name))
        .chain(s.peers.into_iter().map(|p| (p.id.0, p.name)))
        .filter(|(_, n)| !n.is_empty())
        .collect()
}

#[cfg(test)]
#[path = "sharing/tests.rs"]
mod tests;
