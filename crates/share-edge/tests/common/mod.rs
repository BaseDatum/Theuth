//! Fakes of the certificate backends: an in-memory zone, a CA that signs the CSR it is given after driving the
//! validator like a real one, and a CAA resolver. No network.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use futures::FutureExt as _;
use futures::future::BoxFuture;
use share_edge::acme::{Ca, Caa, CaaLookup, Reason, Validator, Window, caa_lines, parse_caa_text};
use share_edge::certs::Backends;
use share_edge::dns::{DnsZone, Rtype, TxtProbe};

/// The in-memory zone; `get` refuses a host with two records, as zone-update does.
#[derive(Debug, Default)]
pub struct FakeZone {
    pub recs: Mutex<BTreeMap<(Rtype, String), Vec<String>>>,
    /// Most TXT records that existed at once, zone-wide.
    pub max_txt: AtomicUsize,
    /// `create` panics for this host.
    pub panic_on: Mutex<Option<String>>,
    /// Every fqdn a TXT probe asked for.
    pub probes: Mutex<Vec<String>>,
}

impl FakeZone {
    pub fn value(&self, t: Rtype, host: &str) -> Option<Vec<String>> {
        self.recs.lock().unwrap().get(&(t, host.to_string())).cloned()
    }

    pub fn put(&self, t: Rtype, host: &str, v: &str) {
        self.recs
            .lock()
            .unwrap()
            .entry((t, host.to_string()))
            .or_default()
            .push(v.to_string());
    }

    pub fn hosts(&self) -> Vec<(Rtype, String)> {
        self.recs.lock().unwrap().keys().cloned().collect()
    }
}

/// The fake zone is its own authoritative server: a probe sees exactly what is in the map.
impl TxtProbe for FakeZone {
    fn wait<'a>(
        &'a self,
        zone: &'a str,
        fqdn: &'a str,
        value: &'a str,
        _deadline: std::time::Duration,
    ) -> BoxFuture<'a, anyhow::Result<()>> {
        async move {
            self.probes.lock().unwrap().push(fqdn.to_string());
            let host = fqdn.trim_end_matches('.').strip_suffix(&format!(".{zone}")).unwrap_or(fqdn);
            anyhow::ensure!(
                self.value(Rtype::Txt, host).is_some_and(|v| v.iter().any(|x| x == value)),
                "{fqdn} not visible"
            );
            Ok(())
        }
        .boxed()
    }
}

impl DnsZone for FakeZone {
    fn get(&self, rtype: Rtype, host: String) -> BoxFuture<'_, anyhow::Result<Option<String>>> {
        async move {
            match self.value(rtype, &host) {
                None => Ok(None),
                Some(v) if v.len() == 1 => Ok(Some(v[0].clone())),
                Some(v) => anyhow::bail!("{} records at {host}", v.len()),
            }
        }
        .boxed()
    }

    fn create(&self, rtype: Rtype, host: String, value: String) -> BoxFuture<'_, anyhow::Result<()>> {
        async move {
            if self.panic_on.lock().unwrap().as_deref() == Some(host.as_str()) {
                panic!("fake provider exploded on {host}");
            }
            self.put(rtype, &host, &value);
            let txt: usize = self
                .recs
                .lock()
                .unwrap()
                .iter()
                .filter(|((t, _), _)| *t == Rtype::Txt)
                .map(|(_, v)| v.len())
                .sum();
            self.max_txt.fetch_max(txt, Ordering::SeqCst);
            Ok(())
        }
        .boxed()
    }

    fn delete_all(&self, rtype: Rtype, host: String) -> BoxFuture<'_, anyhow::Result<()>> {
        async move {
            self.recs.lock().unwrap().remove(&(rtype, host));
            Ok(())
        }
        .boxed()
    }
}

pub const ACCOUNT: &str = "https://acme.test/acct/1";

/// A CA that validates through the given validator (checking the TXT is in the zone), then signs the CSR.
#[derive(Debug)]
pub struct FakeCa {
    zone: Arc<FakeZone>,
    issuer: rcgen::Issuer<'static, rcgen::KeyPair>,
    pub issued: AtomicUsize,
    pub replaces: Mutex<Vec<Option<String>>>,
    pub revoked: Mutex<Vec<(String, Reason)>>,
    pub ari: Mutex<Option<Window>>,
    /// `(started, go)`: the next `issue` signals `started` after validation and waits for `go` before signing.
    pub gate: Mutex<Option<(tokio::sync::oneshot::Sender<()>, tokio::sync::oneshot::Receiver<()>)>>,
    /// The next this many `revoke` calls fail.
    pub fail_revokes: AtomicUsize,
}

