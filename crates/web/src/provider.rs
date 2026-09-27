//! The cross-plugin contract (design doc 16 §3): a search provider, what it
//! returns, and the messages `search-providers` uses to register one with
//! the [`WEB_ACTOR`]. Built-in engines (`crate::engines`) and external
//! providers implement the same trait; the aggregator cannot tell them apart.

use std::sync::Arc;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Registered name of the provider registry actor (`crate::actor::WebActor`).
pub const WEB_ACTOR: &str = "web";

/// Result category a provider can answer for (16 §2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    /// The open web.
    General,
    /// Dated news.
    News,
    /// Code, repositories, developer docs.
    It,
    /// Papers and academic sources.
    Science,
}

/// Recency filter; each provider maps it to its own date syntax.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum Recency {
    /// Past 24 hours.
    Day,
    /// Past 7 days.
    Week,
    /// Past month.
    Month,
    /// Past year.
    Year,
}

/// One search as the aggregator hands it to every selected provider.
#[derive(Clone, Debug)]
pub struct Query {
    /// The user's words, verbatim.
    pub text: String,
    /// Category the caller asked for; selects providers.
    pub category: Category,
    /// 1..=20; providers may return fewer, never more.
    pub limit: usize,
    /// Hostname restriction (`site:`).
    pub site: Option<String>,
    /// Recency filter.
    pub recent: Option<Recency>,
}

/// One hit as a provider saw it, before merging.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct RawResult {
    /// Page title.
    pub title: String,
    /// Absolute URL.
    pub url: String,
    /// Provider snippet, tags stripped.
    pub snippet: String,
    /// ISO-8601 date or whatever the provider gave, verbatim.
    pub published: Option<String>,
    /// Provider-specific relevance hint (stars, score); only breaks ties.
    pub score_hint: Option<f64>,
}

/// SERP feature kinds (16 §4). Every variant has at least one producing
/// provider in `ProviderMeta::features`; a kind nobody produces is deleted.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum FeatureKind {
    /// Direct answer: calculation, conversion, fact.
    #[default]
    Answer,
    /// "Did you mean" / altered query.
    Spelling,
    /// Entity card: text, attributes, source, image.
    KnowledgePanel,
    /// Engine-generated summary with citations.
    AiSummary,
    /// Question → answer pairs.
    Faq,
    /// Dated headlines with source.
    News,
    /// Dictionary sense.
    Definition,
    /// Related searches / topics.
    Related,
    /// Video results.
    Videos,
    /// Image results.
    Images,
    /// Forum threads.
    Discussions,
}

impl FeatureKind {
    /// Render order in the tool text (16 §4.4).
    pub const ORDER: [FeatureKind; 11] = [
        FeatureKind::Answer,
        FeatureKind::Spelling,
        FeatureKind::KnowledgePanel,
        FeatureKind::AiSummary,
        FeatureKind::Faq,
        FeatureKind::News,
        FeatureKind::Definition,
        FeatureKind::Related,
        FeatureKind::Videos,
        FeatureKind::Images,
        FeatureKind::Discussions,
    ];

    /// Wire/text name (`snake_case`).
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            FeatureKind::Answer => "answer",
            FeatureKind::Spelling => "spelling",
            FeatureKind::KnowledgePanel => "knowledge_panel",
            FeatureKind::AiSummary => "ai_summary",
            FeatureKind::Faq => "faq",
            FeatureKind::News => "news",
            FeatureKind::Definition => "definition",
            FeatureKind::Related => "related",
            FeatureKind::Videos => "videos",
            FeatureKind::Images => "images",
            FeatureKind::Discussions => "discussions",
        }
    }
}

/// One entry inside a feature: an FAQ pair, a related term, a headline.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct FeatureItem {
    /// Question, term, or headline.
    pub title: String,
    /// Answer or body, when the provider gives one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// Where it points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// Date as the provider gave it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<String>,
}

/// A citation or source link.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Link {
    /// Link text.
    pub title: String,
    /// Absolute URL.
    pub url: String,
}

/// One SERP feature, normalised across providers (16 §4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Feature {
    /// What kind of module this is.
    pub kind: FeatureKind,
    /// Producing provider name.
    pub provider: String,
    /// Module title (entity name, question, headline group).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Body text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Entries: FAQ pairs, related terms, headlines.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<FeatureItem>,
    /// Citations / sources.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    /// Image URL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Infobox rows.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attributes: Vec<(String, String)>,
}

