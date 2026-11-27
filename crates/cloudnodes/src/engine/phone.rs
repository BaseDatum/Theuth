//! The phone-only create path (design doc 224 §16.12). An engine that cannot be a mesh member
//! (the on-device iOS engine: no iroh) makes a node that forms its own single-node mesh; the app
//! then pairs with the node's engine over the relay as a UI client, the way it reaches any remote
//! engine, and switches to it.
//!
//! 1. `cloud_create` mints a one-time link claim ([`link::claim::Claim`], 30 minutes); its PSK
//!    goes into this engine's vault (`cloud/link-claim.<name>`), the record keeps only its id and
//!    expiry ([`NodeLink`]).
//! 2. The provider makes the server with the claim as its "ticket" (user data / Fly file); the
//!    node's `theuth-install.sh join` runs `t mesh devices claim`, which arms the node's pairing
//!    and parks its link identity on the relay.
//! 3. This job polls the relay (`GET /claim/<id>`), checks the MAC with the PSK, and marks the node
//!    ready with its link identity.
//! 4. The app asks `cloud_link_pairing` for the pairing string, pairs, calls `cloud_link_paired`
//!    (the PSK is deleted here; the node spent it at the pairing).
//!
//! Background-safe: every step is persisted in the record (or the vault) before the next, and a
//! resumed job (iOS froze or killed the app) asks the provider whether the server already exists
//! before creating it, then goes back to polling the relay. Nothing here ever logs the claim.

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use link::claim::{Claim, MAX_TTL_MS};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{Cloud, estimate_secs, now_ms, view};
use crate::providers::{CloudError, CreateReq, Created, HttpRequest, Provider, Result, catalog};
use crate::records::Record;
use crate::types::{CloudNode, DEFAULT_VOLUME_GB, NodeLink, NodeState, Stage};

/// How long a phone's link claim lives: the node boots, installs and starts within it (≈4 min
/// measured), and the rest covers the phone sitting frozen in the background.
pub const CLAIM_TTL_MS: u64 = MAX_TTL_MS;
/// Provider create attempts (a call iOS cut off when it froze the app is retried).
const CREATE_TRIES: u32 = 3;

/// The vault row holding the claim's PSK while the job runs.
pub(super) fn claim_row(name: &str) -> String {
    format!("cloud/link-claim.{name}")
}

/// The vault row's JSON. No `Debug`.
#[derive(Serialize, Deserialize)]
struct StoredClaim {
    /// base64url PSK.
    psk: String,
    expires_ms: u64,
    relay: Option<String>,
}

/// `https://h` → `wss://h`, `http://h` → `ws://h`: the link relay of theuth-relay's origin.
pub(super) fn wss_of(origin: &str) -> String {
    if let Some(rest) = origin.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = origin.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        origin.to_owned()
    }
}

impl Cloud {
    /// This engine cannot be a mesh member (iOS): nodes form their own mesh (224 §16.12). When
    /// the iOS engine can join meshes, this turns off and new nodes join the phone's mesh.
    pub fn set_own_mesh(&self, on: bool) {
        self.own_mesh.store(on, Ordering::Relaxed);
    }

    /// See [`Cloud::set_own_mesh`].
    pub fn own_mesh(&self) -> bool {
        self.own_mesh.load(Ordering::Relaxed)
    }

    /// A fresh claim for `name`: its PSK into the vault (replacing a dead one of an earlier
    /// attempt), its public half for the record.
    pub(super) async fn mint_claim(&self, name: &str) -> Result<NodeLink> {
        let claim = Claim::mint(Some(&wss_of(&self.creds.relay)), now_ms(), CLAIM_TTL_MS);
        let value = Zeroizing::new(
            serde_json::to_string(&StoredClaim {
                psk: URL_SAFE_NO_PAD.encode(claim.psk()),
                expires_ms: claim.expires_ms,
                relay: claim.relay.clone(),
            })
            .map_err(|e| CloudError(e.to_string()))?,
        );
        let row = claim_row(name);
        for _ in 0..3 {
            let expected = self.creds.store.get_versioned(&row).map(|(v, _)| v);
            if self
                .creds
                .store
                .set_if_version(&row, &value, expected)
                .await?
            {
                return Ok(NodeLink {
                    claim_id: claim.id(),
                    expires_ms: claim.expires_ms,
                    engine: None,
                    engine_name: None,
                    paired: false,
                });
            }
        }
        Err(CloudError(format!("{row}: concurrent writes; try again")))
    }

