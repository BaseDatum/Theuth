//! `search_web` and `read_url` (design doc 16 §2; not `web_search`: genai turns
//! any tool of that name into Anthropic's server-side search). `search_web` is
//! one call: SERP fan-out, then the top hits are read concurrently (the render
//! provider, else the raw fetcher) and reduced to their query-relevant
//! paragraphs ([`crate::excerpt::best_passages`]). Descriptions are a pure
//! function of the live provider set ([`ProviderMetas`]), so a provider
//! registered later shows up on the next `ListTools`; schemas are static.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kameo::actor::ActorRef;
use runtime::Runtime;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::task::JoinSet;
use tools::{Activity, Risk, Tool, ToolCx, ToolOutput, Tracked, activity};

use crate::SharedConfig;
use crate::actor::{ProviderMetas, Read, Render, Search, WebActor};
use crate::aggregate::{RankedResult, ReadPage, category_name};
use crate::engines::BUILTIN;
use crate::fetch::{FetchFormat, FetchResponse, Fetcher, check_url};
use crate::progress::{ProgressCx, report};
use crate::provider::{Category, FeatureKind, ProviderMeta, Query, Recency};
use crate::render;

/// Plugin prompt block: `search_web` answers in one call; the other tools are
/// for one page in full or whole sites.
pub const PROMPT: &str = "`search_web()` (python) is the one call for anything current or outside your knowledge: \
it searches, reads the top pages and returns the relevant passages, so answer from a single call \
without preface. Cite as markdown links to the URLs it returns. Reach for `read_url()` only when you \
need one specific page in full, `scrape_url()`/`crawl()` for whole sites.";

/// Most hits one `search_web` call reads.
pub const MAX_READ: usize = 8;

