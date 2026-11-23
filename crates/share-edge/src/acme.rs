//! ACME (210 §6.0–§6.4): the `Ca` and `Validator` traits the job runner sees, the instant-acme impl, the account
//! file, `acme register`, and the CAA rule (§8.2).

use std::fmt;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context as _, anyhow, bail};
use futures::FutureExt as _;
use futures::future::BoxFuture;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, CertificateIdentifier, ChallengeType,
    ExternalAccountKey, Identifier, NewAccount, NewOrder, OrderStatus, RetryPolicy,
    RevocationReason, RevocationRequest,
};
use rustls::pki_types::CertificateDer;
use rustls::pki_types::pem::PemObject as _;
use serde::Deserialize;

use crate::config::{Config, Secrets};

/// Why a certificate is revoked (§6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// Release, inactivity expiry, a ban.
    CessationOfOperation,
    /// A rotation.
    Superseded,
    /// A removal marked compromised, an abuse report.
    KeyCompromise,
}

/// An ARI suggested window, Unix ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    /// Window start.
    pub start_ms: u64,
    /// Window end.
    pub end_ms: u64,
}

/// Per-authorization work of one validation method (§6.0).
pub trait Validator: Send + Sync {
    /// `dns-01` (P1) or `dns-persist-01` (P2).
    fn challenge(&self) -> &'static str;
    /// Whether authorizations may run at once; `dns-01` over zone-update: no (§6.1).
    fn parallel(&self) -> bool;
    /// Publish `value` for `ident` (`sky.tun.zip` or `*.sky.tun.zip`) and wait until it can be validated.
    fn prepare<'a>(&'a self, ident: &'a str, value: &'a str) -> BoxFuture<'a, anyhow::Result<()>>;
    /// Remove what `prepare` published.
    fn cleanup<'a>(&'a self, ident: &'a str) -> BoxFuture<'a, anyhow::Result<()>>;
}

/// One configured CA (§6.0); an instant-acme `Account` inside, or a fake in tests.
pub trait Ca: Send + Sync + fmt::Debug {
    /// `letsencrypt` · `gts`.
    fn id(&self) -> &str;
    /// The account URI (CAA `accounturi`).
    fn account_uri(&self) -> &str;
    /// Order `names` (`[<n>.D, *.<n>.D]`), validate through `v`, finalize with `csr` (the engine's); the PEM
    /// chain. `replaces` is the previous chain of an ARI renewal or rotation.
    fn issue<'a>(
        &'a self,
        names: &'a [String; 2],
        csr: &'a [u8],
        replaces: Option<&'a str>,
        v: &'a dyn Validator,
    ) -> BoxFuture<'a, anyhow::Result<String>>;
    /// The ARI window of `chain_pem`, where offered.
    fn renewal_window<'a>(&'a self, chain_pem: &'a str) -> BoxFuture<'a, Option<Window>>;
    /// Revoke the leaf of `chain_pem`.
    fn revoke<'a>(&'a self, chain_pem: &'a str, reason: Reason) -> BoxFuture<'a, anyhow::Result<()>>;
}

/// The leaf of a PEM chain, DER.
pub fn leaf_der(chain_pem: &str) -> anyhow::Result<CertificateDer<'static>> {
    CertificateDer::pem_slice_iter(chain_pem.as_bytes())
        .next()
        .ok_or_else(|| anyhow!("empty chain"))?
        .map_err(|e| anyhow!("chain PEM: {e:?}"))
}

/// What the edge needs to know of a leaf certificate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeafInfo {
    /// Unix ms.
    pub not_before_ms: u64,
    /// Unix ms.
    pub not_after_ms: u64,
    /// sha256 of the leaf's SPKI, comparable with `check_csr`'s.
    pub spki: share_proto::SpkiHash,
    /// DNS SANs, lowercase.
    pub sans: Vec<String>,
}

