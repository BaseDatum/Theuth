//! 221 §6: external connections. Their non-secret config lives in `secrets.connections.<conn>`
//! (`config.yaml`, engine-local); their credentials in sealed engine-local `meta` rows under
//! `ext/<backend>/<conn>/` ([`ConnStore`]); the backend objects in [`Externals`]. A backend crate
//! offers a [`BackendFactory`] ([`RegisterBackendFactory`]); every configured connection of that
//! backend is then built and registered, always after its four always-ask entries exist
//! (§6.3: never a window in which the connection is un-gated).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::external::{BwKind, ConnStatus, ExternalBackend, Externals, valid_conn};
use crate::settings::{self, ConnSettings, ConnectionConfig, ExternalSettings};
use crate::vault::Vault;
use crate::{SecretsActor, SecretsConfig};

/// `meta` stamp: `conn`'s always-ask entries were seeded once; a removed entry stays removed.
pub fn seeded_stamp(conn: &str) -> String {
    format!("ext/seeded/{conn}")
}

/// Bitwarden server preset (§6.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum BwServer {
    /// `vault.bitwarden.com`.
    Us,
    /// `vault.bitwarden.eu`.
    Eu,
    /// Official self-host or Vaultwarden at `base_url`.
    SelfHosted,
}

/// How a Bitwarden connection logs in (§6.2).
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum BwLogin {
    /// Personal API key (default).
    #[default]
    ApiKey,
    /// Email + master password + 2FA (B5b).
    Password,
    /// The organisation's SSO, then the master password on the unlock card (§9.6).
    Sso,
}

/// §6.1 / §6.2: a connection's non-secret config, tagged by `backend`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "backend", rename_all = "snake_case")]
pub enum ConnConfig {
    /// AWS Secrets Manager.
    Aws {
        /// A name from the AWS profile list; `None` = the default chain.
        #[serde(default)]
        profile: Option<String>,
        /// Secrets Manager is regional.
        region: String,
        /// Default KMS key for create.
        #[serde(default)]
        kms_key_id: Option<String>,
        /// List/browse scope (a `ListSecrets` name filter).
        #[serde(default)]
        prefix: Option<String>,
    },
    /// Bitwarden Password Manager.
    Bw {
        /// `us` | `eu` | `self_hosted`.
        server: BwServer,
        /// `self_hosted`: `https://vault.example.org`.
        #[serde(default)]
        base_url: Option<String>,
        /// Override; default `<base>/api`.
        #[serde(default)]
        api_url: Option<String>,
        /// Override; default `<base>/identity`.
        #[serde(default)]
        identity_url: Option<String>,
        /// Override of the web vault URL Send links use.
        #[serde(default)]
        web_vault_url: Option<String>,
        /// Shown on the unlock card; prelogin needs it.
        email: String,
        /// `api_key` | `password` | `sso`.
        #[serde(default)]
        login: BwLogin,
        /// `sso`: the organisation's SSO identifier (`connection_sso_begin` may set it).
        #[serde(default)]
        org_identifier: Option<String>,
    },
}

impl ConnConfig {
    /// `aws` | `bw`.
    pub fn backend(&self) -> &'static str {
        match self {
            Self::Aws { .. } => "aws",
            Self::Bw { .. } => "bw",
        }
    }

    /// §6.3 `ConnInfo.summary`: `work-admin · eu-west-1` | `me@… · vault.bitwarden.eu`.
    pub fn summary(&self) -> String {
        match self {
            Self::Aws {
                profile, region, ..
            } => format!("{} · {region}", profile.as_deref().unwrap_or("default")),
            Self::Bw {
                server,
                base_url,
                email,
                ..
            } => {
                let host = match server {
                    BwServer::Us => "vault.bitwarden.com".to_owned(),
                    BwServer::Eu => "vault.bitwarden.eu".to_owned(),
                    BwServer::SelfHosted => base_url
                        .as_deref()
                        .map(|u| {
                            let u = u.split_once("://").map_or(u, |(_, rest)| rest);
                            u.split('/').next().unwrap_or(u).to_owned()
                        })
                        .unwrap_or_else(|| "self-hosted".to_owned()),
                };
                format!("{email} · {host}")
            }
        }
    }

    /// The stored form: this config's keys beside the D20 `settings`.
    pub fn stored(&self, settings: &ConnSettings) -> Result<ConnectionConfig, String> {
        let mut m = match serde_json::to_value(self) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => return Err("connection config is not an object".to_owned()),
        };
        if let Ok(serde_json::Value::Object(s)) = serde_json::to_value(settings) {
            m.extend(s);
        }
        serde_json::from_value(serde_json::Value::Object(m)).map_err(|e| e.to_string())
    }

    /// The config part of a stored connection.
    pub fn of(stored: &ConnectionConfig) -> Result<ConnConfig, String> {
        let mut m = match serde_json::to_value(&stored.other) {
            Ok(serde_json::Value::Object(m)) => m,
            _ => serde_json::Map::new(),
        };
        m.insert("backend".into(), stored.backend.clone().into());
        serde_json::from_value(serde_json::Value::Object(m)).map_err(|e| e.to_string())
    }

    /// R16/R19: every Bitwarden server URL (`base_url`, `api_url`, `identity_url`,
    /// `web_vault_url`) is `https://`, or `http://` to a loopback host only, so the master
    /// password, the API key and Send links never cross the network in clear.
    pub fn check_urls(&self) -> Result<(), String> {
        let Self::Bw {
            base_url,
            api_url,
            identity_url,
            web_vault_url,
            ..
        } = self
        else {
            return Ok(());
        };
        for (key, url) in [
            ("base_url", base_url),
            ("api_url", api_url),
            ("identity_url", identity_url),
            ("web_vault_url", web_vault_url),
        ] {
            if let Some(url) = url
                && !secure_url(url)
            {
                return Err(format!(
                    "{key} {url:?} must be https:// (http:// only for localhost)"
                ));
            }
        }
        Ok(())
    }
}

/// `https://…`, or `http://` to `localhost`, `127.0.0.0/8` or `[::1]`.
fn secure_url(url: &str) -> bool {
    let Some((scheme, rest)) = url.trim().split_once("://") else {
        return false;
    };
    match scheme.to_ascii_lowercase().as_str() {
        "https" => true,
        "http" => {
            let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
            let host = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
            let host = match host.strip_prefix('[') {
                Some(v6) => v6.split_once(']').map_or("", |(h, _)| h),
                None => host.split(':').next().unwrap_or(""),
            };
            host.parse::<std::net::IpAddr>()
                .map_or(host.eq_ignore_ascii_case("localhost"), |ip| {
                    ip.is_loopback()
                })
        }
        _ => false,
    }
}

/// §6.3 `ConnInfo.ask`: which of the connection's always-ask entries are in the global set.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AskInfo {
    /// `secret:<b>/<conn>:read`.
    pub read: bool,
    /// `…:write`.
    pub write: bool,
    /// `…:delete`.
    pub delete: bool,
    /// `secret:bw/<conn>:send`; `None` for a backend without Sends.
    pub send: Option<bool>,
}

