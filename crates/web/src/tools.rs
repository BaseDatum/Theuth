//! `search_web` and `read_url` (design doc 16 §2; not `web_search`: genai turns
//! any tool of that name into Anthropic's server-side search). Descriptions are a pure
//! function of the live provider set ([`ProviderMetas`]), so a provider
//! registered later shows up on the next `ListTools`; schemas are static.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Instant;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::WebConfig;
use crate::actor::{ProviderMetas, Render, Search, WebActor};
use crate::aggregate::category_name;
use crate::engines::BUILTIN;
use crate::fetch::{FetchFormat, FetchResponse, Fetcher, check_url};
use crate::provider::{Category, FeatureKind, ProviderMeta, Query, Recency};
use crate::render;

/// Appended to the search description when only built-in engines are registered.
pub const NO_EXTERNAL_PROVIDER: &str = "No external search provider is configured; Firecrawl's free tier works out of the box once the `firecrawl` secret is set (/secrets add firecrawl; key from firecrawl.dev).";

/// Arguments for [`SearchTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// What to search for.
    pub query: String,
    /// Results to return, 1..=20 (default 8).
    #[serde(default)]
    pub limit: Option<u8>,
    /// Result category: general (default), news, it (code and repositories), science (papers).
    #[serde(default)]
    pub category: Option<Category>,
    /// Restrict to one hostname, e.g. `docs.rs`.
    #[serde(default)]
    pub site: Option<String>,
    /// Only results from the last day / week / month / year.
    #[serde(default)]
    pub recent: Option<Recency>,
    /// Subset of the configured providers by name (default: all serving the category).
    #[serde(default)]
    pub providers: Option<Vec<String>>,
    /// Inline the cleaned text of the top N results, 0..=5 (default 0).
    #[serde(default)]
    pub fetch_top: Option<u8>,
}

/// Arguments for [`FetchTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct FetchArgs {
    /// Absolute http(s) URL.
    pub url: String,
    /// Content cap in characters, 200..=16000 (default from config, 4000).
    #[serde(default)]
    pub max_chars: Option<u32>,
    /// `markdown` (default; keeps headings and links) or `text` (cheaper prose).
    #[serde(default)]
    pub format: Option<FetchFormat>,
    /// Render through a browser-backed provider (JS-only pages). Needs one registered.
    #[serde(default)]
    pub rendered: Option<bool>,
}

fn structured<T: Serialize>(text: String, data: &T, is_error: bool) -> ToolOutput {
    ToolOutput {
        is_error,
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

fn names<'a>(metas: impl Iterator<Item = &'a ProviderMeta>) -> Vec<&'a str> {
    metas.map(|m| m.name).collect()
}

/// `search_web` description for `metas` (16 §2, §4.5): what it does, each
/// provider with its categories and feature kinds, the union of feature kinds,
/// a billing note when any provider is metered, and [`NO_EXTERNAL_PROVIDER`]
/// when no provider came from outside [`BUILTIN`].
#[must_use]
pub fn search_description(metas: &[ProviderMeta]) -> String {
    let mut s = String::from(
        "Search the web through the configured providers and merge the results: one ranked list \
         (title, URL, snippet, which providers agreed), then any SERP features. Prefer this over \
         fetching whole pages; use fetch_top or read_url to read a hit.",
    );
    if metas.is_empty() {
        s.push_str(" No providers are registered; searches fail until one is.");
    } else {
        let list: Vec<String> = metas
            .iter()
            .map(|m| {
                let cats: Vec<&str> = m.categories.iter().map(|c| category_name(*c)).collect();
                let feats: Vec<&str> = m.features.iter().map(|f| f.as_str()).collect();
                if feats.is_empty() {
                    format!("{} ({})", m.name, cats.join(", "))
                } else {
                    format!("{} ({}; {})", m.name, cats.join(", "), feats.join(", "))
                }
            })
            .collect();
        s.push_str(&format!(" Providers: {}.", list.join(", ")));
        let kinds: BTreeSet<FeatureKind> = metas
            .iter()
            .flat_map(|m| m.features.iter().copied())
            .collect();
        if !kinds.is_empty() {
            let kinds: Vec<&str> = FeatureKind::ORDER
                .iter()
                .filter(|k| kinds.contains(*k))
                .map(|k| k.as_str())
                .collect();
            s.push_str(&format!(" Features it can return: {}.", kinds.join(", ")));
        }
        let billable = names(metas.iter().filter(|m| m.billable));
        if !billable.is_empty() {
            s.push_str(&format!(
                " fetch_top and rendered fetch are billed by {}.",
                billable.join(", ")
            ));
        }
    }
    if metas.iter().all(|m| BUILTIN.contains(&m.name)) {
        s.push(' ');
        s.push_str(NO_EXTERNAL_PROVIDER);
    }
    s
}

