//! One-time link claims (design doc 224 §16.12, the phone-only path).
//!
//! A phone that is not a mesh member makes a cloud node that forms its own mesh, and connects to it
//! as a UI client over the relay — the way the app reaches any remote engine. The phone cannot
//! hand the node anything after boot (no SSH, no inbound port), so the one credential travels in
//! the user data: a **claim** `L1.<hex psk>.<expires ms>[.<hex relay>]`.
//!
//! - The node's engine (`t mesh devices claim <claim>` from `theuth-install.sh join`) arms its
//!   ENGINE identity's single pairing with the claim's PSK until the claim's expiry ([`ClaimPairing`]),
//!   keeps the claim in `<data_dir>/link/claim` (0600) so a restart re-arms it, and parks its public
//!   key and name on the relay under [`Claim::id`] (`PUT /claim/<id>`, `crates/relay/src/claim.rs`)
//!   with [`Claim::mac`].
//! - The phone reads the parked entry, checks the MAC, builds the ordinary pairing string
//!   ([`Claim::pairing`]) and pairs (Noise `IKpsk2`). The first pairing consumes the PSK
//!   (`conn::commit_pairing` → [`consumed`]): the file is deleted and the id recorded in
//!   `<data_dir>/link/claims-used`, so the same claim is refused afterwards. The long-lived
//!   credential is the phone's device key in the node's `devices` table, as for any pairing.
//!
//! The engine identity (`<data_dir>/link/identity`, on disk) is used, not the mesh identity
//! (`mesh/device_key`, in the vault): a sealed node (design doc 225) still answers on it, so the
//! phone can reach a restarted node to unseal it.

use std::path::Path;

use hmac::{Hmac, KeyInit as _, Mac as _};
use kameo::message::{Context, Message};
use serde::Serialize;
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use crate::identity::{b64, now_ms, unb64};
use crate::pairing::{self, Pending};
use crate::types::ClaimAccepted;
use crate::{Expire, LinkActor, yaml_str};

/// The claim string's prefix (a mesh ticket starts with `1.`).
pub const PREFIX: &str = "L1.";
/// The longest a claim may live: a node boots, installs and starts its engine within it (≈4 min
/// measured on DigitalOcean, 224 §4); the rest is the phone being frozen in the background.
pub const MAX_TTL_MS: u64 = 30 * 60_000;
/// What the join loop matches to stop retrying a claim (`theuth-install.sh`).
pub const INVALID: &str = "invalid or expired link claim";
/// Ditto, for a claim this engine already paired with.
pub const USED: &str = "link claim already used";
const FILE: &str = "claim";
const USED_FILE: &str = "claims-used";
const ID_DOMAIN: &[u8] = b"theuth-link-claim-id-1\0";
const MAC_DOMAIN: &[u8] = b"theuth-link-claim-mac-1\0";

/// A parsed claim. `Debug` never shows the PSK.
#[derive(Clone)]
pub struct Claim {
    psk: Zeroizing<[u8; 32]>,
    /// Unix ms; dead afterwards.
    pub expires_ms: u64,
    /// The link relay (`wss://…`) the phone uses; `None` = the node's default.
    pub relay: Option<String>,
}

impl std::fmt::Debug for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Claim")
            .field("id", &self.id())
            .field("expires_ms", &self.expires_ms)
            .field("relay", &self.relay)
            .field("psk", &"<redacted>")
            .finish()
    }
}

fn hex_ok(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(hex_ok_byte)
}

/// Only `[0-9a-f.]` after [`PREFIX`]: safe inside the user data's single quotes.
pub fn looks_like(s: &str) -> bool {
    s.strip_prefix(PREFIX)
        .is_some_and(|rest| !rest.is_empty() && rest.bytes().all(|b| b == b'.' || hex_ok_byte(b)))
}

fn hex_ok_byte(b: u8) -> bool {
    b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
}

/// `wss://h` → `https://h`, `ws://h` → `http://h` (the relay's HTTP routes), trailing `/` dropped.
pub fn https_origin(relay: &str) -> String {
    let r = relay.trim().trim_end_matches('/');
    if let Some(rest) = r.strip_prefix("wss://") {
        format!("https://{rest}")
    } else if let Some(rest) = r.strip_prefix("ws://") {
        format!("http://{rest}")
    } else {
        r.to_owned()
    }
}

