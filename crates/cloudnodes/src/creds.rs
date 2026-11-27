//! Provider credentials (design doc 224 §3, §13.3): vault rows `cloud/digitalocean`,
//! `cloud/fly` holding [`StoredCred`] as JSON, written with `set_if_version`; "Sign in with
//! DigitalOcean" through theuth-relay's code exchange (PKCE-style: the relay hands the tokens
//! only to whoever holds the verifier), and refresh of an expired OAuth token through the relay.
//! Nothing here logs a token or puts one in a URL, an event or an error.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::Digest as _;
use zeroize::{Zeroize as _, Zeroizing};

use crate::providers::{CloudError, HttpRequest, Result, Transport};
use crate::types::{CredentialSource, ProviderKind};

/// Refresh this long before the access token expires.
const REFRESH_MARGIN_MS: u64 = 120_000;

/// The vault row of `kind` (`None` for providers that store nothing).
pub fn row(kind: ProviderKind) -> Option<String> {
    match kind {
        ProviderKind::Digitalocean => Some(format!("{}digitalocean", secrets::CLOUD_PREFIX)),
        ProviderKind::Fly => Some(format!("{}fly", secrets::CLOUD_PREFIX)),
        ProviderKind::Kubernetes | ProviderKind::Ssh | ProviderKind::Lightsail => None,
    }
}

/// One stored credential. Zeroed on drop; `Debug` shows no token.
#[derive(Clone, Serialize, Deserialize)]
pub struct StoredCred {
    /// The API token (OAuth access token, pasted or imported token).
    pub access_token: String,
    /// OAuth only: redeemed at the relay for a fresh access token.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    /// OAuth only: when `access_token` stops working (Unix ms).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at_ms: Option<u64>,
    /// How it got here.
    pub source: CredentialSource,
    /// Fly.io organization slug (`None`: `personal`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org: Option<String>,
}

impl Drop for StoredCred {
    fn drop(&mut self) {
        self.access_token.zeroize();
        if let Some(t) = self.refresh_token.as_mut() {
            t.zeroize();
        }
    }
}

impl std::fmt::Debug for StoredCred {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StoredCred")
            .field("source", &self.source)
            .field("expires_at_ms", &self.expires_at_ms)
            .field("org", &self.org)
            .finish_non_exhaustive()
    }
}

impl StoredCred {
    /// A copy of the access token for a provider client.
    pub fn token(&self) -> Zeroizing<String> {
        Zeroizing::new(self.access_token.clone())
    }

    /// An OAuth token at or past its refresh point that has a refresh token.
    pub fn needs_refresh(&self, now_ms: u64) -> bool {
        self.refresh_token.is_some()
            && self
                .expires_at_ms
                .is_some_and(|t| t <= now_ms.saturating_add(REFRESH_MARGIN_MS))
    }

    /// From a DigitalOcean token response (`access_token`, `refresh_token`, `expires_in` s).
    pub fn from_token_response(v: &Value, now_ms: u64) -> Result<Self> {
        let access = v["access_token"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| CloudError("the sign-in answered without an access token".into()))?;
        Ok(Self {
            access_token: access.to_owned(),
            refresh_token: v["refresh_token"].as_str().map(str::to_owned),
            expires_at_ms: v["expires_in"]
                .as_u64()
                .map(|s| now_ms.saturating_add(s.saturating_mul(1000))),
            source: CredentialSource::Oauth,
            org: None,
        })
    }
}

/// The vault seam (the real one is `secrets::Vault`; tests use a map).
#[async_trait]
pub trait SecretStore: Send + Sync {
    /// Current version and value of `name`.
    fn get_versioned(&self, name: &str) -> Option<(u32, Zeroizing<String>)>;
    /// `Ok(false)`: the current version is not `expected` (someone else wrote first).
    async fn set_if_version(&self, name: &str, value: &str, expected: Option<u32>) -> Result<bool>;
    /// Remove `name` (no error when it is absent).
    async fn delete(&self, name: &str) -> Result<()>;
}

