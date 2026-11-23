//! External search providers (design doc 16 §3, §6, §8; 38 §5): optional
//! SearXNG, Firecrawl, OpenSERP and the Brave API (`brave_api`), all off
//! unless configured. [`SearchProvidersPlugin`] reads the `search_providers`
//! config section and the secrets vault (37 §3.6: keys `firecrawl`, `brave`,
//! `openserp` by default; each block names its own), builds every enabled
//! provider, and registers each with the `web` plugin's actor over the bus
//! ([`web::RegisterProvider`]). The plugin stays alive as the
//! [`SearchProviders`] actor ([`SEARCH_PROVIDERS_ACTOR`]) so Settings › Search
//! can read, persist and re-apply the provider set at runtime, and so a new
//! key ([`secrets::SecretsEvent::Changed`]) registers its provider without a
//! restart. The one tool this crate owns is [`xai::XSearchTool`] (`search_x`,
//! design doc 80), registered on the tool registry while an `xai` credential
//! exists.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use serde::{Deserialize, Serialize};
use web::{Provider, ProviderError};

/// Hub RPC methods and typed clients.
pub mod rpc_hub;

/// Assemble this plugin's RPC methods.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::merge_described([
        rpc_hub::rpc_module(server),
    ]).expect("search-providers RPC composition")
}

pub mod brave;
pub mod firecrawl;
pub mod openserp;
pub mod searxng;
pub mod xai;

/// Registered name of the [`SearchProviders`] actor.
pub const SEARCH_PROVIDERS_ACTOR: &str = "search-providers";

/// Provider names in the order Settings › Search lists them.
pub const NAMES: [&str; 4] = ["searxng", "openserp", "firecrawl", "brave_api"];

/// `search_providers:` section (16 §6). Each block's presence enables it
/// (plus its `enabled` flag). Nothing is on by default except the Firecrawl
/// block, which stays inert until its secret exists; the keyless engines
/// built into `web` (38 §5) cover search without any of these.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchProvidersConfig {
    /// Hosted or self-hosted Firecrawl; needs its secret.
    pub firecrawl: Option<FirecrawlConfig>,
    /// Self-hosted SearXNG instance; keyless.
    pub searxng: Option<SearxngConfig>,
    /// Self-hosted (or hosted, with a key) OpenSERP.
    pub openserp: Option<OpenserpConfig>,
    /// Brave Search API; needs its secret.
    pub brave: Option<BraveConfig>,
    /// `search_x` through xAI (80); needs the `xai` key or login.
    pub x: Option<xai::XConfig>,
}

impl Default for SearchProvidersConfig {
    fn default() -> Self {
        Self {
            firecrawl: Some(FirecrawlConfig::default()),
            searxng: None,
            openserp: None,
            brave: None,
            x: Some(xai::XConfig::default()),
        }
    }
}

/// `search_providers.firecrawl`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct FirecrawlConfig {
    /// Set `false` to keep the block but skip the provider.
    pub enabled: bool,
    /// API root; `http://127.0.0.1:3002` for self-hosted.
    pub base_url: String,
    /// Vault key holding the API key.
    pub secret: String,
    /// Deadline for one search.
    pub timeout_ms: u64,
    /// Registration order among external providers (lower first).
    pub priority: u32,
}

impl Default for FirecrawlConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "https://api.firecrawl.dev".into(),
            secret: "firecrawl".into(),
            timeout_ms: 8000,
            priority: 30,
        }
    }
}

/// `search_providers.searxng`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SearxngConfig {
    /// Set `false` to keep the block but skip the provider.
    pub enabled: bool,
    /// Instance root, e.g. `http://127.0.0.1:8080`; empty = not configured.
    pub base_url: String,
    /// Deadline for one search.
    pub timeout_ms: u64,
    /// Registration order among external providers (lower first).
    pub priority: u32,
}

impl Default for SearxngConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: String::new(),
            timeout_ms: 6000,
            priority: 10,
        }
    }
}

