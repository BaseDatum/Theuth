//! The edge's configuration (210 §8.1, appendix §8.1): a YAML file plus env overrides with prefix
//! `THEUTH_SHARE_EDGE_` and `__` as the nesting separator, env winning. Secrets (§8.3) come from exactly one of
//! a file named in the config or an env var, never from the YAML itself.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context as _, bail};
use figment::Figment;
use figment::providers::{Env, Format as _, Yaml};
use serde::Deserialize;
use share_proto::Admission;

/// Env prefix of every override.
pub const ENV_PREFIX: &str = "THEUTH_SHARE_EDGE_";
/// Default `--config` path.
pub const DEFAULT_CONFIG: &str = "/etc/theuth-share-edge/config.yaml";

/// The whole config. Unknown keys are ignored so the appendix's full `config.example.yaml` (which names keys of
/// later milestones) loads today.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Config {
    /// `D`, sent in `/v1/info` and `welcome`.
    pub domain: String,
    /// `share-edge.sqlite`, `iroh.key`, `acme/`.
    pub data_dir: PathBuf,
    /// The visitor TLS listener (TCP).
    pub listen: SocketAddr,
    /// The edge's own iroh endpoint.
    pub iroh: IrohConfig,
    /// Admin socket and health port.
    pub admin: AdminConfig,
    /// `open` · `invite` · `closed`; changed live by `admin admission`.
    pub admission: Admission,
    /// Reserved names a mesh may hold.
    pub reserved_per_mesh: u32,
    /// Engines attached at once per mesh.
    pub engines_per_mesh: u32,
    /// Longest accepted delegation lifetime.
    #[serde(with = "dur")]
    pub delegation_max_ttl: Duration,
    /// Minimum cooldown after a release (§5.2).
    #[serde(with = "dur")]
    pub reclaim_cooldown: Duration,
    /// Operator reserved words, beside the built-in ones.
    pub reserved_words: Vec<String>,
    /// One lowercase brand per line (§9.4).
    pub brand_blocklist_file: Option<PathBuf>,
    /// Terms page, outside `D`.
    pub terms_url: String,
    /// Current terms revision; a `hello` carrying it is `terms_ok`.
    pub terms_rev: String,
    /// Abuse page, outside `D`.
    pub abuse_url: String,
    /// Abuse mail address.
    pub abuse_contact: String,
    /// Oldest client version served (`426 client_too_old` below it). Not in the appendix's example; added here.
    pub min_client: String,
    /// Refuse to run without the CAA lines (§8.2). Checked by the ACME phase.
    pub require_caa: bool,
    /// ACME settings; the fields the ACME phase needs beyond these are added there.
    pub acme: AcmeConfig,
    /// DNS provider for Path Z.
    pub dns: DnsConfig,
    /// §9.4 limits.
    pub limits: Limits,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            domain: String::new(),
            data_dir: PathBuf::from("/var/lib/theuth-share-edge"),
            listen: "[::]:443".parse().expect("static addr"),
            iroh: IrohConfig::default(),
            admin: AdminConfig::default(),
            admission: Admission::Open,
            reserved_per_mesh: 1,
            engines_per_mesh: 8,
            delegation_max_ttl: Duration::from_millis(share_proto::limits::DELEGATION_MAX_TTL_MS),
            reclaim_cooldown: Duration::from_secs(30 * 86_400),
            reserved_words: Vec::new(),
            brand_blocklist_file: None,
            terms_url: String::new(),
            terms_rev: String::new(),
            abuse_url: String::new(),
            abuse_contact: String::new(),
            min_client: "0.0.0".into(),
            require_caa: true,
            acme: AcmeConfig::default(),
            dns: DnsConfig::default(),
            limits: Limits::default(),
        }
    }
}

/// `iroh:`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct IrohConfig {
    /// UDP bind address.
    pub listen: SocketAddr,
    /// Home relay URL; empty = no relay.
    pub relay: String,
    /// Public addresses put into the ticket (a LoadBalancer's IP).
    pub public_addrs: Vec<SocketAddr>,
    /// Publish to pkarr. TODO(210-M4): wire a pkarr publisher; today nothing is published.
    pub pkarr: bool,
}

impl Default for IrohConfig {
    fn default() -> Self {
        IrohConfig {
            listen: "[::]:7850".parse().expect("static addr"),
            relay: String::new(),
            public_addrs: Vec::new(),
            pkarr: true,
        }
    }
}

/// `admin:`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct AdminConfig {
    /// Unix socket of the admin protocol (§8.5); default `<data_dir>/admin.sock`.
    pub socket: Option<PathBuf>,
    /// `/healthz` and `/metrics`; empty = off.
    pub http: String,
}