#[async_trait]
impl SecretStore for secrets::Vault {
    fn get_versioned(&self, name: &str) -> Option<(u32, Zeroizing<String>)> {
        secrets::Vault::get_versioned(self, name)
    }

    async fn set_if_version(&self, name: &str, value: &str, expected: Option<u32>) -> Result<bool> {
        secrets::Vault::set_if_version(self, name, value, "cloudnodes", expected)
            .await
            .map(|r| r.is_some())
            .map_err(CloudError)
    }

    async fn delete(&self, name: &str) -> Result<()> {
        match secrets::Vault::delete(self, name).await {
            Ok(()) => Ok(()),
            // Already gone.
            Err(_) if self.get_versioned(name).is_none() => Ok(()),
            Err(e) => Err(CloudError(e)),
        }
    }
}

/// Read, write and refresh the `cloud/*` rows.
#[derive(Clone)]
pub struct Creds {
    /// The vault.
    pub store: Arc<dyn SecretStore>,
    /// Where relay calls go.
    pub http: Arc<dyn Transport>,
    /// theuth-relay's https origin (`https://relay.theuth.io`).
    pub relay: String,
}

impl Creds {
    /// The stored credential and its version.
    pub fn load(&self, kind: ProviderKind) -> Result<Option<(u32, StoredCred)>> {
        let Some(row) = row(kind) else {
            return Ok(None);
        };
        let Some((v, value)) = self.store.get_versioned(&row) else {
            return Ok(None);
        };
        serde_json::from_str(&value)
            .map(|c| Some((v, c)))
            .map_err(|_| {
                CloudError(format!(
                    "the stored {row} credential is unreadable; sign in again"
                ))
            })
    }

    /// Replace the credential (whatever version is there).
    pub async fn save(&self, kind: ProviderKind, cred: &StoredCred) -> Result<()> {
        let row = row(kind).ok_or_else(|| CloudError(format!("{kind:?} stores no credential")))?;
        let value =
            Zeroizing::new(serde_json::to_string(cred).map_err(|e| CloudError(e.to_string()))?);
        for _ in 0..3 {
            let expected = self.store.get_versioned(&row).map(|(v, _)| v);
            if self.store.set_if_version(&row, &value, expected).await? {
                return Ok(());
            }
        }
        Err(CloudError(format!("{row}: concurrent writes; try again")))
    }

    /// Delete the row of `kind`.
    pub async fn forget(&self, kind: ProviderKind) -> Result<()> {
        match row(kind) {
            Some(row) => self.store.delete(&row).await,
            None => Ok(()),
        }
    }

    /// A usable credential: refreshed through the relay when it is an OAuth token near expiry.
    pub async fn current(&self, kind: ProviderKind, now_ms: u64) -> Result<Option<StoredCred>> {
        let Some((v, cred)) = self.load(kind)? else {
            return Ok(None);
        };
        if kind != ProviderKind::Digitalocean || !cred.needs_refresh(now_ms) {
            return Ok(Some(cred));
        }
        let refresh = Zeroizing::new(cred.refresh_token.clone().unwrap_or_default());
        let resp = self
            .http
            .send(HttpRequest {
                method: "POST".into(),
                url: format!("{}/oauth/digitalocean/refresh", self.relay),
                json: Some(json!({ "refresh_token": refresh.as_str() })),
                ..HttpRequest::default()
            })
            .await
            .map_err(|e| CloudError(format!("refreshing the DigitalOcean sign-in: {e:#}")))?;
        if resp.status != 200 {
            return Err(CloudError(format!(
                "the DigitalOcean sign-in expired and could not be refreshed (HTTP {}); sign in again",
                resp.status
            )));
        }
        let mut fresh = StoredCred::from_token_response(&resp.json()?, now_ms)?;
        if fresh.refresh_token.is_none() {
            fresh.refresh_token = cred.refresh_token.clone();
        }
        let row = row(kind).unwrap_or_default();
        let value =
            Zeroizing::new(serde_json::to_string(&fresh).map_err(|e| CloudError(e.to_string()))?);
        if !self.store.set_if_version(&row, &value, Some(v)).await? {
            // Another writer (a refresh on another engine, a new sign-in) was first: use theirs.
            return Ok(self.load(kind)?.map(|(_, c)| c));
        }
        Ok(Some(fresh))
    }
}