/// Parse the leaf of `chain_pem`.
pub fn leaf_info(chain_pem: &str) -> anyhow::Result<LeafInfo> {
    use x509_parser::prelude::{FromDer as _, GeneralName, X509Certificate};
    let der = leaf_der(chain_pem)?;
    let (_, cert) = X509Certificate::from_der(der.as_ref()).map_err(|e| anyhow!("leaf: {e}"))?;
    let ms = |t: i64| u64::try_from(t).unwrap_or(0).saturating_mul(1000);
    let sans = cert
        .subject_alternative_name()
        .ok()
        .flatten()
        .map(|s| {
            s.value
                .general_names
                .iter()
                .filter_map(|n| match n {
                    GeneralName::DNSName(d) => Some(d.to_ascii_lowercase()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(LeafInfo {
        not_before_ms: ms(cert.validity().not_before.timestamp()),
        not_after_ms: ms(cert.validity().not_after.timestamp()),
        spki: share_proto::SpkiHash::of(cert.public_key().raw),
        sans,
    })
}

/// The instant-acme CA (`aws-lc-rs`, `time`, `x509-parser`).
pub struct AcmeCa {
    id: String,
    account: Account,
    profile: String,
}

impl fmt::Debug for AcmeCa {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AcmeCa")
            .field("id", &self.id)
            .field("account", &self.account.id())
            .finish_non_exhaustive()
    }
}

/// How long one authorization may stay `pending` after its challenge is ready.
const AUTH_TIMEOUT: Duration = Duration::from_secs(180);

impl AcmeCa {
    /// Load the account of `ca` from `<data_dir>/acme/<ca>.json`; `None` when never registered.
    pub async fn load(cfg: &Config, ca: &str) -> anyhow::Result<Option<AcmeCa>> {
        let path = cfg.account_path(ca);
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(e).with_context(|| format!("read {}", path.display())),
        };
        let creds: AccountCredentials =
            serde_json::from_slice(&bytes).with_context(|| format!("parse {}", path.display()))?;
        let account = Account::builder()?
            .from_credentials(creds)
            .await
            .with_context(|| format!("ACME account {}", path.display()))?;
        Ok(Some(AcmeCa {
            id: ca.to_string(),
            account,
            profile: cfg.acme.profile.clone(),
        }))
    }

    async fn wait_authorization(order: &mut instant_acme::Order, ident: &str) -> anyhow::Result<()> {
        let deadline = tokio::time::Instant::now() + AUTH_TIMEOUT;
        loop {
            tokio::time::sleep(Duration::from_secs(2)).await;
            // `refresh` drops the cached authorization states, so the stream below fetches them again.
            order.refresh().await?;
            let mut auths = order.authorizations();
            let mut status = None;
            while let Some(a) = auths.next().await {
                let a = a?;
                if a.identifier().to_string() == ident {
                    status = Some(a.status);
                    break;
                }
            }
            match status {
                Some(AuthorizationStatus::Valid) => return Ok(()),
                Some(AuthorizationStatus::Pending) if tokio::time::Instant::now() < deadline => {}
                other => bail!("authorization {ident}: {other:?}"),
            }
        }
    }

    async fn issue_inner(
        &self,
        names: &[String; 2],
        csr: &[u8],
        replaces: Option<&str>,
        v: &dyn Validator,
    ) -> anyhow::Result<String> {
        let ids = [Identifier::Dns(names[0].clone()), Identifier::Dns(names[1].clone())];
        let old = replaces.map(leaf_der).transpose()?;
        let mut new = NewOrder::new(&ids);
        if !self.profile.is_empty() {
            new = new.profile(&self.profile);
        }
        if let Some(der) = &old {
            new = new.replaces(CertificateIdentifier::try_from(der)?.into_owned());
        }
        let mut order = self.account.new_order(&new).await.context("newOrder")?;
        // One authorization at a time (§6.1): zone-update cannot hold two TXT values at one name.
        loop {
            order.refresh().await?;
            let mut current: Option<String> = None;
            {
                let mut auths = order.authorizations();
                while let Some(a) = auths.next().await {
                    let mut a = a?;
                    match a.status {
                        AuthorizationStatus::Valid => continue,
                        AuthorizationStatus::Pending => {}
                        other => bail!("authorization {}: {other:?}", a.identifier()),
                    }
                    let mut ch = a
                        .challenge(ChallengeType::Dns01)
                        .ok_or_else(|| anyhow!("no dns-01 challenge offered"))?;
                    let ident = ch.identifier().to_string();
                    let value = ch.key_authorization().dns_value();
                    if let Err(e) = v.prepare(&ident, &value).await {
                        let _ = v.cleanup(&ident).await;
                        return Err(e);
                    }
                    if let Err(e) = ch.set_ready().await {
                        let _ = v.cleanup(&ident).await;
                        return Err(e.into());
                    }
                    current = Some(ident);
                    break;
                }
            }
            let Some(ident) = current else { break };
            let r = Self::wait_authorization(&mut order, &ident).await;
            v.cleanup(&ident).await?;
            r?;
        }
        let policy = RetryPolicy::new().timeout(Duration::from_secs(120));
        match order.poll_ready(&policy).await? {
            OrderStatus::Ready => {}
            s => bail!("order not ready: {s:?}"),
        }
        order.finalize_csr(csr).await.context("finalize")?;
        Ok(order.poll_certificate(&policy).await?)
    }
}

fn unix_ms(t: time::OffsetDateTime) -> u64 {
    u64::try_from(t.unix_timestamp()).unwrap_or(0).saturating_mul(1000)
}

impl Ca for AcmeCa {
    fn id(&self) -> &str {
        &self.id
    }

    fn account_uri(&self) -> &str {
        self.account.id()
    }

    fn issue<'a>(
        &'a self,
        names: &'a [String; 2],
        csr: &'a [u8],
        replaces: Option<&'a str>,
        v: &'a dyn Validator,
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        self.issue_inner(names, csr, replaces, v).boxed()
    }

    fn renewal_window<'a>(&'a self, chain_pem: &'a str) -> BoxFuture<'a, Option<Window>> {
        async move {
            let der = leaf_der(chain_pem).ok()?;
            let id = CertificateIdentifier::try_from(&der).ok()?;
            match self.account.renewal_info(&id).await {
                Ok((info, _)) => Some(Window {
                    start_ms: unix_ms(info.suggested_window.start),
                    end_ms: unix_ms(info.suggested_window.end),
                }),
                Err(e) => {
                    tracing::debug!("ARI: {e}");
                    None
                }
            }
        }
        .boxed()
    }

    fn revoke<'a>(&'a self, chain_pem: &'a str, reason: Reason) -> BoxFuture<'a, anyhow::Result<()>> {
        async move {
            let der = leaf_der(chain_pem)?;
            let reason = match reason {
                Reason::CessationOfOperation => RevocationReason::CessationOfOperation,
                Reason::Superseded => RevocationReason::Superseded,
                Reason::KeyCompromise => RevocationReason::KeyCompromise,
            };
            self.account
                .revoke(&RevocationRequest {
                    certificate: &der,
                    reason: Some(reason),
                })
                .await?;
            Ok(())
        }
        .boxed()
    }
}

/// The CAA issuer domain of `ca`.
pub fn issuer_domain(ca: &str) -> &'static str {
    match ca {
        "gts" => "pki.goog",
        _ => "letsencrypt.org",
    }
}

/// The CAA `validationmethods` of `ca` under `cfg`.
pub fn validation_method(cfg: &Config, ca: &str) -> String {
    match ca {
        "gts" => cfg.acme.cas.gts.validation.clone(),
        _ => "dns-01".into(),
    }
}

/// The two CAA lines of §8.2 for an account.
pub fn caa_lines(domain: &str, ca: &str, account_uri: &str, method: &str) -> [String; 2] {
    let v = format!("{}; accounturi={account_uri}; validationmethods={method}", issuer_domain(ca));
    [
        format!("{domain}. CAA 0 issue \"{v}\""),
        format!("{domain}. CAA 0 issuewild \"{v}\""),
    ]
}

/// One CAA record: tag and value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caa {
    /// `issue`, `issuewild`, `iodef`, …
    pub tag: String,
    /// The value without quotes.
    pub value: String,
}