/// `search_providers.openserp`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OpenserpConfig {
    /// Set `false` to keep the block but skip the provider.
    pub enabled: bool,
    /// Server root, e.g. `http://127.0.0.1:7000`.
    pub base_url: String,
    /// Upstream engines for `/mega/search`.
    pub engines: Vec<String>,
    /// Vault key holding the hosted-API key; unused for self-hosted.
    pub secret: String,
    /// Deadline for one search (OpenSERP drives real browsers: allow seconds).
    pub timeout_ms: u64,
    /// Registration order among external providers (lower first).
    pub priority: u32,
}

impl Default for OpenserpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            base_url: "http://127.0.0.1:7000".into(),
            engines: vec!["google".into(), "bing".into(), "duckduckgo".into()],
            secret: "openserp".into(),
            timeout_ms: 8000,
            priority: 20,
        }
    }
}

/// `search_providers.brave`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BraveConfig {
    /// Set `false` to keep the block but skip the provider.
    pub enabled: bool,
    /// Vault key holding the subscription token.
    pub secret: String,
    /// Deadline for one search.
    pub timeout_ms: u64,
    /// Registration order among external providers (lower first).
    pub priority: u32,
}

impl Default for BraveConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            secret: "brave".into(),
            timeout_ms: 5000,
            priority: 40,
        }
    }
}

/// One provider as Settings › Search edits it (wire form).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProviderSettings {
    /// `searxng`, `openserp`, `firecrawl` or `brave_api`.
    pub name: String,
    /// Block present and enabled.
    pub enabled: bool,
    /// Server root; `None` for Brave (fixed endpoint).
    pub base_url: Option<String>,
    /// Vault key holding the credential; `None` for SearXNG (keyless).
    pub secret: Option<String>,
    /// The vault has a value under `secret` (never the value itself).
    pub secret_set: bool,
    /// Deadline for one search.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub timeout_ms: u64,
    /// Registration order (lower first).
    pub priority: u32,
    /// OpenSERP upstream engines; `None` elsewhere.
    pub engines: Option<Vec<String>>,
    /// Registered with `web` right now (enabled, and keyed when it needs a key).
    pub registered: bool,
}

/// Everything Settings › Search shows: providers plus the `web` defaults.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SearchSettings {
    /// One entry per known provider, sorted by priority.
    pub providers: Vec<ProviderSettings>,
    /// `web:` read/budget defaults.
    pub defaults: web::settings::SearchDefaults,
}

/// Non-empty, trimmed credential.
fn key<'a>(credentials: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    credentials
        .get(name)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

/// Every provider the config enables and the credentials allow, sorted by
/// `priority`. Blocks that need a key and have none, or a SearXNG block
/// without a `base_url`, are skipped with a log line.
#[must_use]
pub fn build(
    cfg: &SearchProvidersConfig,
    credentials: &HashMap<String, String>,
) -> Vec<Arc<dyn Provider>> {
    let mut out: Vec<(u32, Arc<dyn Provider>)> = Vec::new();
    if let Some(sx) = cfg.searxng.as_ref().filter(|c| c.enabled) {
        if sx.base_url.trim().is_empty() {
            tracing::info!("searxng: enabled without base_url; skipped");
        } else {
            out.push((
                sx.priority,
                Arc::new(searxng::Searxng::new(
                    &sx.base_url,
                    Duration::from_millis(sx.timeout_ms),
                )),
            ));
        }
    }
    if let Some(fc) = cfg.firecrawl.as_ref().filter(|c| c.enabled) {
        match key(credentials, &fc.secret) {
            Some(k) => out.push((
                fc.priority,
                Arc::new(firecrawl::Firecrawl::new(
                    &fc.base_url,
                    k,
                    Duration::from_millis(fc.timeout_ms),
                )),
            )),
            None => tracing::info!(
                secret = %fc.secret,
                "firecrawl: no secret; optional — the built-in engines cover search without it"
            ),
        }
    }
    if let Some(os) = cfg.openserp.as_ref().filter(|c| c.enabled) {
        out.push((
            os.priority,
            Arc::new(openserp::Openserp::new(
                &os.base_url,
                &os.engines,
                key(credentials, &os.secret),
                Duration::from_millis(os.timeout_ms),
            )),
        ));
    }
    if let Some(b) = cfg.brave.as_ref().filter(|c| c.enabled) {
        match key(credentials, &b.secret) {
            Some(k) => out.push((
                b.priority,
                Arc::new(brave::Brave::new(k, Duration::from_millis(b.timeout_ms))),
            )),
            None => tracing::warn!(
                secret = %b.secret,
                "search-providers: brave_api needs its secret (Settings > Search, /secrets add or `t --secret-set brave`); skipped"
            ),
        }
    }
    out.sort_by_key(|(p, _)| *p);
    out.into_iter().map(|(_, p)| p).collect()
}

