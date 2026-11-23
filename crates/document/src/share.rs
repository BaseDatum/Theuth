//! Sharing (design 59 §8): share links, members, the join proof, role
//! enforcement and revocation. Handlers on the [`DocumentActor`]; the wire
//! types live in [`crate::meta`].
//!
//! Proof (§8.3, as built): `HMAC-SHA256(key = sha256(secret), handshake_hash ‖
//! guest_pub)`. The harness stores only `sha256(secret)`, sealed with the mesh data key
//! (`ShareLink::secret_hash_sealed`, SEAL) and opened here at proof time; so that hash — not the
//! secret — is the HMAC key; the browser hashes the fragment secret before signing. The hash
//! never sits in the clear on disk, and only engines holding the mesh key (every hub) can check it.

use std::fmt;

use base64::Engine as _;
use hmac::{Hmac, KeyInit as _, Mac};
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use rand::Rng as _;
use sha2::Sha256;

use crate::actor::DocumentActor;
use crate::hub::{DocError, Route};
use crate::meta::{
    DocEvent, DocId, DocKind, Joined, Member, Members, PALETTE, Principal, Role, SOURCE_READONLY,
    ShareLink,
};
use crate::notebook::ItemKind;
use crate::relay::{Client, Cmd};
use crate::store::now_ms;
use secrets::Vault;

type HmacSha256 = Hmac<Sha256>;

pub(crate) fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) fn unb64url(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(s)
        .ok()
}

/// `ShareLink::secret_hash_sealed` for `hash`: sealed with the mesh key, the link id as aad so a
/// ciphertext cannot be moved between links.
pub(crate) fn seal_hash(vault: &Vault, link_id: &str, hash: &[u8]) -> String {
    b64url(&vault.seal_with_mesh_key(link_id.as_bytes(), hash))
}

/// 183 §6.9.11: `ask` → `bool` after a vault-key rotation: under `vault.retire_hold("document")`,
/// re-seal every share link that opens only under a retained mesh key, through the normal meta
/// write. `false` = not the hub here (the hub owns the writes; a link whose re-seal never lands
/// stops opening at retire, and the UI offers Re-share).
#[derive(Clone, Copy, Debug)]
pub struct Reseal;

impl Message<Reseal> for DocumentActor {
    type Reply = bool;

    async fn handle(&mut self, _: Reseal, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.reseal().await
    }
}

impl DocumentActor {
    /// [`Reseal`]'s body; also run when this engine becomes the hub (183 R1), so links sealed
    /// under a retained key survive a hub move after a rotation. 184 §7.4: the holder re-seals
    /// every document, as before; every other engine re-seals the ones it serves or homes, and
    /// `merge_meta` keeps whichever seal opens. `true` = a pass ran (every engine with a vault).
    pub(crate) async fn reseal(&mut self) -> bool {
        let Some(vault) = self.store.vault() else {
            return self.hub.holding();
        };
        let _hold = vault.retire_hold("document");
        let docs = match self.store.list(None, u32::MAX).await {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!("documents: share-link re-seal: {e:#}");
                return false;
            }
        };
        let all = self.hub.holding();
        for d in docs {
            if !all
                && !self.serves(&d.id)
                && !self.home_of(&d.id).is_some_and(|h| self.hub.is_me(&h))
            {
                continue;
            }
            if let Some(o) = self.open.get_mut(&d.id) {
                if reseal_links(&vault, &mut o.meta.links) {
                    self.save_members(&d.id).await;
                }
                continue;
            }
            let Ok(mut meta) = self.store.read_meta(&d.id) else {
                continue;
            };
            if reseal_links(&vault, &mut meta.links)
                && let Err(e) = self.store.write_meta(&meta).await
            {
                tracing::warn!(doc = %d.id, error = %e, "meta.json: re-sealed links not saved");
            }
        }
        true
    }
}

/// Re-seal each link sealed under a retained key; `true` when one changed.
pub(crate) fn reseal_links(vault: &Vault, links: &mut [ShareLink]) -> bool {
    let mut changed = false;
    for l in links {
        if let Some(bytes) = unb64url(&l.secret_hash_sealed)
            && let Some(new) = vault.reseal_with_mesh_key(l.id.as_bytes(), &bytes)
        {
            l.secret_hash_sealed = b64url(&new);
            changed = true;
        }
    }
    changed
}