/// One "Sign in with DigitalOcean" attempt: `state` names it at the relay, `verifier` redeems it.
pub struct OauthAttempt {
    /// Names the attempt at the relay (visible in the browser's URL).
    pub state: String,
    /// Proves this engine started it; never leaves the engine but in the redeem body.
    pub verifier: Zeroizing<String>,
    /// `b64url(sha256(verifier))`, sent with the start URL.
    pub challenge: String,
}

impl OauthAttempt {
    /// Fresh random state and verifier.
    pub fn fresh() -> Self {
        let state: [u8; 32] = rand::random();
        let verifier: [u8; 32] = rand::random();
        let verifier = Zeroizing::new(URL_SAFE_NO_PAD.encode(verifier));
        Self {
            state: URL_SAFE_NO_PAD.encode(state),
            challenge: challenge(&verifier),
            verifier,
        }
    }

    /// What the system browser opens.
    pub fn start_url(&self, relay: &str) -> String {
        format!(
            "{relay}/oauth/digitalocean/start?state={}&challenge={}",
            self.state, self.challenge
        )
    }
}

/// `b64url(sha256(verifier))`, no padding.
pub fn challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(sha2::Sha256::digest(verifier.as_bytes()))
}

/// Polls to tolerate `404 Not Found` before seeing a `202` from the relay (~30 s at the default
/// 2 s cadence), so polling ahead of the browser opening `/start` does not abort against an older
/// relay.
const NOT_FOUND_GRACE_POLLS: u32 = 15;

