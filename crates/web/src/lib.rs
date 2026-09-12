//! Web search and fetch (design doc 16). `WebPlugin` reads the `web` config
//! section, spawns the provider registry actor ([`actor::WebActor`] as
//! [`provider::WEB_ACTOR`]), and registers `web_search` / `web_fetch` on the
//! tool registry. External providers arrive over the bus
//! ([`provider::RegisterProvider`]) from the `search-providers` plugin.

pub mod actor;
pub mod aggregate;
pub mod engines;
pub mod fetch;
pub mod provider;
pub mod render;
pub mod tools;
pub mod url;

pub use actor::WebActor;
pub use provider::{
    Category, Feature, FeatureItem, FeatureKind, Link, Provider, ProviderError, ProviderMeta,
    Query, RawResult, Recency, RegisterProvider, Rendered, UnregisterProvider, WEB_ACTOR,
};

use std::sync::Arc;
use std::time::Duration;

use ::tools::{REGISTRY_ACTOR, RegisterTool, ToolRegistry};
use kameo::actor::Spawn;
use runtime::Runtime;
use serde::{Deserialize, Serialize};

/// `web:` config section (16 §6). Every field has a default; the section may be absent.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WebConfig {
    /// `false`: no actor, no tools.
    pub enabled: bool,
    /// Built-in engines to run (`crate::engines::BUILTIN` names).
    pub engines: Vec<String>,
    /// Per-provider timeout.
    pub timeout_ms: u64,
    /// Deadline for one whole search.
    pub overall_timeout_ms: u64,
    /// Default `max_chars` of `web_fetch`.
    pub fetch_max_chars: usize,
    /// Per-page cap for `fetch_top`.
    pub fetch_top_chars: usize,
    /// Per-feature cap in the search text (16 §4.4).
    pub feature_max_chars: usize,
    /// Body cap of the fetcher.
    pub max_fetch_bytes: usize,
    /// Redirect hops the fetcher follows (each re-checked against the SSRF guard).
    pub max_redirects: usize,
    /// Sent on every request.
    pub user_agent: String,
}

impl Default for WebConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            engines: engines::BUILTIN.iter().map(|s| (*s).to_owned()).collect(),
            timeout_ms: 4000,
            overall_timeout_ms: 6000,
            fetch_max_chars: 4000,
            fetch_top_chars: 1500,
            feature_max_chars: 600,
            max_fetch_bytes: 2_097_152,
            max_redirects: 5,
            user_agent: "wth/0.1 (+https://github.com/skymoore/wth)".to_owned(),
        }
    }
}

/// The plugin: reads `web`, spawns [`WebActor`] as [`WEB_ACTOR`] with the
/// configured built-in engines, registers `web_search` and `web_fetch` once
/// the tool registry is up (knowledge pattern). The search tool registers even
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
        let fetcher = Arc::new(fetch::Fetcher::new(
            &cfg.user_agent,
            cfg.max_fetch_bytes,
            cfg.max_redirects,
        )?);
        let creds = runtime::credentials(&rt.data_dir);
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
        let actor = WebActor::spawn(WebActor::new(
            cfg.clone(),
            client,
            providers,
            Arc::clone(&metas),
        ));
        actor.register(WEB_ACTOR)?;

        let rt = rt.clone();
        tokio::spawn(async move {
            if let Some(reg) = rt.lookup_when::<ToolRegistry>(REGISTRY_ACTOR).await {
                let _ = reg
                    .ask(RegisterTool(Arc::new(tools::SearchTool {
                        actor: actor.clone(),
                        fetcher: Arc::clone(&fetcher),
                        providers: Arc::clone(&metas),
                        cfg: cfg.clone(),
                    })))
                    .await;
                let _ = reg
                    .ask(RegisterTool(Arc::new(tools::FetchTool {
                        actor,
                        fetcher,
                        providers: metas,
                        cfg,
                    })))
                    .await;
            }
        });
        Ok(())
    }
}
