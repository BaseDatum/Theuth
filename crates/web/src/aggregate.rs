//! Fan-out, merge and rank (design doc 16 §3 selection, §4.3 feature dedupe,
//! §4.6 drift): every selected provider runs concurrently under a per-provider
//! timeout and an overall deadline; hits merge by canonical URL with
//! reciprocal-rank fusion (k = 60) plus a small agreement boost (α = 0.001);
//! features dedupe by `(kind, title, first item)` and render in
//! [`FeatureKind::ORDER`].

use std::collections::hash_map::Entry;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

use crate::WebConfig;
use crate::provider::{Category, Feature, FeatureKind, Provider, ProviderError, Query, RawResult};

/// RRF constant \(k\) (Cormack et al.).
pub const RRF_K: f64 = 60.0;
/// Agreement boost per extra provider; small so rank stays primary
/// (a rank-1 term is \(1/61 \approx 0.0164\)).
pub const AGREEMENT_ALPHA: f64 = 0.001;
/// Providers searched at once.
pub const MAX_CONCURRENT: usize = 6;
/// Consecutive empty `Ok` responses before a provider counts as drifted (16 §4.6).
pub const DRIFT_AFTER: u8 = 3;

/// One merged hit.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RankedResult {
    /// 1-based.
    pub rank: u32,
    /// Title from the best-ranked hit.
    pub title: String,
    /// Canonical URL, never truncated.
    pub url: String,
    /// Snippet from the best-ranked hit, filled from others when blank.
    pub snippet: String,
    /// Providers that returned this URL, sorted.
    pub providers: Vec<String>,
    /// Publication date if any provider gave one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
}

/// One top hit `search_web` read: the passages of its page most relevant to
/// the query, or why it could not be read.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ReadPage {
    /// The hit's URL as searched (joins [`RankedResult::url`]).
    pub url: String,
    /// Page title when the read produced one.
    pub title: Option<String>,
    /// Query-relevant paragraphs in document order; empty on failure.
    pub excerpts: Vec<String>,
    /// Why the page could not be read.
    pub error: Option<String>,
    /// Wall time of the read in milliseconds.
    pub took_ms: u64,
    /// How the page was obtained: `fetch` (plain HTTP), `render` (a browser),
    /// empty on error.
    pub tier: String,
}

/// What one search produced; the `structured` half of the tool output.
#[derive(Clone, Debug, Default, Serialize)]
pub struct SearchResponse {
    /// The query text as searched.
    pub query: String,
    /// Wall time of the whole fan-out.
    pub took_ms: u64,
    /// Merged, ranked, at most `limit`.
    pub results: Vec<RankedResult>,
    /// Deduped SERP features in render order.
    pub features: Vec<Feature>,
    /// Providers that answered, sorted.
    pub ok: Vec<String>,
    /// `(provider, why)` for every provider that did not, sorted.
    pub failed: Vec<(String, String)>,
    /// Top hits read by the tool, in rank order; empty from [`search`] and when `read: 0`.
    pub pages: Vec<ReadPage>,
}

/// Raw outcome of [`fan_out`], before merging.
#[derive(Debug, Default)]
pub struct FanoutOutcome {
    /// Per provider, hits in the provider's own order.
    pub hits: Vec<(String, Vec<RawResult>)>,
    /// Every feature every provider returned.
    pub features: Vec<Feature>,
    /// Providers that returned `Ok` (empty lists included).
    pub ok: Vec<String>,
    /// Providers that errored or timed out. One returning
    /// [`ProviderError::Unsupported`] appears in neither list.
    pub failed: Vec<(String, String)>,
}

/// Zero-yield drift detector (16 §4.6): counts consecutive `Ok` responses with
/// no results and no features per provider. Shared across searches.
#[derive(Debug, Default)]
pub struct Drift(Mutex<HashMap<String, u8>>);