/// The HMAC key a link holds; `None` when the field is empty, damaged, or sealed under another
/// mesh key.
fn open_hash(vault: &Vault, link: &ShareLink) -> Option<Vec<u8>> {
    let bytes = unb64url(&link.secret_hash_sealed)?;
    vault
        .open_with_mesh_key(link.id.as_bytes(), &bytes)
        .map(|k| k.to_vec())
}

/// 184 §7.4: `merge_meta`'s `opens` — the link's seal opens under this engine's mesh key.
pub(crate) fn seal_opens(vault: Option<&Vault>, link: &ShareLink) -> bool {
    vault.is_some_and(|v| open_hash(v, link).is_some())
}

/// `sha256(secret)`, the value a link stores and the proof's key.
pub fn secret_hash(secret: &[u8]) -> [u8; 32] {
    use sha2::Digest;
    Sha256::digest(secret).into()
}

/// The join proof: `HMAC-SHA256(secret_hash, handshake_hash ‖ guest_pub)` (§8.3 as built).
pub fn proof(secret_hash: &[u8; 32], handshake_hash: &[u8], guest_pub: &[u8; 32]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(secret_hash).expect("any key length");
    mac.update(handshake_hash);
    mac.update(guest_pub);
    mac.finalize().into_bytes().into()
}

/// Constant-time check of `proof` against the link's sealed hash; no vault = no key = refused.
pub(crate) fn verify(
    vault: Option<&Vault>,
    link: &ShareLink,
    handshake_hash: &[u8],
    guest_pub: &[u8; 32],
    proof: &[u8],
) -> bool {
    let Some(key) = vault.and_then(|v| open_hash(v, link)) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(&key) else {
        return false;
    };
    mac.update(handshake_hash);
    mac.update(guest_pub);
    mac.verify_slice(proof).is_ok()
}

