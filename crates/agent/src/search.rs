//! Fuzzy session search via [`nucleo`] (helix/fzf scoring), design docs 26
//! and 49.
//!
//! An [`Index`] is every session's title plus the text of its visible user
//! and assistant messages, built from the store in one scan and cached by the
//! supervisor until a turn finishes, a title changes, a context edit lands or
//! a session is deleted. [`Index::search`] scores every entry against the
//! query, keeps the best hit per session (title hits boosted), and returns a
//! snippet around the matched characters with their offsets, so a frontend
//! can highlight them. Search stays raw transcript search (48 §2.2): a row
//! removed from the model's context is still indexed, and its hit says so.
//!
//! Query policy, snippets and highlight ranges are the shared `search` crate's
//! primitives every category uses; only the session records, the title
//! boost and the one-hit-per-session collapse live here.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use ::search::{FuzzyMatcher, MatchRange, SearchPage, SearchText, snippet};
use serde::{Deserialize, Serialize};

use crate::SessionId;
use crate::loop_::text_of;
use crate::store::Store;

/// The supervisor's cached [`Index`]. [`Cache::invalidate`] bumps a generation
/// (lock-free, from the event handler). A query rebuilds when the cached
/// index is older than the generation: [`Cache::page`] starts one build off
/// the caller's thread and answers `indexing` meanwhile. Builds coalesce on
/// one lock, held from the moment a stale slot is seen until the build
/// publishes, so concurrent pages find it busy and spawn nothing. A build
/// publishes under the generation it read, so a change during a build makes
/// the next query rebuild rather than trust it. Matching runs on a blocking
/// thread, one query at a time; a second concurrent query is refused, not
/// queued.
pub struct Cache {
    generation: AtomicU64,
    index: Arc<tokio::sync::Mutex<Option<Built>>>,
    matching: Arc<tokio::sync::Semaphore>,
}

/// A finished build: the generation it read, and the index or why not.
type Built = (u64, Result<Arc<Index>, String>);