/// `acme:`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AcmeConfig {
    /// `mailto:` contact of the account.
    pub contact: String,
    /// ACME profile of every order (`classic`, 90 days; `tlsserver`, 45 days). Empty = the CA's default.
    pub profile: String,
    /// `letsencrypt` in P1.
    pub primary: String,
    /// Per-CA settings.
    pub cas: CasConfig,
    /// How often the job runner refreshes ARI windows and looks for renewals (§6.3: 6 h).
    #[serde(with = "dur")]
    pub renew_check: Duration,
    /// Lease of one job attempt; a crashed attempt is picked up again after it.
    #[serde(with = "dur")]
    pub job_lease: Duration,
    /// Attempts before a job is `failed`.
    pub max_attempts: u32,
    /// How long to poll the zone's authoritative servers for a written challenge TXT before readying the
    /// challenge anyway.
    #[serde(with = "dur")]
    pub propagation: Duration,
    /// Wait after clearing the first challenge TXT before writing the second (§6.1: Let's Encrypt's
    /// recursors cache up to 60 s).
    #[serde(with = "dur")]
    pub auth_gap: Duration,
}

impl Default for AcmeConfig {
    fn default() -> Self {
        AcmeConfig {
            contact: String::new(),
            profile: "classic".into(),
            primary: "letsencrypt".into(),
            cas: CasConfig::default(),
            renew_check: Duration::from_secs(6 * 3600),
            job_lease: Duration::from_secs(15 * 60),
            max_attempts: 12,
            propagation: Duration::from_secs(30),
            auth_gap: Duration::from_secs(60),
        }
    }
}

/// `acme.cas:`.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(default)]
pub struct CasConfig {
    /// `acme.cas.letsencrypt` (P1, Path Z).
    pub letsencrypt: LeConfig,
    /// `acme.cas.gts` (P2).
    pub gts: GtsConfig,
}

/// `acme.cas.letsencrypt:`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct LeConfig {
    /// ACME directory URL.
    pub directory: String,
    /// New-name certificates per 7 days, one token bucket refilled continuously (§6.5).
    pub new_certs_per_week: u32,
}

impl Default for LeConfig {
    fn default() -> Self {
        LeConfig {
            directory: "https://acme-v02.api.letsencrypt.org/directory".into(),
            new_certs_per_week: 45,
        }
    }
}

/// `acme.cas.gts:` (P2).
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct GtsConfig {
    /// Path P switch.
    pub enabled: bool,
    /// ACME directory URL.
    pub directory: String,
    /// CAA `validationmethods` of the GTS lines.
    pub validation: String,
    /// File holding the EAB JSON.
    pub eab_file: Option<PathBuf>,
}

impl Default for GtsConfig {
    fn default() -> Self {
        GtsConfig {
            enabled: false,
            directory: "https://dv.acme-v02.api.pki.goog/directory".into(),
            validation: "dns-persist-01".into(),
            eab_file: None,
        }
    }
}

/// `dns:`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct DnsConfig {
    /// `porkbun` in P1.
    pub provider: String,
    /// The zone; empty = `domain`.
    pub zone: String,
    /// File holding the provider credential JSON.
    pub credential_file: Option<PathBuf>,
    /// CNAME target of every name, `edge.D`.
    pub target: String,
}

impl Default for DnsConfig {
    fn default() -> Self {
        DnsConfig {
            provider: "porkbun".into(),
            zone: String::new(),
            credential_file: None,
            target: String::new(),
        }
    }
}

/// `limits:` (§9.4), the ones enforced in M1. TODO(210-M3): the per-network, bucket and byte limits.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct Limits {
    /// Visitor streams per tunnel.
    pub conns_per_tunnel: u32,
    /// `open_bi` deadline per visitor stream.
    pub open_bi_timeout_secs: u64,
    /// Visitor splice idle reset.
    pub idle_secs: u64,
    /// Routes per tunnel.
    pub routes_per_tunnel: u32,
    /// Connections without a valid `hello`.
    pub pre_hello_conns: u32,
    /// Seconds a connection may stay without a valid `hello`.
    pub hello_timeout_secs: u64,
    /// Visitor connections at once.
    pub global_conns: u32,
    /// Visitor connections per IP (IPv6 /64).
    pub conns_per_ip: u32,
    /// New visitor connections per second per IP (IPv6 /64).
    pub new_conns_per_ip_sec: u32,
    /// `GET /v1/info` requests per minute per client network (IPv4, IPv6 /64, or relay URL).
    pub info_per_ip_min: u32,
}

