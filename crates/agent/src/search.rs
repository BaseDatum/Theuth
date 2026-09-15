//! Fuzzy session search via [`nucleo`] (helix/fzf scoring), design doc 26.
//!
//! An [`Index`] is every session's title plus the text of its visible user
//! and assistant messages, built from the store in one scan and cached by the
//! supervisor until a turn finishes, a title changes or a session is deleted.
//! [`Index::search`] scores every entry against the query, keeps the best
//! hit per session (title hits boosted), and returns a snippet around the
//! matched characters with their offsets, so a frontend can highlight them.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};
use serde::{Deserialize, Serialize};

use crate::SessionId;
use crate::loop_::text_of;
use crate::store::Store;

/// The supervisor's cached [`Index`]. [`Cache::invalidate`] bumps a generation
/// (lock-free, from the event handler); [`Cache::search`] rebuilds when the
/// cached index is older than the generation, then matches off the runtime
/// threads. Concurrent searches serialise on the build only.
#[derive(Default)]
pub struct Cache {
    generation: AtomicU64,
    index: tokio::sync::Mutex<Option<(u64, Arc<Index>)>>,
}

impl Cache {
    /// The stored rows changed; the next search rebuilds.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// [`Index::search`] against a current index, building one from `store` if needed.
    pub async fn search(
        &self,
        store: &Store,
        query: String,
        limit: u32,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let generation = self.generation.load(Ordering::Acquire);
        let index = {
            let mut slot = self.index.lock().await;
            match &*slot {
                Some((g, index)) if *g == generation => index.clone(),
                _ => {
                    let index = Arc::new(Index::build(store).await?);
                    tracing::debug!(entries = index.len(), "search index built");
                    *slot = Some((generation, index.clone()));
                    index
                }
            }
        };
        Ok(tokio::task::spawn_blocking(move || index.search(&query, limit)).await?)
    }
}

/// Hard cap on hits per query.
pub const MAX_LIMIT: u32 = 50;

/// Hits when the caller omits `limit`.
pub const DEFAULT_LIMIT: u32 = 30;

/// Characters of context around the matched span.
const SNIPPET_CHARS: usize = 160;

/// A title hit outranks a body hit of the same score band.
const TITLE_BOOST: u32 = 32;

/// What matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum HitKind {
    /// The session's title matched.
    Title,
    /// A message's text matched.
    Message,
}

/// Who wrote the matched message.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum HitRole {
    /// The user's input.
    User,
    /// The model's reply.
    Assistant,
}

/// `[start, end)` in chars of `snippet`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MatchRange {
    /// First matched char.
    pub start: u32,
    /// One past the last matched char.
    pub end: u32,
}

/// One session's best match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SearchHit {
    /// The session.
    pub session: SessionId,
    /// The session's title (may be empty for a title-less session).
    pub title: String,
    /// What matched.
    pub kind: HitKind,
    /// `seq` of the matched message; `None` for a title hit.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub seq: Option<i64>,
    /// Author of the matched message; `None` for a title hit.
    pub role: Option<HitRole>,
    /// Text around the match, `…`-trimmed on either side.
    pub snippet: String,
    /// Matched spans inside `snippet`, ascending, non-overlapping.
    pub ranges: Vec<MatchRange>,
    /// nucleo score, plus [`TITLE_BOOST`] for a title hit.
    pub score: u32,
    /// Messages of this session that matched (0 for a title-only hit).
    pub matches: u32,
    /// The session's `updated_at` (RFC 3339), the tie-breaker after score.
    pub updated_at: String,
}

struct Title {
    id: SessionId,
    title: String,
    updated_at: String,
}

struct Doc {
    session: SessionId,
    seq: i64,
    role: HitRole,
    text: String,
}

/// Everything searchable, extracted once from the store.
pub struct Index {
    titles: Vec<Title>,
    docs: Vec<Doc>,
}

struct Candidate<'a> {
    session: SessionId,
    kind: HitKind,
    seq: Option<i64>,
    role: Option<HitRole>,
    haystack: &'a str,
    indices: Vec<u32>,
    score: u32,
}

impl Index {
    /// Every session's title and the text of its visible (`seq < cut_at`)
    /// user and assistant messages. Tool results are left out: they are
    /// large, and what matters in them the assistant repeats.
    pub async fn build(store: &Store) -> anyhow::Result<Index> {
        let titles = store
            .search_titles()
            .await?
            .into_iter()
            .map(|(id, title, updated_at)| Title {
                id,
                title,
                updated_at,
            })
            .collect();
        let docs = store
            .search_bodies()
            .await?
            .into_iter()
            .filter_map(|(session, seq, message)| {
                let role = match message.role {
                    genai::chat::ChatRole::User => HitRole::User,
                    genai::chat::ChatRole::Assistant => HitRole::Assistant,
                    _ => return None,
                };
                let text = text_of(&message.content);
                (!text.trim().is_empty()).then_some(Doc {
                    session,
                    seq,
                    role,
                    text,
                })
            })
            .collect();
        Ok(Index { titles, docs })
    }