impl Claim {
    /// A fresh claim: 32 random bytes, living `ttl_ms` (at most [`MAX_TTL_MS`]) from `now_ms`.
    pub fn mint(relay: Option<&str>, now_ms: u64, ttl_ms: u64) -> Self {
        Self::from_parts(rand::random(), now_ms + ttl_ms.min(MAX_TTL_MS), relay.map(str::to_owned))
    }

    /// From stored parts (the phone keeps the PSK in its vault while the create job runs).
    pub fn from_parts(psk: [u8; 32], expires_ms: u64, relay: Option<String>) -> Self {
        Self {
            psk: Zeroizing::new(psk),
            expires_ms,
            relay,
        }
    }

    /// The PSK.
    pub fn psk(&self) -> &[u8; 32] {
        &self.psk
    }

    /// Before its expiry.
    pub fn live(&self, now_ms: u64) -> bool {
        now_ms < self.expires_ms
    }

    /// `L1.<hex psk>.<expires ms>[.<hex relay>]`.
    pub fn encode(&self) -> Zeroizing<String> {
        let mut s = format!("{PREFIX}{}.{}", hex::encode(*self.psk), self.expires_ms);
        if let Some(r) = &self.relay {
            s.push('.');
            s.push_str(&hex::encode(r.as_bytes()));
        }
        Zeroizing::new(s)
    }

    /// Inverse of [`Claim::encode`]; a relay must be `ws://` or `wss://`.
    pub fn parse(s: &str) -> Option<Self> {
        let rest = s.trim().strip_prefix(PREFIX)?;
        let mut parts = rest.split('.');
        let psk_hex = parts.next().filter(|p| p.len() == 64 && hex_ok(p))?;
        let expires_ms: u64 = parts
            .next()
            .filter(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))?
            .parse()
            .ok()?;
        let relay = match parts.next() {
            None => None,
            Some(r) => {
                if !hex_ok(r) {
                    return None;
                }
                let r = String::from_utf8(hex::decode(r).ok()?).ok()?;
                if !(r.starts_with("wss://") || r.starts_with("ws://")) {
                    return None;
                }
                Some(r)
            }
        };
        if parts.next().is_some() {
            return None;
        }
        let mut psk = Zeroizing::new([0u8; 32]);
        hex::decode_to_slice(psk_hex, psk.as_mut()).ok()?;
        Some(Self {
            psk,
            expires_ms,
            relay,
        })
    }

    /// The relay rendezvous id: `base64url(sha256(domain ‖ psk)[..16])` — not secret, and it
    /// reveals nothing about the PSK.
    pub fn id(&self) -> String {
        let mut h = Sha256::new();
        h.update(ID_DOMAIN);
        h.update(*self.psk);
        b64(&h.finalize()[..16])
    }

    fn mac_of(&self, engine: &str, name: &str) -> Hmac<Sha256> {
        let mut m = Hmac::<Sha256>::new_from_slice(&*self.psk).expect("any key length");
        m.update(MAC_DOMAIN);
        m.update(&(engine.len() as u64).to_be_bytes());
        m.update(engine.as_bytes());
        m.update(name.as_bytes());
        m
    }

    /// `base64url(HMAC-SHA256(psk, domain ‖ len(engine) ‖ engine ‖ name))`: proves the parked
    /// entry came from an engine holding the claim.
    pub fn mac(&self, engine: &str, name: &str) -> String {
        b64(&self.mac_of(engine, name).finalize().into_bytes())
    }

    /// Constant-time check of a parked entry's MAC.
    pub fn verify(&self, engine: &str, name: &str, mac: &str) -> bool {
        unb64(mac).is_some_and(|m| self.mac_of(engine, name).verify_slice(&m).is_ok())
    }

    /// The ordinary 3-part pairing string for the node's engine identity `engine` (base64url
    /// public key) on `relay` (`wss://…`); `None` when `engine` is not a key.
    pub fn pairing(&self, relay: &str, engine: &str, name: &str) -> Option<Zeroizing<String>> {
        let public: [u8; 32] = unb64(engine)?.try_into().ok()?;
        Some(Zeroizing::new(pairing::build(relay, &public, &self.psk, name, None)))
    }
}

