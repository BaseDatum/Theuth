//! Web search and fetch (design doc 16). `WebPlugin` reads the `web` config
//! section, spawns the provider registry actor ([`actor::WebActor`] as
//! [`provider::WEB_ACTOR`]), and registers `search_web` (SERP plus the
//! query-relevant passages of the top hits, read in one call), `read_url`
//! and `http_request` (46 §5: one explicit API call, structured reply) on
//! the tool registry, with the plugin's prompt block. External providers
//! arrive over the bus ([`provider::RegisterProvider`]) from the
//! `search-providers` plugin.

pub mod actor;
pub mod aggregate;
pub mod compact;
pub mod date;
pub mod engines;
pub mod excerpt;
pub mod fetch;
pub mod health;
pub mod http;
pub mod progress;
pub mod provider;
pub mod rank;
pub mod render;
pub mod settings;
pub mod sources;
pub mod tools;
pub mod url;

pub use actor::WebActor;
pub use provider::{
    Category, Feature, FeatureItem, FeatureKind, Link, Provider, ProviderError, ProviderMeta,
    Query, RawResult, Recency, RegisterProvider, Rendered, UnregisterProvider, WEB_ACTOR,
};

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use ::tools::{REGISTRY_ACTOR, RegisterPrompt, RegisterTool, ToolRegistry};
use kameo::actor::Spawn;
use runtime::Runtime;
use serde::{Deserialize, Serialize};

/// `web:` config section (16 §6). Every field has a default; the section may be absent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// `false`: no actor, no tools.
    pub enabled: bool,
    /// Built-in engines to run (`crate::engines::BUILTIN` names; default [`engines::DEFAULT_ENGINES`]).
    pub engines: Vec<String>,
    /// Per-provider timeout.
    pub timeout_ms: u64,
    /// Deadline for one whole search.
    pub overall_timeout_ms: u64,
    /// Default `max_chars` of `read_url`.
    pub fetch_max_chars: usize,
    /// Top hits `search_web` reads and excerpts by default (the `read` argument).
    pub read_top: usize,
    /// Passage budget in characters per read page.
    pub read_chars: usize,
    /// Deadline for the whole read phase of `search_web`; pages still in
    /// flight when it lapses are reported as `skipped: "budget"`.
    pub read_budget_ms: u64,
    /// Cap on one browser-rendered read inside `search_web` (a stalled render
    /// must not eat the budget); `read_url {rendered: true}` is uncapped.
    pub render_cap_ms: u64,
    /// `recent` used when the call passes none.
    pub recent_default: Option<provider::Recency>,
    /// `false`: the keyless built-in engines run only when no external
    /// provider serves the category.
    pub keyless_fallback: bool,
    /// `true`: a degraded search is an error result (routines treat it as a failure).
    pub degraded_is_error: bool,
    /// Per-feature cap in the search text (16 §4.4).
    pub feature_max_chars: usize,
    /// Body cap of the fetcher.
    pub max_fetch_bytes: usize,
    /// Redirect hops the fetcher follows (each re-checked against the SSRF guard).
    pub max_redirects: usize,
    /// Sent on every request.
    pub user_agent: String,
    /// `Accept-Language` sent on every page fetch (122 D4).
    pub accept_language: String,
    /// Under `impersonate: on_challenge`: escalate from `reqwest` to the
    /// Chrome-emulating client after a diagnosed 403/503 challenge (122 R1-D1);
    /// ignored under `always` and `never`.
    pub header_ladder: bool,
    /// When page fetches use the Chrome-emulating `wreq` client: `always`
    /// (default), `on_challenge`, `never` (122 revision 1).
    pub impersonate: fetch::client::Impersonate,
    /// The web tools reach localhost, LAN addresses and `.local` names (16 §5,
    /// amended 2026-11-11). Cloud metadata endpoints never; restricted (capped)
    /// contact states never (`tools::egress`). Read at start: a change applies after a restart.
    pub allow_private_network: bool,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engines: engines::DEFAULT_ENGINES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            timeout_ms: 4000,
            overall_timeout_ms: 6000,
            fetch_max_chars: 4000,
            read_top: 5,
            read_chars: 2500,
            read_budget_ms: 6000,
            render_cap_ms: 3000,
            recent_default: None,
            keyless_fallback: true,
            degraded_is_error: false,
            feature_max_chars: 600,
            max_fetch_bytes: 2_097_152,
            max_redirects: 5,
            user_agent: engines::USER_AGENT.to_owned(),
            accept_language: "en-US,en;q=0.9".to_owned(),
            header_ladder: true,
            impersonate: fetch::client::Impersonate::default(),
            allow_private_network: true,
        }
    }
}