/// §6.3: one connection as the UIs show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConnInfo {
    /// The connection name.
    pub conn: String,
    /// `aws` | `bw`.
    pub backend: String,
    /// What the backend says now (no I/O); `unsupported` when this build has no such backend.
    pub status: ConnStatus,
    /// `work-admin · eu-west-1` | `me@… · vault.bitwarden.eu`.
    pub summary: String,
    /// The always-ask switches.
    pub ask: AskInfo,
    /// The D20 settings.
    pub settings: ConnSettings,
    /// Bitwarden: how it logs in (`None` for AWS).
    #[serde(default)]
    pub login: Option<BwLogin>,
}

/// AWS value shape for a `secret_request` destination.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AwsFormat {
    /// One field → `SecretString`.
    String,
    /// Every field → `{"<field.name>": value}`.
    Json,
}

/// §7.3: where a `secret_request` answer is stored instead of (or as well as) the local vault.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Destination {
    /// The connection (`prod`, `personal`).
    pub connection: String,
    /// AWS: the secret name (`app/ci/token`). Bitwarden: the item's name (`GitHub — CI`).
    pub name: String,
    /// Update this entry (its `ref`) instead of creating one. The card says "replaces".
    #[serde(default)]
    pub replace: Option<String>,
    /// Also keep a local copy under each field's name (default false: the manager is the truth).
    #[serde(default)]
    pub keep_local: bool,
    /// AWS: KMS key; default the connection's, else `aws/secretsmanager`.
    #[serde(default)]
    pub kms_key_id: Option<String>,
    /// AWS: `string` (one field) | `json` (every field).
    #[serde(default)]
    pub format: Option<AwsFormat>,
    /// Bitwarden: organisation name.
    #[serde(default)]
    pub org: Option<String>,
    /// Bitwarden: collection name.
    #[serde(default)]
    pub collection: Option<String>,
    /// Bitwarden: folder name.
    #[serde(default)]
    pub folder: Option<String>,
    /// Bitwarden: `login` | `secure_note`.
    #[serde(default)]
    pub item: Option<BwKind>,
    /// Bitwarden: per form field, the Bitwarden field it fills (`password` | `username` | `totp`
    /// | `notes` | `field:<name>`).
    #[serde(default)]
    pub fields: Option<BTreeMap<String, String>>,
    /// Bitwarden: login URI.
    #[serde(default)]
    pub uri: Option<String>,
}

/// §7.3: a [`Destination`] as the request card shows it, resolved on the asking engine.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct DestinationView {
    /// What the agent (or the card) chose.
    #[serde(flatten)]
    pub destination: Destination,
    /// `aws` | `bw`.
    pub backend: String,
    /// The connection's [`ConnInfo::summary`].
    pub summary: String,
    /// Whether the connection is locked now: the card shows the unlock field first.
    pub status: ConnStatus,
    /// The entry's current revision when `replace` is set.
    #[serde(default)]
    pub current_rev: Option<String>,
}

/// The sealed engine-local rows of one connection (`ext/<backend>/<conn>/<key>`, D11). Sealed with
/// this engine's key; never replicated, never listed, never expandable.
#[derive(Clone)]
pub struct ConnStore {
    vault: Arc<Vault>,
    prefix: String,
}

impl std::fmt::Debug for ConnStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ConnStore({})", self.prefix)
    }
}

impl ConnStore {
    /// The rows of `backend/conn`.
    pub fn new(vault: Arc<Vault>, backend: &str, conn: &str) -> Self {
        Self {
            vault,
            prefix: format!("ext/{backend}/{conn}/"),
        }
    }