fn used(dir: &Path, id: &str) -> bool {
    std::fs::read_to_string(dir.join(USED_FILE)).is_ok_and(|s| s.lines().any(|l| l == id))
}

/// The claim may arm a pairing at `now`: live, not longer than [`MAX_TTL_MS`] (a minute of clock
/// skew allowed), not used before on this engine.
pub fn check(claim: &str, now: u64, link_dir: &Path) -> Result<Claim, String> {
    let c = Claim::parse(claim).ok_or(INVALID)?;
    if !c.live(now) || c.expires_ms > now + MAX_TTL_MS + 60_000 {
        return Err(INVALID.into());
    }
    if used(link_dir, &c.id()) {
        return Err(USED.into());
    }
    Ok(c)
}

/// `conn::commit_pairing`: a pairing just consumed `psk`. When it was the stored claim's, the
/// claim file goes and its id is recorded, so the claim never arms again.
pub(crate) fn consumed(data_dir: &Path, psk: &[u8; 32]) {
    let dir = data_dir.join("link");
    let Ok(s) = std::fs::read_to_string(dir.join(FILE)) else {
        return;
    };
    let Some(c) = Claim::parse(&s) else {
        return;
    };
    if c.psk() != psk {
        return;
    }
    let id = c.id();
    let mut list = std::fs::read_to_string(dir.join(USED_FILE)).unwrap_or_default();
    list.push_str(&id);
    list.push('\n');
    if let Err(e) = platform::files::write_private(&dir.join(USED_FILE), list.as_bytes()) {
        tracing::warn!("link: recording the used claim: {e}");
    }
    let _ = std::fs::remove_file(dir.join(FILE));
    tracing::info!("link: a device paired with the one-time link claim; it is spent");
}

/// At start: re-arm a stored claim (the engine restarted before the phone paired), or drop a dead
/// one.
pub(crate) async fn rearm(data_dir: &Path, actor: &kameo::actor::ActorRef<LinkActor>) {
    let path = data_dir.join("link").join(FILE);
    let Ok(s) = std::fs::read_to_string(&path) else {
        return;
    };
    let s = Zeroizing::new(s);
    match check(&s, now_ms(), &data_dir.join("link")) {
        Ok(_) => match actor
            .ask(ClaimPairing {
                claim: s.trim().to_owned(),
            })
            .await
        {
            Ok(_) => tracing::info!("link: re-armed the one-time link claim"),
            Err(e) => tracing::warn!("link: re-arming the link claim: {e}"),
        },
        Err(why) => {
            tracing::info!("link: dropping the stored link claim ({why})");
            let _ = std::fs::remove_file(&path);
        }
    }
}

/// `ask` → `Result<ClaimAccepted, String>`: arm the engine identity's pairing with a link claim
/// (turning Link on, with the claim's relay), keep the claim for a restart, park this engine on
/// the relay. Idempotent while the claim is armed. `Debug` never shows the claim.
#[derive(Clone)]
pub struct ClaimPairing {
    /// `L1.…`.
    pub claim: String,
}

impl std::fmt::Debug for ClaimPairing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClaimPairing(<redacted>)")
    }
}

/// What the node parks on the relay (`crates/relay/src/claim.rs` `Entry`).
#[derive(Serialize)]
struct Parked {
    engine: String,
    name: String,
    mac: String,
}

impl Message<ClaimPairing> for LinkActor {
    type Reply = Result<ClaimAccepted, String>;