/// Why a join was refused (§8.3). `Display` is the wire code the guest page keys its copy on;
/// `NotHub` displays as [`DocError::NotHub`] does (`NotHub:<engine id>`), so the RPC layer maps it too.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinError {
    /// This engine does not hold the hub lease (69); the holder, when known.
    NotHub(Option<mesh::EngineId>),
    /// The document has no links at all.
    LinkUnknown,
    /// `doc_link_revoke` happened.
    LinkRevoked,
    /// Past `expires_ms`.
    LinkExpired,
    /// The proof does not verify.
    BadProof,
    /// `max_members` reached.
    Full,
    /// `documents.sharing` is not `on`.
    SharingOff,
    /// Z184-R3: this key was revoked after the link it presents was made; a newer link admits it.
    MemberRevoked,
    /// Store or load failure.
    Other(String),
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotHub(hub) => write!(f, "{}", DocError::NotHub { hub: hub.clone() }),
            Self::LinkUnknown => f.write_str("link_unknown"),
            Self::LinkRevoked => f.write_str("link_revoked"),
            Self::LinkExpired => f.write_str("link_expired"),
            Self::BadProof => f.write_str("bad_proof"),
            Self::Full => f.write_str("members_full"),
            Self::SharingOff => f.write_str("sharing_off"),
            Self::MemberRevoked => f.write_str("member_revoked"),
            Self::Other(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for JoinError {}

/// 184 §4.3 step 4: the `JoinError::Other` a parked `Join` answers after `JOIN_PARK_MS` while a
/// hand-back runs; the guest page retries it, and it never counts as a refusal.
pub const BUSY: &str = "busy";

impl JoinError {
    /// 184: `Other("busy")`, which the caller retries ([`BUSY`]).
    pub fn retryable(&self) -> bool {
        matches!(self, Self::Other(s) if s == BUSY)
    }
}

impl DocumentActor {
    /// §8.1, checked on every member call: a `Principal::Member` must be in `doc`'s members
    /// (`not_member`), be an editor when `write` (`viewer_readonly`), and sharing must not be
    /// `disabled` (`sharing_disabled`). Owners, the agent and the file always pass.
    ///
    /// 144 §11: a member of a live notebook holding `doc` as a `note` or `source` item is admitted at
    /// the notebook's role (the best one when several notebooks hold it) — the derived grant.
    pub(crate) async fn admit(
        &mut self,
        doc: &DocId,
        by: &Principal,
        write: bool,
    ) -> Result<(), String> {
        let Some(pub_) = by.member() else {
            return Ok(());
        };
        if self.cfg.sharing == "disabled" {
            return Err("sharing_disabled".to_owned());
        }
        let o = self.load(doc)?;
        // Z184-R2: a member no later than its key's revoke tombstone is not one.
        let live = |m: &&crate::meta::Member| {
            m.pub_ == pub_ && o.meta.revoked.get(pub_).is_none_or(|at| m.added_ms > *at)
        };
        let role = match o.meta.members.iter().find(live) {
            Some(m) => m.role,
            None => {
                let kind = o.meta.kind;
                self.derived_role(doc, kind, pub_)
                    .await
                    .ok_or_else(|| "not_member".to_owned())?
            }
        };
        if write && role == Role::Viewer {
            return Err("viewer_readonly".to_owned());
        }
        Ok(())
    }

    /// 144 §11: the role `pub_` holds on `doc` through a notebook: `Editor` when any live notebook
    /// holding it has them as an editor, `Viewer` when only as a viewer, `None` otherwise. Sessions
    /// and artifacts never grant (D13); a notebook's own members are direct, never derived.
    pub(crate) async fn derived_role(
        &mut self,
        doc: &DocId,
        kind: DocKind,
        pub_: &str,
    ) -> Option<Role> {
        let item_kind = match kind {
            DocKind::Note | DocKind::Board | DocKind::MeetingNote => ItemKind::Note,
            DocKind::Source => ItemKind::Source,
            _ => return None,
        };
        let notebooks = self
            .store
            .notebooks_of(item_kind, doc.as_str(), true)
            .await
            .ok()?;
        let mut best = None;
        for nb in notebooks {
            let members = match self.open.get(&nb) {
                Some(o) => o.meta.members.clone(),
                None => self.store.read_meta(&nb).ok()?.members,
            };
            match members.iter().find(|m| m.pub_ == pub_).map(|m| m.role) {
                Some(Role::Editor) => return Some(Role::Editor),
                Some(Role::Viewer) => best = Some(Role::Viewer),
                None => {}
            }
        }
        best
    }

    /// 144 §3.1 `source_readonly`: a `DocKind::Source` document is written by `Principal::Helper`
    /// (ingest, the insight job) and `File` only; owners, members and the agent are refused.
    pub(crate) fn source_writable(&mut self, doc: &DocId, by: &Principal) -> Result<(), String> {
        let o = self.load(doc)?;
        if o.meta.kind == DocKind::Source && !matches!(by, Principal::Helper | Principal::File) {
            return Err(SOURCE_READONLY.to_owned());
        }
        Ok(())
    }

    /// 144 §11: after a notebook stops granting (item removed, member revoked or demoted, notebook
    /// deleted), end every subscription to `doc` by a member who has no direct membership and no
    /// remaining derived grant with `Closed{reason}`.
    pub(crate) async fn revoke_derived(&mut self, doc: &DocId, reason: &str) {
        let Some(subs) = self.subs.get(doc) else {
            return;
        };
        let mut pubs: Vec<String> = subs.iter().filter_map(|s| s.member.clone()).collect();
        pubs.sort_unstable();
        pubs.dedup();
        if pubs.is_empty() {
            return;
        }
        let Ok(o) = self.load(doc) else { return };
        let kind = o.meta.kind;
        let direct: Vec<String> = o.meta.members.iter().map(|m| m.pub_.clone()).collect();
        for pub_ in pubs {
            if direct.contains(&pub_) {
                continue;
            }
            if self.derived_role(doc, kind, &pub_).await.is_none() {
                self.close_member(doc, &pub_, reason);
            }
        }
    }

    /// [`Self::revoke_derived`] over every live note and source item of `notebook` — what a member
    /// revoke or demotion on the notebook, or its deletion, costs the member's open tabs.
    pub(crate) async fn revoke_derived_of(&mut self, notebook: &DocId, reason: &str) {
        let Ok(items) = self.store.items(notebook).await else {
            return;
        };
        for item in items {
            if matches!(item.kind, ItemKind::Note | ItemKind::Source)
                && let Some(doc) = DocId::parse(&item.item)
            {
                self.revoke_derived(&doc, reason).await;
            }
        }
    }

    /// The owner's view (§6.1 `doc_members`): links without their hash.
    pub(crate) fn members_of(&self, doc: &DocId) -> Result<Members, String> {
        let o = self
            .open
            .get(doc)
            .ok_or_else(|| format!("{doc} is not loaded"))?;
        Ok(Members {
            members: o.meta.members.clone(),
            links: o.meta.links.iter().map(ShareLink::public).collect(),
        })
    }

    /// §11: block id → `"<name> · doc-member:<pub8>"` for every block a member last wrote; the
    /// label `doc_read` puts on that block. A removed member keeps labelling their text as "former member".
    pub(crate) fn author_labels(&self, doc: &DocId) -> std::collections::HashMap<String, String> {
        let Some(o) = self.open.get(doc) else {
            return Default::default();
        };
        o.meta
            .authors
            .iter()
            .map(|(id, pub_)| {
                let name = o
                    .meta
                    .members
                    .iter()
                    .find(|m| m.pub_ == *pub_)
                    .map_or("former member", |m| m.name.as_str());
                (
                    id.clone(),
                    format!("{name} · doc-member:{}", &pub_[..pub_.len().min(8)]),
                )
            })
            .collect()
    }

    /// Persist `meta.json` after a members/links change; a failure is logged, the in-memory state stands.
    pub(crate) async fn save_members(&mut self, doc: &DocId) {
        let Some(meta) = self.open.get(doc).map(|o| o.meta.clone()) else {
            return;
        };
        if let Err(e) = self.store.write_meta(&meta).await {
            tracing::warn!(doc = %doc, error = %e, "meta.json: members not saved");
        }
    }

    /// Save, tell every subscriber the room changed, and the bus (213 §4 `document.sharing@1`).
    async fn members_changed(&mut self, doc: &DocId) {
        self.save_members(doc).await;
        if let Ok(Members { members, links }) = self.members_of(doc) {
            self.fan_out(doc, DocEvent::Members { members, links }, None);
        }
        self.rt
            .publish(crate::meta::DocumentSharing { doc: doc.clone() })
            .await;
    }

    /// `pub_`'s subscriptions to every note/source item of `notebook` that they hold only through it
    /// end with `Closed{reason}` (a demotion on the notebook: the item tabs re-open at the new role).
    async fn close_derived_member(&mut self, notebook: &DocId, pub_: &str, reason: &str) {
        let Ok(items) = self.store.items(notebook).await else {
            return;
        };
        for item in items {
            if !matches!(item.kind, ItemKind::Note | ItemKind::Source) {
                continue;
            }
            let Some(doc) = DocId::parse(&item.item) else {
                continue;
            };
            let direct = self
                .open
                .get(&doc)
                .is_some_and(|o| o.meta.members.iter().any(|m| m.pub_ == pub_));
            if !direct {
                self.close_member(&doc, pub_, reason);
            }
        }
    }

    /// End `pub_`'s subscriptions to `doc` with `Closed{reason}` (§8.1 demote, §8.4 revoke).
    fn close_member(&mut self, doc: &DocId, pub_: &str, reason: &str) {
        let Some(subs) = self.subs.get_mut(doc) else {
            return;
        };
        subs.retain(|s| {
            if s.member.as_deref() != Some(pub_) {
                return true;
            }
            let _ = s.tx.try_send(DocEvent::Closed {
                reason: reason.to_owned(),
            });
            false
        });
        if subs.is_empty() {
            self.subs.remove(doc);
        }
    }
}

// ---- messages -------------------------------------------------------------

/// `ask` → `Result<(ShareLink, String), String>`: a new link (§8.2); the second item is the
/// base64url secret, returned exactly once — the store keeps `sha256(secret)`.
#[derive(Clone, Debug)]
pub struct LinkCreate {
    /// Document.
    pub doc: DocId,
    /// Role joiners get.
    pub role: Role,
    /// Preset display name.
    pub name: Option<String>,
    /// Expiry, unix ms.
    pub expires_ms: Option<i64>,
}

impl Message<LinkCreate> for DocumentActor {
    type Reply = DelegatedReply<Result<(ShareLink, String), DocError>>;

    async fn handle(&mut self, m: LinkCreate, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        // 184 §4.6, §7.3: a live meta write on the document's server.
        let doc = m.doc.clone();
        if let Some((m, tx)) = self.gate_here(&doc, m, tx) {
            tx.send(self.link_create(m).await);
        }
        reply
    }
}

impl DocumentActor {
    async fn link_create(&mut self, m: LinkCreate) -> Result<(ShareLink, String), DocError> {
        if self.cfg.sharing != "on" {
            return Err("sharing_off".into());
        }
        let Some(vault) = self.store.vault() else {
            return Err("secrets vault unavailable; cannot seal the link secret".into());
        };
        let mut secret = [0u8; 32];
        let mut id = [0u8; 8];
        rand::rng().fill_bytes(&mut secret);
        rand::rng().fill_bytes(&mut id);
        let id = format!("lnk_{}", b64url(&id));
        let link = ShareLink {
            secret_hash_sealed: seal_hash(&vault, &id, &secret_hash(&secret)),
            id,
            role: m.role,
            name: m
                .name
                .filter(|n| !n.trim().is_empty())
                .map(|n| n.trim().to_owned()),
            created_ms: now_ms(),
            expires_ms: m.expires_ms,
            revoked_ms: None,
        };
        let o = self.load(&m.doc)?;
        o.meta.links.push(link.clone());
        self.members_changed(&m.doc).await;
        Ok((link.public(), b64url(&secret)))
    }
}

/// `ask` → `Result<(), String>`: stop new joins through `link` (§8.4); its members stay.
#[derive(Clone, Debug)]
pub struct LinkRevoke {
    /// Document.
    pub doc: DocId,
    /// `ShareLink::id`.
    pub link: String,
}

impl Message<LinkRevoke> for DocumentActor {
    type Reply = DelegatedReply<Result<(), DocError>>;

    async fn handle(&mut self, m: LinkRevoke, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        if let Some((m, tx)) = self.gate_here(&doc, m, tx) {
            tx.send(self.link_revoke(m).await);
        }
        reply
    }
}

impl DocumentActor {
    async fn link_revoke(&mut self, m: LinkRevoke) -> Result<(), DocError> {
        let o = self.load(&m.doc)?;
        let link = o
            .meta
            .links
            .iter_mut()
            .find(|l| l.id == m.link)
            .ok_or_else(|| "link_unknown".to_owned())?;
        if link.revoked_ms.is_none() {
            link.revoked_ms = Some(now_ms());
            self.members_changed(&m.doc).await;
        }
        Ok(())
    }
}

/// `ask` → `Result<Joined, JoinError>`: a guest proves a link secret for *this* connection (§8.3)
/// and becomes a member. The URL carries no link id (§8.2), so the proof is checked against every
/// live link of the document — a few HMACs. A static that is already a member is answered without
/// checking any link (return visit; membership outlives the link, §8.2).
#[derive(Clone, Debug)]
pub struct Join {
    /// Document.
    pub doc: DocId,
    /// The guest's X25519 static (from the Noise handshake).
    pub guest_pub: [u8; 32],
    /// The Noise handshake hash of the guest's connection.
    pub handshake_hash: Vec<u8>,
    /// `HMAC-SHA256(sha256(secret), handshake_hash ‖ guest_pub)`.
    pub proof: Vec<u8>,
    /// Self-chosen name; the link's preset wins when set.
    pub name: Option<String>,
}

impl Message<Join> for DocumentActor {
    type Reply = DelegatedReply<Result<Joined, JoinError>>;

    async fn handle(&mut self, m: Join, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.route_doc(&m.doc, false) {
            Ok(Route::Serve) => tx.send(self.join(m).await),
            // §4.3 step 4: a hand-back is running; the guest retries `busy` inside its deadline.
            Ok(Route::Park) => {
                let doc = m.doc.clone();
                self.park(&doc, m, tx, Some(JoinError::Other(BUSY.to_owned())));
            }
            Ok(Route::Forward(hub)) => {
                let client = Client::Member(b64url(&m.guest_pub));
                let doc = m.doc.clone();
                self.forward(hub, &doc, client, Cmd::Join { join: m, reply: tx });
            }
            Err(DocError::NotHub { hub }) => tx.send(Err(JoinError::NotHub(hub))),
            Err(DocError::Other(e)) => tx.send(Err(JoinError::Other(e))),
        }
        reply
    }
}

impl DocumentActor {
    async fn join(&mut self, m: Join) -> Result<Joined, JoinError> {
        if self.cfg.sharing == "disabled" {
            return Err(JoinError::SharingOff);
        }
        let max = self.cfg.max_members;
        let pub_ = b64url(&m.guest_pub);
        let vault = self.store.vault();
        let o = self.load(&m.doc).map_err(JoinError::Other)?;
        if let Some(existing) = o.meta.members.iter().find(|x| x.pub_ == pub_) {
            return Ok(Joined {
                role: existing.role,
                name: existing.name.clone(),
                color: existing.color.clone(),
            });
        }
        let now = now_ms();
        // Every link is tried, live or not, so the error names the link the guest actually holds.
        let Some(link) = o.meta.links.iter().find(|l| {
            verify(
                vault.as_deref(),
                l,
                &m.handshake_hash,
                &m.guest_pub,
                &m.proof,
            )
        }) else {
            return Err(if o.meta.links.is_empty() {
                JoinError::LinkUnknown
            } else {
                JoinError::BadProof
            });
        };
        if link.revoked_ms.is_some() {
            return Err(JoinError::LinkRevoked);
        }
        if !link.live(now) {
            return Err(JoinError::LinkExpired);
        }
        // Z184-R3: a revoke blocks every link that existed then (the guest would just reload it);
        // only a link made afterwards admits the key again.
        if o.meta
            .revoked
            .get(&pub_)
            .is_some_and(|at| link.created_ms <= *at)
        {
            return Err(JoinError::MemberRevoked);
        }
        if o.meta.members.len() >= max {
            return Err(JoinError::Full);
        }
        let name = link
            .name
            .clone()
            .or(m
                .name
                .filter(|n| !n.trim().is_empty())
                .map(|n| n.trim().to_owned()))
            .unwrap_or_else(|| "Guest".to_owned());
        // Later than any revoke of this key, so other engines' tombstones keep the new member.
        let added_ms = o.meta.revoked.get(&pub_).map_or(now, |at| now.max(at + 1));
        let member = Member {
            pub_,
            name,
            color: PALETTE[o.meta.members.len() % PALETTE.len()].to_owned(),
            role: link.role,
            added_ms,
            via: Some(link.id.clone()),
            last_seen_ms: now,
        };
        let joined = Joined {
            role: member.role,
            name: member.name.clone(),
            color: member.color.clone(),
        };
        o.meta.members.push(member);
        self.members_changed(&m.doc).await;
        Ok(joined)
    }
}

/// `ask` → `Result<(), String>`: the owner renames or re-roles a member (§8.1, §8.5). A demotion
/// to `viewer` ends their subscriptions with `Closed{demoted}`; the client re-opens read-only.
#[derive(Clone, Debug)]
pub struct MemberSet {
    /// Document.
    pub doc: DocId,
    /// `Member::pub_`.
    pub pub_: String,
    /// New role, if changing.
    pub role: Option<Role>,
    /// New name, if changing.
    pub name: Option<String>,
}

impl Message<MemberSet> for DocumentActor {
    type Reply = DelegatedReply<Result<(), DocError>>;

    async fn handle(&mut self, m: MemberSet, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        if let Some((m, tx)) = self.gate_here(&doc, m, tx) {
            tx.send(self.member_set(m).await);
        }
        reply
    }
}

impl DocumentActor {
    async fn member_set(&mut self, m: MemberSet) -> Result<(), DocError> {
        let o = self.load(&m.doc)?;
        let member = o
            .meta
            .members
            .iter_mut()
            .find(|x| x.pub_ == m.pub_)
            .ok_or_else(|| "not_member".to_owned())?;
        let mut demoted = false;
        if let Some(role) = m.role {
            demoted = member.role == Role::Editor && role == Role::Viewer;
            member.role = role;
        }
        if let Some(name) = m.name.filter(|n| !n.trim().is_empty()) {
            member.name = name.trim().to_owned();
        }
        if demoted {
            self.close_member(&m.doc, &m.pub_, "demoted");
            // 144 §11: the derived editors on the notebook's items re-open read-only too.
            if self
                .open
                .get(&m.doc)
                .is_some_and(|o| o.meta.kind == DocKind::Notebook)
            {
                let nb = m.doc.clone();
                let pub_ = m.pub_.clone();
                self.close_derived_member(&nb, &pub_, "demoted").await;
            }
        }
        self.members_changed(&m.doc).await;
        Ok(())
    }
}

/// `ask` → `Result<(), String>`: remove a member (§8.4): `Closed{revoked}` to their subscriptions;
/// the caller (rpc) then drops their conns through `link`. Their next connection finds no membership.
#[derive(Clone, Debug)]
pub struct MemberRevoke {
    /// Document.
    pub doc: DocId,
    /// `Member::pub_`.
    pub pub_: String,
}

impl Message<MemberRevoke> for DocumentActor {
    type Reply = DelegatedReply<Result<(), DocError>>;

    async fn handle(
        &mut self,
        m: MemberRevoke,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        if let Some((m, tx)) = self.gate_here(&doc, m, tx) {
            tx.send(self.member_revoke(m).await);
        }
        reply
    }
}

impl DocumentActor {
    async fn member_revoke(&mut self, m: MemberRevoke) -> Result<(), DocError> {
        let o = self.load(&m.doc)?;
        let Some(gone) = o.meta.members.iter().find(|x| x.pub_ == m.pub_) else {
            return Err("not_member".into());
        };
        // Z184-R2: a tombstone the merge keeps, so no other engine's copy brings the member back.
        let at = crate::store::now_ms().max(gone.added_ms);
        o.meta.revoked.insert(m.pub_.clone(), at);
        o.meta.members.retain(|x| x.pub_ != m.pub_);
        self.close_member(&m.doc, &m.pub_, "revoked");
        self.members_changed(&m.doc).await;
        // 144 §11: a notebook member loses every derived grant the same way.
        if self
            .open
            .get(&m.doc)
            .is_some_and(|o| o.meta.kind == DocKind::Notebook)
        {
            self.revoke_derived_of(&m.doc, "revoked").await;
        }
        Ok(())
    }
}

/// `ask` → `Result<(), String>`: a member renames themself (§8.5 `doc_set_name`); editors only
/// (`viewer_readonly`), members only (`not_member`). Owners have no name to set here.
#[derive(Clone, Debug)]
pub struct SetName {
    /// Document.
    pub doc: DocId,
    /// The caller.
    pub by: Principal,
    /// New name.
    pub name: String,
}

impl Message<SetName> for DocumentActor {
    type Reply = DelegatedReply<Result<(), DocError>>;

    async fn handle(&mut self, m: SetName, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        match self.route_doc(&m.doc, false) {
            Ok(Route::Serve) => tx.send(self.set_name(m).await),
            Ok(Route::Park) => {
                let doc = m.doc.clone();
                self.park(&doc, m, tx, None);
            }
            Ok(Route::Forward(hub)) => self.forward(
                hub,
                &m.doc,
                Client::of(&m.by),
                Cmd::SetName {
                    name: m.name,
                    reply: tx,
                },
            ),
            Err(e) => tx.send(Err(e)),
        }
        reply
    }
}

impl DocumentActor {
    async fn set_name(&mut self, m: SetName) -> Result<(), DocError> {
        let pub_ =
            m.by.member()
                .ok_or_else(|| "not_member".to_owned())?
                .to_owned();
        self.admit(&m.doc, &m.by, true).await?;
        let name = m.name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err("bad_name".into());
        }
        let o = self.load(&m.doc)?;
        if let Some(member) = o.meta.members.iter_mut().find(|x| x.pub_ == pub_) {
            member.name = name.to_owned();
        }
        self.members_changed(&m.doc).await;
        Ok(())
    }
}

