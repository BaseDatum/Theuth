//! DNS records of Path Z (210 §6.0, §6.4, D17; appendix §6.1): the thin `DnsZone` trait over one provider, the
//! zone-update Porkbun impl, and the checked helpers every caller goes through (read-back after each write and
//! delete, provider panics contained).

use std::fmt;
use std::panic::AssertUnwindSafe;

use anyhow::{Context as _, anyhow, bail};
use futures::FutureExt as _;
use futures::future::BoxFuture;
use serde::Deserialize;

use crate::config::SecretSource;

/// The record types the edge writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Rtype {
    /// `edge.D` only (operator-written today).
    A,
    /// `<n>.D` and `*.<n>.D` → `edge.D` (D17).
    Cname,
    /// `_acme-challenge.<n>.D`, one at a time (§6.1).
    Txt,
}

impl Rtype {
    /// The `dns_records.type` text.
    pub fn as_str(self) -> &'static str {
        match self {
            Rtype::A => "A",
            Rtype::Cname => "CNAME",
            Rtype::Txt => "TXT",
        }
    }

    /// Inverse of `as_str`.
    pub fn parse(s: &str) -> Option<Rtype> {
        match s {
            "A" => Some(Rtype::A),
            "CNAME" => Some(Rtype::Cname),
            "TXT" => Some(Rtype::Txt),
            _ => None,
        }
    }
}

impl fmt::Display for Rtype {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One DNS zone. Hosts are relative to the zone (`sky`, `*.sky`, `_acme-challenge.sky`). Callers use the checked
/// helpers below, never these directly.
pub trait DnsZone: Send + Sync + fmt::Debug {
    /// The single record of `rtype` at `host`; an error when the host holds more than one.
    fn get(&self, rtype: Rtype, host: String) -> BoxFuture<'_, anyhow::Result<Option<String>>>;
    /// Add a record.
    fn create(&self, rtype: Rtype, host: String, value: String) -> BoxFuture<'_, anyhow::Result<()>>;
    /// Delete every record of `rtype` at `host`.
    fn delete_all(&self, rtype: Rtype, host: String) -> BoxFuture<'_, anyhow::Result<()>>;
}

/// Run one provider call; a panic inside it is an error of this attempt, never a worker crash (§6.1).
pub async fn contain<T>(f: BoxFuture<'_, anyhow::Result<T>>) -> anyhow::Result<T> {
    match AssertUnwindSafe(f).catch_unwind().await {
        Ok(r) => r,
        Err(p) => Err(anyhow!("DNS provider panicked: {}", panic_text(&*p))),
    }
}

/// The message of a panic payload.
pub fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "non-string panic".into())
}

fn same(rtype: Rtype, a: &str, b: &str) -> bool {
    let norm = |s: &str| s.trim().trim_matches('"').trim_end_matches('.').to_string();
    match rtype {
        Rtype::Txt => norm(a) == norm(b),
        _ => norm(a).eq_ignore_ascii_case(&norm(b)),
    }
}

/// Make `host` hold exactly `value`, then read it back.
pub async fn ensure(zone: &dyn DnsZone, rtype: Rtype, host: &str, value: &str) -> anyhow::Result<()> {
    match contain(zone.get(rtype, host.into())).await {
        Ok(Some(v)) if same(rtype, &v, value) => return Ok(()),
        Ok(None) => {}
        // A different value, or several (zone-update refuses to read those): clear and write.
        Ok(Some(_)) | Err(_) => remove(zone, rtype, host).await?,
    }
    contain(zone.create(rtype, host.into(), value.into())).await?;
    match contain(zone.get(rtype, host.into())).await? {
        Some(v) if same(rtype, &v, value) => Ok(()),
        other => bail!("read-back of {rtype} {host}: expected {value:?}, found {other:?}"),
    }
}

/// Delete every `rtype` record at `host`, then read back that none is left (Porkbun deletes are unchecked).
pub async fn remove(zone: &dyn DnsZone, rtype: Rtype, host: &str) -> anyhow::Result<()> {
    contain(zone.delete_all(rtype, host.into())).await?;
    match contain(zone.get(rtype, host.into())).await? {
        None => Ok(()),
        Some(v) => bail!("read-back of {rtype} {host} after delete: still {v:?}"),
    }
}

/// `fqdn` relative to `zone` (`sky.tun.zip` in `tun.zip` → `sky`).
pub fn relative(fqdn: &str, zone: &str) -> anyhow::Result<String> {
    let fqdn = fqdn.trim_end_matches('.');
    fqdn.strip_suffix(zone)
        .and_then(|h| h.strip_suffix('.'))
        .filter(|h| !h.is_empty())
        .map(str::to_string)
        .ok_or_else(|| anyhow!("{fqdn} is not below zone {zone}"))
}

/// The two D17 hosts of a name, relative to `zone`.
pub fn name_hosts(name: &str, domain: &str, zone: &str) -> anyhow::Result<[String; 2]> {
    let bare = relative(&format!("{name}.{domain}"), zone)?;
    Ok([bare.clone(), format!("*.{bare}")])
}