/// Resolves `D`'s CAA set; a trait so tests never touch DNS.
pub trait CaaLookup: Send + Sync + fmt::Debug {
    /// Every CAA record at `domain`.
    fn caa<'a>(&'a self, domain: &'a str) -> BoxFuture<'a, anyhow::Result<Vec<Caa>>>;
}

/// Whether `records` hold `issue` and `issuewild` for `issuer` naming `account_uri` and allowing `method`.
pub fn caa_ok(records: &[Caa], issuer: &str, account_uri: &str, method: &str) -> bool {
    let matches = |r: &Caa| {
        let mut parts = r.value.split(';').map(str::trim);
        if parts.next() != Some(issuer) {
            return false;
        }
        let mut acct = false;
        let mut meth = false;
        for p in parts {
            if let Some((k, v)) = p.split_once('=') {
                match k.trim() {
                    "accounturi" => acct = v.trim() == account_uri,
                    "validationmethods" => meth = v.split(',').any(|m| m.trim() == method),
                    _ => {}
                }
            }
        }
        acct && meth
    };
    ["issue", "issuewild"]
        .iter()
        .all(|tag| records.iter().any(|r| r.tag.eq_ignore_ascii_case(tag) && matches(r)))
}

/// Check every CA's CAA lines at `domain`; an error naming the missing lines.
pub async fn require_caa(
    lookup: &dyn CaaLookup,
    cfg: &Config,
    cas: &[Arc<dyn Ca>],
) -> anyhow::Result<()> {
    let records = lookup.caa(&cfg.domain).await.context("CAA lookup")?;
    for ca in cas {
        let method = validation_method(cfg, ca.id());
        if !caa_ok(&records, issuer_domain(ca.id()), ca.account_uri(), &method) {
            let [a, b] = caa_lines(&cfg.domain, ca.id(), ca.account_uri(), &method);
            bail!("require_caa: {} lacks the CAA lines of CA {}:\n  {a}\n  {b}", cfg.domain, ca.id());
        }
    }
    Ok(())
}