/// `read_url` description: names the rendering provider when one is registered.
#[must_use]
pub fn fetch_description(metas: &[ProviderMeta]) -> String {
    let mut s = String::from(
        "Fetch one http(s) page and return its readable content as markdown (default) or plain \
         text, capped at max_chars. Private, loopback and link-local destinations are refused.",
    );
    match metas.iter().find(|m| m.renders) {
        Some(m) => {
            s.push_str(&format!(
                " rendered: true loads the page in a browser via {} for JavaScript-only sites",
                m.name
            ));
            if m.billable {
                s.push_str(" (billed)");
            }
            s.push('.');
        }
        None => s.push_str(" No rendering provider is registered, so rendered: true fails."),
    }
    s
}

fn metas(p: &ProviderMetas) -> Vec<ProviderMeta> {
    p.read().map(|m| m.clone()).unwrap_or_default()
}

/// `search_web`: fan out to the providers, merge, rank, optionally fetch the top hits.
pub struct SearchTool {
    /// The provider registry.
    pub actor: ActorRef<WebActor>,
    /// For `fetch_top`.
    pub fetcher: Arc<Fetcher>,
    /// Live provider set, for the description.
    pub providers: ProviderMetas,
    /// Limits.
    pub cfg: WebConfig,
}

#[async_trait::async_trait]
impl Tool for SearchTool {
    const NAME: &'static str = "search_web";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = SearchArgs;

    fn description(&self) -> String {
        search_description(&metas(&self.providers))
    }

    async fn call(&self, a: SearchArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let text = a.query.trim();
        if text.is_empty() {
            return Ok(ToolOutput::err("query must not be empty"));
        }
        let limit = a.limit.unwrap_or(8);
        if !(1..=20).contains(&limit) {
            return Ok(ToolOutput::err(format!(
                "limit must be 1..=20, got {limit}"
            )));
        }
        let fetch_top = a.fetch_top.unwrap_or(0);
        if fetch_top > 5 {
            return Ok(ToolOutput::err(format!(
                "fetch_top must be 0..=5, got {fetch_top}"
            )));
        }
        let q = Query {
            text: text.to_owned(),
            category: a.category.unwrap_or(Category::General),
            limit: usize::from(limit),
            site: a.site.filter(|s| !s.trim().is_empty()),
            recent: a.recent,
        };
        let resp = match self
            .actor
            .ask(Search {
                query: q,
                providers: a.providers,
            })
            .await
        {
            Ok(r) => r,
            Err(kameo::error::SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
            Err(e) => anyhow::bail!("web actor unavailable: {e}"),
        };

        let mut fetched = Vec::new();
        if fetch_top > 0 {
            let mut set = JoinSet::new();
            for (i, r) in resp.results.iter().take(usize::from(fetch_top)).enumerate() {
                let (actor, f, url, max) = (
                    self.actor.clone(),
                    Arc::clone(&self.fetcher),
                    r.url.clone(),
                    self.cfg.fetch_top_chars,
                );
                set.spawn(async move {
                    let page = match url::Url::parse(&url) {
                        Ok(parsed) => match actor.ask(Render(parsed)).await {
                            Ok(rendered) => {
                                let truncated = rendered.markdown.chars().count() > max;
                                let content = if truncated {
                                    rendered.markdown.chars().take(max).collect()
                                } else {
                                    rendered.markdown
                                };
                                Ok(FetchResponse {
                                    url: rendered.url,
                                    title: rendered.title,
                                    content,
                                    truncated,
                                    took_ms: 0,
                                })
                            }
                            Err(_) => f
                                .fetch(&url, FetchFormat::Text, max)
                                .await
                                .map_err(|e| e.to_string()),
                        },
                        Err(_) => f
                            .fetch(&url, FetchFormat::Text, max)
                            .await
                            .map_err(|e| e.to_string()),
                    };
                    (i, url, page)
                });
            }
            while let Some(r) = set.join_next().await {
                if let Ok(t) = r {
                    fetched.push(t);
                }
            }
            fetched.sort_by_key(|(i, _, _)| *i);
        }
        let fetched: Vec<(String, Result<FetchResponse, String>)> =
            fetched.into_iter().map(|(_, u, p)| (u, p)).collect();

        let all_failed = resp.ok.is_empty() && !resp.failed.is_empty();
        let text = render::search_text(&resp, self.cfg.feature_max_chars, &fetched);
        Ok(structured(text, &resp, all_failed))
    }
}

/// `read_url`: SSRF-guarded page fetch, or a provider-rendered one.
pub struct FetchTool {
    /// For `rendered: true`.
    pub actor: ActorRef<WebActor>,
    /// The raw path.
    pub fetcher: Arc<Fetcher>,
    /// Live provider set, for the description.
    pub providers: ProviderMetas,
    /// Limits.
    pub cfg: WebConfig,
}

#[async_trait::async_trait]
impl Tool for FetchTool {
    const NAME: &'static str = "read_url";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = FetchArgs;