/// `ask` → `Result<Members, String>`: `doc_members` (owner only, enforced by rpc).
#[derive(Clone, Debug)]
pub struct MembersOf {
    /// Document.
    pub doc: DocId,
}

impl Message<MembersOf> for DocumentActor {
    type Reply = DelegatedReply<Result<Members, DocError>>;

    async fn handle(&mut self, m: MembersOf, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let doc = m.doc.clone();
        if let Some((m, tx)) = self.gate_here(&doc, m, tx) {
            let r = self.load(&m.doc).map(|_| ()).map_err(DocError::from);
            tx.send(r.and_then(|()| Ok(self.members_of(&m.doc)?)));
        }
        reply
    }
}

/// One document of [`SharedDocs`]: its sharing as this engine holds it (links without their hash).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SharedDoc {
    /// Document.
    pub doc: DocId,
    /// Title.
    pub title: String,
    /// Members and links.
    pub sharing: Members,
}

/// `ask` → `Result<Vec<SharedDoc>, String>` (213 §3 `sharing_list`): every live document with
/// members or links, newest update first, served on any engine: the open copy when loaded, else
/// `meta.json` (mirrored meta on a non-serving engine, eventually consistent). Owner only (rpc).
#[derive(Clone, Copy, Debug, Default)]
pub struct SharedDocs;