impl Default for Cache {
    fn default() -> Self {
        Cache {
            generation: AtomicU64::new(0),
            index: Arc::default(),
            matching: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

impl Cache {
    /// The stored rows changed; the next search rebuilds.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// [`crate::SearchSessionPage`]: the bounded page for `query`, promptly.
    /// A cold or stale cache answers `indexing` and starts one build; an
    /// over-long query is `Err`; a blank query is an empty ready page and
    /// touches no cache.
    pub async fn page(
        self: &Arc<Self>,
        store: &Store,
        query: String,
        limit: Option<u32>,
    ) -> Result<SearchPage<SearchHit>, String> {
        ::search::validate_query(&query)?;
        let limit = ::search::limit(limit);
        if query.trim().is_empty() {
            return Ok(SearchPage::ready(Vec::new(), limit));
        }
        let index = match self.current(store) {
            None => return Ok(SearchPage::indexing()),
            Some(Err(e)) => return Ok(SearchPage::error(e)),
            Some(Ok(index)) => index,
        };
        // One past the limit: `has_more` is a fact, not a guess.
        Ok(match self.matched(index, query, limit + 1).await {
            Ok(hits) => SearchPage::ready(hits, limit),
            Err(e) => SearchPage::error(e.to_string()),
        })
    }

    /// The published index for the current generation, or `None` while a
    /// build for it is running (started here if nobody had).
    fn current(self: &Arc<Self>, store: &Store) -> Option<Result<Arc<Index>, String>> {
        let generation = self.generation.load(Ordering::Acquire);
        let Ok(mut slot) = self.index.clone().try_lock_owned() else {
            return None; // A build holds the lock.
        };
        if let Some((g, built)) = &*slot
            && *g == generation
        {
            return Some(built.clone());
        }
        // The guard travels with the build: nobody else can see the stale
        // slot and start a second one.
        let (cache, store) = (self.clone(), store.clone());
        tokio::spawn(async move {
            let _ = cache.build_into(&mut slot, &store).await;
        });
        None
    }

    /// With the slot locked: return its index if it is current, else build
    /// one and publish it (or the failure) under the generation it read.
    async fn build_into(
        &self,
        slot: &mut Option<Built>,
        store: &Store,
    ) -> Result<Arc<Index>, String> {
        let generation = self.generation.load(Ordering::Acquire);
        if let Some((g, built)) = &*slot
            && *g == generation
        {
            return built.clone();
        }
        let built = Index::build(store)
            .await
            .map(Arc::new)
            .map_err(|e| e.to_string());
        match &built {
            Ok(index) => tracing::debug!(entries = index.len(), "search index built"),
            Err(e) => tracing::warn!(error = %e, "search index build failed"),
        }
        *slot = Some((generation, built.clone()));
        built
    }

    /// [`Index::search`] on a blocking thread, holding the single matching
    /// permit until it returns; refused when another query holds it.
    async fn matched(
        &self,
        index: Arc<Index>,
        query: String,
        limit: u32,
    ) -> anyhow::Result<Vec<SearchHit>> {
        let permit = self
            .matching
            .clone()
            .try_acquire_owned()
            .map_err(|_| anyhow::anyhow!("another session search is running; retry shortly"))?;
        Ok(tokio::task::spawn_blocking(move || {
            let hits = index.search(&query, limit);
            drop(permit);
            hits
        })
        .await?)
    }
}

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

/// One session's best match.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SearchHit {
    /// The session.
    pub session: SessionId,
    /// The session's display title (66 §8): its title, else its first user
    /// line cut to 60 chars, else `Untitled chat · <Mon d>`. Never empty.
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
    /// The session's `created_at` (RFC 3339).
    pub created_at: String,
    /// The session's `updated_at` (RFC 3339), the tie-breaker after score.
    pub updated_at: String,
    /// The matched message was removed from the model's context (48 §2.2);
    /// the transcript still holds it. `false` for title hits.
    #[serde(default)]
    pub context_removed: bool,
    /// The session's root is archived (51 §3): hidden from the sidebar, still
    /// found here.
    #[serde(default)]
    pub archived: bool,
}

struct Title {
    id: SessionId,
    title: SearchText,
    created_at: String,
    updated_at: String,
    archived: bool,
    /// First user line of the transcript, the display title of a title-less session.
    first_line: Option<String>,
}

struct Doc {
    session: SessionId,
    seq: i64,
    role: HitRole,
    text: SearchText,
    /// The row's whole-message mask is set.
    removed: bool,
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
    text: &'a SearchText,
    score: u32,
    removed: bool,
}

impl Index {
    /// Every session's title and the text of its visible (`seq < cut_at`)
    /// user and assistant messages. Tool results are left out: they are
    /// large, and what matters in them the assistant repeats.
    pub async fn build(store: &Store) -> anyhow::Result<Index> {
        let docs: Vec<Doc> = store
            .search_bodies()
            .await?
            .into_iter()
            .filter_map(|(session, seq, message, removed)| {
                let role = match message.role {
                    genai::chat::ChatRole::User => HitRole::User,
                    genai::chat::ChatRole::Assistant => HitRole::Assistant,
                    _ => return None,
                };
                let text = text_of(&message.content);
                (!text.trim().is_empty()).then(|| Doc {
                    session,
                    seq,
                    role,
                    text: SearchText::new(text),
                    removed,
                })
            })
            .collect();
        // Bodies arrive ordered by (session, seq): the first user row wins.
        let mut first_lines: HashMap<SessionId, String> = HashMap::new();
        for d in &docs {
            if d.role == HitRole::User && !first_lines.contains_key(&d.session) {
                if let Some(line) = crate::attach::first_line(d.text.as_str()) {
                    first_lines.insert(d.session, line);
                }
            }
        }
        let titles = store
            .search_titles()
            .await?
            .into_iter()
            .map(|(id, title, created_at, updated_at, archived)| Title {
                first_line: first_lines.remove(&id),
                id,
                title: SearchText::new(title),
                created_at,
                updated_at,
                archived,
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
    /// updated, then session id; at most `limit` hits. Every session is
    /// considered before the cut, so `limit + 1` is an honest lookahead.
    /// Empty query → no hits.
    pub fn search<'a>(&'a self, query: &str, limit: u32) -> Vec<SearchHit> {
        let mut matcher = FuzzyMatcher::new(query);
        if matcher.is_empty() || self.titles.is_empty() {
            return Vec::new();
        }

        // Per session: the best candidate, and how many messages matched.
        // Scores only here; indices for the few hits that survive the cut.
        let mut best: HashMap<SessionId, Candidate<'a>> = HashMap::new();
        let mut counts: HashMap<SessionId, u32> = HashMap::new();
        let mut consider = |c: Candidate<'a>| {
            if c.kind == HitKind::Message {
                *counts.entry(c.session).or_insert(0) += 1;
            }
            match best.get(&c.session) {
                Some(prev) if prev.score >= c.score => {}
                _ => {
                    best.insert(c.session, c);
                }
            }
        };
        for t in &self.titles {
            if let Some(score) = matcher.score(&t.title) {
                consider(Candidate {
                    session: t.id,
                    kind: HitKind::Title,
                    seq: None,
                    role: None,
                    text: &t.title,
                    score: score.saturating_add(TITLE_BOOST),
                    removed: false,
                });
            }
        }
        for d in &self.docs {
            if let Some(score) = matcher.score(&d.text) {
                consider(Candidate {
                    session: d.session,
                    kind: HitKind::Message,
                    seq: Some(d.seq),
                    role: Some(d.role),
                    text: &d.text,
                    score,
                    removed: d.removed,
                });
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
        ranked.truncate(limit as usize);
        ranked
            .into_iter()
            .map(|c| {
                let (text, ranges) = match matcher.indices(c.text) {
                    Some((_, indices)) => snippet(c.text.as_str(), indices, SNIPPET_CHARS),
                    None => (String::new(), Vec::new()),
                };
                let t = by_id.get(&c.session);
                SearchHit {
                    session: c.session,
                    title: t
                        .map(|t| t.display_title())
                        .unwrap_or_else(|| "Untitled chat".to_owned()),
                    kind: c.kind,
                    seq: c.seq,
                    role: c.role,
                    snippet: text,
                    ranges,
                    score: c.score,
                    matches: counts.get(&c.session).copied().unwrap_or(0),
                    created_at: t.map(|t| t.created_at.clone()).unwrap_or_default(),
                    updated_at: t.map(|t| t.updated_at.clone()).unwrap_or_default(),
                    context_removed: c.removed,
                    archived: t.is_some_and(|t| t.archived),
                }
            })
            .collect()
    }
}

impl Title {
    /// [`crate::attach::session_title`] over the row.
    fn display_title(&self) -> String {
        let created = chrono::DateTime::parse_from_rfc3339(&self.created_at)
            .ok()
            .map(|t| t.with_timezone(&chrono::Utc));
        crate::attach::session_title(self.title.as_str(), self.first_line.as_deref(), created)
    }
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
                    title: SearchText::new(*t),
                    created_at: "2026-09-01T00:00:00Z".to_owned(),
                    updated_at: format!("2026-09-{:02}T00:00:00Z", i + 1),
                    archived: false,
                    first_line: docs
                        .iter()
                        .find(|(s, seq, _)| s == id && *seq >= 0)
                        .map(|(_, _, text)| text.to_string()),
                })
                .collect(),
            docs: docs
                .iter()
                .map(|(s, seq, text)| Doc {
                    session: sid(*s),
                    seq: *seq,
                    role: HitRole::User,
                    text: SearchText::new(*text),
                    removed: *seq < 0,
                })
                .collect(),
        }
    }

    #[test]
    fn empty_query_returns_nothing() {
        let idx = index(&[(1, "deploy runbook")], &[]);
        assert!(idx.search("  ", 10).is_empty());
    }

    /// 66 §8: a hit always carries a human title — the session's, else the
    /// first user line (≤ 60 chars), else `Untitled chat · <Mon d>`.
    #[test]
    fn title_less_sessions_get_a_display_title() {
        let long = "x".repeat(70);
        let idx = index(
            &[(1, "deploy runbook"), (2, ""), (3, ""), (4, "")],
            &[
                (1, 1, "deploy it"),
                (2, 1, "please deploy the relay"),
                (3, 1, &long),
            ],
        );
        let hits = idx.search("deploy", 10);
        let by = |n: u128| {
            hits.iter()
                .find(|h| h.session == sid(n))
                .unwrap()
                .title
                .clone()
        };
        assert_eq!(by(1), "deploy runbook");
        assert_eq!(by(2), "please deploy the relay");
        let mut idx2 = index(&[(4, "")], &[(4, 1, "deploy")]);
        idx2.titles[0].first_line = None;
        let h = &idx2.search("deploy", 10)[0];
        assert!(h.title.starts_with("Untitled chat · Sep "), "{}", h.title);
        assert_eq!(h.created_at, "2026-09-01T00:00:00Z");
        let idx3 = index(&[(3, "")], &[(3, 1, &long)]);
        let h = &idx3.search("xxx", 10)[0];
        assert_eq!(h.title.chars().count(), 61);
        assert!(h.title.ends_with('…'));
    }

    /// A removed row stays searchable (raw transcript search, 48 §2.2) and its
    /// hit is labeled; a title hit never is.
    #[test]
    fn removed_rows_stay_indexed_and_labeled() {
        // The test builder marks negative seqs as removed from context.
        let idx = index(
            &[(1, "notes"), (2, "notes")],
            &[(1, -3, "the old screenshot"), (2, 4, "the old screenshot")],
        );
        let hits = idx.search("screenshot", 10);
        assert_eq!(hits.len(), 2);
        let by = |n: u128| hits.iter().find(|h| h.session == sid(n)).unwrap();
        assert!(by(1).context_removed);
        assert!(!by(2).context_removed);
        assert!(!idx.search("notes", 10).iter().any(|h| h.context_removed));
    }

    /// An archived session is still found (51 §3) and every kind of hit on
    /// it says so; a live one never does.
    #[test]
    fn archived_sessions_stay_indexed_and_labeled() {
        let mut idx = index(
            &[(1, "deploy notes"), (2, "other")],
            &[(1, 3, "deploy the edge"), (2, 4, "deploy the core")],
        );
        idx.titles[0].archived = true;
        let hits = idx.search("deploy", 10);
        let by = |n: u128| hits.iter().find(|h| h.session == sid(n)).unwrap();
        assert!(by(1).archived);
        assert!(!by(2).archived);
        assert!(idx.search("edge", 10)[0].archived);
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
    fn fzf_syntax_is_honoured() {
        let idx = index(&[(1, "deploy checklist"), (2, "dpy")], &[]);
        assert_eq!(idx.search("dpy", 10).len(), 2, "fuzzy: both match");
        let exact = idx.search("'dpy", 10);
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].session, sid(2));
        assert_eq!(idx.search("dpy !check", 10).len(), 1);
    }