impl FakeCa {
    pub fn new(zone: Arc<FakeZone>) -> FakeCa {
        let mut params = rcgen::CertificateParams::new(Vec::<String>::new()).unwrap();
        params.is_ca = rcgen::IsCa::Ca(rcgen::BasicConstraints::Unconstrained);
        let key = rcgen::KeyPair::generate().unwrap();
        FakeCa {
            zone,
            issuer: rcgen::Issuer::new(params, key),
            issued: AtomicUsize::new(0),
            replaces: Mutex::new(Vec::new()),
            revoked: Mutex::new(Vec::new()),
            ari: Mutex::new(None),
            gate: Mutex::new(None),
            fail_revokes: AtomicUsize::new(0),
        }
    }
}

impl Ca for FakeCa {
    fn id(&self) -> &str {
        "letsencrypt"
    }

    fn account_uri(&self) -> &str {
        ACCOUNT
    }

    fn issue<'a>(
        &'a self,
        names: &'a [String; 2],
        csr: &'a [u8],
        replaces: Option<&'a str>,
        v: &'a dyn Validator,
    ) -> BoxFuture<'a, anyhow::Result<String>> {
        async move {
            for n in names {
                let value = format!("tok-{n}");
                v.prepare(n, &value).await?;
                let base = n.strip_prefix("*.").unwrap_or(n);
                let label = base.strip_suffix(".tun.zip").unwrap();
                let host = format!("_acme-challenge.{label}");
                anyhow::ensure!(
                    self.zone.value(Rtype::Txt, &host) == Some(vec![value.clone()]),
                    "TXT for {n} not in the zone"
                );
                v.cleanup(n).await?;
            }
            let gate = self.gate.lock().unwrap().take();
            if let Some((started, go)) = gate {
                let _ = started.send(());
                let _ = go.await;
            }
            let der = rustls::pki_types::CertificateSigningRequestDer::from(csr.to_vec());
            let mut req = rcgen::CertificateSigningRequestParams::from_der(&der)?;
            let now = time::OffsetDateTime::now_utc();
            req.params.not_before = now - time::Duration::hours(1);
            req.params.not_after = now + time::Duration::days(90);
            let cert = req.signed_by(&self.issuer)?;
            self.issued.fetch_add(1, Ordering::SeqCst);
            self.replaces.lock().unwrap().push(replaces.map(str::to_string));
            Ok(cert.pem())
        }
        .boxed()
    }

    fn renewal_window<'a>(&'a self, _chain_pem: &'a str) -> BoxFuture<'a, Option<Window>> {
        async move { *self.ari.lock().unwrap() }.boxed()
    }

    fn revoke<'a>(&'a self, chain_pem: &'a str, reason: Reason) -> BoxFuture<'a, anyhow::Result<()>> {
        async move {
            if self
                .fail_revokes
                .try_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
            {
                anyhow::bail!("CA unavailable");
            }
            self.revoked.lock().unwrap().push((chain_pem.to_string(), reason));
            Ok(())
        }
        .boxed()
    }
}

/// CAA answers.
#[derive(Debug, Default)]
pub struct FakeCaa(pub Vec<Caa>);

impl FakeCaa {
    /// The two lines `acme register` prints for the fake account.
    pub fn registered() -> FakeCaa {
        FakeCaa(
            caa_lines("tun.zip", "letsencrypt", ACCOUNT, "dns-01")
                .iter()
                .map(|l| parse_caa_text(l.split_once("CAA ").unwrap().1).unwrap())
                .collect(),
        )
    }
}

impl CaaLookup for FakeCaa {
    fn caa<'a>(&'a self, _domain: &'a str) -> BoxFuture<'a, anyhow::Result<Vec<Caa>>> {
        async move { Ok(self.0.clone()) }.boxed()
    }
}

pub struct Fakes {
    pub zone: Arc<FakeZone>,
    pub ca: Arc<FakeCa>,
    pub backends: Backends,
}

pub fn fakes() -> Fakes {
    let zone = Arc::new(FakeZone::default());
    let ca = Arc::new(FakeCa::new(zone.clone()));
    let backends = Backends {
        zone: zone.clone(),
        ca: ca.clone(),
        caa: Arc::new(FakeCaa::registered()),
        probe: zone.clone(),
    };
    Fakes { zone, ca, backends }
}