/// Poll `{relay}/oauth/digitalocean/redeem` every `every` until the tokens arrive (200), the
/// attempt is refused (403/410/other, or 404 after pending/grace window) or `tries` polls answered
/// pending (202).
pub async fn redeem(
    http: &dyn Transport,
    relay: &str,
    attempt: &OauthAttempt,
    every: Duration,
    tries: u32,
    now_ms: impl Fn() -> u64,
) -> Result<StoredCred> {
    let mut seen_pending = false;
    for i in 0..tries {
        if i > 0 && !every.is_zero() {
            tokio::time::sleep(every).await;
        }
        let resp = match http
            .send(HttpRequest {
                method: "POST".into(),
                url: format!("{relay}/oauth/digitalocean/redeem"),
                json: Some(
                    json!({ "state": attempt.state, "verifier": attempt.verifier.as_str() }),
                ),
                ..HttpRequest::default()
            })
            .await
        {
            Ok(r) => r,
            // The relay unreachable for a moment: keep polling.
            Err(e) => {
                tracing::debug!("cloudnodes: oauth redeem: {e:#}");
                continue;
            }
        };
        match resp.status {
            200 => return StoredCred::from_token_response(&resp.json()?, now_ms()),
            202 => {
                seen_pending = true;
                continue;
            }
            403 => {
                return Err(CloudError(
                    "the sign-in was refused (verifier mismatch)".into(),
                ));
            }
            404 if !seen_pending && i < NOT_FOUND_GRACE_POLLS => continue,
            404 | 410 => return Err(CloudError("the sign-in expired; try again".into())),
            s => {
                return Err(CloudError(format!(
                    "the sign-in failed at the relay (HTTP {s})"
                )));
            }
        }
    }
    Err(CloudError(
        "the sign-in was not completed within 10 minutes".into(),
    ))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::providers::http::testing::FakeTransport;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    #[derive(Default)]
    pub struct MemStore(pub Mutex<HashMap<String, (u32, String)>>);

    #[async_trait]
    impl SecretStore for MemStore {
        fn get_versioned(&self, name: &str) -> Option<(u32, Zeroizing<String>)> {
            self.0
                .lock()
                .get(name)
                .map(|(v, s)| (*v, Zeroizing::new(s.clone())))
        }
        async fn set_if_version(
            &self,
            name: &str,
            value: &str,
            expected: Option<u32>,
        ) -> Result<bool> {
            let mut m = self.0.lock();
            let cur = m.get(name).map(|(v, _)| *v);
            if cur != expected {
                return Ok(false);
            }
            m.insert(name.into(), (cur.unwrap_or(0) + 1, value.into()));
            Ok(true)
        }
        async fn delete(&self, name: &str) -> Result<()> {
            self.0.lock().remove(name);
            Ok(())
        }
    }

    #[test]
    fn challenge_is_sha256_b64url() {
        // RFC 7636 appendix B.
        assert_eq!(
            challenge("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk"),
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        let a = OauthAttempt::fresh();
        assert_eq!(a.challenge, challenge(&a.verifier));
        let url = a.start_url("https://relay.theuth.io");
        assert!(url.starts_with("https://relay.theuth.io/oauth/digitalocean/start?state="));
        assert!(
            !url.contains(a.verifier.as_str()),
            "the verifier never leaves the engine"
        );
    }

    #[tokio::test]
    async fn redeem_polls_pending_then_takes_the_tokens() {
        let http = FakeTransport::new();
        http.once("POST", "/oauth/digitalocean/redeem", 202, json!({"pending": true}))
            .once("POST", "/oauth/digitalocean/redeem", 202, json!({"pending": true}))
            .on(
                "POST",
                "/oauth/digitalocean/redeem",
                200,
                json!({"access_token": "doo_v1_a", "refresh_token": "dor_v1_r", "expires_in": 2_592_000}),
            );
        let a = OauthAttempt::fresh();
        let c = redeem(&*http, "https://relay.test", &a, Duration::ZERO, 10, || {
            1_000
        })
        .await
        .unwrap();
        assert_eq!(c.access_token, "doo_v1_a");
        assert_eq!(c.refresh_token.as_deref(), Some("dor_v1_r"));
        assert_eq!(c.expires_at_ms, Some(1_000 + 2_592_000_000));
        assert_eq!(c.source, CredentialSource::Oauth);
        let reqs = http.requests();
        assert_eq!(reqs.len(), 3);
        assert_eq!(reqs[0].json.as_ref().unwrap()["state"], a.state.as_str());
        assert_eq!(
            reqs[0].json.as_ref().unwrap()["verifier"],
            a.verifier.as_str()
        );
        assert!(reqs[0].bearer.is_none());
        assert!(
            format!("{c:?}").find("doo_v1").is_none(),
            "Debug hides tokens"
        );

        // Expired at the relay; never completed.
        let gone = FakeTransport::new();
        gone.on("POST", "/redeem", 410, json!({}));
        let e = redeem(&*gone, "https://relay.test", &a, Duration::ZERO, 10, || 0).await;
        assert!(e.unwrap_err().0.contains("expired"));
        let pending = FakeTransport::new();
        pending.on("POST", "/redeem", 202, json!({"pending": true}));
        let e = redeem(&*pending, "https://relay.test", &a, Duration::ZERO, 3, || 0).await;
        assert!(e.unwrap_err().0.contains("10 minutes"));
        assert_eq!(pending.requests().len(), 3);
    }

    #[tokio::test]
    async fn redeem_tolerates_initial_404_before_start_but_fails_on_410_and_post_pending_404() {
        let a = OauthAttempt::fresh();

        // Initial 404s before the browser opens `/start` are tolerated and succeed once 202/200 arrive.
        let http = FakeTransport::new();
        http.once(
            "POST",
            "/oauth/digitalocean/redeem",
            404,
            json!({"error": "unknown sign-in"}),
        )
        .once(
            "POST",
            "/oauth/digitalocean/redeem",
            404,
            json!({"error": "unknown sign-in"}),
        )
        .once(
            "POST",
            "/oauth/digitalocean/redeem",
            202,
            json!({"pending": true}),
        )
        .on(
            "POST",
            "/oauth/digitalocean/redeem",
            200,
            json!({"access_token": "doo_v1_ok", "refresh_token": "dor_v1_ok", "expires_in": 3600}),
        );
        let cred = redeem(&*http, "https://relay.test", &a, Duration::ZERO, 20, || 0)
            .await
            .unwrap();
        assert_eq!(cred.access_token, "doo_v1_ok");
        assert_eq!(http.requests().len(), 4);

        // 410 still fails immediately on the first poll.
        let gone = FakeTransport::new();
        gone.on(
            "POST",
            "/oauth/digitalocean/redeem",
            410,
            json!({"error": "sign-in expired"}),
        );
        let err = redeem(&*gone, "https://relay.test", &a, Duration::ZERO, 20, || 0)
            .await
            .unwrap_err();
        assert_eq!(err.0, "the sign-in expired; try again");
        assert_eq!(gone.requests().len(), 1);

        // 404 AFTER seeing 202 fails immediately (not tolerated).
        let after_pending = FakeTransport::new();
        after_pending
            .once(
                "POST",
                "/oauth/digitalocean/redeem",
                202,
                json!({"pending": true}),
            )
            .on(
                "POST",
                "/oauth/digitalocean/redeem",
                404,
                json!({"error": "unknown sign-in"}),
            );
        let err = redeem(
            &*after_pending,
            "https://relay.test",
            &a,
            Duration::ZERO,
            20,
            || 0,
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, "the sign-in expired; try again");
        assert_eq!(after_pending.requests().len(), 2);

        // Persistent 404s past the grace window eventually fail as expired.
        let never_started = FakeTransport::new();
        never_started.on(
            "POST",
            "/oauth/digitalocean/redeem",
            404,
            json!({"error": "unknown sign-in"}),
        );
        let err = redeem(
            &*never_started,
            "https://relay.test",
            &a,
            Duration::ZERO,
            NOT_FOUND_GRACE_POLLS + 5,
            || 0,
        )
        .await
        .unwrap_err();
        assert_eq!(err.0, "the sign-in expired; try again");
        assert_eq!(
            never_started.requests().len(),
            (NOT_FOUND_GRACE_POLLS + 1) as usize
        );
    }

    #[tokio::test]
    async fn an_expired_oauth_token_is_refreshed_through_the_relay() {
        let store = Arc::new(MemStore::default());
        let http = FakeTransport::new();
        http.on(
            "POST",
            "https://relay.test/oauth/digitalocean/refresh",
            200,
            json!({"access_token": "new", "refresh_token": "r2", "expires_in": 100}),
        );
        let creds = Creds {
            store: store.clone(),
            http: http.clone(),
            relay: "https://relay.test".into(),
        };
        let mut c = StoredCred::from_token_response(
            &json!({"access_token": "old", "refresh_token": "r1", "expires_in": 3600}),
            0,
        )
        .unwrap();
        creds.save(ProviderKind::Digitalocean, &c).await.unwrap();
        // Still fresh: no request.
        let got = creds
            .current(ProviderKind::Digitalocean, 1_000)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.access_token, "old");
        assert!(http.requests().is_empty());
        // Past expiry: refreshed, stored, the old refresh token sent once.
        let now = 3_600_000;
        let got = creds
            .current(ProviderKind::Digitalocean, now)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.access_token, "new");
        assert_eq!(got.expires_at_ms, Some(now + 100_000));
        let reqs = http.requests();
        assert_eq!(reqs.len(), 1);
        assert_eq!(reqs[0].json.as_ref().unwrap()["refresh_token"], "r1");
        let (v, stored) = creds.load(ProviderKind::Digitalocean).unwrap().unwrap();
        assert_eq!((v, stored.access_token.as_str()), (2, "new"));
        assert_eq!(stored.refresh_token.as_deref(), Some("r2"));
        // A pasted token never refreshes.
        c.source = CredentialSource::Pasted;
        c.refresh_token = None;
        creds.save(ProviderKind::Fly, &c).await.unwrap();
        assert!(
            creds
                .current(ProviderKind::Fly, u64::MAX / 2)
                .await
                .unwrap()
                .is_some()
        );
        assert_eq!(http.requests().len(), 1);
        creds.forget(ProviderKind::Fly).await.unwrap();
        assert!(creds.load(ProviderKind::Fly).unwrap().is_none());
    }
}