impl Message<SharedDocs> for DocumentActor {
    type Reply = Result<Vec<SharedDoc>, String>;

    async fn handle(&mut self, _: SharedDocs, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let ids = self.store.shared().await.map_err(|e| format!("{e:#}"))?;
        let mut out = Vec::with_capacity(ids.len());
        for doc in ids {
            let meta = match self.open.get(&doc) {
                Some(o) => o.meta.clone(),
                None => match self.store.read_meta(&doc) {
                    Ok(m) => m,
                    Err(e) => {
                        tracing::debug!(doc = %doc, error = %e, "sharing list: meta.json unreadable");
                        continue;
                    }
                },
            };
            if meta.members.is_empty() && meta.links.is_empty() {
                continue;
            }
            out.push(SharedDoc {
                doc,
                title: meta.title,
                sharing: Members {
                    members: meta.members,
                    links: meta.links.iter().map(ShareLink::public).collect(),
                },
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn vault() -> Vault {
        let dir = tempfile::tempdir().unwrap();
        Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap()
    }

    fn link(vault: &Vault, secret: &[u8]) -> ShareLink {
        ShareLink {
            id: "lnk_test".into(),
            role: Role::Editor,
            secret_hash_sealed: seal_hash(vault, "lnk_test", &secret_hash(secret)),
            name: None,
            created_ms: 0,
            expires_ms: None,
            revoked_ms: None,
        }
    }

    #[tokio::test]
    async fn sealed_hash_round_trips_under_the_mesh_key_only() {
        let (a, b) = (vault().await, vault().await);
        let secret = [7u8; 32];
        let hash = secret_hash(&secret);
        let l = link(&a, &secret);
        assert_ne!(l.secret_hash_sealed, b64url(&hash), "not plaintext");
        assert_eq!(open_hash(&a, &l).as_deref(), Some(&hash[..]));
        assert_eq!(open_hash(&b, &l), None, "another mesh key does not open it");
        let mut moved = l.clone();
        moved.id = "lnk_other".into();
        assert_eq!(
            open_hash(&a, &moved),
            None,
            "aad binds the ciphertext to its link"
        );
        assert_eq!(open_hash(&a, &l.public()), None);
    }

    #[tokio::test]
    async fn proof_binds_secret_connection_and_key() {
        let v = vault().await;
        let secret = [7u8; 32];
        let l = link(&v, &secret);
        let h = [1u8; 32];
        let g = [2u8; 32];
        let p = proof(&secret_hash(&secret), &h, &g);
        assert!(verify(Some(&v), &l, &h, &g, &p));
        // Another connection, another guest key, another secret, a truncated proof: all refused.
        assert!(!verify(Some(&v), &l, &[9u8; 32], &g, &p));
        assert!(!verify(Some(&v), &l, &h, &[3u8; 32], &p));
        assert!(!verify(Some(&v), &link(&v, &[8u8; 32]), &h, &g, &p));
        assert!(!verify(Some(&v), &l, &h, &g, &p[..31]));
        // The wire copy carries no key and verifies nothing; neither does an engine without a vault.
        assert!(!verify(Some(&v), &l.public(), &h, &g, &p));
        assert!(!verify(None, &l, &h, &g, &p));
    }

    #[tokio::test]
    async fn link_liveness() {
        let mut l = link(&vault().await, &[0u8; 32]);
        assert!(l.live(10));
        l.expires_ms = Some(10);
        assert!(!l.live(10));
        assert!(l.live(9));
        l.expires_ms = None;
        l.revoked_ms = Some(1);
        assert!(!l.live(0));
    }

    /// 183 §6.9.11, §13 row 45: a link sealed before a rotation still verifies (trial with the
    /// retained key), re-seals under the new key, and keeps verifying once the old key is
    /// destroyed; a copy that missed the re-seal stops (the UI offers Re-share).
    #[tokio::test]
    async fn share_links_reseal_across_a_rotation() {
        let dir = tempfile::tempdir().unwrap();
        let v = Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap();
        let secret = [7u8; 32];
        let (h, g) = ([1u8; 32], [2u8; 32]);
        let p = proof(&secret_hash(&secret), &h, &g);
        let mut links = vec![link(&v, &secret)];
        let missed = links[0].clone();
        v.rotate("a", vec![], vec![], 0, false).await.unwrap();
        assert!(verify(Some(&v), &links[0], &h, &g, &p), "both keys valid");
        assert!(reseal_links(&v, &mut links));
        assert!(
            !reseal_links(&v, &mut links),
            "already under the current key"
        );
        v.maintain(now_ms() as u64 + secrets::vault::RETIRE_GRACE_MS + 1_000)
            .await
            .unwrap();
        assert_eq!(v.held_keys().len(), 1, "the old key is destroyed");
        assert!(verify(Some(&v), &links[0], &h, &g, &p));
        assert!(!verify(Some(&v), &missed, &h, &g, &p));
    }
}