    /// The whole corpus is ranked before the cut, so a `limit + 1` lookahead
    /// past the old 50 cap reports `has_more` instead of masking it.
    #[test]
    fn lookahead_past_max_limit_sets_has_more() {
        let titles: Vec<(u128, String)> = (1..=60).map(|i| (i, format!("deploy {i}"))).collect();
        let refs: Vec<(u128, &str)> = titles.iter().map(|(i, t)| (*i, t.as_str())).collect();
        let idx = index(&refs, &[]);
        assert_eq!(idx.search("deploy", 51).len(), 51);
        let page = SearchPage::ready(idx.search("deploy", 51), 50);
        assert_eq!(page.hits.len(), 50);
        assert!(page.has_more);
        let page = SearchPage::ready(idx.search("deploy", 61), 60);
        assert!(!page.has_more);
    }

    /// The first pass keeps scores only; the hits that survive still carry
    /// scalar-accurate ranges through non-ASCII text.
    #[test]
    fn ranges_survive_non_ascii_text() {
        let idx = index(
            &[(1, "misc")],
            &[(1, 1, "naïve 👨\u{200d}👩 déploiement done")], // no-emoji: allow
        );
        let hits = idx.search("déploiement", 10);
        assert_eq!(hits.len(), 1);
        let r = hits[0].ranges[0];
        let s: String = hits[0]
            .snippet
            .chars()
            .skip(r.start as usize)
            .take((r.end - r.start) as usize)
            .collect();
        assert_eq!(s, "déploiement");
    }