    /// Entries indexed (titles + message texts); for logging and tests.
    pub fn len(&self) -> usize {
        self.titles.len() + self.docs.len()
    }

    /// Whether nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Best hit per session for `query`, best score first, then most recently
    /// updated. Empty query → no hits. `limit` is clamped to `1..=MAX_LIMIT`.
    pub fn search(&self, query: &str, limit: u32) -> Vec<SearchHit> {
        let q = query.trim();
        if q.is_empty() || self.titles.is_empty() {
            return Vec::new();
        }
        let limit = limit.clamp(1, MAX_LIMIT) as usize;
        // fzf syntax: atoms split on spaces, `'exact`, `^prefix`, `suffix$`, `!not`.
        let pattern = Pattern::parse(q, CaseMatching::Smart, Normalization::Smart);
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buf = Vec::new();
        let mut indices = Vec::new();

        let mut candidates: Vec<Candidate> = Vec::new();
        for t in &self.titles {
            indices.clear();
            let Some(score) = pattern.indices(
                Utf32Str::new(&t.title, &mut buf),
                &mut matcher,
                &mut indices,
            ) else {
                continue;
            };
            candidates.push(Candidate {
                session: t.id,
                kind: HitKind::Title,
                seq: None,
                role: None,
                haystack: &t.title,
                indices: indices.clone(),
                score: score.saturating_add(TITLE_BOOST),
            });
        }
        for d in &self.docs {
            indices.clear();
            let Some(score) =
                pattern.indices(Utf32Str::new(&d.text, &mut buf), &mut matcher, &mut indices)
            else {
                continue;
            };
            candidates.push(Candidate {
                session: d.session,
                kind: HitKind::Message,
                seq: Some(d.seq),
                role: Some(d.role),
                haystack: &d.text,
                indices: indices.clone(),
                score,
            });
        }

        // Per session: the best candidate, and how many messages matched.
        let mut best: HashMap<SessionId, Candidate> = HashMap::new();
        let mut counts: HashMap<SessionId, u32> = HashMap::new();
        for c in candidates {
            if c.kind == HitKind::Message {
                *counts.entry(c.session).or_insert(0) += 1;
            }
            match best.get(&c.session) {
                Some(prev) if prev.score >= c.score => {}
                _ => {
                    best.insert(c.session, c);
                }
            }
        }
        let by_id: HashMap<SessionId, &Title> = self.titles.iter().map(|t| (t.id, t)).collect();
        let mut ranked: Vec<Candidate> = best.into_values().collect();
        ranked.sort_by(|a, b| {
            let ua = by_id.get(&a.session).map(|t| t.updated_at.as_str());
            let ub = by_id.get(&b.session).map(|t| t.updated_at.as_str());
            b.score
                .cmp(&a.score)
                .then_with(|| ub.cmp(&ua))
                .then_with(|| a.session.0.cmp(&b.session.0))
        });
        ranked.truncate(limit);
        ranked
            .into_iter()
            .map(|c| {
                let (snippet, ranges) = snippet(c.haystack, &c.indices);
                let t = by_id.get(&c.session);
                SearchHit {
                    session: c.session,
                    title: t.map(|t| t.title.clone()).unwrap_or_default(),
                    kind: c.kind,
                    seq: c.seq,
                    role: c.role,
                    snippet,
                    ranges,
                    score: c.score,
                    matches: counts.get(&c.session).copied().unwrap_or(0),
                    updated_at: t.map(|t| t.updated_at.clone()).unwrap_or_default(),
                }
            })
            .collect()
    }
}

/// Up to [`SNIPPET_CHARS`] chars centred on the matched span, `…` where cut,
/// and the matched runs re-based to snippet offsets.
fn snippet(haystack: &str, indices: &[u32]) -> (String, Vec<MatchRange>) {
    let chars: Vec<char> = haystack.chars().collect();
    let n = chars.len();
    if n == 0 {
        return (String::new(), Vec::new());
    }
    let mut sorted: Vec<usize> = indices
        .iter()
        .map(|&i| i as usize)
        .filter(|&i| i < n)
        .collect();
    sorted.sort_unstable();
    sorted.dedup();

    let (start, end) = match (sorted.first(), sorted.last()) {
        (Some(&first), Some(&last)) => {
            let span = last - first + 1;
            if span >= SNIPPET_CHARS {
                (first, (first + SNIPPET_CHARS).min(n))
            } else {
                let pad = (SNIPPET_CHARS - span) / 2;
                let end = (first.saturating_sub(pad) + SNIPPET_CHARS).min(n);
                (end.saturating_sub(SNIPPET_CHARS.min(n)), end)
            }
        }
        _ => (0, n.min(SNIPPET_CHARS)),
    };

    let mut out = String::new();
    let prefix = u32::from(start > 0);
    if start > 0 {
        out.push('…');
    }
    out.extend(&chars[start..end]);
    if end < n {
        out.push('…');
    }
    let local: Vec<u32> = sorted
        .into_iter()
        .filter(|&i| i >= start && i < end)
        .map(|i| (i - start) as u32 + prefix)
        .collect();
    (out, runs(&local))
}