    pub(super) fn load_claim(&self, name: &str) -> Result<Claim> {
        let (_, v) = self
            .creds
            .store
            .get_versioned(&claim_row(name))
            .ok_or_else(|| {
                CloudError(format!(
                    "the one-time link claim of {name} is gone (used, expired or destroyed)"
                ))
            })?;
        let s: StoredClaim = serde_json::from_str(&v)
            .map_err(|_| CloudError(format!("the link claim of {name} is unreadable")))?;
        let psk = Zeroizing::new(URL_SAFE_NO_PAD.decode(&s.psk).unwrap_or_default());
        let psk: [u8; 32] = psk
            .as_slice()
            .try_into()
            .map_err(|_| CloudError(format!("the link claim of {name} is unreadable")))?;
        Ok(Claim::from_parts(psk, s.expires_ms, s.relay))
    }

    /// Delete the claim's PSK (paired, failed, destroyed); fine when absent.
    pub(super) async fn forget_claim(&self, name: &str) {
        if let Err(e) = self.creds.store.delete(&claim_row(name)).await {
            tracing::warn!("cloudnodes: {name}: forgetting the link claim: {e}");
        }
    }

    /// Run (or resume) the phone-only create job of `name` as a detached task.
    pub(super) fn spawn_phone_job(self: &Arc<Self>, name: String) {
        let me = self.clone();
        let job = name.clone();
        let task = tokio::spawn(async move {
            if let Err(e) = me.phone_job(&job).await {
                me.fail(&job, &e.0);
                me.forget_claim(&job).await;
            }
            me.jobs.lock().remove(&job);
        });
        self.jobs.lock().insert(name, task.abort_handle());
    }

    async fn phone_job(&self, name: &str) -> Result<()> {
        let rec = self
            .records
            .get(name)?
            .ok_or_else(|| CloudError(format!("no cloud node named {name}")))?;
        let claim = self.load_claim(name)?;
        let deadline = claim.expires_ms.saturating_add(self.pace.grace_ms);
        let (_, install_secs) = estimate_secs(rec.spec.provider);
        if rec.created.is_none() {
            let provider = self
                .provider(rec.spec.provider, rec.spec.k8s.as_ref())
                .await?;
            let created = self
                .ensure_server(&rec, provider.as_ref(), &claim, deadline)
                .await?;
            self.update(name, |r| {
                r.created = Some(created);
                r.stage = Some(Stage::Install);
            })?;
        }
        self.progress(
            name,
            Some(Stage::Install),
            "Installing the newest signed theuth release",
            Some(30),
            Some(install_secs),
        );
        self.wait_online(name, &claim, deadline, install_secs).await
    }