/// Appended to the search description when only built-in engines are registered.
pub const NO_EXTERNAL_PROVIDER: &str = "Search runs on the built-in keyless engines; a SearXNG or OpenSERP instance, or a brave_api/firecrawl secret, under Settings > Search (search_providers) adds a provider that does not rate-limit.";

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
    /// Top hits to read and excerpt, 0..=8 (default 5); 0 = SERP only.
    #[serde(default)]
    pub read: Option<u8>,
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
/// *searching* provider (render-only ones such as `scrape`/`browser` answer no
/// category and are left out) with its categories and feature kinds, the union
/// of feature kinds, a billing note when any provider is metered, and
/// [`NO_EXTERNAL_PROVIDER`] when no searching provider came from outside
/// [`BUILTIN`].
#[must_use]
pub fn search_description(all: &[ProviderMeta]) -> String {
    let metas: Vec<&ProviderMeta> = all.iter().filter(|m| !m.categories.is_empty()).collect();
    let mut s = String::from(
        "Search the web and read the top results in one call: a ranked list (title, URL, snippet) \
         with the passages of each read page most relevant to the query, then any SERP features. \
         Use it whenever the answer needs current or external information; set read: 0 for a \
         cheaper SERP-only pass. site= and recent= filter the returned rows; the top hits are read \
         in rank order and every unread row says why (not-relevant, budget). A result that starts \
         with `WARNING: degraded search` (degraded: true) came from too few engines: say so rather \
         than answering from it.",
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
        let billable = names(metas.iter().copied().filter(|m| m.billable));
        if !billable.is_empty() {
            s.push_str(&format!(
                " Reading hits through {} is billed.",
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

/// `search_web`: fan out to the providers, merge, rank, then read and excerpt the top hits.
pub struct SearchTool {
    /// The provider registry (search and render).
    pub actor: ActorRef<WebActor>,
    /// Raw-fetch fallback for hits the render provider cannot read.
    pub fetcher: Arc<Fetcher>,
    /// Live provider set, for the description.
    pub providers: ProviderMetas,
    /// Live limits (Settings › Search edits them).
    pub cfg: SharedConfig,
    /// Bus for [`crate::progress::ScrapeProgress`]; `None` in library use.
    pub rt: Option<Runtime>,
}

fn snapshot(cfg: &SharedConfig) -> crate::WebConfig {
    cfg.read().map(|c| c.clone()).unwrap_or_default()
}

fn elapsed_ms(start: Instant) -> u64 {
    u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX)
}

/// Fewer prose chars than this and the page is a client-rendered shell or a
/// bot wall: reported as an error, not excerpted (a junk excerpt reads as an
/// answer).
const MIN_PROSE: usize = 100;

/// Read `url` through the render provider ([`Read`]: its cheapest tier that
/// yields real content) within `render_cap`; excerpt the page for `query`. A
/// provider error falls back to the raw fetcher under what is left of
/// `budget`; a render timeout does not - the provider already tried the
/// fetch tier and found it thin, so the raw page is the same shell, and the
/// model is told the page needs the browser. `activity` is the invocation's
/// guard, carried explicitly because this runs in a spawned task: the
/// actor's detached read holds it past the timeout.
// Every argument is a distinct per-read input shared by the two call sites; a struct would only be built there.
#[allow(clippy::too_many_arguments)]
async fn read_page(
    actor: ActorRef<WebActor>,
    activity: Option<Activity>,
    fetcher: Arc<Fetcher>,
    url: String,
    query: String,
    chars: usize,
    render_cap: Duration,
    budget: Duration,
) -> ReadPage {
    let start = Instant::now();
    let page = match url::Url::parse(&url) {
        Err(e) => Err(format!("bad url: {e}")),
        Ok(parsed) => match tokio::time::timeout(
            render_cap.min(budget),
            actor.ask(Tracked {
                message: Read(parsed),
                activity,
            }),
        )
        .await
        {
            Ok(Ok(r)) => Ok((r.title, r.markdown, r.tier)),
            Err(_) => Err(format!(
                "render cap of {:.1}s exceeded (JavaScript page); read_url with rendered: true waits for it",
                render_cap.as_secs_f32()
            )),
            Ok(Err(e)) => {
                tracing::debug!(url = %url, error = %e, "read provider failed; raw fetch");
                let remaining = budget.saturating_sub(start.elapsed());
                match tokio::time::timeout(
                    remaining,
                    fetcher.fetch(&url, FetchFormat::Markdown, render::SEARCH_MAX_CHARS),
                )
                .await
                {
                    Ok(Ok(f)) => Ok((f.title, f.content, "fetch".to_owned())),
                    Ok(Err(e)) => Err(e.to_string()),
                    Err(_) => Err(format!(
                        "read budget of {:.1}s exceeded",
                        budget.as_secs_f32()
                    )),
                }
            }
        },
    };
    match page {
        Ok((_, markdown, tier)) if crate::excerpt::prose_chars(&markdown) < MIN_PROSE => ReadPage {
            url,
            title: None,
            excerpts: Vec::new(),
            error: Some("no readable text (JavaScript-only page or bot wall)".into()),
            skipped: None,
            took_ms: elapsed_ms(start),
            tier,
        },
        Ok((title, markdown, tier)) => ReadPage {
            url,
            title: title.filter(|t| !t.trim().is_empty()),
            excerpts: crate::excerpt::best_passages(&markdown, &query, chars),
            error: None,
            skipped: None,
            took_ms: elapsed_ms(start),
            tier,
        },
        Err(e) => ReadPage {
            url,
            title: None,
            excerpts: Vec::new(),
            error: Some(e),
            skipped: None,
            took_ms: elapsed_ms(start),
            tier: String::new(),
        },
    }
}

/// A page that was not read, with the reason.
fn skipped(url: &str, why: impl Into<String>) -> ReadPage {
    ReadPage {
        url: url.to_owned(),
        title: None,
        excerpts: Vec::new(),
        error: None,
        skipped: Some(why.into()),
        took_ms: 0,
        tier: String::new(),
    }
}

/// Which rows to read, in rank order: the first `read` hits whose title,
/// snippet and URL share enough query tokens - half of them, at most two, at
/// least one; a row two or more providers agree on needs only one, since a
/// decoy comes from one flagged engine (measured 2026-09-27: "Anthropic
/// announcement" rows say "Anthropic", never "announcement"; "PrintToPdf"
/// pages say "Print to PDF", so tokens also match the text with spaces
/// removed; the Bun releases page says neither "latest" nor "version"). The
/// rest are `not-relevant` (decoys, homonyms), non-http URLs are `error: …`,
/// and rows past the slots are `budget`. Returns one `Ok(url)` (read it) or
/// `Err(page)` (already explained) per row.
fn plan_reads(
    results: &[RankedResult],
    terms: &[String],
    read: usize,
) -> Vec<Result<String, Box<ReadPage>>> {
    let need_alone = terms.len().div_ceil(2).clamp(1, 2);
    let mut slots = read;
    results
        .iter()
        .map(|r| {
            if !(r.url.starts_with("http://") || r.url.starts_with("https://")) {
                return Err(Box::new(skipped(&r.url, "error: not an http(s) URL")));
            }
            if slots == 0 {
                return Err(Box::new(skipped(&r.url, "budget")));
            }
            let hay = format!("{} {} {}", r.title, r.snippet, r.url).to_lowercase();
            let squashed: String = hay.chars().filter(|c| !c.is_whitespace()).collect();
            let matched = terms
                .iter()
                .filter(|t| hay.contains(t.as_str()) || squashed.contains(t.as_str()))
                .count();
            let need = if r.providers.len() >= 2 {
                1
            } else {
                need_alone
            };
            if matched < need {
                return Err(Box::new(skipped(&r.url, "not-relevant")));
            }
            slots -= 1;
            Ok(r.url.clone())
        })
        .collect()
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

    async fn call(&self, a: SearchArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let cfg = snapshot(&self.cfg);
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
        let read = match a.read {
            Some(r) if usize::from(r) > MAX_READ => {
                return Ok(ToolOutput::err(format!(
                    "read must be 0..={MAX_READ}, got {r}"
                )));
            }
            Some(r) => usize::from(r),
            None => cfg.read_top.min(MAX_READ),
        };
        let q = Query {
            text: text.to_owned(),
            category: a.category.unwrap_or(Category::General),
            limit: usize::from(limit),
            site: a.site.filter(|s| !s.trim().is_empty()),
            recent: a.recent.or(cfg.recent_default),
        };
        let pcx = ProgressCx {
            session: cx.session,
            call_id: cx.call_id.clone(),
        };
        report(self.rt.as_ref(), Some(&pcx), &[], &[], read).await;
        let started = Instant::now();
        let mut resp = match self
            .actor
            .ask(Tracked::new(Search {
                query: q,
                providers: a.providers,
            }))
            .await
        {
            Ok(r) => r,
            Err(kameo::error::SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
            Err(e) => anyhow::bail!("web actor unavailable: {e}"),
        };

        if read > 0 && !resp.results.is_empty() {
            let terms = crate::excerpt::tokens(text);
            let plan = plan_reads(&resp.results, &terms, read);
            let mut inflight: Vec<String> = plan.iter().filter_map(|p| p.clone().ok()).collect();
            let mut done: Vec<String> = Vec::with_capacity(inflight.len());
            report(self.rt.as_ref(), Some(&pcx), &inflight, &done, read).await;
            let budget = Duration::from_millis(cfg.read_budget_ms.max(1));
            let render_cap = Duration::from_millis(cfg.render_cap_ms.max(1));
            let activity = activity::current();
            let mut set = JoinSet::new();
            for (i, url) in inflight.iter().enumerate() {
                let fut = read_page(
                    self.actor.clone(),
                    activity.clone(),
                    Arc::clone(&self.fetcher),
                    url.clone(),
                    text.to_owned(),
                    cfg.read_chars,
                    render_cap,
                    budget,
                );
                set.spawn(async move { (i, fut.await) });
            }
            let phase = Instant::now();
            let mut finished: Vec<Option<ReadPage>> = vec![None; inflight.len()];
            loop {
                let remaining = budget.saturating_sub(phase.elapsed());
                let Ok(joined) = tokio::time::timeout(remaining, set.join_next()).await else {
                    set.abort_all();
                    break;
                };
                let Some(joined) = joined else { break };
                let Ok((i, page)) = joined else { continue };
                inflight.retain(|u| *u != page.url);
                done.push(page.url.clone());
                if let Some(slot) = finished.get_mut(i) {
                    *slot = Some(page);
                }
                report(self.rt.as_ref(), Some(&pcx), &inflight, &done, read).await;
            }
            let mut finished = finished.into_iter();
            resp.pages = plan
                .into_iter()
                .map(|p| match p {
                    Err(page) => *page,
                    Ok(url) => finished
                        .next()
                        .flatten()
                        .unwrap_or_else(|| skipped(&url, "budget")),
                })
                .collect();
        }
        resp.took_ms = elapsed_ms(started);

        let all_failed = resp.ok.is_empty() && !resp.failed.is_empty();
        let text = render::search_text(&resp, cfg.feature_max_chars);
        Ok(structured(
            text,
            &resp,
            all_failed || (resp.degraded && cfg.degraded_is_error),
        ))
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
    /// Live limits.
    pub cfg: SharedConfig,
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
            None => snapshot(&self.cfg).fetch_max_chars,
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
            let r = match self.actor.ask(Tracked::new(Render(url))).await {
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
    fn external_hint_iff_only_builtins() {
        let google = meta("google", &[FeatureKind::Answer], false);
        assert!(search_description(&[]).ends_with(NO_EXTERNAL_PROVIDER));
        assert!(search_description(&[google]).ends_with(NO_EXTERNAL_PROVIDER));
        let brave_api = meta("brave_api", &[FeatureKind::Faq, FeatureKind::News], true);
        let bing = meta("bing", &[], false);
        let d = search_description(&[bing, brave_api]);
        assert!(!d.contains(NO_EXTERNAL_PROVIDER), "{d}");
        assert!(
            d.contains("Reading hits through brave_api is billed."),
            "{d}"
        );
        assert!(d.contains("Features it can return: faq, news."));
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

    fn hit(rank: u32, title: &str, url: &str, snippet: &str) -> RankedResult {
        RankedResult {
            rank,
            title: title.into(),
            url: url.into(),
            snippet: snippet.into(),
            providers: vec!["a".into()],
            published: None,
        }
    }

    fn terms(q: &str) -> Vec<String> {
        crate::excerpt::tokens(q)
    }

    fn plan(results: &[RankedResult], q: &str, read: usize) -> Vec<String> {
        plan_reads(results, &terms(q), read)
            .into_iter()
            .map(|p| match p {
                Ok(url) => format!("read {url}"),
                Err(page) => format!("{} {}", page.skipped.unwrap_or_default(), page.url),
            })
            .collect()
    }

    #[test]
    fn reads_in_rank_order_and_explains_every_skip() {
        let results = [
            hit(1, "RTINGS monitor review", "https://d.example/", ""),
            hit(2, "Unrelated", "https://c.example/", "nothing here"),
            hit(
                3,
                "Encyclopedia: Monitor",
                "https://b.example/",
                "rtings monitor review methodology",
            ),
            hit(4, "rtings monitor", "ftp://a.example/", ""),
            hit(5, "RTINGS monitor review 2", "https://e.example/", ""),
        ];
        assert_eq!(
            plan(&results, "rtings monitor review", 2),
            [
                "read https://d.example/",
                "not-relevant https://c.example/",
                "read https://b.example/",
                "error: not an http(s) URL ftp://a.example/",
                "budget https://e.example/",
            ]
        );
    }

    #[test]
    fn one_token_query_needs_one_match() {
        let results = [
            hit(
                1,
                "Bun releases",
                "https://github.com/oven-sh/bun/releases",
                "",
            ),
            hit(
                2,
                "Android 17",
                "https://en.wikipedia.org/wiki/Android_17",
                "",
            ),
        ];
        assert_eq!(
            plan(&results, "bun", 8),
            [
                "read https://github.com/oven-sh/bun/releases",
                "not-relevant https://en.wikipedia.org/wiki/Android_17",
            ]
        );
    }

    #[test]
    fn two_token_queries_need_one_match_and_camel_case_matches_spaced_text() {
        let results = [
            hit(
                1,
                "Introducing Claude Opus 5.5 - Anthropic",
                "https://www.anthropic.com/claude-opus-5-5",
                "5 days ago",
            ),
            hit(
                2,
                "How to print to PDF - Microsoft Q&A",
                "https://learn.microsoft.com/en-us/answers/q/1",
                "Microsoft Print to PDF",
            ),
            hit(3, "Unrelated", "https://c.example/", "nothing"),
        ];
        assert_eq!(
            plan(&results, "Anthropic announcement", 8)[0],
            "read https://www.anthropic.com/claude-opus-5-5"
        );
        assert_eq!(
            plan(&results, "PrintToPdf", 8)[1],
            "read https://learn.microsoft.com/en-us/answers/q/1"
        );
        assert_eq!(
            plan(&results, "PrintToPdf", 8)[2],
            "not-relevant https://c.example/"
        );
        let agreed = RankedResult {
            providers: vec!["a".into(), "b".into()],
            ..hit(
                1,
                "Releases: oven-sh/bun - GitHub",
                "https://github.com/oven-sh/bun/releases",
                "",
            )
        };
        assert_eq!(
            plan(std::slice::from_ref(&agreed), "bun latest version", 8)[0],
            "read https://github.com/oven-sh/bun/releases"
        );
        assert_eq!(
            plan(
                &[RankedResult {
                    providers: vec!["a".into()],
                    ..agreed
                }],
                "bun latest version",
                8
            )[0],
            "not-relevant https://github.com/oven-sh/bun/releases"
        );
    }

    /// Renderer that takes 10 s: the search reader must cut it at the cap.
    struct Slow;

    #[async_trait::async_trait]
    impl crate::provider::Provider for Slow {
        fn meta(&self) -> ProviderMeta {
            ProviderMeta {
                name: "slow",
                categories: &[],
                features: &[],
                renders: true,
                billable: false,
            }
        }

        async fn search(
            &self,
            _: &reqwest::Client,
            _: &Query,
        ) -> Result<
            (
                Vec<crate::provider::RawResult>,
                Vec<crate::provider::Feature>,
            ),
            crate::provider::ProviderError,
        > {
            Err(crate::provider::ProviderError::Unsupported)
        }

        async fn render(
            &self,
            _: &reqwest::Client,
            url: &url::Url,
        ) -> Result<crate::provider::Rendered, crate::provider::ProviderError> {
            tokio::time::sleep(Duration::from_secs(10)).await;
            Ok(crate::provider::Rendered {
                url: url.to_string(),
                markdown: "x".repeat(500),
                ..Default::default()
            })
        }
    }

    #[tokio::test]
    async fn ten_second_render_is_cut_at_the_cap() {
        use kameo::actor::Spawn as _;
        let actor = WebActor::spawn(WebActor::with_config(
            crate::WebConfig::default(),
            reqwest::Client::new(),
            vec![Arc::new(Slow)],
            ProviderMetas::default(),
        ));
        let fetcher = Arc::new(Fetcher::new("test", 1024, 1).unwrap());
        let start = Instant::now();
        let page = read_page(
            actor,
            None,
            fetcher,
            "https://example.com/app".into(),
            "query".into(),
            2500,
            Duration::from_millis(300),
            Duration::from_secs(6),
        )
        .await;
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "{:?}",
            start.elapsed()
        );
        let err = page.error.expect("capped read is an error");
        assert!(err.starts_with("render cap of 0.3s exceeded"), "{err}");
        assert!(page.excerpts.is_empty() && page.skipped.is_none());
    }
}