/// Wire view of `cfg` given which secrets exist and which providers are registered.
#[must_use]
pub fn settings_view(
    cfg: &SearchProvidersConfig,
    secret_set: &dyn Fn(&str) -> bool,
    registered: &[String],
) -> Vec<ProviderSettings> {
    let is_reg = |n: &str| registered.iter().any(|r| r == n);
    let sx = cfg.searxng.clone().unwrap_or(SearxngConfig {
        enabled: false,
        ..SearxngConfig::default()
    });
    let os = cfg.openserp.clone().unwrap_or(OpenserpConfig {
        enabled: false,
        ..OpenserpConfig::default()
    });
    let fc = cfg.firecrawl.clone().unwrap_or(FirecrawlConfig {
        enabled: false,
        ..FirecrawlConfig::default()
    });
    let b = cfg.brave.clone().unwrap_or(BraveConfig {
        enabled: false,
        ..BraveConfig::default()
    });
    let mut v = vec![
        ProviderSettings {
            name: "searxng".into(),
            enabled: sx.enabled,
            base_url: Some(sx.base_url),
            secret: None,
            secret_set: false,
            timeout_ms: sx.timeout_ms,
            priority: sx.priority,
            engines: None,
            registered: is_reg("searxng"),
        },
        ProviderSettings {
            name: "openserp".into(),
            enabled: os.enabled,
            base_url: Some(os.base_url),
            secret_set: secret_set(&os.secret),
            secret: Some(os.secret),
            timeout_ms: os.timeout_ms,
            priority: os.priority,
            engines: Some(os.engines),
            registered: is_reg("openserp"),
        },
        ProviderSettings {
            name: "firecrawl".into(),
            enabled: fc.enabled,
            base_url: Some(fc.base_url),
            secret_set: secret_set(&fc.secret),
            secret: Some(fc.secret),
            timeout_ms: fc.timeout_ms,
            priority: fc.priority,
            engines: None,
            registered: is_reg("firecrawl"),
        },
        ProviderSettings {
            name: "brave_api".into(),
            enabled: b.enabled,
            base_url: None,
            secret_set: secret_set(&b.secret),
            secret: Some(b.secret),
            timeout_ms: b.timeout_ms,
            priority: b.priority,
            engines: None,
            registered: is_reg("brave_api"),
        },
    ];
    v.sort_by_key(|p| p.priority);
    v
}