impl Drift {
    /// Record one fan-out. Moves providers that hit [`DRIFT_AFTER`] empty calls
    /// from `ok` to `failed` and logs at `warn`; any yield resets the count.
    pub fn observe(&self, outcome: &mut FanoutOutcome) {
        let Ok(mut counts) = self.0.lock() else {
            return;
        };
        let mut drifted = Vec::new();
        for name in &outcome.ok {
            let yielded = outcome
                .hits
                .iter()
                .any(|(n, hits)| n == name && !hits.is_empty())
                || outcome.features.iter().any(|f| &f.provider == name);
            let count = counts.entry(name.clone()).or_default();
            if yielded {
                *count = 0;
                continue;
            }
            *count = count.saturating_add(1);
            if *count >= DRIFT_AFTER {
                tracing::warn!(provider = %name, calls = *count, "no results; response shape may have changed");
                drifted.push(name.clone());
            }
        }
        for name in drifted {
            outcome.ok.retain(|n| n != &name);
            outcome.hits.retain(|(n, _)| n != &name);
            outcome.failed.push((
                name,
                "no results; response shape may have changed".to_owned(),
            ));
        }
    }
}

/// Wire name of a category (`Category`'s serde form).
#[must_use]
pub fn category_name(c: Category) -> &'static str {
    match c {
        Category::General => "general",
        Category::News => "news",
        Category::It => "it",
        Category::Science => "science",
    }
}

/// Providers for a query: those serving `category`, narrowed by `names`.
/// Registration order is kept. Errors name unknown providers and the known
/// set, or say when nothing serves the category.
pub fn select(
    providers: &[Arc<dyn Provider>],
    category: Category,
    names: Option<&[String]>,
) -> Result<Vec<Arc<dyn Provider>>, String> {
    if let Some(names) = names {
        let unknown: Vec<&str> = names
            .iter()
            .map(String::as_str)
            .filter(|n| !providers.iter().any(|p| p.meta().name == *n))
            .collect();
        if !unknown.is_empty() {
            let known: Vec<&str> = providers.iter().map(|p| p.meta().name).collect();
            return Err(format!(
                "unknown provider(s) {}; configured: {}",
                unknown.join(", "),
                known.join(", ")
            ));
        }
    }
    let selected: Vec<Arc<dyn Provider>> = providers
        .iter()
        .filter(|p| p.meta().categories.contains(&category))
        .filter(|p| names.is_none_or(|ns| ns.iter().any(|n| n == p.meta().name)))
        .cloned()
        .collect();
    if selected.is_empty() {
        return Err(match names {
            Some(ns) => format!(
                "none of {} serves category {}",
                ns.join(", "),
                category_name(category)
            ),
            None => format!(
                "no configured provider serves category {}",
                category_name(category)
            ),
        });
    }
    Ok(selected)
}

/// Run `providers` concurrently: at most [`MAX_CONCURRENT`] at a time, each
/// under `per_provider`, all under `overall`. Stragglers past the deadline are
/// aborted and recorded as failures. Never fails.
pub async fn fan_out(
    providers: &[Arc<dyn Provider>],
    client: &reqwest::Client,
    q: &Query,
    per_provider: Duration,
    overall: Duration,
) -> FanoutOutcome {
    let mut outcome = FanoutOutcome::default();
    let mut pending: HashSet<String> = providers.iter().map(|p| p.meta().name.to_owned()).collect();
    let sem = Arc::new(Semaphore::new(MAX_CONCURRENT));
    let start = Instant::now();
    let mut set = JoinSet::new();

    for p in providers {
        let (p, client, q, sem) = (Arc::clone(p), client.clone(), q.clone(), Arc::clone(&sem));
        set.spawn(async move {
            let name = p.meta().name.to_owned();
            let Ok(_permit) = sem.acquire().await else {
                return (name, Err(ProviderError::Timeout));
            };
            match tokio::time::timeout(per_provider, p.search(&client, &q)).await {
                Ok(r) => (name, r),
                Err(_) => (
                    name,
                    Err(ProviderError::Http(format!(
                        "timed out after {}ms",
                        per_provider.as_millis()
                    ))),
                ),
            }
        });
    }

    let mut deadline_hit = false;
    while !pending.is_empty() {
        let remaining = overall.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            deadline_hit = true;
            break;
        }
        match tokio::time::timeout(remaining, set.join_next()).await {
            Ok(Some(Ok((name, r)))) => {
                pending.remove(&name);
                absorb(&mut outcome, name, r);
            }
            // Panicked task: the name stays pending and is attributed below.
            Ok(Some(Err(e))) => tracing::debug!(error = %e, "provider task failed"),
            Ok(None) => break,
            Err(_) => {
                deadline_hit = true;
                break;
            }
        }
    }
    set.abort_all();
    while let Some(r) = set.join_next().await {
        if let Ok((name, r)) = r {
            pending.remove(&name);
            absorb(&mut outcome, name, r);
        }
    }
    let why = if deadline_hit {
        format!("overall deadline of {}ms exceeded", overall.as_millis())
    } else {
        "provider task failed".to_owned()
    };
    for name in pending {
        outcome.failed.push((name, why.clone()));
    }
    outcome.ok.sort();
    outcome.failed.sort();
    outcome
}