impl Default for Limits {
    fn default() -> Self {
        Limits {
            conns_per_tunnel: share_proto::limits::VISITOR_STREAMS_PER_TUNNEL,
            open_bi_timeout_secs: share_proto::limits::OPEN_BI_TIMEOUT_MS / 1000,
            idle_secs: 900,
            routes_per_tunnel: 64,
            pre_hello_conns: 256,
            hello_timeout_secs: 10,
            global_conns: 20_000,
            conns_per_ip: 64,
            new_conns_per_ip_sec: 20,
            info_per_ip_min: 30,
        }
    }
}

/// Where a secret comes from. `Debug` never shows an env value.
#[derive(Clone, PartialEq, Eq)]
pub enum SecretSource {
    /// Read the file on every use, so a rotated key needs no restart (§8.3).
    File(PathBuf),
    /// The value of an env var, read at start.
    Env(String),
}

impl std::fmt::Debug for SecretSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SecretSource::File(p) => write!(f, "File({})", p.display()),
            SecretSource::Env(_) => f.write_str("Env(<redacted>)"),
        }
    }
}

impl SecretSource {
    /// The secret's current value.
    pub fn read(&self) -> anyhow::Result<String> {
        match self {
            SecretSource::File(p) => {
                std::fs::read_to_string(p).with_context(|| format!("read secret {}", p.display()))
            }
            SecretSource::Env(v) => Ok(v.clone()),
        }
    }
}

/// The edge's secrets (§8.3).
#[derive(Debug, Clone, Default)]
pub struct Secrets {
    /// The Porkbun credential JSON.
    pub dns: Option<SecretSource>,
    /// The GTS EAB JSON (P2).
    pub gts_eab: Option<SecretSource>,
}

/// One secret's rule: `inline_key` must not be in the YAML; exactly one of `file` and `env_var` may be set.
struct SecretRule {
    inline_key: &'static str,
    file_key: &'static str,
    env_var: &'static str,
}

const DNS_SECRET: SecretRule = SecretRule {
    inline_key: "dns.credential",
    file_key: "dns.credential_file",
    env_var: "THEUTH_SHARE_EDGE_DNS__CREDENTIAL",
};
const GTS_EAB_SECRET: SecretRule = SecretRule {
    inline_key: "acme.cas.gts.eab",
    file_key: "acme.cas.gts.eab_file",
    env_var: "THEUTH_SHARE_EDGE_ACME__CAS__GTS__EAB",
};

impl SecretRule {
    fn resolve(
        &self,
        yaml: &Figment,
        file: Option<&Path>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> anyhow::Result<Option<SecretSource>> {
        if yaml.find_value(self.inline_key).is_ok() {
            bail!(
                "`{}` must not be in the config file: name a file in `{}` or set {}",
                self.inline_key,
                self.file_key,
                self.env_var
            );
        }
        let env = env(self.env_var).filter(|v| !v.is_empty());
        match (file, env) {
            (Some(_), Some(_)) => bail!(
                "both `{}` and {} are set: use exactly one",
                self.file_key,
                self.env_var
            ),
            (Some(f), None) => Ok(Some(SecretSource::File(f.to_path_buf()))),
            (None, Some(v)) => Ok(Some(SecretSource::Env(v))),
            (None, None) => Ok(None),
        }
    }
}

impl Config {
    /// Load `path` (must exist) with env overrides and resolve the secrets from the process env.
    pub fn load(path: &Path) -> anyhow::Result<(Config, Secrets)> {
        if !path.exists() {
            bail!("config {} not found", path.display());
        }
        Self::resolve(Figment::from(Yaml::file(path)), &|k| std::env::var(k).ok())
    }