/// The challenge TXT host of an ACME identifier (`*.sky.tun.zip` and `sky.tun.zip` → `_acme-challenge.sky`).
pub fn challenge_host(ident: &str, zone: &str) -> anyhow::Result<String> {
    let base = ident.strip_prefix("*.").unwrap_or(ident);
    relative(&format!("_acme-challenge.{base}"), zone)
}

/// The Porkbun credential (§8.3). zone-update's `porkbun::Auth` names the fields `key`/`secret`; Porkbun's own
/// API names them `apikey`/`secretapikey`; both spellings are accepted.
#[derive(Deserialize)]
struct PorkbunCredential {
    #[serde(alias = "apikey")]
    key: String,
    #[serde(alias = "secretapikey")]
    secret: String,
}

/// zone-update's concrete Porkbun client (not the boxed trait object: its generic `create_record` is
/// `Self: Sized`, so a CNAME needs the concrete type). The credential is re-read on every call, so a rotated
/// key needs no restart; each call runs in `spawn_blocking` (zone-update is blocking `ureq`).
pub struct PorkbunZone {
    zone: String,
    credential: SecretSource,
}

impl fmt::Debug for PorkbunZone {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PorkbunZone").field("zone", &self.zone).finish_non_exhaustive()
    }
}

impl PorkbunZone {
    /// A client for `zone` with the credential from `credential`. Fails early on an unreadable credential.
    pub fn new(zone: &str, credential: SecretSource) -> anyhow::Result<PorkbunZone> {
        let z = PorkbunZone {
            zone: zone.to_string(),
            credential,
        };
        z.client().context("dns credential")?;
        Ok(z)
    }

    fn client(&self) -> anyhow::Result<zone_update::porkbun::Porkbun> {
        let c: PorkbunCredential = serde_json::from_str(&self.credential.read()?)
            .context("dns credential: expected JSON {\"key\": …, \"secret\": …}")?;
        Ok(zone_update::porkbun::Porkbun::new(
            zone_update::Config {
                domain: self.zone.clone(),
                dry_run: false,
            },
            zone_update::porkbun::Auth {
                key: c.key,
                secret: c.secret,
            },
        ))
    }

    fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(zone_update::porkbun::Porkbun) -> zone_update::errors::Result<T> + Send + 'static,
    ) -> BoxFuture<'_, anyhow::Result<T>> {
        async move {
            let client = self.client()?;
            match tokio::task::spawn_blocking(move || f(client)).await {
                Ok(r) => r.map_err(|e| anyhow!("porkbun: {e}")),
                Err(e) if e.is_panic() => {
                    Err(anyhow!("porkbun client panicked: {}", panic_text(&*e.into_panic())))
                }
                Err(e) => Err(anyhow!("porkbun call: {e}")),
            }
        }
        .boxed()
    }
}

fn record_type(t: Rtype) -> zone_update::RecordType {
    match t {
        Rtype::A => zone_update::RecordType::A,
        Rtype::Cname => zone_update::RecordType::CNAME,
        Rtype::Txt => zone_update::RecordType::TXT,
    }
}

impl DnsZone for PorkbunZone {
    fn get(&self, rtype: Rtype, host: String) -> BoxFuture<'_, anyhow::Result<Option<String>>> {
        use zone_update::DnsProvider as _;
        self.call(move |c| c.get_record::<String>(record_type(rtype), &host))
    }

    fn create(&self, rtype: Rtype, host: String, value: String) -> BoxFuture<'_, anyhow::Result<()>> {
        use zone_update::DnsProvider as _;
        // TODO(210-M1): zone-update sends TTL 300; P0-1 records whether Porkbun accepts it (D17 plans 600).
        self.call(move |c| c.create_record(record_type(rtype), &host, &value))
    }

    fn delete_all(&self, rtype: Rtype, host: String) -> BoxFuture<'_, anyhow::Result<()>> {
        use zone_update::DnsProvider as _;
        self.call(move |c| c.delete_all_records(record_type(rtype), &host))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_are_relative_to_the_zone() {
        assert_eq!(
            name_hosts("sky", "tun.zip", "tun.zip").unwrap(),
            ["sky".to_string(), "*.sky".to_string()]
        );
        assert_eq!(
            name_hosts("sky", "share.example.com", "example.com").unwrap(),
            ["sky.share".to_string(), "*.sky.share".to_string()]
        );
        assert_eq!(
            challenge_host("*.sky.tun.zip", "tun.zip").unwrap(),
            "_acme-challenge.sky"
        );
        assert_eq!(challenge_host("sky.tun.zip", "tun.zip").unwrap(), "_acme-challenge.sky");
        assert!(relative("sky.other.zip", "tun.zip").is_err());
        assert!(relative("tun.zip", "tun.zip").is_err());
    }

    #[test]
    fn credential_accepts_both_spellings() {
        let a: PorkbunCredential = serde_json::from_str(r#"{"key":"pk1_a","secret":"sk1_b"}"#).unwrap();
        let b: PorkbunCredential =
            serde_json::from_str(r#"{"apikey":"pk1_a","secretapikey":"sk1_b"}"#).unwrap();
        assert_eq!((a.key, a.secret), (b.key, b.secret));
    }
}