/// The live `web` config, shared by the actor (which applies
/// [`settings::SetDefaults`]) and the tools (which read limits per call).
pub type SharedConfig = Arc<RwLock<WebConfig>>;

/// The plugin: reads `web`, then in a task waits for the secrets vault (37
/// §3.6: the `github` key), spawns [`WebActor`] as [`WEB_ACTOR`] with the
/// configured built-in engines, and registers `search_web`, `read_url` and
/// `http_request` once the tool registry is up (knowledge pattern). The search
/// tool registers even
/// with zero engines: external providers may arrive over the bus, and the
/// description tells the user how to add one.
pub struct WebPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for WebPlugin {
    fn name(&self) -> &'static str {
        "web"
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        let cfg: WebConfig = rt.section("web").unwrap_or_else(|e| {
            tracing::debug!("no [web] config section, using defaults: {e}");
            WebConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("web disabled by config");
            return Ok(());
        }
        let client = reqwest::Client::builder()
            .user_agent(&cfg.user_agent)
            .timeout(Duration::from_millis(cfg.timeout_ms.max(1)))
            .build()?;
        let fetcher = Arc::new(
            fetch::Fetcher::new(&cfg.user_agent, cfg.max_fetch_bytes, cfg.max_redirects)?
                .with_policy(
                    cfg.impersonate,
                    cfg.header_ladder,
                    Some(&cfg.accept_language),
                )
                .with_private_network(cfg.allow_private_network)?,
        );
        // Own client: the page fetcher's 8 s per-hop timeout and 2 MiB cap are
        // page policy; the API tool's bounds are fixed by 46 §5.
        let api_fetcher = Arc::new(
            fetch::Fetcher::new_with_timeouts(
                &cfg.user_agent,
                http::MAX_BYTES,
                cfg.max_redirects,
                http::TIMEOUT,
                fetch::CONNECT_TIMEOUT,
            )?
            .with_private_network(cfg.allow_private_network)?,
        );
        let rt = rt.clone();
        tokio::spawn(async move {
            let vault = secrets::handle(&rt).await;
            if vault.is_none() {
                tracing::warn!("web: secrets vault unavailable; starting without API keys");
            }
            let creds: HashMap<String, String> = ["github"]
                .iter()
                .filter_map(|k| {
                    let v = vault.as_ref()?.get(k, 0)?;
                    Some((k.to_string(), v.as_str().to_owned()))
                })
                .collect();
            let providers: Vec<Arc<dyn Provider>> = cfg
                .engines
                .iter()
                .filter_map(|name| {
                    let p = engines::builtin(name, &creds);
                    if p.is_none() {
                        tracing::warn!(engine = %name, "unknown built-in engine in web.engines");
                    }
                    p
                })
                .collect();
            let metas = actor::ProviderMetas::default();
            let shared: SharedConfig = Arc::new(RwLock::new(cfg.clone()));
            let actor = WebActor::spawn(WebActor::new(
                Arc::clone(&shared),
                Some(rt.data_dir.join("config.yaml")),
                client,
                providers,
                Arc::clone(&metas),
            ));
            if let Err(e) = actor.register(WEB_ACTOR) {
                tracing::error!("web: registering {WEB_ACTOR}: {e}");
                return;
            }
            if let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await {
                let _ = reg
                    .ask(RegisterTool(Arc::new(tools::SearchTool {
                        actor: actor.clone(),
                        fetcher: Arc::clone(&fetcher),
                        providers: Arc::clone(&metas),
                        cfg: Arc::clone(&shared),
                        rt: Some(rt.clone()),
                    })))
                    .await;
                let _ = reg
                    .ask(RegisterTool(Arc::new(tools::FetchTool {
                        actor,
                        fetcher,
                        providers: metas,
                        cfg: shared,
                    })))
                    .await;
                let _ = reg
                    .ask(RegisterTool(Arc::new(http::HttpRequestTool {
                        fetcher: api_fetcher,
                        deadline: http::TIMEOUT,
                    })))
                    .await;
                let _ = reg
                    .ask(RegisterPrompt {
                        plugin: "web",
                        text: tools::PROMPT.to_owned(),
                    })
                    .await;
            }
        });
        Ok(())
    }
}