/// A page read through a provider (16 §5 `rendered`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Rendered {
    /// Final URL after the provider's redirects.
    pub url: String,
    /// Page title.
    pub title: Option<String>,
    /// Page body as markdown.
    pub markdown: String,
    /// How the body was obtained: `fetch` (plain HTTP + converter) or
    /// `render` (a browser). `search_web` shows it so the model knows when a
    /// thin page is worth `read_url {rendered: true}`.
    pub tier: String,
}

/// Static facts about a provider; drives selection and the tool description.
#[derive(Clone, Copy, Debug)]
pub struct ProviderMeta {
    /// Unique, lowercase; the `providers:` argument and `Failed:` lines use it.
    pub name: &'static str,
    /// Categories it answers for.
    pub categories: &'static [Category],
    /// Feature kinds this provider can produce (16 §4.1).
    pub features: &'static [FeatureKind],
    /// Implements [`Provider::render`].
    pub renders: bool,
    /// Hosted and metered: the description says so next to reading hits / `rendered`.
    pub billable: bool,
}

/// Why one provider failed; the aggregator reports it and carries on.
#[derive(Debug, thiserror::Error)]
pub enum ProviderError {
    /// Transport or non-success status not covered below.
    #[error("{0}")]
    Http(String),
    /// Anti-bot / consent / 403 with the marker seen.
    #[error("blocked: {0}")]
    Blocked(String),
    /// 429 or a rate-limit body.
    #[error("rate limited")]
    RateLimited,
    /// 401/403 on a keyed provider.
    #[error("unauthorized: check the provider's secret")]
    Unauthorized,
    /// Response did not have the expected shape.
    #[error("bad response: {0}")]
    Parse(String),
    /// Per-provider timeout.
    #[error("timed out")]
    Timeout,
    /// `render` on a provider that does not render, or a query shape the
    /// provider cannot serve (e.g. `recent` on an engine without a time
    /// filter); the aggregator skips it silently rather than reporting a failure.
    #[error("not supported by this provider")]
    Unsupported,
}

impl From<reqwest::Error> for ProviderError {
    fn from(e: reqwest::Error) -> Self {
        if e.is_timeout() {
            ProviderError::Timeout
        } else {
            ProviderError::Http(e.to_string())
        }
    }
}

/// A search engine, built-in or external. `client` is the shared reqwest
/// client (user agent and timeouts already set); providers must not build
/// their own except to pin a different timeout.
#[async_trait::async_trait]
pub trait Provider: Send + Sync + 'static {
    /// Static facts: name, categories, feature kinds, render/billing flags.
    fn meta(&self) -> ProviderMeta;

    /// Own deadline for one `search`, when configured (external providers
    /// carry `timeout_ms`); `None` = the aggregator's `web.timeout_ms`.
    fn timeout(&self) -> Option<std::time::Duration> {
        None
    }

    /// Run one query; at most `q.limit` results plus any SERP features.
    async fn search(
        &self,
        client: &reqwest::Client,
        q: &Query,
    ) -> Result<(Vec<RawResult>, Vec<Feature>), ProviderError>;

    /// Fetch `url` through the provider's browser (always the browser —
    /// `read_url {rendered: true}` relies on it). Only when `meta().renders`.
    async fn render(
        &self,
        client: &reqwest::Client,
        url: &url::Url,
    ) -> Result<Rendered, ProviderError> {
        let _ = (client, url);
        Err(ProviderError::Unsupported)
    }

    /// Read `url` the cheapest way that yields the page's real content;
    /// providers with a fast path override it (scrape: fetch, browser only
    /// when the fetch is thin). `render` must always be the browser.
    async fn read(
        &self,
        client: &reqwest::Client,
        url: &url::Url,
    ) -> Result<Rendered, ProviderError> {
        self.render(client, url).await
    }
}

/// `ask` on the [`WEB_ACTOR`]: add (or replace by name) a provider. Reply `()`.
pub struct RegisterProvider(pub Arc<dyn Provider>);

/// `ask` on the [`WEB_ACTOR`]: remove a provider by name. Reply `()`.
pub struct UnregisterProvider(pub String);