    async fn handle(
        &mut self,
        m: ClaimPairing,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let m = Zeroizing::new(m.claim);
        let link_dir = self.ctx.rt.data_dir.join("link");
        let claim = check(&m, now_ms(), &link_dir)?;
        // Link on, on the claim's relay (the one the phone dials).
        let relay_changed = {
            let st = self.ctx.lock();
            claim.relay.as_ref().is_some_and(|r| *r != st.cfg.relay)
        };
        if let Some(r) = claim.relay.as_ref().filter(|_| relay_changed) {
            self.write("relay", &yaml_str(r))?;
        }
        let was_enabled = self.ctx.lock().cfg.enabled;
        if !was_enabled {
            self.write("enabled", "true")?;
        }
        {
            let mut st = self.ctx.lock();
            if let Some(r) = claim.relay.as_ref().filter(|_| relay_changed) {
                st.cfg.relay = r.clone();
            }
            st.cfg.enabled = true;
        }
        self.ensure_identity()?;
        if self.uplink.is_none() || relay_changed {
            self.start_uplink();
        }
        if relay_changed && let Some(up) = self.mesh.take() {
            up.stop();
        }
        self.sync_mesh(ctx.actor_ref().clone()).await;
        // The engine identity's single pairing, with the claim's PSK until the claim's expiry.
        let (parked, origin, armed) = {
            let mut st = self.ctx.lock();
            let identity = st.identity.clone().ok_or(crate::DISABLED)?;
            let name = st.name().to_owned();
            let relay = st.cfg.relay.clone();
            let armed = st.pairing.as_ref().is_some_and(|p| p.secret == *claim.psk());
            if !armed {
                st.pairing = Some(Pending {
                    secret: *claim.psk(),
                    key: pairing::build(&relay, &identity.public, claim.psk(), &name, None),
                    expires_at_ms: claim.expires_ms,
                });
            }
            let parked = Parked {
                mac: claim.mac(&identity.id, &name),
                engine: identity.id,
                name,
            };
            (parked, https_origin(&relay), armed)
        };
        if !armed {
            if let Some(t) = self.pair_timer.take() {
                t.abort();
            }
            let me = ctx.actor_ref().clone();
            let secret = *claim.psk();
            let left = claim.expires_ms.saturating_sub(now_ms());
            self.pair_timer = Some(tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(left)).await;
                let _ = me.tell(Expire(secret)).await;
            }));
        }
        std::fs::create_dir_all(&link_dir).map_err(|e| format!("{}: {e}", link_dir.display()))?;
        platform::files::write_private(&link_dir.join(FILE), claim.encode().as_bytes())
            .map_err(|e| format!("storing the link claim: {e}"))?;
        let id = claim.id();
        tokio::spawn(park(origin, id.clone(), parked, claim.expires_ms));
        Ok(ClaimAccepted {
            id,
            expires_ms: claim.expires_ms,
        })
    }
}