/// Validate and fold `edits` into `cfg`; every edited block becomes present.
/// `Err` names the offending field.
pub fn apply_settings(
    cfg: &mut SearchProvidersConfig,
    edits: &[ProviderSettings],
) -> Result<(), String> {
    for e in edits {
        if !(500..=60_000).contains(&e.timeout_ms) {
            return Err(format!("{}: timeout_ms must be 500..=60000", e.name));
        }
        let url = e.base_url.as_deref().map(str::trim).unwrap_or_default();
        if !(url.is_empty() || url.starts_with("http://") || url.starts_with("https://")) {
            return Err(format!(
                "{}: base_url must start with http:// or https://",
                e.name
            ));
        }
        let secret = e
            .secret
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        match e.name.as_str() {
            "searxng" => {
                let c = cfg.searxng.get_or_insert_with(SearxngConfig::default);
                c.enabled = e.enabled;
                c.base_url = url.trim_end_matches('/').to_owned();
                c.timeout_ms = e.timeout_ms;
                c.priority = e.priority;
            }
            "openserp" => {
                let c = cfg.openserp.get_or_insert_with(OpenserpConfig::default);
                c.enabled = e.enabled;
                if !url.is_empty() {
                    c.base_url = url.trim_end_matches('/').to_owned();
                }
                c.secret = secret.unwrap_or_else(|| "openserp".into());
                c.timeout_ms = e.timeout_ms;
                c.priority = e.priority;
                if let Some(engines) = &e.engines {
                    let engines: Vec<String> = engines
                        .iter()
                        .map(|s| s.trim().to_ascii_lowercase())
                        .filter(|s| !s.is_empty())
                        .collect();
                    if !engines.is_empty() {
                        c.engines = engines;
                    }
                }
            }
            "firecrawl" => {
                let c = cfg.firecrawl.get_or_insert_with(FirecrawlConfig::default);
                c.enabled = e.enabled;
                if !url.is_empty() {
                    c.base_url = url.trim_end_matches('/').to_owned();
                }
                c.secret = secret.unwrap_or_else(|| "firecrawl".into());
                c.timeout_ms = e.timeout_ms;
                c.priority = e.priority;
            }
            "brave_api" => {
                let c = cfg.brave.get_or_insert_with(BraveConfig::default);
                c.enabled = e.enabled;
                c.secret = secret.unwrap_or_else(|| "brave".into());
                c.timeout_ms = e.timeout_ms;
                c.priority = e.priority;
            }
            other => {
                return Err(format!(
                    "unknown provider {other}; known: {}",
                    NAMES.join(", ")
                ));
            }
        }
    }
    Ok(())
}

fn yaml_str(s: &str) -> String {
    serde_json::to_string(s).expect("a str serialises to JSON")
}

/// Persist `cfg` under `search_providers:` in `path`, one scalar per line
/// (comments elsewhere in the file survive). A block written as a flow
/// mapping (`brave: {}`) is opened into block form first.
pub fn persist(path: &std::path::Path, cfg: &SearchProvidersConfig) -> std::io::Result<()> {
    open_flow_blocks(path)?;
    let w = |block: &str, key: &str, value: String| {
        runtime::write_nested_config_value(path, "search_providers", block, key, &value)
    };
    if let Some(c) = &cfg.searxng {
        w("searxng", "enabled", c.enabled.to_string())?;
        w("searxng", "base_url", yaml_str(&c.base_url))?;
        w("searxng", "timeout_ms", c.timeout_ms.to_string())?;
        w("searxng", "priority", c.priority.to_string())?;
    }
    if let Some(c) = &cfg.openserp {
        w("openserp", "enabled", c.enabled.to_string())?;
        w("openserp", "base_url", yaml_str(&c.base_url))?;
        w("openserp", "engines", format!("[{}]", c.engines.join(", ")))?;
        w("openserp", "secret", yaml_str(&c.secret))?;
        w("openserp", "timeout_ms", c.timeout_ms.to_string())?;
        w("openserp", "priority", c.priority.to_string())?;
    }
    if let Some(c) = &cfg.firecrawl {
        w("firecrawl", "enabled", c.enabled.to_string())?;
        w("firecrawl", "base_url", yaml_str(&c.base_url))?;
        w("firecrawl", "secret", yaml_str(&c.secret))?;
        w("firecrawl", "timeout_ms", c.timeout_ms.to_string())?;
        w("firecrawl", "priority", c.priority.to_string())?;
    }
    if let Some(c) = &cfg.brave {
        w("brave", "enabled", c.enabled.to_string())?;
        w("brave", "secret", yaml_str(&c.secret))?;
        w("brave", "timeout_ms", c.timeout_ms.to_string())?;
        w("brave", "priority", c.priority.to_string())?;
    }
    if let Some(c) = &cfg.x {
        w("x", "enabled", c.enabled.to_string())?;
        w("x", "model", yaml_str(&c.model))?;
        w("x", "timeout_ms", c.timeout_ms.to_string())?;
    }
    Ok(())
}