    /// The value under `key` (`client_secret`, `user_key`, …).
    pub async fn get(&self, key: &str) -> Result<Option<Zeroizing<Vec<u8>>>, String> {
        self.vault
            .local_secret(&format!("{}{key}", self.prefix))
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Store `value` under `key`.
    pub async fn put(&self, key: &str, value: &[u8]) -> Result<(), String> {
        self.vault
            .set_local_secret(&format!("{}{key}", self.prefix), value)
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Delete the row under `key`.
    pub async fn delete(&self, key: &str) -> Result<(), String> {
        crate::store::meta_delete(self.vault.pool(), &format!("{}{key}", self.prefix))
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// Delete every row of this connection.
    pub async fn clear(&self) -> Result<(), String> {
        self.vault
            .delete_local_prefix(&self.prefix)
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// Builds the connections of one backend; offered by the binary for each backend it links
/// (`secrets-aws`, `secrets-bitwarden` behind the `bitwarden` feature).
pub trait BackendFactory: Send + Sync + 'static {
    /// `aws` | `bw`.
    fn backend(&self) -> &'static str;
    /// The backend for `conn`. No I/O. `store` holds the connection's sealed credentials.
    fn build(
        &self,
        conn: &str,
        cfg: &ConnConfig,
        store: ConnStore,
    ) -> Result<Arc<dyn ExternalBackend>, String>;
}

/// `ask` → `()`: build every configured connection of this backend now (once their always-ask
/// entries are seeded) and every one added later.
#[derive(Clone)]
pub struct RegisterBackendFactory(pub Arc<dyn BackendFactory>);

/// The connection part of the [`SecretsActor`].
#[derive(Default)]
pub(crate) struct Connections {
    /// `secrets.connections`.
    pub(crate) conns: BTreeMap<String, ConnectionConfig>,
    /// `secrets.external`.
    pub(crate) external: ExternalSettings,
    /// Where `config.yaml` is; `None` = nothing persists (tests).
    pub(crate) config_path: Option<PathBuf>,
    pub(crate) factories: HashMap<&'static str, Arc<dyn BackendFactory>>,
    /// The always-ask entries of every configured connection exist (or the agent surface is off).
    pub(crate) ready: bool,
    /// `secrets.tools`: the agent surface (the external tools) is on.
    pub(crate) tools: bool,
    /// §12: the Send tools' context, once a Bitwarden connection exists (also serves the RPCs).
    pub(crate) send: Option<crate::tools_send::SendCx>,
    /// §12.3: `artifact:<id>` Send sources, installed by the binary ([`SetArtifactSource`]).
    pub(crate) artifacts: Option<Arc<dyn crate::tools_send::ArtifactSource>>,
}

impl Connections {
    /// From the `secrets` section.
    pub(crate) fn from_config(cfg: &SecretsConfig, config_path: Option<PathBuf>) -> Self {
        Self {
            conns: cfg.connections.clone(),
            external: cfg.external.clone(),
            config_path,
            factories: HashMap::new(),
            ready: false,
            tools: cfg.tools,
            send: None,
            artifacts: None,
        }
    }
}

impl SecretsActor {
    /// §7: the `secret_external_*` tools and the prompt paragraph, once a connection exists and
    /// the agent surface is on. Re-registering refreshes the paragraph's connection list. §12:
    /// with a Bitwarden connection, the Send context (kept for the RPCs) and, with the agent
    /// surface on, the Send tools.
    async fn register_tools(&mut self, me: ActorRef<SecretsActor>) {
        if self.connections.conns.is_empty() {
            return;
        }
        let cx = crate::tools_external::ExtCx {
            actor: me,
            vault: self.vault.clone(),
            externals: self.externals.clone(),
        };
        let bw = self.connections.conns.values().any(|c| c.backend == "bw");
        let fresh_send = bw && self.connections.send.is_none();
        if fresh_send {
            self.connections.send = Some(crate::tools_send::SendCx::new(
                cx.clone(),
                self.timeout,
                self.connections.artifacts.clone(),
            ));
        }
        if !self.connections.tools {
            return;
        }
        let Some(reg) = registry() else { return };
        let names: Vec<String> = self.connections.conns.keys().cloned().collect();
        crate::tools_external::register(&reg, cx, &names).await;
        if fresh_send && let Some(send) = &self.connections.send {
            crate::tools_send::register(&reg, send.clone()).await;
        }
    }
}

/// `ask` → `Option<SendCx>`: the Send context, once a Bitwarden connection exists (§12 RPCs).
#[derive(Clone, Copy, Debug)]
pub struct GetSendCx;

impl Message<GetSendCx> for SecretsActor {
    type Reply = Option<crate::tools_send::SendCx>;

    async fn handle(&mut self, _: GetSendCx, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.connections.send.clone()
    }
}

/// `ask` → `()`: the engine's artifact lookup for `artifact:<id>` Send sources (§12.3); `secrets`
/// cannot depend on `artifact` (cycle), so the binary sends it at start. The Send tools are
/// re-registered with it when they already exist.
#[derive(Clone)]
pub struct SetArtifactSource(pub Arc<dyn crate::tools_send::ArtifactSource>);

impl Message<SetArtifactSource> for SecretsActor {
    type Reply = ();

    async fn handle(&mut self, m: SetArtifactSource, _: &mut Context<Self, Self::Reply>) {
        self.connections.artifacts = Some(m.0.clone());
        let Some(send) = &mut self.connections.send else {
            return;
        };
        send.artifacts = Some(m.0);
        if self.connections.tools
            && let Some(reg) = registry()
        {
            crate::tools_send::register(&reg, send.clone()).await;
        }
    }
}

fn registry() -> Option<ActorRef<::tools::ToolRegistry>> {
    ActorRef::<::tools::ToolRegistry>::lookup(::tools::REGISTRY_ACTOR)
        .ok()
        .flatten()
}

impl SecretsActor {
    /// The connection `conn`, split into its config and settings.
    fn conn_parts(&self, conn: &str) -> Result<(ConnConfig, ConnSettings), String> {
        let stored = self
            .connections
            .conns
            .get(conn)
            .ok_or_else(|| format!("unknown connection {conn}"))?;
        Ok((ConnConfig::of(stored)?, stored.settings.clone()))
    }

    /// Persist `secrets.connections` whole.
    fn persist_connections(&self) -> Result<(), String> {
        let Some(path) = &self.connections.config_path else {
            return Ok(());
        };
        let value = serde_json::to_string(&self.connections.conns).map_err(|e| e.to_string())?;
        runtime::write_config_value(path, "secrets", "connections", &value)
            .map_err(|e| format!("{} could not be written: {e}", path.display()))
    }

    /// Build and register `conn` when its backend has a factory and seeding is done.
    fn build(&self, conn: &str) {
        if !self.connections.ready {
            return;
        }
        let Ok((cfg, settings)) = self.conn_parts(conn) else {
            return;
        };
        let Some(f) = self.connections.factories.get(cfg.backend()) else {
            return;
        };
        // A hand-edited config with a clear-text server URL is never built (R19).
        if let Err(e) = cfg.check_urls() {
            tracing::warn!("secrets: connection {conn} not built: {e}");
            return;
        }
        let store = ConnStore::new(self.vault.clone(), cfg.backend(), conn);
        match f.build(conn, &cfg, store) {
            Ok(b) => {
                b.apply_settings(&settings);
                self.externals.set_account(conn, cfg.summary());
                if let Err(e) = self.externals.insert(b) {
                    tracing::warn!("secrets: connection {conn}: {e}");
                }
            }
            Err(e) => tracing::warn!("secrets: connection {conn} not built: {e}"),
        }
    }

    /// Add the always-ask entries of `backend/conn` (harness-wide) and stamp them seeded.
    /// Without a tool registry (`secrets.tools: false`) nothing is seeded and nothing stamped, so
    /// a later start with the tools on still seeds (R4).
    async fn seed(&self, backend: &str, conn: &str) -> Result<(), String> {
        let Some(reg) = registry() else {
            return Ok(());
        };
        ::tools::secrets::seed_ask(&reg, backend, conn).await?;
        crate::store::meta_put(self.vault.pool(), &seeded_stamp(conn), b"1")
            .await
            .map_err(|e| format!("{e:#}"))
    }

    /// `conn` as the UIs show it.
    async fn info(&self, conn: &str) -> Result<ConnInfo, String> {
        let (cfg, settings) = self.conn_parts(conn)?;
        let global = match registry() {
            Some(reg) => reg
                .ask(::tools::GetAlwaysAsk { session: None })
                .await
                .map(|s| s.global)
                .unwrap_or_default(),
            None => Vec::new(),
        };
        let on = |d| global.contains(&::tools::secrets::always_ask_key(cfg.backend(), conn, d));
        use ::tools::secrets::Direction;
        let status = match self.externals.by_conn(conn) {
            Some(b) => b.status(),
            None => ConnStatus::Unsupported(format!("this build has no {} backend", cfg.backend())),
        };
        Ok(ConnInfo {
            conn: conn.to_owned(),
            backend: cfg.backend().to_owned(),
            status,
            summary: cfg.summary(),
            ask: AskInfo {
                read: on(Direction::Read),
                write: on(Direction::Write),
                delete: on(Direction::Delete),
                send: Direction::of(cfg.backend())
                    .contains(&Direction::Send)
                    .then(|| on(Direction::Send)),
            },
            login: match &cfg {
                ConnConfig::Bw { login, .. } => Some(*login),
                ConnConfig::Aws { .. } => None,
            },
            settings,
        })
    }
}

/// `ask` → `Arc<Externals>`: the registered connections, for the RPC methods that call a backend
/// (the network call runs outside the actor).
#[derive(Clone, Copy, Debug)]
pub struct GetExternals;

impl Message<GetExternals> for SecretsActor {
    type Reply = Arc<Externals>;

    async fn handle(&mut self, _: GetExternals, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.externals.clone()
    }
}

impl Message<RegisterBackendFactory> for SecretsActor {
    type Reply = ();

    async fn handle(
        &mut self,
        m: RegisterBackendFactory,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let backend = m.0.backend();
        self.connections.factories.insert(backend, m.0);
        let conns: Vec<String> = self
            .connections
            .conns
            .iter()
            .filter(|(_, c)| c.backend == backend)
            .map(|(n, _)| n.clone())
            .collect();
        for conn in conns {
            self.build(&conn);
        }
    }
}

/// `ask` → `()`: §6.3 "seeding by hand": every configured connection without the `ext/seeded/`
/// stamp gets its always-ask entries now (a hand-edited `config.yaml`), then every connection
/// with a factory is built. Sent once at start, after the tool registry is up (or at once when
/// the agent surface is off: nothing reads placeholders then).
#[derive(Clone, Copy, Debug)]
pub struct SeedConnections;

impl Message<SeedConnections> for SecretsActor {
    type Reply = ();

    async fn handle(&mut self, _: SeedConnections, ctx: &mut Context<Self, Self::Reply>) {
        let conns: Vec<(String, String)> = self
            .connections
            .conns
            .iter()
            .map(|(n, c)| (n.clone(), c.backend.clone()))
            .collect();
        for (conn, backend) in &conns {
            let stamped = crate::store::meta_get(self.vault.pool(), &seeded_stamp(conn)).await;
            if matches!(stamped, Ok(Some(_))) {
                continue;
            }
            if let Err(e) = self.seed(backend, conn).await {
                // Unseeded, the connection stays unregistered: references to it fail closed.
                tracing::warn!("secrets: connection {conn} not seeded, not registered: {e}");
                continue;
            }
        }
        self.connections.ready = true;
        // Without the tool registry nothing reads placeholders: the connections serve the
        // owner's UI unseeded (R4).
        let tools_off = registry().is_none();
        for (conn, _) in conns {
            let stamped = crate::store::meta_get(self.vault.pool(), &seeded_stamp(&conn)).await;
            if tools_off || matches!(stamped, Ok(Some(_))) {
                self.build(&conn);
            }
        }
        self.register_tools(ctx.actor_ref().clone()).await;
    }
}

/// `ask` → `Result<Vec<ConnInfo>, String>`: every configured connection, by name.
#[derive(Clone, Copy, Debug)]
pub struct ListConnections;

impl Message<ListConnections> for SecretsActor {
    type Reply = Result<Vec<ConnInfo>, String>;

    async fn handle(
        &mut self,
        _: ListConnections,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let mut out = Vec::new();
        for conn in self.connections.conns.keys() {
            out.push(self.info(conn).await?);
        }
        Ok(out)
    }
}

/// `ask` → `Result<ConnInfo, String>`: one connection.
#[derive(Clone, Debug)]
pub struct ConnInfoOf(pub String);

impl Message<ConnInfoOf> for SecretsActor {
    type Reply = Result<ConnInfo, String>;

    async fn handle(&mut self, m: ConnInfoOf, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.info(&m.0).await
    }
}

/// `ask` → `Result<String, String>`: §9.6, the SSO identifier `connection_sso_begin` uses for
/// `conn`. `set: Some` stores it in `config.yaml` first; `None` reads the stored one. `Err` for a
/// connection that does not log in with SSO, or has no identifier.
#[derive(Clone, Debug)]
pub struct SsoIdentifier {
    /// The connection.
    pub conn: String,
    /// A new identifier to store.
    pub set: Option<String>,
}

impl Message<SsoIdentifier> for SecretsActor {
    type Reply = Result<String, String>;

    async fn handle(
        &mut self,
        m: SsoIdentifier,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (cfg, _) = self.conn_parts(&m.conn)?;
        let ConnConfig::Bw {
            login: BwLogin::Sso,
            org_identifier,
            ..
        } = cfg
        else {
            return Err(format!("{} does not log in with SSO", m.conn));
        };
        let Some(set) = m.set.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) else {
            return org_identifier.filter(|s| !s.is_empty()).ok_or_else(|| {
                format!("{} has no SSO identifier; enter the organisation's", m.conn)
            });
        };
        if org_identifier.as_deref() == Some(set.as_str()) {
            return Ok(set);
        }
        let entry = self
            .connections
            .conns
            .get_mut(&m.conn)
            .ok_or_else(|| format!("unknown connection {}", m.conn))?;
        let old = entry.other.insert(
            "org_identifier".into(),
            serde_yaml::Value::String(set.clone()),
        );
        if let Err(e) = self.persist_connections() {
            if let Some(entry) = self.connections.conns.get_mut(&m.conn) {
                match old {
                    Some(v) => entry.other.insert("org_identifier".into(), v),
                    None => entry.other.remove("org_identifier"),
                };
            }
            return Err(e);
        }
        Ok(set)
    }
}

/// `ask` → `Result<ConnSettings, String>`: one connection's D20 settings.
#[derive(Clone, Debug)]
pub struct ConnSettingsOf(pub String);

impl Message<ConnSettingsOf> for SecretsActor {
    type Reply = Result<ConnSettings, String>;

    async fn handle(
        &mut self,
        m: ConnSettingsOf,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.conn_parts(&m.0).map(|(_, s)| s)
    }
}

/// `ask` → `Result<Arc<dyn ExternalBackend>, String>`: a throwaway backend for `cfg` (§9.1 "Test"),
/// never registered; the caller probes it.
#[derive(Clone, Debug)]
pub struct BuildProbe {
    /// The (new) connection's name.
    pub conn: String,
    /// Its config.
    pub cfg: ConnConfig,
}

impl Message<BuildProbe> for SecretsActor {
    type Reply = Result<Arc<dyn ExternalBackend>, String>;

    async fn handle(&mut self, m: BuildProbe, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        m.cfg.check_urls()?;
        let backend = m.cfg.backend();
        let f = self
            .connections
            .factories
            .get(backend)
            .ok_or_else(|| format!("this build has no {backend} backend"))?;
        let store = ConnStore::new(self.vault.clone(), backend, &m.conn);
        f.build(&m.conn, &m.cfg, store)
    }
}

/// `ask` → `Result<ConnInfo, String>`: §6.3 `add_secret_connection`. Seeds the always-ask entries
/// first, then writes `config.yaml`, then registers the backend.
#[derive(Clone, Debug)]
pub struct AddConnection {
    /// `[a-z0-9-]{1,32}`, unique across backends.
    pub conn: String,
    /// Its config; settings start at their defaults.
    pub cfg: ConnConfig,
}

impl Message<AddConnection> for SecretsActor {
    type Reply = Result<ConnInfo, String>;

    async fn handle(
        &mut self,
        m: AddConnection,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if !valid_conn(&m.conn) {
            return Err(format!(
                "invalid connection name {:?}: [a-z0-9-], 1 to 32 characters",
                m.conn
            ));
        }
        if self.connections.conns.contains_key(&m.conn) {
            return Err(format!("connection {} exists", m.conn));
        }
        m.cfg.check_urls()?;
        let stored = m.cfg.stored(&ConnSettings::default())?;
        self.seed(m.cfg.backend(), &m.conn).await?;
        self.connections.conns.insert(m.conn.clone(), stored);
        if let Err(e) = self.persist_connections() {
            self.connections.conns.remove(&m.conn);
            return Err(e);
        }
        self.build(&m.conn);
        self.register_tools(ctx.actor_ref().clone()).await;
        self.info(&m.conn).await
    }
}

/// `ask` → `Result<(), String>`: §6.3 `remove_secret_connection` (the RPC checks the human press).
/// Logs out, drops the backend, its `meta` rows, its always-ask entries and its config.
#[derive(Clone, Debug)]
pub struct RemoveConnection(pub String);

impl Message<RemoveConnection> for SecretsActor {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RemoveConnection,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let conn = m.0;
        let (cfg, _) = self.conn_parts(&conn)?;
        let backend = cfg.backend();
        // §12.4: the connection's live Sends are revoked before its credentials go.
        if backend == "bw"
            && let Some(send) = &self.connections.send
        {
            for e in crate::tools_send::revoke_all(send, &conn).await {
                tracing::warn!("secrets: removing {conn}: Send not revoked: {e}");
            }
        }
        if let Some(b) = self.externals.remove(backend, &conn) {
            b.logout().await;
            b.lock().await;
        }
        ConnStore::new(self.vault.clone(), backend, &conn)
            .clear()
            .await?;
        self.connections.conns.remove(&conn);
        self.persist_connections()?;
        // Not an un-gating: the connection is gone (§6.3).
        if let Some(reg) = registry() {
            for tool in ::tools::secrets::ask_keys(backend, &conn) {
                let _ = reg
                    .ask(::tools::SetAlwaysAsk {
                        session: None,
                        tool,
                        on: false,
                    })
                    .await;
            }
        }
        crate::store::meta_delete(self.vault.pool(), &seeded_stamp(&conn))
            .await
            .map_err(|e| format!("{e:#}"))
    }
}

/// `ask` → `Result<bool, String>` (whether the change loosens a gate): set one D20 key of `conn`
/// (`conn: None` = harness-wide `secrets.external`). With `dry_run` nothing changes: the RPC asks
/// first, raises the human-only card for a loosening from a non-press caller, then commits.
#[derive(Clone, Debug)]
pub struct SetSetting {
    /// The connection, or `None` for `secrets.external`.
    pub conn: Option<String>,
    /// The D20 key.
    pub key: String,
    /// Its new value.
    pub value: serde_json::Value,
    /// Only classify.
    pub dry_run: bool,
}

impl Message<SetSetting> for SecretsActor {
    type Reply = Result<bool, String>;

    async fn handle(&mut self, m: SetSetting, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let Some(conn) = m.conn else {
            let (next, loosens) = settings::set(&self.connections.external, &m.key, m.value)?;
            if m.dry_run {
                return Ok(loosens);
            }
            if let Some(path) = &self.connections.config_path {
                let value = serde_json::to_string(
                    &serde_json::to_value(&next).map_err(|e| e.to_string())?[&m.key],
                )
                .map_err(|e| e.to_string())?;
                runtime::write_nested_config_value(path, "secrets", "external", &m.key, &value)
                    .map_err(|e| format!("{} could not be written: {e}", path.display()))?;
            }
            self.connections.external = next;
            return Ok(loosens);
        };
        let (_, current) = self.conn_parts(&conn)?;
        let (next, loosens) = settings::set(&current, &m.key, m.value)?;
        if m.dry_run {
            return Ok(loosens);
        }
        let entry = self
            .connections
            .conns
            .get_mut(&conn)
            .ok_or_else(|| format!("unknown connection {conn}"))?;
        let old = std::mem::replace(&mut entry.settings, next.clone());
        if let Err(e) = self.persist_connections() {
            if let Some(entry) = self.connections.conns.get_mut(&conn) {
                entry.settings = old;
            }
            return Err(e);
        }
        if let Some(b) = self.externals.by_conn(&conn) {
            b.apply_settings(&next);
        }
        Ok(loosens)
    }
}

/// `ask` → [`ExternalSettings`]: `secrets.external`.
#[derive(Clone, Copy, Debug)]
pub struct GetExternalSettings;

impl Message<GetExternalSettings> for SecretsActor {
    type Reply = ExternalSettings;

    async fn handle(
        &mut self,
        _: GetExternalSettings,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        self.connections.external.clone()
    }
}

/// `ask` → `Vec<Arc<dyn ExternalBackend>>`: the registered connections whose `lock_on_sleep` is on
/// (the desktop reported sleep or screen lock); the RPC locks them.
#[derive(Clone, Copy, Debug)]
pub struct LockOnSleep;

impl Message<LockOnSleep> for SecretsActor {
    type Reply = Vec<Arc<dyn ExternalBackend>>;

    async fn handle(&mut self, _: LockOnSleep, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.connections
            .conns
            .iter()
            .filter(|(_, c)| c.settings.lock_on_sleep)
            .filter_map(|(n, _)| self.externals.by_conn(n))
            .collect()
    }
}

/// `tell`: append one line to `<data_dir>/logs/secrets-external.log` (§11 audit: reveals,
/// connection changes). Never a value.
#[derive(Clone, Debug)]
pub struct Audit(pub String);

impl Message<Audit> for SecretsActor {
    type Reply = ();

    async fn handle(&mut self, m: Audit, _: &mut Context<Self, Self::Reply>) {
        tracing::info!(target: "secrets::audit", "{}", m.0);
        let dir = self.rt.data_dir.join("logs");
        let line = format!(
            "{} {}\n",
            crate::expiry::rfc3339(platform::time::now_ms()),
            m.0
        );
        let write = || -> std::io::Result<()> {
            use std::io::Write as _;
            std::fs::create_dir_all(&dir)?;
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(dir.join("secrets-external.log"))?
                .write_all(line.as_bytes())
        };
        if let Err(e) = write() {
            tracing::warn!("secrets: audit line not written: {e}");
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use kameo::actor::Spawn;
    use runtime::Runtime;

    use super::*;
    use crate::KeyStore;
    use crate::external::{
        DeleteMode, DeleteOutcome, ExtEntry, ExtError, ExtRef, ExtVersion, ListPage, ListQuery,
        Location, Value,
    };

    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// Run `body` alone on its own runtime: the tests that register process-wide actors
    /// (`tools::registry::start`: the registry and the approval service) must not overlap.
    pub(crate) fn serial<F: Future>(body: F) -> F::Output {
        let _lock = SERIAL.lock().unwrap_or_else(|p| p.into_inner());
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("test runtime")
            .block_on(body)
    }

    pub(crate) const VALUE: &str = "bw-test-value-long";

    /// A `bw` connection that starts locked and unlocks with the password `pw`.
    struct Locked {
        conn: String,
        locked: AtomicBool,
        store: ConnStore,
    }

    #[async_trait::async_trait]
    impl ExternalBackend for Locked {
        fn backend(&self) -> &'static str {
            "bw"
        }
        fn conn(&self) -> &str {
            &self.conn
        }
        fn status(&self) -> ConnStatus {
            if self.locked.load(Ordering::SeqCst) {
                ConnStatus::Locked
            } else {
                ConnStatus::Ready
            }
        }
        async fn probe(&self) -> Result<ConnStatus, ExtError> {
            Ok(self.status())
        }
        async fn list(&self, _: ListQuery) -> Result<ListPage, ExtError> {
            Ok(ListPage::default())
        }
        async fn describe(&self, _: &ExtRef) -> Result<ExtEntry, ExtError> {
            Err(ExtError::NotFound)
        }
        async fn get(&self, _: &ExtRef) -> Result<(Zeroizing<String>, ExtVersion), ExtError> {
            if self.locked.load(Ordering::SeqCst) {
                return Err(ExtError::Locked);
            }
            let v = ExtVersion {
                id: "rev".into(),
                stages: Vec::new(),
                changed: None,
            };
            Ok((Zeroizing::new(VALUE.to_owned()), v))
        }
        async fn create(
            &self,
            _: &Location,
            _: Value<'_>,
        ) -> Result<(ExtRef, ExtVersion), ExtError> {
            Err(ExtError::Unsupported("create".into()))
        }
        async fn update(
            &self,
            _: &ExtRef,
            _: Value<'_>,
            _: Option<&ExtVersion>,
        ) -> Result<ExtVersion, ExtError> {
            Err(ExtError::Unsupported("update".into()))
        }
        async fn delete(
            &self,
            _: &ExtRef,
            _: DeleteMode,
            _: Option<&ExtVersion>,
        ) -> Result<DeleteOutcome, ExtError> {
            Err(ExtError::Unsupported("delete".into()))
        }
        async fn restore(&self, _: &ExtRef) -> Result<ExtVersion, ExtError> {
            Err(ExtError::Unsupported("restore".into()))
        }
        async fn lock(&self) {
            self.locked.store(true, Ordering::SeqCst);
        }
        async fn login(
            &self,
            form: crate::external::LoginForm,
        ) -> Result<crate::external::LoginStep, ExtError> {
            let crate::external::LoginForm::ApiKey { client_secret, .. } = form else {
                return Err(ExtError::Unsupported("password login".into()));
            };
            self.store
                .put("client_secret", client_secret.as_bytes())
                .await
                .map_err(ExtError::Sdk)?;
            Ok(crate::external::LoginStep::Done)
        }
        async fn unlock(&self, password: Zeroizing<String>) -> Result<ConnStatus, ExtError> {
            if password.as_str() != "pw" {
                return Err(ExtError::Denied("wrong password".into()));
            }
            self.locked.store(false, Ordering::SeqCst);
            Ok(ConnStatus::Ready)
        }
    }

    struct Factory;

    impl BackendFactory for Factory {
        fn backend(&self) -> &'static str {
            "bw"
        }
        fn build(
            &self,
            conn: &str,
            _: &ConnConfig,
            store: ConnStore,
        ) -> Result<Arc<dyn ExternalBackend>, String> {
            Ok(Arc::new(Locked {
                conn: conn.to_owned(),
                locked: AtomicBool::new(true),
                store,
            }))
        }
    }

    fn bw() -> ConnConfig {
        ConnConfig::Bw {
            server: BwServer::Eu,
            base_url: None,
            api_url: None,
            identity_url: None,
            web_vault_url: None,
            email: "me@example.org".into(),
            login: BwLogin::ApiKey,
            org_identifier: None,
        }
    }

    /// R19: Bitwarden server URLs are https, or http to a loopback host only.
    #[test]
    fn bitwarden_urls_need_https_off_loopback() {
        let with = |url: &str| ConnConfig::Bw {
            server: BwServer::SelfHosted,
            base_url: Some(url.into()),
            api_url: None,
            identity_url: None,
            web_vault_url: None,
            email: "me@example.org".into(),
            login: BwLogin::ApiKey,
            org_identifier: None,
        };
        for ok in [
            "https://vault.example.org",
            "HTTPS://vault.example.org/",
            "http://localhost:8080",
            "http://127.0.0.1:8000/x",
            "http://[::1]:80",
        ] {
            assert_eq!(with(ok).check_urls(), Ok(()), "{ok}");
        }
        for bad in [
            "http://vault.example.org",
            "http://localhost.evil.org",
            "http://127.0.0.1@evil.org",
            "http://192.168.1.2",
            "vault.example.org",
            "ftp://localhost",
        ] {
            assert!(with(bad).check_urls().is_err(), "{bad}");
        }
        let mut web = bw();
        if let ConnConfig::Bw { web_vault_url, .. } = &mut web {
            *web_vault_url = Some("http://send.example.org".into());
        }
        assert_eq!(
            web.check_urls(),
            Err(
                "web_vault_url \"http://send.example.org\" must be https:// (http:// only for localhost)"
                    .into()
            )
        );
    }

    /// The plugin's start, by hand: the registry (and approval service), the actor over the
    /// config in `dir/config.yaml`, seeding at load, the factory.
    async fn start(
        dir: &std::path::Path,
    ) -> (
        ActorRef<SecretsActor>,
        ActorRef<::tools::ToolRegistry>,
        Arc<Vault>,
    ) {
        let rt = Runtime::new(figment::Figment::new(), dir.to_path_buf());
        let reg = ::tools::registry::start(&rt).await.unwrap();
        let (a, vault) = restart(dir).await;
        (a, reg, vault)
    }

    /// The secrets side only, reading `dir/config.yaml` afresh (an engine restart: the
    /// registry's always-ask set persists in the same file).
    async fn restart(dir: &std::path::Path) -> (ActorRef<SecretsActor>, Arc<Vault>) {
        use figment::providers::Format as _;
        let rt = Runtime::new(
            figment::Figment::from(figment::providers::Yaml::file(dir.join("config.yaml"))),
            dir.to_path_buf(),
        );
        let cfg: SecretsConfig = if rt.config.contains("secrets") {
            rt.section("secrets").unwrap()
        } else {
            SecretsConfig::default()
        };
        let vault = Arc::new(Vault::open(dir, KeyStore::File).await.unwrap());
        let a = SecretsActor::spawn(
            SecretsActor::new(vault.clone(), rt, Duration::from_secs(5))
                .with_config(&cfg, Some(dir.join("config.yaml"))),
        );
        a.ask(SeedConnections).await.unwrap();
        a.ask(RegisterBackendFactory(Arc::new(Factory)))
            .await
            .unwrap();
        (a, vault)
    }

    async fn global(reg: &ActorRef<::tools::ToolRegistry>) -> Vec<String> {
        reg.ask(::tools::GetAlwaysAsk { session: None })
            .await
            .unwrap()
            .global
    }

    const KEYS: [&str; 4] = [
        "secret:bw/personal:delete",
        "secret:bw/personal:read",
        "secret:bw/personal:send",
        "secret:bw/personal:write",
    ];

    /// 221 §13 B4: a connection added by hand-editing config is seeded once at load; an entry
    /// the user removed stays removed after a restart; `add` seeds before it registers, and
    /// `remove` takes the entries, the config and the `meta` rows with it.
    #[test]
    fn hand_edited_connection_is_seeded_once() {
        serial(async {
            let dir = tempfile::tempdir().unwrap();
            std::fs::write(
                dir.path().join("config.yaml"),
                "secrets:\n  connections:\n    personal:\n      backend: bw\n      server: eu\n      email: me@example.org\n",
            )
            .unwrap();
            let (a, reg, _) = start(dir.path()).await;
            assert_eq!(global(&reg).await, KEYS);
            let info = a.ask(ListConnections).await.unwrap();
            assert_eq!(info.len(), 1);
            assert_eq!(info[0].status, ConnStatus::Locked, "built and registered");
            assert_eq!(info[0].summary, "me@example.org · vault.bitwarden.eu");
            assert_eq!(
                info[0].ask,
                AskInfo {
                    read: true,
                    write: true,
                    delete: true,
                    send: Some(true)
                }
            );
            reg.ask(::tools::SetAlwaysAsk {
                session: None,
                tool: KEYS[1].into(),
                on: false,
            })
            .await
            .unwrap();
            a.kill();

            let (a, vault) = restart(dir.path()).await;
            assert!(
                !global(&reg).await.contains(&KEYS[1].to_owned()),
                "stays removed"
            );
            assert!(!a.ask(ListConnections).await.unwrap()[0].ask.read);

            a.ask(AddConnection {
                conn: "work".into(),
                cfg: bw(),
            })
            .await
            .unwrap();
            assert!(
                global(&reg)
                    .await
                    .contains(&"secret:bw/work:read".to_owned())
            );
            let yaml = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
            assert!(yaml.contains("work"), "{yaml}");
            assert!(
                a.ask(AddConnection {
                    conn: "work".into(),
                    cfg: bw(),
                })
                .await
                .is_err(),
                "names are unique"
            );
            let store = ConnStore::new(vault.clone(), "bw", "work");
            store.put("client_secret", b"s").await.unwrap();
            a.ask(RemoveConnection("work".into())).await.unwrap();
            assert!(!global(&reg).await.iter().any(|k| k.contains("bw/work")));
            assert_eq!(store.get("client_secret").await.unwrap(), None);
            let yaml = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
            assert!(!yaml.contains("work"), "{yaml}");
            assert_eq!(a.ask(ListConnections).await.unwrap().len(), 1);
        });
    }

    /// 221 §13 B4: `ext/` rows open only with this engine's key, and never show as secrets.
    #[tokio::test]
    async fn connection_rows_are_engine_local_and_hidden() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let va = Arc::new(Vault::open(a.path(), KeyStore::File).await.unwrap());
        let store = ConnStore::new(va.clone(), "bw", "personal");
        store
            .put("client_secret", b"api-secret-value")
            .await
            .unwrap();
        assert_eq!(
            store
                .get("client_secret")
                .await
                .unwrap()
                .as_deref()
                .map(Vec::as_slice),
            Some(&b"api-secret-value"[..])
        );
        assert!(va.list().is_empty(), "not a vault row");
        assert!(va.get("ext/bw/personal/client_secret", 0).is_none());
        let mut args = serde_json::json!({"a": "{{secret:ext/bw/personal/client_secret}}"});
        assert!(va.expand(&mut args).is_err(), "never expandable");
        // The same row under another engine's key does not open.
        let sealed = crate::store::meta_get(va.pool(), "ext/bw/personal/client_secret")
            .await
            .unwrap()
            .unwrap();
        let vb = Arc::new(Vault::open(b.path(), KeyStore::File).await.unwrap());
        crate::store::meta_put(vb.pool(), "ext/bw/personal/client_secret", &sealed)
            .await
            .unwrap();
        assert!(
            ConnStore::new(vb, "bw", "personal")
                .get("client_secret")
                .await
                .is_err()
        );
    }

    /// 221 §13 B4: reveal from the internal connection is refused with no value and no card;
    /// from the owner's press it returns the value and writes an audit line; with
    /// `allow_reveal` off it is refused. D20 (j): loosening a setting from the internal
    /// connection raises the human-only card (a deny keeps it); tightening asks nothing.
    #[test]
    fn reveal_and_loosening_need_the_owners_press() {
        serial(async {
            let dir = tempfile::tempdir().unwrap();
            let (a, _reg, _) = start(dir.path()).await;
            a.ask(AddConnection {
                conn: "personal".into(),
                cfg: bw(),
            })
            .await
            .unwrap();
            let ext = a.ask(GetExternals).await.unwrap();
            ext.by_conn("personal")
                .unwrap()
                .unlock(Zeroizing::new("pw".into()))
                .await
                .unwrap();
            let r = "bw/personal/2f1c0000-0000-0000-0000-000000000000#password";
            let approval = ActorRef::<::tools::approval::Approval>::lookup(::tools::APPROVAL_ACTOR)
                .unwrap()
                .unwrap();
            let pending = || async { approval.ask(::tools::PendingApprovals).await.unwrap() };

            let internal = {
                let mut x = jsonrpsee::Extensions::new();
                x.insert(rpc_core::RpcPeer::internal());
                x
            };
            let (ui, _guard) = rpc_core::RpcPeer::open();
            let owner = {
                let mut x = jsonrpsee::Extensions::new();
                x.insert(ui);
                x
            };
            let press = crate::rpc::human_press(&owner);
            let not_press = crate::rpc::human_press(&internal);
            assert!(press && !not_press);

            let e = crate::rpc::reveal(&a, not_press, "personal", r)
                .await
                .unwrap_err();
            assert_eq!(e, "reveal is an owner action in the desktop app");
            assert!(pending().await.is_empty(), "no card");
            assert_eq!(
                crate::rpc::reveal(&a, press, "personal", r).await.unwrap(),
                VALUE
            );
            let log =
                std::fs::read_to_string(dir.path().join("logs/secrets-external.log")).unwrap();
            assert!(
                log.contains(&format!("revealed {r}")) && !log.contains(VALUE),
                "{log}"
            );

            // Tightening from the internal connection: no card.
            crate::rpc::set_setting(
                &a,
                not_press,
                Some("personal".into()),
                "allow_reveal".into(),
                false.into(),
            )
            .await
            .unwrap();
            assert!(pending().await.is_empty());
            let e = crate::rpc::reveal(&a, press, "personal", r)
                .await
                .unwrap_err();
            assert!(e.starts_with("reveal is off for personal"), "{e}");

            // Loosening from the internal connection: the human-only card; a deny keeps it off.
            let loosen = tokio::spawn({
                let a = a.clone();
                async move {
                    crate::rpc::set_setting(
                        &a,
                        false,
                        Some("personal".into()),
                        "allow_reveal".into(),
                        true.into(),
                    )
                    .await
                }
            });
            let card = loop {
                if let Some(c) = pending().await.into_iter().next() {
                    break c;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            assert!(card.human_only);
            assert_eq!(card.tool, "secrets.setting");
            approval
                .ask(::tools::Resolve {
                    id: card.id,
                    decision: ::tools::Decision::Deny,
                })
                .await
                .unwrap();
            assert!(loosen.await.unwrap().is_err());
            assert!(
                !a.ask(ConnSettingsOf("personal".into()))
                    .await
                    .unwrap()
                    .allow_reveal
            );
            // The owner's press loosens directly.
            crate::rpc::set_setting(
                &a,
                press,
                Some("personal".into()),
                "allow_reveal".into(),
                true.into(),
            )
            .await
            .unwrap();
            assert!(
                a.ask(ConnSettingsOf("personal".into()))
                    .await
                    .unwrap()
                    .allow_reveal
            );
            assert!(pending().await.is_empty());
            let yaml = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
            let cfg: serde_yaml::Value = serde_yaml::from_str(&yaml).unwrap();
            assert_eq!(
                cfg["secrets"]["connections"]["personal"]["allow_reveal"],
                true
            );
        });
    }

    /// R3/R21: off the owner's press, `copy_send_link` and `create_send` raise a human-only card
    /// and go ahead only on the owner's approval (a deny refuses: no link leaves); the press
    /// needs no card.
    #[test]
    fn send_link_and_create_send_off_press_need_the_owners_approval() {
        serial(async {
            let dir = tempfile::tempdir().unwrap();
            let (a, _reg, _) = start(dir.path()).await;
            a.ask(AddConnection {
                conn: "personal".into(),
                cfg: bw(),
            })
            .await
            .unwrap();
            let approval = ActorRef::<::tools::approval::Approval>::lookup(::tools::APPROVAL_ACTOR)
                .unwrap()
                .unwrap();
            let pending = || async { approval.ask(::tools::PendingApprovals).await.unwrap() };
            let internal = || {
                let mut x = jsonrpsee::Extensions::new();
                x.insert(rpc_core::RpcPeer::internal());
                x
            };
            let (ui, _guard) = rpc_core::RpcPeer::open();
            let mut owner = jsonrpsee::Extensions::new();
            owner.insert(ui);

            crate::rpc::copy_link_gate(&a, &owner, "s1").await.unwrap();
            assert!(pending().await.is_empty(), "a press needs no card");

            for (decision, ok) in [
                (::tools::Decision::Deny, false),
                (::tools::Decision::Allow, true),
            ] {
                let gate = tokio::spawn({
                    let (a, ext) = (a.clone(), internal());
                    async move { crate::rpc::copy_link_gate(&a, &ext, "s1").await }
                });
                let card = loop {
                    if let Some(c) = pending().await.into_iter().next() {
                        break c;
                    }
                    tokio::time::sleep(Duration::from_millis(10)).await;
                };
                assert!(card.human_only);
                assert_eq!(card.tool, "secrets.copy_send_link");
                approval
                    .ask(::tools::Resolve {
                        id: card.id,
                        decision,
                    })
                    .await
                    .unwrap();
                assert_eq!(gate.await.unwrap().is_ok(), ok);
            }

            // `send_owner_only` is on by default: the TUI's create_send waits on the card.
            let gate = tokio::spawn({
                let a = a.clone();
                async move { crate::rpc::create_send_gate(&a, "personal", &serde_json::json!({})).await }
            });
            let card = loop {
                if let Some(c) = pending().await.into_iter().next() {
                    break c;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            };
            assert!(card.human_only);
            assert_eq!(card.tool, "secrets.create_send");
            approval
                .ask(::tools::Resolve {
                    id: card.id,
                    decision: ::tools::Decision::Allow,
                })
                .await
                .unwrap();
            gate.await.unwrap().unwrap();
        });
    }

    /// §9.4: a placeholder on a locked connection raises the unlock card; the typed password
    /// unlocks it and the call resolves; a declined card fails the call.
    #[test]
    fn locked_connection_raises_the_unlock_card() {
        serial(async {
            let dir = tempfile::tempdir().unwrap();
            let (a, _reg, vault) = start(dir.path()).await;
            a.ask(AddConnection {
                conn: "personal".into(),
                cfg: bw(),
            })
            .await
            .unwrap();
            let ext = a.ask(GetExternals).await.unwrap();
            let hook = crate::external::Hook::new(vault, ext.clone(), Duration::from_secs(5));
            let args = serde_json::json!({"c": "{{secret:bw/personal/x#password}}"});
            let cx = || ::tools::secrets::ResolveCx {
                session: runtime::SessionId::new(),
                call_id: "1".into(),
            };
            // Answer every card as it comes: first decline, then the password.
            let answers =
                tokio::spawn({
                    let ext = ext.clone();
                    async move {
                        for pw in [None, Some("pw")] {
                            loop {
                                let ids: Vec<_> =
                                    ext.unlock_requests().into_iter().map(|v| v.id).collect();
                                if let Some(id) = ids.first() {
                                    assert!(ext.answer_unlock(
                                        *id,
                                        pw.map(|p| Zeroizing::new(p.to_owned()))
                                    ));
                                    break;
                                }
                                tokio::time::sleep(Duration::from_millis(10)).await;
                            }
                        }
                    }
                });
            use ::tools::secrets::Secrets as _;
            let e = hook.resolve_external(&args, cx()).await.unwrap_err();
            assert_eq!(e, "bw/personal is locked; the user declined to unlock it");
            let f = hook.resolve_external(&args, cx()).await.unwrap();
            assert_eq!(f.get("{{secret:bw/personal/x#password}}"), Some(VALUE));
            answers.await.unwrap();

            // R17: two calls on a locked connection raise one card; both take its outcome.
            ext.by_conn("personal").unwrap().lock().await;
            let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let answering = tokio::spawn({
                let (ext, done) = (ext.clone(), done.clone());
                async move {
                    let mut cards = 0;
                    while !done.load(std::sync::atomic::Ordering::SeqCst) {
                        for v in ext.unlock_requests() {
                            assert_eq!(v.conn, "personal");
                            if ext.answer_unlock(v.id, Some(Zeroizing::new("pw".to_owned()))) {
                                cards += 1;
                            }
                        }
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    cards
                }
            });
            let (x, y) = tokio::join!(
                hook.resolve_external(&args, cx()),
                hook.resolve_external(&args, cx())
            );
            done.store(true, std::sync::atomic::Ordering::SeqCst);
            assert!(x.is_ok() && y.is_ok());
            assert_eq!(answering.await.unwrap(), 1, "one card for both calls");
        });
    }
}