fn absorb(
    outcome: &mut FanoutOutcome,
    name: String,
    r: Result<(Vec<RawResult>, Vec<Feature>), ProviderError>,
) {
    if outcome.ok.contains(&name) || outcome.failed.iter().any(|(n, _)| n == &name) {
        return;
    }
    match r {
        Ok((hits, features)) => {
            tracing::debug!(provider = %name, hits = hits.len(), features = features.len(), "provider ok");
            outcome.ok.push(name.clone());
            outcome.hits.push((name, hits));
            outcome.features.extend(features);
        }
        // The provider cannot serve this query shape (e.g. a recency filter
        // it has no syntax for): neither ok nor failed, so the drift detector
        // and the `Failed:` list ignore it.
        Err(ProviderError::Unsupported) => {
            tracing::debug!(provider = %name, "provider skipped: query shape unsupported");
        }
        Err(e) => {
            tracing::debug!(provider = %name, error = %e, "provider failed");
            outcome.failed.push((name, e.to_string()));
        }
    }
}

struct Merged {
    pretty: String,
    title: String,
    snippet: String,
    published: Option<String>,
    /// Best (lowest) 1-based rank per provider: one RRF term each.
    ranks: HashMap<String, u32>,
    best_rank: u32,
}

/// RRF term for a 1-based rank: \(1/(k + \mathrm{rank})\).
#[must_use]
pub fn rrf(rank: u32) -> f64 {
    1.0 / (RRF_K + f64::from(rank.max(1)))
}

#[allow(clippy::cast_precision_loss)]
fn score(m: &Merged) -> f64 {
    let sum: f64 = m.ranks.values().map(|&r| rrf(r)).sum();
    sum + AGREEMENT_ALPHA * (m.ranks.len() as f64 - 1.0)
}