/// `  block: {}` / `  block: { … }` lines under `search_providers:` become
/// `  block:` so [`runtime::write_nested_config_value`] can add keys.
fn open_flow_blocks(path: &std::path::Path) -> std::io::Result<()> {
    let text = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let mut changed = false;
    let mut in_section = false;
    let lines: Vec<String> = text
        .lines()
        .map(|l| {
            if !l.starts_with(char::is_whitespace) {
                in_section = l.trim_end() == "search_providers:";
                return l.to_owned();
            }
            if in_section
                && l.len() - l.trim_start().len() == 2
                && let Some((k, v)) = l.trim().split_once(':')
                && v.trim().starts_with('{')
                && v.trim().ends_with('}')
            {
                changed = true;
                return format!("  {k}:");
            }
            l.to_owned()
        })
        .collect();
    if changed {
        std::fs::write(path, lines.join("\n") + "\n")?;
    }
    Ok(())
}

/// Ask → `Vec<ProviderSettings>` (secret presence, registration state).
#[derive(Clone, Copy, Debug, Default)]
pub struct GetProviders;

/// Ask → `Result<Vec<ProviderSettings>, String>`: validate, persist to
/// config.yaml, re-register with `web`. A failed write changes nothing.
#[derive(Clone, Debug)]
pub struct SetProviders(pub Vec<ProviderSettings>);

/// The `x` block when `search_x` should be advertised: enabled and an `xai`
/// credential exists (80 §0.5).
#[must_use]
pub fn advertise_x(cfg: &SearchProvidersConfig, has_credential: bool) -> Option<&xai::XConfig> {
    cfg.x.as_ref().filter(|c| c.enabled && has_credential)
}

/// Owns the provider config at runtime: builds providers from config +
/// vault, (un)registers them with the `web` actor, persists edits; registers
/// `search_x` on the tool registry while an `xai` credential exists.
#[derive(kameo::Actor)]
pub struct SearchProviders {
    cfg: SearchProvidersConfig,
    config_path: PathBuf,
    vault: Option<Arc<secrets::Vault>>,
    oauth: oauth::Store,
    env_key: Option<String>,
    web: ActorRef<web::WebActor>,
    registry: Option<ActorRef<tools::ToolRegistry>>,
    registered: Vec<String>,
    x_registered: bool,
}

impl SearchProviders {
    fn credentials(&self) -> HashMap<String, String> {
        let names = [
            self.cfg.firecrawl.as_ref().map(|c| c.secret.clone()),
            self.cfg.openserp.as_ref().map(|c| c.secret.clone()),
            self.cfg.brave.as_ref().map(|c| c.secret.clone()),
        ];
        names
            .into_iter()
            .flatten()
            .filter_map(|k| {
                let v = self.vault.as_ref()?.get(&k, 0)?;
                Some((k, v.as_str().to_owned()))
            })
            .collect()
    }

    fn secret_set(&self, name: &str) -> bool {
        self.vault
            .as_ref()
            .is_some_and(|v| v.get(name, 0).is_some())
    }

