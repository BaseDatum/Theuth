//! External search providers (design doc 16 §3, §6): Brave, Firecrawl,
//! SearXNG and OpenSERP. [`SearchProvidersPlugin`] reads the
//! `search_providers` config section and the secrets vault (37 §3.6: keys
//! `firecrawl`, `brave`, `openserp`), builds every enabled provider, and
//! registers each with the `web` plugin's actor over the bus
//! ([`web::RegisterProvider`]). This crate owns no tools.

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use web::{Provider, ProviderError};

pub mod brave;
pub mod firecrawl;
pub mod openserp;
pub mod searxng;

/// `search_providers:` section (16 §6). Each block's presence enables it;
/// Firecrawl is present by default so an installation with no other provider
/// gets a real web index as soon as a `firecrawl:` key exists.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct SearchProvidersConfig {
    /// Hosted or self-hosted Firecrawl; needs the `firecrawl` secret.
    pub firecrawl: Option<FirecrawlConfig>,
    /// Self-hosted SearXNG instance; keyless.
    pub searxng: Option<SearxngConfig>,
    /// Self-hosted (or hosted, with `openserp:` key) OpenSERP.
    pub openserp: Option<OpenserpConfig>,
    /// Brave Search API; needs the `brave` secret.
    pub brave: Option<BraveConfig>,
}

impl Default for SearchProvidersConfig {
    fn default() -> Self {
        Self {
            firecrawl: Some(FirecrawlConfig::default()),
            searxng: None,
            openserp: None,
            brave: None,
        }
    }
}

/// `search_providers.firecrawl`.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct FirecrawlConfig {
    /// API root; `http://127.0.0.1:3002` for self-hosted.
    pub base_url: String,
    /// Set `false` to keep the block but skip the provider.
    pub enabled: bool,
}

impl Default for FirecrawlConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.firecrawl.dev".into(),
            enabled: true,
        }
    }
}

/// `search_providers.searxng`.
#[derive(Clone, Debug, Deserialize)]
pub struct SearxngConfig {
    /// Instance root, e.g. `http://127.0.0.1:8080`.
    pub base_url: String,
}

/// `search_providers.openserp`.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct OpenserpConfig {
    /// Server root, e.g. `http://127.0.0.1:7000`.
    pub base_url: String,
    /// Upstream engines for `/mega/search`.
    pub engines: Vec<String>,
    /// Set `false` to keep the block but skip the provider.
    pub enabled: bool,
}

impl Default for OpenserpConfig {
    fn default() -> Self {
        Self {
            base_url: "http://127.0.0.1:7000".into(),
            engines: vec!["google".into(), "bing".into()],
            enabled: true,
        }
    }
}

/// `search_providers.brave`.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct BraveConfig {
    /// Set `false` to keep the block but skip the provider.
    pub enabled: bool,
}

impl Default for BraveConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// Non-empty, trimmed credential.
fn key<'a>(credentials: &'a HashMap<String, String>, name: &str) -> Option<&'a str> {
    credentials
        .get(name)
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
}

/// Every provider the config enables and the credentials allow, in
/// registration order (Firecrawl first: it is the default `rendered` path).
/// Blocks that need a key and have none are skipped with a log line.
#[must_use]
pub fn build(
    cfg: &SearchProvidersConfig,
    credentials: &HashMap<String, String>,
) -> Vec<Arc<dyn Provider>> {
    let mut out: Vec<Arc<dyn Provider>> = Vec::new();
    if let Some(fc) = cfg.firecrawl.as_ref().filter(|c| c.enabled) {
        match key(credentials, "firecrawl") {
            Some(k) => out.push(Arc::new(firecrawl::Firecrawl::new(&fc.base_url, k))),
            None => tracing::info!(
                "firecrawl: no `firecrawl` secret; add one (free tier at firecrawl.dev) with /secrets add or `wth --secret-set firecrawl` to enable web search through it"
            ),
        }
    }
    if let Some(sx) = &cfg.searxng {
        out.push(Arc::new(searxng::Searxng::new(&sx.base_url)));
    }
    if let Some(os) = cfg.openserp.as_ref().filter(|c| c.enabled) {
        out.push(Arc::new(openserp::Openserp::new(
            &os.base_url,
            &os.engines,
            key(credentials, "openserp"),
        )));
    }
    if cfg.brave.as_ref().is_some_and(|c| c.enabled) {
        match key(credentials, "brave") {
            Some(k) => out.push(Arc::new(brave::Brave::new(k))),
            None => tracing::warn!(
                "search-providers: brave needs the `brave` secret (/secrets add or `wth --secret-set brave`); skipped"
            ),
        }
    }
    out
}