    /// A cold page answers `indexing` and starts the build; once it lands the
    /// page is ready with the new rows; an invalidation goes cold again.
    #[tokio::test]
    async fn page_goes_indexing_then_ready_and_invalidate_resets() {
        use ::search::PageState;
        let dir = std::env::temp_dir().join(format!("theuth-search-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        store.set_title(id, "deploy checklist").await.unwrap();
        let cache = Arc::new(Cache::default());

        let page = |q: &'static str| cache.page(&store, q.to_owned(), None);
        let settled = |q: &'static str| async move {
            for _ in 0..200 {
                let p = page(q).await.unwrap();
                if p.state != PageState::Indexing {
                    return p;
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
            panic!("build never published");
        };

        assert_eq!(page("deploy").await.unwrap().state, PageState::Indexing);
        let ready = settled("deploy").await;
        assert_eq!(ready.state, PageState::Ready);
        assert_eq!(ready.hits.len(), 1);
        assert_eq!(ready.hits[0].session, id);

        store.set_title(id, "release notes").await.unwrap();
        assert_eq!(
            settled("release").await.hits.len(),
            0,
            "stale index until invalidated"
        );
        cache.invalidate();
        assert_eq!(page("release").await.unwrap().state, PageState::Indexing);
        assert_eq!(settled("release").await.hits.len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }
}
