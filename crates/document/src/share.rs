//! Sharing (design 59 §8): share links, members, the join proof, role
//! enforcement and revocation. Handlers on the [`DocumentActor`]; the wire
//! types live in [`crate::meta`].
//!
//! Proof (§8.3, as built): `HMAC-SHA256(key = sha256(secret), handshake_hash ‖
//! guest_pub)`. The harness stores only `sha256(secret)` (`ShareLink::secret_hash`),
//! so that hash — not the secret — is the HMAC key; the browser hashes the
//! fragment secret before signing. The hash therefore never leaves this machine.

use std::fmt;

use base64::Engine as _;
use hmac::{Hmac, Mac};
use kameo::message::{Context, Message};
use rand::RngCore;
use sha2::Sha256;

use crate::actor::DocumentActor;
use crate::meta::{DocEvent, DocId, Joined, Member, Members, PALETTE, Principal, Role, ShareLink};
use crate::store::now_ms;

type HmacSha256 = Hmac<Sha256>;

fn b64url(bytes: &[u8]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

fn unb64url(s: &str) -> Option<Vec<u8>> {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.decode(s).ok()
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

/// Constant-time check of `proof` against the link's stored hash.
fn verify(link: &ShareLink, handshake_hash: &[u8], guest_pub: &[u8; 32], proof: &[u8]) -> bool {
    let Some(key) = unb64url(&link.secret_hash) else {
        return false;
    };
    let Ok(mut mac) = HmacSha256::new_from_slice(&key) else {
        return false;
    };
    mac.update(handshake_hash);
    mac.update(guest_pub);
    mac.verify_slice(proof).is_ok()
}

/// Why a join was refused (§8.3). `Display` is the wire code the guest page keys its copy on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JoinError {
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
    /// Store or load failure.
    Other(String),
}

impl fmt::Display for JoinError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LinkUnknown => f.write_str("link_unknown"),
            Self::LinkRevoked => f.write_str("link_revoked"),
            Self::LinkExpired => f.write_str("link_expired"),
            Self::BadProof => f.write_str("bad_proof"),
            Self::Full => f.write_str("members_full"),
            Self::SharingOff => f.write_str("sharing_off"),
            Self::Other(e) => f.write_str(e),
        }
    }
}

impl std::error::Error for JoinError {}

impl DocumentActor {
    /// §8.1, checked on every member call: a `Principal::Member` must be in `doc`'s members
    /// (`not_member`), be an editor when `write` (`viewer_readonly`), and sharing must not be
    /// `disabled` (`sharing_disabled`). Owners, the agent and the file always pass.
    pub(crate) fn admit(&mut self, doc: &DocId, by: &Principal, write: bool) -> Result<(), String> {
        let Some(pub_) = by.member() else {
            return Ok(());
        };
        if self.cfg.sharing == "disabled" {
            return Err("sharing_disabled".to_owned());
        }
        let o = self.load(doc)?;
        let Some(m) = o.meta.members.iter().find(|m| m.pub_ == pub_) else {
            return Err("not_member".to_owned());
        };
        if write && m.role == Role::Viewer {
            return Err("viewer_readonly".to_owned());
        }
        Ok(())
    }