/// Sorted unique offsets → inclusive-exclusive runs of consecutive offsets.
fn runs(offsets: &[u32]) -> Vec<MatchRange> {
    let mut out = Vec::new();
    let Some(&first) = offsets.first() else {
        return out;
    };
    let (mut run_start, mut prev) = (first, first);
    for &i in &offsets[1..] {
        if i == prev + 1 {
            prev = i;
            continue;
        }
        out.push(MatchRange {
            start: run_start,
            end: prev + 1,
        });
        run_start = i;
        prev = i;
    }
    out.push(MatchRange {
        start: run_start,
        end: prev + 1,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sid(n: u128) -> SessionId {
        SessionId(uuid::Uuid::from_u128(n))
    }

    fn index(titles: &[(u128, &str)], docs: &[(u128, i64, &str)]) -> Index {
        Index {
            titles: titles
                .iter()
                .enumerate()
                .map(|(i, (id, t))| Title {
                    id: sid(*id),
                    title: (*t).to_owned(),
                    updated_at: format!("2026-09-{:02}T00:00:00Z", i + 1),
                })
                .collect(),
            docs: docs
                .iter()
                .map(|(s, seq, text)| Doc {
                    session: sid(*s),
                    seq: *seq,
                    role: HitRole::User,
                    text: (*text).to_owned(),
                })
                .collect(),
        }
    }

    #[test]
    fn empty_query_returns_nothing() {
        let idx = index(&[(1, "deploy runbook")], &[]);
        assert!(idx.search("  ", 10).is_empty());
    }

    #[test]
    fn title_hit_outranks_body_hit_and_carries_ranges() {
        let idx = index(
            &[(1, "deploy checklist"), (2, "other")],
            &[(2, 10, "please deploy the service tomorrow")],
        );
        let hits = idx.search("deploy", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].session, sid(1));
        assert_eq!(hits[0].kind, HitKind::Title);
        assert_eq!(hits[0].ranges, vec![MatchRange { start: 0, end: 6 }]);
        assert_eq!(hits[1].kind, HitKind::Message);
        assert_eq!(hits[1].seq, Some(10));
        assert_eq!(hits[1].role, Some(HitRole::User));
    }

    #[test]
    fn one_hit_per_session_keeps_the_best_and_counts_matches() {
        let idx = index(
            &[(1, "misc")],
            &[
                (1, 1, "zzz unrelated text here"),
                (1, 2, "we should redeploy the edge workers"),
                (1, 3, "deploy deploy deploy"),
            ],
        );
        let hits = idx.search("deploy", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].matches, 2);
        let h = &hits[0];
        let r = h.ranges[0];
        let s: String = h
            .snippet
            .chars()
            .skip(r.start as usize)
            .take((r.end - r.start) as usize)
            .collect();
        assert_eq!(s, "deploy");
    }

    #[test]
    fn snippet_is_windowed_around_the_match_with_ellipses() {
        let long = format!("{}needle{}", "a ".repeat(200), " b".repeat(200));
        let (s, ranges) = snippet(&long, &[400, 401, 402, 403, 404, 405]);
        assert!(s.starts_with('…') && s.ends_with('…'));
        assert_eq!(s.chars().count(), SNIPPET_CHARS + 2);
        let r = ranges[0];
        let got: String = s
            .chars()
            .skip(r.start as usize)
            .take((r.end - r.start) as usize)
            .collect();
        assert_eq!(got, "needle");
    }

    #[test]
    fn fzf_syntax_is_honoured() {
        let idx = index(&[(1, "deploy checklist"), (2, "dpy")], &[]);
        assert_eq!(idx.search("dpy", 10).len(), 2, "fuzzy: both match");
        let exact = idx.search("'dpy", 10);
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].session, sid(2));
        assert_eq!(idx.search("dpy !check", 10).len(), 1);
    }

    #[test]
    fn runs_collapse_adjacent_offsets() {
        assert_eq!(
            runs(&[0, 1, 2, 5, 6, 9]),
            vec![
                MatchRange { start: 0, end: 3 },
                MatchRange { start: 5, end: 7 },
                MatchRange { start: 9, end: 10 },
            ]
        );
    }

    #[test]
    fn limit_is_clamped() {
        let titles: Vec<(u128, String)> = (1..=60).map(|i| (i, format!("deploy {i}"))).collect();
        let refs: Vec<(u128, &str)> = titles.iter().map(|(i, t)| (*i, t.as_str())).collect();
        let idx = index(&refs, &[]);
        assert_eq!(idx.search("deploy", 0).len(), 1);
        assert_eq!(idx.search("deploy", 1000).len(), MAX_LIMIT as usize);
    }
}