    /// Register what the config + vault allow; unregister what no longer does.
    async fn sync(&mut self) {
        let providers = build(&self.cfg, &self.credentials());
        let now: Vec<String> = providers.iter().map(|p| p.meta().name.to_owned()).collect();
        for gone in self.registered.iter().filter(|n| !now.contains(n)) {
            if let Err(e) = self.web.ask(web::UnregisterProvider(gone.clone())).await {
                tracing::warn!("search-providers: unregistering {gone} failed: {e}");
            }
        }
        for p in providers {
            let name = p.meta().name;
            if let Err(e) = self.web.ask(web::RegisterProvider(p)).await {
                tracing::warn!("search-providers: registering {name} failed: {e}");
            }
        }
        self.registered = now;
        self.sync_x().await;
    }

    /// Register or unregister `search_x` + its prompt block as the `xai`
    /// credential (vault key, login grant or env) comes and goes.
    async fn sync_x(&mut self) {
        let Some(reg) = &self.registry else { return };
        let has_credential = self.secret_set(xai::NS)
            || self.oauth.load(xai::NS).is_some()
            || self.env_key.is_some();
        match advertise_x(&self.cfg, has_credential) {
            Some(x) if !self.x_registered => {
                let tool = xai::XSearchTool::new(
                    x.clone(),
                    self.vault.clone(),
                    self.oauth.clone(),
                    self.env_key.clone(),
                );
                let _ = reg.ask(tools::RegisterTool(Arc::new(tool))).await;
                let _ = reg
                    .ask(tools::RegisterPrompt {
                        plugin: "search-providers",
                        text: xai::PROMPT.to_owned(),
                    })
                    .await;
                self.x_registered = true;
            }
            None if self.x_registered => {
                use tools::Tool as _;
                let _ = reg
                    .ask(tools::UnregisterTool(xai::XSearchTool::NAME.to_owned()))
                    .await;
                let _ = reg.ask(tools::UnregisterPrompt("search-providers")).await;
                self.x_registered = false;
            }
            _ => {}
        }
    }

    fn view(&self) -> Vec<ProviderSettings> {
        settings_view(&self.cfg, &|n| self.secret_set(n), &self.registered)
    }
}

impl Message<GetProviders> for SearchProviders {
    type Reply = Vec<ProviderSettings>;

    async fn handle(&mut self, _: GetProviders, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        self.view()
    }
}

impl Message<SetProviders> for SearchProviders {
    type Reply = Result<Vec<ProviderSettings>, String>;

    async fn handle(&mut self, m: SetProviders, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let mut next = self.cfg.clone();
        apply_settings(&mut next, &m.0)?;
        persist(&self.config_path, &next).map_err(|e| format!("config.yaml: {e}"))?;
        self.cfg = next;
        self.sync().await;
        tracing::info!(registered = ?self.registered, "search providers updated");
        Ok(self.view())
    }
}

impl Message<secrets::SecretsEvent> for SearchProviders {
    type Reply = ();

    async fn handle(&mut self, m: secrets::SecretsEvent, _: &mut Context<Self, Self::Reply>) {
        if matches!(m, secrets::SecretsEvent::Changed) {
            self.sync().await;
        }
    }
}

/// The plugin: builds the configured providers, hands them to the `web`
/// actor once it is registered (16 §3.1), then stays as [`SearchProviders`].
pub struct SearchProvidersPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for SearchProvidersPlugin {
    fn name(&self) -> &'static str {
        "search-providers"
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        let cfg: SearchProvidersConfig = rt.section("search_providers").unwrap_or_else(|e| {
            tracing::debug!("no [search_providers] config section, using defaults: {e}");
            SearchProvidersConfig::default()
        });
        let rt = rt.clone();
        tokio::spawn(async move {
            let vault = secrets::handle(&rt).await;
            if vault.is_none() {
                tracing::warn!(
                    "search-providers: secrets vault unavailable; starting without API keys"
                );
            }
            let Some(web) = rt.lookup_when::<web::WebActor>(web::WEB_ACTOR).await else {
                tracing::warn!("web plugin not running; nothing to register with");
                return;
            };
            let registry = rt
                .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
                .await;
            let mut state = SearchProviders {
                cfg,
                config_path: rt.data_dir.join("config.yaml"),
                oauth: oauth::Store::new(&rt.data_dir)
                    .with_grants(vault.clone().map(|v| v as Arc<dyn oauth::Grants>)),
                env_key: xai::env_key(),
                vault,
                web,
                registry,
                registered: Vec::new(),
                x_registered: false,
            };
            state.sync().await;
            let actor = kameo::actor::Spawn::spawn(state);
            if let Err(e) = actor.register(SEARCH_PROVIDERS_ACTOR) {
                tracing::error!("search-providers: registering {SEARCH_PROVIDERS_ACTOR}: {e}");
                return;
            }
            rt.subscribe::<SearchProviders, secrets::SecretsEvent>(&actor)
                .await;
        });
        Ok(())
    }
}