/// The plugin: builds the configured providers and hands them to the `web`
/// actor once it is registered (16 §3.1).
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
            let creds: HashMap<String, String> = ["firecrawl", "openserp", "brave"]
                .iter()
                .filter_map(|k| {
                    let v = vault.as_ref()?.get(k, 0)?;
                    Some((k.to_string(), v.as_str().to_owned()))
                })
                .collect();
            let providers = build(&cfg, &creds);
            if providers.is_empty() {
                return;
            }
            let Some(actor) = rt.lookup_when::<web::WebActor>(web::WEB_ACTOR).await else {
                tracing::warn!("web plugin not running; nothing to register with");
                return;
            };
            for p in providers {
                let name = p.meta().name;
                if let Err(e) = actor.ask(web::RegisterProvider(p)).await {
                    tracing::warn!("search-providers: registering {name} failed: {e}");
                }
            }
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

/// Undo the handful of HTML entities Brave leaves in plain-text snippets.
pub(crate) fn unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    s.replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

/// `Some` only for non-empty, trimmed text.
pub(crate) fn non_empty(s: Option<String>) -> Option<String> {
    s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firecrawl_on_by_default_but_needs_a_key() {
        let cfg = SearchProvidersConfig::default();
        let fc = cfg
            .firecrawl
            .as_ref()
            .expect("firecrawl block present by default");
        assert!(fc.enabled);
        assert_eq!(fc.base_url, "https://api.firecrawl.dev");

        assert!(build(&cfg, &HashMap::new()).is_empty());

        let creds = HashMap::from([("firecrawl".to_string(), "fc-key".to_string())]);
        let built = build(&cfg, &creds);
        assert_eq!(built.len(), 1);
        assert_eq!(built[0].meta().name, "firecrawl");
        assert!(built[0].meta().renders);
    }

    #[test]
    fn keyed_blocks_skip_without_credentials_and_keyless_do_not() {
        let cfg = SearchProvidersConfig {
            firecrawl: None,
            searxng: Some(SearxngConfig {
                base_url: "http://127.0.0.1:8080".into(),
            }),
            openserp: Some(OpenserpConfig::default()),
            brave: Some(BraveConfig::default()),
        };
        let names: Vec<_> = build(&cfg, &HashMap::new())
            .iter()
            .map(|p| p.meta().name)
            .collect();
        assert_eq!(names, ["searxng", "openserp"]);
    }

    #[test]
    fn yaml_section_shape() {
        let cfg: SearchProvidersConfig = serde_json::from_str(
            r#"{"firecrawl":{"base_url":"http://127.0.0.1:3002"},"brave":{},"openserp":{"base_url":"http://x:7000"}}"#,
        )
        .expect("config parses");
        assert_eq!(
            cfg.firecrawl.as_ref().map(|f| f.base_url.as_str()),
            Some("http://127.0.0.1:3002")
        );
        assert!(cfg.brave.as_ref().is_some_and(|b| b.enabled));
        assert_eq!(cfg.openserp.as_ref().map(|o| o.engines.len()), Some(2));
        assert!(cfg.searxng.is_none());
    }
}