    /// Load from a YAML string (tests, embedded use) with env overrides.
    pub fn from_yaml(
        yaml: &str,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> anyhow::Result<(Config, Secrets)> {
        Self::resolve(Figment::from(Yaml::string(yaml)), env)
    }

    fn resolve(
        yaml: Figment,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> anyhow::Result<(Config, Secrets)> {
        let cfg: Config = yaml
            .clone()
            .merge(Env::prefixed(ENV_PREFIX).split("__"))
            .extract()
            .context("config")?;
        cfg.validate()?;
        let secrets = Secrets {
            dns: DNS_SECRET.resolve(&yaml, cfg.dns.credential_file.as_deref(), env)?,
            gts_eab: GTS_EAB_SECRET.resolve(&yaml, cfg.acme.cas.gts.eab_file.as_deref(), env)?,
        };
        Ok((cfg, secrets))
    }

    fn validate(&self) -> anyhow::Result<()> {
        if self.domain.is_empty() {
            bail!("`domain` is required");
        }
        if !self.require_caa && self.admission == Admission::Open {
            bail!("`require_caa: false` is refused with `admission: open` (§8.2)");
        }
        if self.domain != self.domain.to_ascii_lowercase() || self.domain.ends_with('.') {
            bail!("`domain` must be lowercase without a trailing dot");
        }
        if let Err(e) = self.min_client.parse::<share_proto::Version>() {
            bail!("`min_client`: {e:?}");
        }
        Ok(())
    }

    /// The admin socket path.
    pub fn admin_socket(&self) -> PathBuf {
        self.admin
            .socket
            .clone()
            .unwrap_or_else(|| self.data_dir.join("admin.sock"))
    }

    /// The sqlite file.
    pub fn db_path(&self) -> PathBuf {
        self.data_dir.join("share-edge.sqlite")
    }

    /// The endpoint key file.
    pub fn key_path(&self) -> PathBuf {
        self.data_dir.join("iroh.key")
    }

    /// The DNS zone the records live in.
    pub fn dns_zone(&self) -> &str {
        if self.dns.zone.is_empty() {
            &self.domain
        } else {
            &self.dns.zone
        }
    }

    /// The CNAME target of every name; default `edge.D`.
    pub fn dns_target(&self) -> String {
        if self.dns.target.is_empty() {
            format!("edge.{}", self.domain)
        } else {
            self.dns.target.clone()
        }
    }

    /// `<data_dir>/acme/<ca>.json`, the ACME account credentials (0600).
    pub fn account_path(&self, ca: &str) -> PathBuf {
        self.data_dir.join("acme").join(format!("{ca}.json"))
    }
}

/// Durations as `25h`, `30d` (humantime) or plain seconds.
mod dur {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer};

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Secs(u64),
        Text(String),
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        match Raw::deserialize(d)? {
            Raw::Secs(s) => Ok(Duration::from_secs(s)),
            Raw::Text(t) => humantime::parse_duration(&t).map_err(serde::de::Error::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: &str = "domain: tun.zip\n";

    fn no_env(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn appendix_keys_and_durations_load() {
        let yaml = "domain: tun.zip\ndelegation_max_ttl: 25h\nreclaim_cooldown: 30d\nadmission: invite\n\
                    limits: {idle_secs: 60, egress_month: 10TiB}\nacme: {primary: letsencrypt, queue_max: 2000}\n";
        let (cfg, secrets) = Config::from_yaml(yaml, &no_env).unwrap();
        assert_eq!(cfg.delegation_max_ttl, Duration::from_secs(25 * 3600));
        assert_eq!(cfg.reclaim_cooldown, Duration::from_secs(30 * 86_400));
        assert_eq!(cfg.admission, Admission::Invite);
        assert_eq!(cfg.limits.idle_secs, 60);
        assert!(secrets.dns.is_none());
    }

    #[test]
    fn inline_secret_in_yaml_is_refused() {
        let yaml = format!("{BASE}dns: {{credential: '{{\"key\":\"pk1_x\"}}'}}\n");
        let err = Config::from_yaml(&yaml, &no_env).unwrap_err().to_string();
        assert!(err.contains("dns.credential"), "{err}");
        let yaml = format!("{BASE}acme: {{cas: {{gts: {{eab: x}}}}}}\n");
        assert!(Config::from_yaml(&yaml, &no_env).is_err());
    }

    #[test]
    fn file_and_env_together_are_refused() {
        let yaml = format!("{BASE}dns: {{credential_file: /run/secrets/dns.json}}\n");
        let env = |k: &str| (k == DNS_SECRET.env_var).then(|| "{}".to_string());
        let err = Config::from_yaml(&yaml, &env).unwrap_err().to_string();
        assert!(err.contains("exactly one"), "{err}");
    }

    #[test]
    fn file_or_env_alone_is_the_source() {
        let yaml = format!("{BASE}dns: {{credential_file: /run/secrets/dns.json}}\n");
        let (_, s) = Config::from_yaml(&yaml, &no_env).unwrap();
        assert_eq!(
            s.dns,
            Some(SecretSource::File("/run/secrets/dns.json".into()))
        );
        let env = |k: &str| (k == DNS_SECRET.env_var).then(|| "{\"key\":\"k\"}".to_string());
        let (_, s) = Config::from_yaml(BASE, &env).unwrap();
        assert_eq!(s.dns, Some(SecretSource::Env("{\"key\":\"k\"}".into())));
        assert_eq!(format!("{:?}", s.dns.unwrap()), "Env(<redacted>)");
    }

    #[test]
    fn missing_domain_is_refused() {
        assert!(Config::from_yaml("listen: \"[::]:443\"\n", &no_env).is_err());
    }
}