/// Map a non-success status the way every hosted API agrees on; callers
/// special-case what differs (Firecrawl 402, OpenSERP 403) before this.
pub(crate) fn http_error(status: reqwest::StatusCode) -> ProviderError {
    match status.as_u16() {
        401 | 403 => ProviderError::Unauthorized,
        429 => ProviderError::RateLimited,
        code => ProviderError::Http(format!("HTTP {code}")),
    }
}

/// Decode a JSON body; a malformed one is `ProviderError::Parse`.
pub(crate) fn parse<T: serde::de::DeserializeOwned>(body: &str) -> Result<T, ProviderError> {
    serde_json::from_str(body).map_err(|e| ProviderError::Parse(e.to_string()))
}

/// `site:host q` when the query restricts by site.
pub(crate) fn site_query(q: &web::Query) -> String {
    match &q.site {
        Some(site) => format!("site:{site} {}", q.text),
        None => q.text.clone(),
    }
}

/// `Some` only for non-empty, trimmed text.
pub(crate) fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_registers_by_default_until_a_firecrawl_key_exists() {
        let cfg = SearchProvidersConfig::default();
        let fc = cfg
            .firecrawl
            .as_ref()
            .expect("firecrawl block present by default");
        assert!(fc.enabled);
        assert_eq!(fc.base_url, "https://api.firecrawl.dev");
        assert!(cfg.searxng.is_none(), "searxng is opt-in");

        assert!(build(&cfg, &HashMap::new()).is_empty());

        let creds = HashMap::from([("firecrawl".to_string(), "fc-key".to_string())]);
        let built = build(&cfg, &creds);
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].meta().name, "firecrawl");
        assert!(built[0].meta().renders);
    }

    #[test]
    fn keyed_blocks_skip_without_credentials_and_priority_orders() {
        let cfg = SearchProvidersConfig {
            firecrawl: None,
            searxng: Some(SearxngConfig {
                base_url: "http://127.0.0.1:8080".into(),
                priority: 50,
                ..SearxngConfig::default()
            }),
            openserp: Some(OpenserpConfig::default()),
            brave: Some(BraveConfig::default()),
            x: None,
        };
        let names: Vec<_> = build(&cfg, &HashMap::new())
            .iter()
            .map(|p| p.meta().name)
            .collect();
        assert_eq!(names, ["openserp", "searxng"]);
        let empty_sx = SearchProvidersConfig {
            searxng: Some(SearxngConfig::default()),
            firecrawl: None,
            ..SearchProvidersConfig::default()
        };
        assert!(
            build(&empty_sx, &HashMap::new()).is_empty(),
            "no base_url: skipped"
        );
    }

    #[test]
    fn yaml_section_shape_is_backwards_compatible() {
        let cfg: SearchProvidersConfig = serde_json::from_str(
            r#"{"firecrawl":{"base_url":"http://127.0.0.1:3002"},"brave":{},"openserp":{"base_url":"http://x:7000"},"searxng":{"base_url":"http://s"}}"#,
        )
        .expect("config parses");
        assert_eq!(
            cfg.firecrawl.as_ref().map(|f| f.base_url.as_str()),
            Some("http://127.0.0.1:3002")
        );
        assert!(
            cfg.brave
                .as_ref()
                .is_some_and(|b| b.enabled && b.secret == "brave")
        );
        assert_eq!(cfg.openserp.as_ref().map(|o| o.engines.len()), Some(3));
        assert!(cfg.searxng.as_ref().is_some_and(|s| s.enabled));
    }

    #[test]
    fn settings_view_apply_and_persist_round_trip() {
        let mut cfg = SearchProvidersConfig::default();
        let view = settings_view(&cfg, &|n| n == "brave", &["firecrawl".into()]);
        let names: Vec<&str> = view.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, ["searxng", "openserp", "firecrawl", "brave_api"]);
        let sx = &view[0];
        assert!(!sx.enabled && sx.secret.is_none() && !sx.registered);
        let b = &view[3];
        assert!(!b.enabled && b.secret_set && b.base_url.is_none());
        assert!(view[2].registered);

        let mut edits = view.clone();
        edits[0].enabled = true;
        edits[0].base_url = Some("http://127.0.0.1:8089/".into());
        edits[3].enabled = true;
        edits[3].timeout_ms = 7000;
        apply_settings(&mut cfg, &edits).expect("valid");
        assert_eq!(
            cfg.searxng.as_ref().unwrap().base_url,
            "http://127.0.0.1:8089"
        );
        assert!(
            cfg.brave
                .as_ref()
                .is_some_and(|b| b.enabled && b.timeout_ms == 7000)
        );

        let mut bad = edits.clone();
        bad[1].base_url = Some("ftp://x".into());
        assert!(
            apply_settings(&mut cfg, &bad)
                .unwrap_err()
                .contains("openserp")
        );
        bad = edits.clone();
        bad[2].timeout_ms = 10;
        assert!(
            apply_settings(&mut cfg, &bad)
                .unwrap_err()
                .contains("timeout_ms")
        );

        let dir = std::env::temp_dir().join(format!("sp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        std::fs::write(
            &path,
            "agent:\n  model: x # keep\nsearch_providers:\n  brave: {}\n  openserp:\n    base_url: http://old:7000\n",
        )
        .unwrap();
        persist(&path, &cfg).expect("persisted");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("agent:\n  model: x # keep\n"), "{text}");
        assert!(text.contains("  brave:\n"), "{text}");
        assert!(
            text.contains("    base_url: \"http://127.0.0.1:7000\"\n"),
            "{text}"
        );
        use figment::providers::Format as _;
        let figment = figment::Figment::new().merge(figment::providers::Yaml::file(&path));
        let back: SearchProvidersConfig = figment
            .extract_inner("search_providers")
            .expect("yaml parses back");
        assert_eq!(
            back.searxng.as_ref().unwrap().base_url,
            "http://127.0.0.1:8089"
        );
        assert!(back.brave.as_ref().unwrap().enabled);
        assert_eq!(
            back.openserp.as_ref().unwrap().engines,
            ["google", "bing", "duckduckgo"]
        );
        let x = back.x.as_ref().expect("x block persisted");
        assert!(x.enabled && x.model == "grok-4.7" && x.timeout_ms == 90_000);
        assert!(text.contains("  x:\n"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn search_x_is_advertised_only_enabled_with_a_credential() {
        let cfg = SearchProvidersConfig::default();
        assert!(advertise_x(&cfg, true).is_some());
        assert!(advertise_x(&cfg, false).is_none());
        let off = SearchProvidersConfig {
            x: Some(xai::XConfig {
                enabled: false,
                ..xai::XConfig::default()
            }),
            ..SearchProvidersConfig::default()
        };
        assert!(advertise_x(&off, true).is_none());
        let absent = SearchProvidersConfig {
            x: None,
            ..SearchProvidersConfig::default()
        };
        assert!(advertise_x(&absent, true).is_none());
    }
}