    /// The server exists at the provider: found by its tags (a resumed job, or a create call iOS
    /// cut off by freezing the app), else created with the claim as its ticket.
    async fn ensure_server(
        &self,
        rec: &Record,
        provider: &dyn Provider,
        claim: &Claim,
        deadline: u64,
    ) -> Result<Created> {
        let name = rec.spec.name.as_str();
        let (server_secs, install_secs) = estimate_secs(rec.spec.provider);
        let ticket = claim.encode();
        let req = CreateReq {
            name: name.to_owned(),
            region: rec.spec.region.clone(),
            memory_gb: rec.spec.memory_gb,
            volume_gb: rec.spec.volume_gb.unwrap_or(DEFAULT_VOLUME_GB),
            ticket: ticket.to_string(),
            manifest: catalog::MANIFEST.into(),
            image: catalog::ENGINE_IMAGE.into(),
            k8s: rec.spec.k8s.clone(),
            desktop: rec.spec.desktop.unwrap_or(true),
        };
        let stage = parking_lot::Mutex::new(Stage::Server);
        let say = |s: Stage, line: &str| {
            let advanced = {
                let mut cur = stage.lock();
                let adv = s > *cur;
                if adv {
                    *cur = s;
                }
                adv
            };
            if advanced && let Err(e) = self.update(name, |r| r.stage = Some(s)) {
                tracing::warn!("cloudnodes: {name}: {e}");
            }
            let pct = if s == Stage::Server { 10 } else { 25 };
            self.progress(
                name,
                Some(s),
                line,
                Some(pct),
                Some(server_secs + install_secs),
            );
        };
        self.progress(
            name,
            Some(Stage::Server),
            "Creating the server",
            Some(2),
            Some(server_secs + install_secs),
        );
        let mut tries = 0;
        loop {
            if let Ok(Some(st)) = provider.status(name).await
                && st.server_state.is_some()
            {
                return Ok(Created {
                    ip: st.ip,
                    size_slug: st.size_slug.unwrap_or_default(),
                    ..Created::default()
                });
            }
            tries += 1;
            match provider.create(&req, &say).await {
                Ok(c) => return Ok(c),
                Err(e) if tries < CREATE_TRIES && now_ms() < deadline => {
                    tracing::info!("cloudnodes: {name}: create attempt {tries} failed: {e}");
                    self.progress(name, None, &format!("Retrying: {e}"), None, None);
                    tokio::time::sleep(self.pace.poll).await;
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Install → Join → Ready: poll the relay until the node parked its link identity under the
    /// claim's id with a MAC that checks out, until the claim's deadline.
    async fn wait_online(
        &self,
        name: &str,
        claim: &Claim,
        deadline: u64,
        install_secs: u32,
    ) -> Result<()> {
        let url = format!("{}/claim/{}", self.creds.relay, claim.id());
        let started = Instant::now();
        let (engine, engine_name) = loop {
            let req = HttpRequest {
                method: "GET".into(),
                url: url.clone(),
                bearer: None,
                json: None,
                form: None,
            };
            match self.creds.http.send(req).await {
                Ok(r) if r.status == 200 => {
                    let v = r.json().unwrap_or_default();
                    let s = |k: &str| v.get(k).and_then(|x| x.as_str()).unwrap_or_default();
                    if claim.verify(s("engine"), s("name"), s("mac")) {
                        break (s("engine").to_owned(), s("name").to_owned());
                    }
                    tracing::warn!(
                        "cloudnodes: {name}: ignoring a relay answer whose MAC is wrong"
                    );
                }
                Ok(_) => {}
                Err(e) => tracing::debug!("cloudnodes: {name}: relay: {e:#}"),
            }
            if self.records.get(name)?.is_none() {
                return Err(CloudError(format!("{name} was removed")));
            }
            if now_ms() >= deadline {
                return Err(CloudError(
                    "the node did not come online before its 30-minute link claim expired (its \
                     install failed or was too slow, or its theuth release predates phone \
                     pairing); destroy it and create it again"
                        .into(),
                ));
            }
            let elapsed = u32::try_from(started.elapsed().as_secs()).unwrap_or(u32::MAX);
            let frac = (f64::from(elapsed) / f64::from(install_secs.max(1))).min(0.95);
            self.progress(
                name,
                Some(Stage::Install),
                "Waiting for the node to install theuth and come online",
                Some(30 + (frac * 60.0) as u8),
                Some(install_secs.saturating_sub(elapsed).max(10)),
            );
            tokio::time::sleep(self.pace.poll).await;
        };
        self.update(name, |r| {
            if let Some(l) = r.link.as_mut() {
                l.engine = Some(engine.clone());
                l.engine_name = Some(engine_name.clone());
            }
            r.stage = Some(Stage::Join);
        })?;
        self.update(name, |r| {
            r.state = NodeState::Ready;
            r.stage = Some(Stage::Ready);
            r.error = None;
        })?;
        self.progress(
            name,
            Some(Stage::Ready),
            "Online — connecting this device",
            Some(100),
            Some(0),
        );
        Ok(())
    }

    /// `cloud_link_pairing`: the pairing string for the node's engine (RPC answer only).
    pub async fn link_pairing(&self, name: &str) -> Result<Zeroizing<String>> {
        let rec = self
            .records
            .get(name)?
            .ok_or_else(|| CloudError(format!("no cloud node named {name}")))?;
        let link = rec
            .link
            .ok_or_else(|| CloudError(format!("{name} joined the mesh; it pairs no device")))?;
        if link.paired {
            return Err(CloudError(format!(
                "this device already paired with {name}"
            )));
        }
        let engine = link
            .engine
            .ok_or_else(|| CloudError(format!("{name} is not online yet")))?;
        let claim = self.load_claim(name)?;
        if !claim.live(now_ms()) {
            self.forget_claim(name).await;
            return Err(CloudError(format!(
                "the one-time link claim of {name} expired; destroy it and create it again"
            )));
        }
        let relay = claim
            .relay
            .clone()
            .unwrap_or_else(|| wss_of(&self.creds.relay));
        claim
            .pairing(&relay, &engine, link.engine_name.as_deref().unwrap_or(name))
            .ok_or_else(|| CloudError(format!("{name} answered with a malformed link identity")))
    }

    /// `cloud_link_paired`: the app paired; forget the PSK, mark the node.
    pub async fn link_paired(&self, name: &str) -> Result<CloudNode> {
        self.forget_claim(name).await;
        let rec = self.update(name, |r| {
            if let Some(l) = r.link.as_mut() {
                l.paired = true;
            }
        })?;
        Ok(view(&rec, None, None))
    }
}