/// `PUT <relay>/claim/<id>` until the relay keeps it or the claim expires.
async fn park(origin: String, id: String, body: Parked, expires_ms: u64) {
    let http = match reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .build()
    {
        Ok(h) => h,
        Err(e) => {
            tracing::warn!("link: no HTTP client to park the claim: {e}");
            return;
        }
    };
    let url = format!("{origin}/claim/{id}");
    loop {
        match http.put(&url).json(&body).send().await {
            Ok(r) if r.status().is_success() => {
                tracing::info!("link: parked this engine on the relay for the phone's claim");
                return;
            }
            Ok(r) if r.status() == reqwest::StatusCode::CONFLICT => {
                tracing::warn!("link: the relay holds another engine under this claim");
                return;
            }
            Ok(r) => tracing::debug!("link: parking the claim: HTTP {}", r.status()),
            Err(e) => tracing::debug!("link: parking the claim: {e}"),
        }
        if now_ms() >= expires_ms {
            tracing::warn!("link: the link claim expired before the relay took it");
            return;
        }
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_800_000_000_000;

    #[test]
    fn encode_parse_round_trip_and_charset() {
        let c = Claim::mint(Some("wss://relay.example:8443"), NOW, 10 * 60_000);
        let s = c.encode();
        assert!(looks_like(&s), "{}", s.as_str());
        let p = Claim::parse(&s).unwrap();
        assert_eq!(p.psk(), c.psk());
        assert_eq!(p.expires_ms, NOW + 10 * 60_000);
        assert_eq!(p.relay.as_deref(), Some("wss://relay.example:8443"));
        let bare = Claim::from_parts([7; 32], NOW, None);
        assert_eq!(Claim::parse(&bare.encode()).unwrap().relay, None);
        assert_eq!(
            Claim::mint(None, NOW, 24 * 3600_000).expires_ms,
            NOW + MAX_TTL_MS,
            "capped"
        );
        for bad in [
            "1.ABCDEF",
            "L1.",
            "L1.zz.1",
            &format!("L1.{}.1x", "a".repeat(64)),
            &format!("L1.{}", "a".repeat(64)),
            &format!("L1.{}.1.{}", "a".repeat(64), hex::encode("https://x")),
            &format!("L1.{}.1.{}.ff", "a".repeat(64), hex::encode("wss://x")),
            &format!("L1.{}'; rm -rf /.1", "a".repeat(64)),
        ] {
            assert!(Claim::parse(bad).is_none(), "{bad}");
        }
        assert!(!looks_like("L1.abc'"));
    }

    #[test]
    fn debug_never_shows_the_psk() {
        let c = Claim::from_parts([0xab; 32], NOW, None);
        let d = format!("{c:?} {:?}", ClaimPairing { claim: c.encode().to_string() });
        assert!(!d.contains(&hex::encode([0xab; 32])), "{d}");
        assert!(!d.contains("abab"), "{d}");
    }

    #[test]
    fn id_mac_and_pairing() {
        let c = Claim::from_parts([3; 32], NOW, None);
        let other = Claim::from_parts([4; 32], NOW, None);
        assert_eq!(c.id(), Claim::from_parts([3; 32], 1, None).id(), "id depends on the psk only");
        assert_eq!(c.id().len(), 22);
        assert_ne!(c.id(), other.id());
        let engine = b64(&[9; 32]);
        let mac = c.mac(&engine, "lab");
        assert!(c.verify(&engine, "lab", &mac));
        assert!(!c.verify(&engine, "lab2", &mac), "name bound");
        assert!(!c.verify(&b64(&[8; 32]), "lab", &mac), "engine bound");
        assert!(!other.verify(&engine, "lab", &mac), "psk bound");
        assert!(!c.verify(&engine, "lab", "not-b64!"));
        let s = c.pairing("wss://relay.example", &engine, "lab").unwrap();
        let p = pairing::parse(&s).unwrap();
        assert_eq!(
            (p.harness_pub, p.secret, p.name.as_str(), p.pending),
            ([9; 32], [3; 32], "lab", None)
        );
        assert_eq!(p.relay_host, "relay.example");
        assert!(c.pairing("wss://r", "short", "lab").is_none());
        assert_eq!(https_origin("wss://relay.theuth.io/"), "https://relay.theuth.io");
        assert_eq!(https_origin("ws://127.0.0.1:8787"), "http://127.0.0.1:8787");
    }

    /// Single use and expiry: a live claim passes `check`; the pairing that consumes its PSK
    /// deletes the stored claim and records its id, after which the same claim is refused.
    #[test]
    fn lifecycle_live_used_expired() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("link");
        std::fs::create_dir_all(&link).unwrap();
        let c = Claim::from_parts([5; 32], NOW + 60_000, None);
        let s = c.encode();
        assert!(check(&s, NOW, &link).is_ok());
        assert_eq!(check(&s, NOW + 60_000, &link).unwrap_err(), INVALID, "expired");
        let long = Claim::from_parts([5; 32], NOW + MAX_TTL_MS + 120_000, None);
        assert_eq!(check(&long.encode(), NOW, &link).unwrap_err(), INVALID, "too long-lived");
        assert_eq!(check("L1.nope", NOW, &link).unwrap_err(), INVALID);

        std::fs::write(link.join(FILE), s.as_bytes()).unwrap();
        consumed(dir.path(), &[6; 32]);
        assert!(link.join(FILE).exists(), "another pairing's psk leaves the claim alone");
        consumed(dir.path(), c.psk());
        assert!(!link.join(FILE).exists(), "the claim file is gone");
        assert_eq!(check(&s, NOW, &link).unwrap_err(), USED);
        let used = std::fs::read_to_string(link.join(USED_FILE)).unwrap();
        assert!(!used.contains(&hex::encode(c.psk())), "only the id is recorded");
    }
}