    /// The owner's view (§6.1 `doc_members`): links without their hash.
    pub(crate) fn members_of(&self, doc: &DocId) -> Result<Members, String> {
        let o = self.open.get(doc).ok_or_else(|| format!("{doc} is not loaded"))?;
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
                (id.clone(), format!("{name} · doc-member:{}", &pub_[..pub_.len().min(8)]))
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

    /// Save and tell every subscriber the room changed.
    async fn members_changed(&mut self, doc: &DocId) {
        self.save_members(doc).await;
        if let Ok(Members { members, links }) = self.members_of(doc) {
            self.fan_out(doc, DocEvent::Members { members, links }, None);
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
    type Reply = Result<(ShareLink, String), String>;

    async fn handle(&mut self, m: LinkCreate, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.cfg.sharing != "on" {
            return Err("sharing_off".to_owned());
        }
        let mut secret = [0u8; 32];
        let mut id = [0u8; 8];
        rand::rng().fill_bytes(&mut secret);
        rand::rng().fill_bytes(&mut id);
        let link = ShareLink {
            id: format!("lnk_{}", b64url(&id)),
            role: m.role,
            secret_hash: b64url(&secret_hash(&secret)),
            name: m.name.filter(|n| !n.trim().is_empty()).map(|n| n.trim().to_owned()),
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
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: LinkRevoke, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
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
    type Reply = Result<Joined, JoinError>;

    async fn handle(&mut self, m: Join, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        if self.cfg.sharing == "disabled" {
            return Err(JoinError::SharingOff);
        }
        let max = self.cfg.max_members;
        let pub_ = b64url(&m.guest_pub);
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
        let Some(link) = o
            .meta
            .links
            .iter()
            .find(|l| verify(l, &m.handshake_hash, &m.guest_pub, &m.proof))
        else {
            return Err(if o.meta.links.is_empty() { JoinError::LinkUnknown } else { JoinError::BadProof });
        };
        if link.revoked_ms.is_some() {
            return Err(JoinError::LinkRevoked);
        }
        if !link.live(now) {
            return Err(JoinError::LinkExpired);
        }
        if o.meta.members.len() >= max {
            return Err(JoinError::Full);
        }
        let name = link
            .name
            .clone()
            .or(m.name.filter(|n| !n.trim().is_empty()).map(|n| n.trim().to_owned()))
            .unwrap_or_else(|| "Guest".to_owned());
        let member = Member {
            pub_,
            name,
            color: PALETTE[o.meta.members.len() % PALETTE.len()].to_owned(),
            role: link.role,
            added_ms: now,
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
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: MemberSet, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
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
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: MemberRevoke, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let o = self.load(&m.doc)?;
        let before = o.meta.members.len();
        o.meta.members.retain(|x| x.pub_ != m.pub_);
        if o.meta.members.len() == before {
            return Err("not_member".to_owned());
        }
        self.close_member(&m.doc, &m.pub_, "revoked");
        self.members_changed(&m.doc).await;
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
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: SetName, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let pub_ = m.by.member().ok_or_else(|| "not_member".to_owned())?.to_owned();
        self.admit(&m.doc, &m.by, true)?;
        let name = m.name.trim();
        if name.is_empty() || name.chars().count() > 64 {
            return Err("bad_name".to_owned());
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
    type Reply = Result<Members, String>;

    async fn handle(&mut self, m: MembersOf, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.load(&m.doc)?;
        self.members_of(&m.doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(secret: &[u8]) -> ShareLink {
        ShareLink {
            id: "lnk_test".into(),
            role: Role::Editor,
            secret_hash: b64url(&secret_hash(secret)),
            name: None,
            created_ms: 0,
            expires_ms: None,
            revoked_ms: None,
        }
    }

    #[test]
    fn proof_binds_secret_connection_and_key() {
        let secret = [7u8; 32];
        let l = link(&secret);
        let h = [1u8; 32];
        let g = [2u8; 32];
        let p = proof(&secret_hash(&secret), &h, &g);
        assert!(verify(&l, &h, &g, &p));
        // Another connection, another guest key, another secret, a truncated proof: all refused.
        assert!(!verify(&l, &[9u8; 32], &g, &p));
        assert!(!verify(&l, &h, &[3u8; 32], &p));
        assert!(!verify(&link(&[8u8; 32]), &h, &g, &p));
        assert!(!verify(&l, &h, &g, &p[..31]));
        // The wire copy carries no key and verifies nothing.
        assert!(!verify(&l.public(), &h, &g, &p));
    }

    #[test]
    fn link_liveness() {
        let mut l = link(&[0u8; 32]);
        assert!(l.live(10));
        l.expires_ms = Some(10);
        assert!(!l.live(10));
        assert!(l.live(9));
        l.expires_ms = None;
        l.revoked_ms = Some(1);
        assert!(!l.live(0));
    }
}