    fn description(&self) -> String {
        fetch_description(&metas(&self.providers))
    }

    async fn call(&self, a: FetchArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let max_chars = match a.max_chars {
            None => self.cfg.fetch_max_chars,
            Some(m) if (200..=16000).contains(&m) => usize::try_from(m).unwrap_or(usize::MAX),
            Some(m) => {
                return Ok(ToolOutput::err(format!(
                    "max_chars must be 200..=16000, got {m}"
                )));
            }
        };
        let resp = if a.rendered.unwrap_or(false) {
            let url = match check_url(&a.url).await {
                Ok(u) => u,
                Err(e) => return Ok(ToolOutput::err(e.to_string())),
            };
            let start = Instant::now();
            let r = match self.actor.ask(Render(url)).await {
                Ok(r) => r,
                Err(kameo::error::SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
                Err(e) => anyhow::bail!("web actor unavailable: {e}"),
            };
            let truncated = r.markdown.chars().count() > max_chars;
            let content = if truncated {
                r.markdown.chars().take(max_chars).collect()
            } else {
                r.markdown
            };
            FetchResponse {
                url: r.url,
                title: r.title,
                content,
                truncated,
                took_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
            }
        } else {
            match self
                .fetcher
                .fetch(&a.url, a.format.unwrap_or_default(), max_chars)
                .await
            {
                Ok(r) => r,
                Err(e) => return Ok(ToolOutput::err(e.to_string())),
            }
        };
        Ok(structured(render::fetch_text(&resp), &resp, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const fn meta(
        name: &'static str,
        features: &'static [FeatureKind],
        billable: bool,
    ) -> ProviderMeta {
        ProviderMeta {
            name,
            categories: &[Category::General],
            features,
            renders: billable,
            billable,
        }
    }

    #[test]
    fn firecrawl_hint_iff_only_builtins() {
        let google = meta("google", &[FeatureKind::Answer], false);
        assert!(search_description(&[]).ends_with(NO_EXTERNAL_PROVIDER));
        assert!(search_description(&[google]).ends_with(NO_EXTERNAL_PROVIDER));
        let brave = meta("brave", &[FeatureKind::Faq, FeatureKind::News], true);
        let ddg = meta("duckduckgo", &[FeatureKind::Answer], false);
        let d = search_description(&[ddg, brave]);
        assert!(!d.contains(NO_EXTERNAL_PROVIDER), "{d}");
        assert!(d.contains("fetch_top and rendered fetch are billed by brave."));
        assert!(d.contains("Features it can return: answer, faq, news."));
    }

    #[test]
    fn only_produced_feature_kinds_are_named() {
        let d = search_description(&[meta("arxiv", &[], false)]);
        assert!(!d.contains("Features it can return"), "{d}");
        assert!(!d.contains("news"), "{d}");
        assert!(d.contains("Providers: arxiv (general)."), "{d}");
    }

    #[test]
    fn fetch_description_names_renderer() {
        assert!(fetch_description(&[]).contains("No rendering provider"));
        let d = fetch_description(&[meta("firecrawl", &[], true)]);
        assert!(d.contains("via firecrawl") && d.contains("(billed)"), "{d}");
    }
}