/// The system resolver (hickory).
#[derive(Debug)]
pub struct SystemCaa;

impl CaaLookup for SystemCaa {
    fn caa<'a>(&'a self, domain: &'a str) -> BoxFuture<'a, anyhow::Result<Vec<Caa>>> {
        async move {
            use hickory_resolver::proto::rr::{RData, RecordType};
            let resolver = hickory_resolver::TokioResolver::builder_tokio()?.build()?;
            let lookup = match resolver.lookup(format!("{domain}."), RecordType::CAA).await {
                Ok(l) => l,
                Err(e) if e.is_no_records_found() => return Ok(Vec::new()),
                Err(e) => return Err(e.into()),
            };
            Ok(lookup
                .answers()
                .iter()
                .filter_map(|r| match &r.data {
                    RData::CAA(c) => parse_caa_text(&c.to_string()),
                    _ => None,
                })
                .collect())
        }
        .boxed()
    }
}

/// `0 issue "letsencrypt.org; …"` → tag and value.
pub fn parse_caa_text(s: &str) -> Option<Caa> {
    let mut it = s.trim().splitn(3, char::is_whitespace);
    let _flags = it.next()?;
    let tag = it.next()?.to_string();
    let value = it.next()?.trim().trim_matches('"').to_string();
    Some(Caa { tag, value })
}

/// The EAB secret JSON (P2, GTS): `{"kid": …, "hmac_key": <base64url>}`.
#[derive(Deserialize)]
struct Eab {
    kid: String,
    hmac_key: String,
}