/// Merge per-provider hit lists by canonical URL and rank them: one RRF term
/// per provider at its best rank, plus [`AGREEMENT_ALPHA`] per extra provider;
/// title/snippet from the best-ranked hit, blanks filled from others; sorted
/// by score, then shorter URL, then title; at most `limit`.
#[must_use]
pub fn merge_rank_dedupe(hits: &[(String, Vec<RawResult>)], limit: usize) -> Vec<RankedResult> {
    let mut map: HashMap<String, Merged> = HashMap::new();
    for (provider, list) in hits {
        for (i, raw) in list.iter().enumerate() {
            let rank = u32::try_from(i + 1).unwrap_or(u32::MAX);
            let Some(canon) = crate::url::canonicalize(&raw.url) else {
                continue;
            };
            let m = match map.entry(canon.key) {
                Entry::Vacant(v) => {
                    v.insert(Merged {
                        pretty: canon.pretty,
                        title: raw.title.clone(),
                        snippet: raw.snippet.clone(),
                        published: raw.published.clone(),
                        ranks: HashMap::from([(provider.clone(), rank)]),
                        best_rank: rank,
                    });
                    continue;
                }
                Entry::Occupied(o) => o.into_mut(),
            };
            let prev = m.ranks.entry(provider.clone()).or_insert(rank);
            *prev = (*prev).min(rank);
            if rank < m.best_rank {
                m.best_rank = rank;
                if !raw.title.is_empty() {
                    m.title.clone_from(&raw.title);
                }
                if !raw.snippet.is_empty() {
                    m.snippet.clone_from(&raw.snippet);
                }
                if raw.published.is_some() {
                    m.published.clone_from(&raw.published);
                }
            } else {
                if m.title.is_empty() {
                    m.title.clone_from(&raw.title);
                }
                if m.snippet.is_empty() {
                    m.snippet.clone_from(&raw.snippet);
                }
                if m.published.is_none() {
                    m.published.clone_from(&raw.published);
                }
            }
        }
    }
    let mut scored: Vec<(f64, Merged)> = map.into_values().map(|m| (score(&m), m)).collect();
    scored.sort_by(|(sa, a), (sb, b)| {
        sb.partial_cmp(sa)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.pretty.len().cmp(&b.pretty.len()))
            .then_with(|| a.title.cmp(&b.title))
            .then_with(|| a.pretty.cmp(&b.pretty))
    });
    scored
        .into_iter()
        .take(limit.max(1))
        .enumerate()
        .map(|(i, (_, m))| {
            let mut providers: Vec<String> = m.ranks.into_keys().collect();
            providers.sort();
            RankedResult {
                rank: u32::try_from(i + 1).unwrap_or(u32::MAX),
                title: m.title,
                url: m.pretty,
                snippet: m.snippet,
                providers,
                published: m.published,
            }
        })
        .collect()
}

/// Dedupe features by `(kind, lowercase title, first item title)` (16 §4.3),
/// ordered by [`FeatureKind::ORDER`] then by the provider's position in
/// `provider_order` (registration order). First occurrence wins.
#[must_use]
pub fn merge_features(features: Vec<Feature>, provider_order: &[String]) -> Vec<Feature> {
    let pos = |f: &Feature| {
        let kind = FeatureKind::ORDER
            .iter()
            .position(|k| *k == f.kind)
            .unwrap_or(usize::MAX);
        let prov = provider_order
            .iter()
            .position(|p| *p == f.provider)
            .unwrap_or(usize::MAX);
        (kind, prov)
    };
    let mut out = features;
    out.sort_by_key(pos);
    let mut seen = HashSet::new();
    out.retain(|f| {
        seen.insert((
            f.kind,
            f.title.as_deref().unwrap_or_default().trim().to_lowercase(),
            f.items.first().map(|i| i.title.trim().to_lowercase()),
        ))
    });
    out
}

/// The whole pipeline: select, fan out, drift-check, merge, rank.
/// `Err` only for a bad selection (unknown name, nothing serves the category);
/// provider failures are reported in `SearchResponse::failed`.
pub async fn search(
    providers: &[Arc<dyn Provider>],
    client: &reqwest::Client,
    q: &Query,
    names: Option<&[String]>,
    cfg: &WebConfig,
    drift: &Drift,
) -> Result<SearchResponse, String> {
    let selected = select(providers, q.category, names)?;
    let order: Vec<String> = selected.iter().map(|p| p.meta().name.to_owned()).collect();
    let start = Instant::now();
    let mut outcome = fan_out(
        &selected,
        client,
        q,
        Duration::from_millis(cfg.timeout_ms.max(1)),
        Duration::from_millis(cfg.overall_timeout_ms.max(1)),
    )
    .await;
    if !q.text.trim().is_empty() {
        drift.observe(&mut outcome);
        outcome.failed.sort();
    }
    Ok(SearchResponse {
        query: q.text.clone(),
        took_ms: u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX),
        results: merge_rank_dedupe(&outcome.hits, q.limit),
        features: merge_features(outcome.features, &order),
        ok: outcome.ok,
        failed: outcome.failed,
        pages: Vec::new(),
    })
}