/// `theuth-share-edge acme register --ca <ca>`: create the account (EAB where configured), write
/// `<data_dir>/acme/<ca>.json` 0600, and return the account URI with the two CAA lines. An existing account
/// file is loaded, never overwritten.
pub async fn register(cfg: &Config, secrets: &Secrets, ca: &str) -> anyhow::Result<(String, [String; 2])> {
    let path = cfg.account_path(ca);
    let method = validation_method(cfg, ca);
    if let Some(existing) = AcmeCa::load(cfg, ca).await? {
        let uri = existing.account_uri().to_string();
        let lines = caa_lines(&cfg.domain, ca, &uri, &method);
        return Ok((uri, lines));
    }
    let (directory, eab) = match ca {
        "letsencrypt" => (cfg.acme.cas.letsencrypt.directory.clone(), None),
        "gts" => {
            let src = secrets
                .gts_eab
                .as_ref()
                .ok_or_else(|| anyhow!("GTS needs an EAB (`acme.cas.gts.eab_file` or env)"))?;
            let e: Eab = serde_json::from_str(&src.read()?).context("EAB JSON {kid, hmac_key}")?;
            let key = share_proto::b64::decode(&e.hmac_key).map_err(|_| anyhow!("EAB hmac_key: base64url"))?;
            (cfg.acme.cas.gts.directory.clone(), Some(ExternalAccountKey::new(e.kid, &key)))
        }
        other => bail!("unknown CA {other}"),
    };
    let contact: Vec<&str> = if cfg.acme.contact.is_empty() {
        Vec::new()
    } else {
        vec![cfg.acme.contact.as_str()]
    };
    let (account, creds) = Account::builder()?
        .create(
            &NewAccount {
                contact: &contact,
                terms_of_service_agreed: true,
                only_return_existing: false,
            },
            directory,
            eab.as_ref(),
        )
        .await
        .context("create ACME account")?;
    write_private(&path, &serde_json::to_vec(&creds)?)?;
    let uri = account.id().to_string();
    let lines = caa_lines(&cfg.domain, ca, &uri, &method);
    Ok((uri, lines))
}

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    if let Some(dir) = path.parent() {
        platform::files::private_dir(dir)?;
    }
    platform::files::create_private(path, bytes).with_context(|| format!("write {}", path.display()))
}

/// The load-or-fail of `run`: every enabled CA's account.
pub async fn load_cas(cfg: &Config) -> anyhow::Result<Vec<Arc<dyn Ca>>> {
    // TODO(210-P2): GTS (`cas.gts.enabled`) as a second CA with its `dns-persist-01` validator.
    let ca = cfg.acme.primary.as_str();
    let Some(acme) = AcmeCa::load(cfg, ca).await? else {
        bail!(
            "no ACME account for {ca} at {}: run `theuth-share-edge acme register --ca {ca}` first (§8.5)",
            cfg.account_path(ca).display()
        );
    };
    Ok(vec![Arc::new(acme)])
}

/// Unix ms of a `SystemTime`.
pub fn system_ms(t: SystemTime) -> u64 {
    t.duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caa_lines_round_trip_through_the_check() {
        let [issue, wild] = caa_lines("tun.zip", "letsencrypt", "https://acme/acct/1", "dns-01");
        assert_eq!(
            issue,
            "tun.zip. CAA 0 issue \"letsencrypt.org; accounturi=https://acme/acct/1; validationmethods=dns-01\""
        );
        let recs: Vec<Caa> = [issue, wild]
            .iter()
            .map(|l| parse_caa_text(l.split_once("CAA ").unwrap().1).unwrap())
            .collect();
        assert!(caa_ok(&recs, "letsencrypt.org", "https://acme/acct/1", "dns-01"));
        assert!(!caa_ok(&recs, "letsencrypt.org", "https://acme/acct/2", "dns-01"));
        assert!(!caa_ok(&recs[..1], "letsencrypt.org", "https://acme/acct/1", "dns-01"));
        let loose = [Caa {
            tag: "issue".into(),
            value: "letsencrypt.org".into(),
        }];
        assert!(!caa_ok(&loose, "letsencrypt.org", "https://acme/acct/1", "dns-01"));
    }
}
