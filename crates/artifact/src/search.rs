//! Artifact search (design doc 49 §3.2): every retained occurrence, matched
//! on name/title/kind and on the complete saved text where the saved bytes
//! carry any — UTF-8 kinds and PDF sources as they are, PDFs and Office
//! documents through [`::tools::doc`]. Images, unrecognised binaries and
//! anything extraction refuses stay metadata-only, make coverage partial and
//! say why.
//!
//! [`Cache`] holds one immutable [`Index`] per store generation
//! ([`Cache::invalidate`] from `State::show`/`delete_session`), built by a
//! single task off the mailbox and published only when complete; a cold
//! search reports `indexing`. Extracted bodies — failures included — are
//! shared by immutable blob or source identity across occurrences and builds,
//! so a query never re-reads or re-converts anything; only an explicit
//! [`RefreshSearch`] ([`Cache::refresh`]) retries what failed.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::{Artifact, Kind};
use ::search::{Coverage, FuzzyMatcher, SearchNotice, SearchPage, SearchText, TextMatch};
use kameo::actor::ActorRef;
use parking_lot::Mutex;
use runtime::SessionId;
use serde::{Deserialize, Serialize};

/// Scalars of surrounding text in an [`ArtifactMatchContext`] (49 §4.2).
pub const CONTEXT_CHARS: usize = 2000;
/// Scalars of context in a hit snippet.
const SNIPPET_CHARS: usize = 160;
/// A name/title hit outranks a body hit of the same score band (the session
/// matcher's `TITLE_BOOST`).
const META_BOOST: u32 = 32;
/// Distinct "content not searchable"/"metadata only" reasons reported per page.
const MAX_NOTICES: usize = 5;

/// Whether a record's saved content was searched or only its metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ContentCoverage {
    /// The complete saved text (or source) was in the index.
    Complete,
    /// Name, title and kind only: an image, an unrecognised binary, or an
    /// extraction failure (the page's notices name the reason).
    MetadataOnly,
}

/// One occurrence that matched: the exact record, never a merged blob.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ArtifactSearchHit {
    /// The record (identity is `call_id`).
    pub artifact: Artifact,
    /// Winning field (`name`, `title`, `kind`, `content`, `source`) with snippet.
    pub r#match: TextMatch,
    /// What of this record the index covers.
    pub content_coverage: ContentCoverage,
}

/// [`ReadSearchMatch`]'s answer: the same winning field a search would pick,
/// with up to [`CONTEXT_CHARS`] scalars around it and ranges rebased to it.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ArtifactMatchContext {
    /// The resolved record.
    pub call_id: String,
    /// Field, context text and highlight ranges.
    pub r#match: TextMatch,
    /// What of this record the index covers.
    pub content_coverage: ContentCoverage,
}

/// `ask` → `Result<SearchPage<ArtifactSearchHit>, String>`: every retained
/// occurrence across sessions, best score first. Runs off the mailbox.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchArtifacts {
    /// fzf-style pattern.
    pub query: String,
    /// Hits per page; `None` → the shared default, clamped to the shared max.
    pub limit: Option<u32>,
}

/// `ask` → `Result<Artifact, String>`: the exact record, or why not (unknown
/// `call_id`, or its owning session no longer exists). A private record is
/// found only through its tree's `session`, which the caller authorized
/// (51 §4.2); `None` sees durable records only.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Get {
    /// Record identity.
    pub call_id: String,
    /// An authorized session whose private tree may hold the record.
    pub session: Option<SessionId>,
}

/// `ask` → `Result<ArtifactMatchContext, String>`: bounded context for the
/// selected occurrence from the indexed projection; `Err` while indexing,
/// when the record is gone, or when nothing in it matches. Durable records
/// only: a private tree is never indexed (51 §4.5).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadSearchMatch {
    /// Record identity.
    pub call_id: String,
    /// The query that produced the hit.
    pub query: String,
}

/// `ask` → `()`: the UI's explicit Refresh (49 §4.3). Drops the current
/// publication and every failed extraction (invalid UTF-8, malformed or
/// oversized documents, conversion errors and timeouts, converter-busy) so
/// the next query rebuilds and retries them; successfully extracted text and
/// metadata-only verdicts stay cached. Queries alone never retry.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct RefreshSearch;

/// Ids of every session the agent supervisor still knows. Asked, never read
/// from `sessions.sqlite`; without the supervisor nothing is verifiable, so
/// the answer is `Err` rather than "all alive".
pub(crate) async fn alive_sessions() -> Result<HashSet<SessionId>, String> {
    let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or("agent actor is not running; sessions cannot be verified")?;
    let sessions = agents
        .ask(agent::ListSessions)
        .await
        .map_err(|e| format!("list sessions: {e}"))?;
    Ok(sessions.into_iter().map(|s| s.id).collect())
}

/// Rows for one build: every occurrence whose owning session is alive.
pub type Fetch = Pin<Box<dyn Future<Output = Result<Vec<Artifact>, String>> + Send>>;

/// What a saved payload contributes to the index.
pub(crate) enum Body {
    /// Searchable text and the field it is reported under (`content`/`source`).
    Text {
        field: &'static str,
        text: SearchText,
    },
    /// Nothing to search by design: an image (no OCR) or an unrecognised
    /// binary. Reported as a `metadata_only` notice; kept across refreshes.
    Metadata(&'static str),
    /// Content exists but is not searchable; a `content_not_searchable`
    /// notice. Kept until [`Cache::refresh`].
    Unavailable(String),
}

/// The immutable identity a body is cached under: the saved source of a
/// compiled PDF, else the blob (`<hash>.<ext>`, so a same-hash rename with
/// another extension extracts on its own).
fn key(a: &Artifact) -> PathBuf {
    match (a.kind, &a.source) {
        (Kind::Pdf, Some(source)) => source.clone(),
        _ => a.path.clone(),
    }
}

async fn prepared(field: &'static str, text: String) -> Body {
    // ponytail: UTF-32 preparation is 4× the text; a 32 MiB artifact costs
    // ~128 MiB resident. Measure 49 §8 before adding a size ceiling.
    tokio::task::spawn_blocking(move || Body::Text {
        field,
        text: SearchText::new(text),
    })
    .await
    .unwrap_or_else(|e| Body::Unavailable(format!("extraction panicked: {e}")))
}

async fn utf8(path: &Path, field: &'static str) -> Body {
    let bytes = match tokio::fs::read(path).await {
        Ok(b) => b,
        Err(e) => return Body::Unavailable(format!("saved file unreadable: {e}")),
    };
    match tokio::task::spawn_blocking(move || String::from_utf8(bytes)).await {
        Ok(Ok(text)) => prepared(field, text).await,
        Ok(Err(_)) => Body::Unavailable("not valid UTF-8".into()),
        Err(e) => Body::Unavailable(format!("extraction panicked: {e}")),
    }
}

/// PDF without a saved source, or any `Kind::File`, through anydoc. `Err` is
/// transient (the single conversion slot is still occupied by a conversion
/// that outlived its deadline) and is not cached; `Ok(Unavailable)` is.
async fn convert(
    a: &Artifact,
    pending: &mut Option<::tools::doc::Pending>,
) -> Result<Body, String> {
    if a.bytes > ::tools::doc::MAX_BYTES as u64 {
        return Ok(Body::Unavailable(format!(
            "{} exceeds the {} MB conversion cap",
            platform::fmt::bytes(a.bytes),
            ::tools::doc::MAX_BYTES >> 20
        )));
    }
    let bytes = match tokio::fs::read(&a.path).await {
        Ok(b) => b,
        Err(e) => return Ok(Body::Unavailable(format!("saved file unreadable: {e}"))),
    };
    let Some(format) = ::tools::doc::format(&bytes) else {
        // The extension promised a document the bytes do not carry: a real
        // failure, never an empty successful body.
        return Ok(match ::tools::doc::Format::from_path(&a.path) {
            Some(promised) => Body::Unavailable(format!(
                "malformed or unsupported {promised:?} document: bytes carry no recognised format"
            )),
            None => Body::Metadata("unrecognised binary: metadata only"),
        });
    };
    if pending.as_ref().is_some_and(|p| !p.is_finished()) {
        return Err(
            "converter busy: an earlier conversion is still running past its deadline".into(),
        );
    }
    let (result, late) = ::tools::doc::convert(bytes, format).await;
    *pending = late;
    Ok(match result {
        Ok(md) => prepared("content", md).await,
        Err(e) => Body::Unavailable(format!("{e:#}")),
    })
}

/// Saved bytes → [`Body`]. Reads `path`/`source` only, never `origin`.
async fn extract(
    a: &Artifact,
    pending: &mut Option<::tools::doc::Pending>,
) -> Result<Body, String> {
    Ok(match (a.kind, &a.source) {
        (Kind::Image, _) => Body::Metadata("image: metadata only, not OCR'd"),
        (Kind::Markdown | Kind::Text | Kind::Csv | Kind::Mermaid, _) => {
            utf8(&a.path, "content").await
        }
        (Kind::Html | Kind::Svg | Kind::App, _) => utf8(&a.path, "source").await,
        (Kind::Pdf, Some(source)) => utf8(source, "source").await,
        (Kind::Pdf | Kind::File, _) => return convert(a, pending).await,
        // The member paths, one per line: a file name inside is findable;
        // member contents are never extracted (35 §3.2).
        (Kind::Archive, _) => match crate::archive::read_listing(&a.path).await {
            Ok(l) => {
                let paths: Vec<&str> = l.entries.iter().map(|e| e.path.as_str()).collect();
                prepared("content", paths.join("\n")).await
            }
            Err(_) => Body::Metadata("archive: no listing (unreadable at show time)"),
        },
    })
}

#[derive(Clone, Copy)]
enum Field {
    Name,
    Title,
    Body,
    Kind,
}

struct Entry {
    artifact: Artifact,
    name: SearchText,
    /// Absent when it equals the name (the default title).
    title: Option<SearchText>,
    kind: SearchText,
    body: Arc<Body>,
}

impl Entry {
    /// The prepared text, wire field name and score boost of `f`.
    fn text(&self, f: Field) -> Option<(&SearchText, &'static str, u32)> {
        match f {
            Field::Name => Some((&self.name, "name", META_BOOST)),
            Field::Title => self.title.as_ref().map(|t| (t, "title", META_BOOST)),
            Field::Kind => Some((&self.kind, "kind", 0)),
            Field::Body => match &*self.body {
                Body::Text { field, text } => Some((text, *field, 0)),
                _ => None,
            },
        }
    }

    /// Best-scoring field; ties go to the earlier of name, title, body, kind.
    fn best(&self, m: &mut FuzzyMatcher) -> Option<(u32, Field)> {
        let mut best: Option<(u32, Field)> = None;
        for f in [Field::Name, Field::Title, Field::Body, Field::Kind] {
            let Some((text, _, boost)) = self.text(f) else {
                continue;
            };
            let Some(score) = m.score(text) else { continue };
            let score = score.saturating_add(boost);
            if best.is_none_or(|(s, _)| score > s) {
                best = Some((score, f));
            }
        }
        best
    }

    fn coverage(&self) -> ContentCoverage {
        match &*self.body {
            Body::Text { .. } => ContentCoverage::Complete,
            Body::Metadata(_) | Body::Unavailable(_) => ContentCoverage::MetadataOnly,
        }
    }
}

/// One published snapshot of the searchable corpus.
pub struct Index {
    entries: Vec<Entry>,
    by_call: HashMap<String, usize>,
    coverage: Coverage,
    notices: Vec<SearchNotice>,
}

impl Index {
    /// `bodies` is keyed by [`key`] and must cover every row. Any non-text
    /// body makes coverage partial, with bounded per-reason counts.
    pub(crate) fn build(rows: Vec<Artifact>, bodies: &HashMap<PathBuf, Arc<Body>>) -> Self {
        let entries: Vec<Entry> = rows
            .into_iter()
            .map(|artifact| {
                let body = bodies
                    .get(&key(&artifact))
                    .cloned()
                    .unwrap_or_else(|| Arc::new(Body::Unavailable("not extracted".into())));
                Entry {
                    name: SearchText::new(artifact.name.clone()),
                    title: (artifact.title != artifact.name)
                        .then(|| SearchText::new(artifact.title.clone())),
                    kind: SearchText::new(artifact.kind.as_str()),
                    artifact,
                    body,
                }
            })
            .collect();
        let mut reasons: HashMap<(&str, &str), u32> = HashMap::new();
        for e in &entries {
            let k = match &*e.body {
                Body::Text { .. } => continue,
                Body::Metadata(reason) => ("metadata_only", *reason),
                Body::Unavailable(reason) => ("content_not_searchable", reason.as_str()),
            };
            *reasons.entry(k).or_insert(0) += 1;
        }
        let mut reasons: Vec<((&str, &str), u32)> = reasons.into_iter().collect();
        reasons.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        let coverage = if reasons.is_empty() {
            Coverage::Complete
        } else {
            Coverage::Partial
        };
        let notices = reasons
            .iter()
            .take(MAX_NOTICES)
            .map(|((code, reason), n)| SearchNotice {
                code: (*code).to_owned(),
                message: (*reason).to_owned(),
                affected: Some(*n),
            })
            .collect();
        let by_call = entries
            .iter()
            .enumerate()
            .map(|(i, e)| (e.artifact.call_id.clone(), i))
            .collect();
        Self {
            entries,
            by_call,
            coverage,
            notices,
        }
    }

    /// Occurrences indexed.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether nothing is indexed.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every occurrence's best field scored over the whole corpus, ordered
    /// score desc, created desc, `call_id` asc; `limit + 1` kept for `has_more`.
    pub fn search(&self, query: &str, limit: u32) -> SearchPage<ArtifactSearchHit> {
        let mut m = FuzzyMatcher::new(query);
        // No atoms (`!` alone, …): everything would match at score 0.
        let mut ranked: Vec<(u32, Field, &Entry)> = if m.is_empty() {
            Vec::new()
        } else {
            self.entries
                .iter()
                .filter_map(|e| e.best(&mut m).map(|(s, f)| (s, f, e)))
                .collect()
        };
        ranked.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.2.artifact.created_at.cmp(&a.2.artifact.created_at))
                .then_with(|| a.2.artifact.call_id.cmp(&b.2.artifact.call_id))
        });
        ranked.truncate(limit as usize + 1);
        let hits = ranked
            .into_iter()
            .filter_map(|(_, f, e)| {
                let (text, field, _) = e.text(f)?;
                let (_, r#match) = m.matched(text, field, SNIPPET_CHARS)?;
                Some(ArtifactSearchHit {
                    artifact: e.artifact.clone(),
                    r#match,
                    content_coverage: e.coverage(),
                })
            })
            .collect();
        let mut page = SearchPage::ready(hits, limit);
        page.coverage = self.coverage;
        page.notices = self.notices.clone();
        page
    }

    /// The same winning field a search picks for `call_id`, with
    /// [`CONTEXT_CHARS`] of context.
    pub fn read_match(&self, call_id: &str, query: &str) -> Result<ArtifactMatchContext, String> {
        let e = self
            .by_call
            .get(call_id)
            .map(|&i| &self.entries[i])
            .ok_or_else(|| format!("{call_id}: not indexed yet; retry shortly"))?;
        let mut m = FuzzyMatcher::new(query);
        let no_match = || format!("{call_id}: nothing in this artifact matches the query");
        if m.is_empty() {
            return Err(no_match());
        }
        let (_, f) = e.best(&mut m).ok_or_else(no_match)?;
        let (text, field, _) = e.text(f).ok_or_else(no_match)?;
        let (_, r#match) = m.matched(text, field, CONTEXT_CHARS).ok_or_else(no_match)?;
        Ok(ArtifactMatchContext {
            call_id: e.artifact.call_id.clone(),
            r#match,
            content_coverage: e.coverage(),
        })
    }
}

#[derive(Default)]
struct Inner {
    /// The index for its generation, or why the build failed.
    published: Option<(u64, Result<Arc<Index>, String>)>,
    /// A build task is running (it rebuilds until the generation holds still).
    building: bool,
    /// Bodies by immutable identity, kept across builds while referenced.
    texts: HashMap<PathBuf, Arc<Body>>,
    /// [`Cache::refresh`] asked: the next build drops failed bodies first.
    purge_failures: bool,
    /// A conversion that outlived its deadline; the slot stays taken until it ends.
    pending: Option<::tools::doc::Pending>,
}

/// The store's derived index: generation counter, one publication, shared
/// bodies, one build task, one matching permit (49 §4.3).
pub struct Cache {
    generation: AtomicU64,
    inner: Mutex<Inner>,
    matching: Arc<tokio::sync::Semaphore>,
}

impl Default for Cache {
    fn default() -> Self {
        Self {
            generation: AtomicU64::new(0),
            inner: Mutex::default(),
            matching: Arc::new(tokio::sync::Semaphore::new(1)),
        }
    }
}

impl Cache {
    /// The rows changed; the current publication is stale.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::Release);
    }

    /// Explicit Refresh: the next build retries every failed extraction
    /// (see [`RefreshSearch`]); successful text and metadata-only verdicts
    /// stay. Flagged rather than purged here so a build already in flight
    /// cannot write the stale failures back.
    pub fn refresh(&self) {
        self.inner.lock().purge_failures = true;
        self.invalidate();
    }

    /// The current index (or the failure of its build), or `None` while one
    /// is being built — started here with `fetch` when none is running.
    pub fn index<F>(self: &Arc<Self>, fetch: F) -> Option<Result<Arc<Index>, String>>
    where
        F: Fn() -> Fetch + Send + 'static,
    {
        let generation = self.generation.load(Ordering::Acquire);
        let mut inner = self.inner.lock();
        if let Some((g, published)) = &inner.published
            && *g == generation
        {
            return Some(published.clone());
        }
        if !inner.building {
            inner.building = true;
            tokio::spawn(self.clone().build(fetch));
        }
        None
    }

    /// Run `f` on the blocking pool under the single matching permit;
    /// `Err` when a match is already running.
    pub async fn matched<T, F>(&self, f: F) -> Result<T, String>
    where
        T: Send + 'static,
        F: FnOnce() -> T + Send + 'static,
    {
        let permit = self
            .matching
            .clone()
            .try_acquire_owned()
            .map_err(|_| "artifact search is busy; retry".to_owned())?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            f()
        })
        .await
        .map_err(|e| format!("artifact search failed: {e}"))
    }

    async fn build<F>(self: Arc<Self>, fetch: F)
    where
        F: Fn() -> Fetch,
    {
        loop {
            let generation = self.generation.load(Ordering::Acquire);
            let result = match fetch().await {
                Ok(rows) => Ok(Arc::new(self.extract_all(rows).await)),
                Err(e) => Err(e),
            };
            let mut inner = self.inner.lock();
            // A mutation landed meanwhile: this publication would be stale.
            if self.generation.load(Ordering::Acquire) != generation {
                continue;
            }
            inner.published = Some((generation, result));
            inner.building = false;
            return;
        }
    }

    /// Bodies for `rows`, extracting what no earlier build cached (serially:
    /// one conversion at a time) and dropping what nothing references.
    /// Transient converter-busy verdicts are never cached, so they retry on
    /// the next build; cached failures retry only after [`Cache::refresh`].
    async fn extract_all(&self, rows: Vec<Artifact>) -> Index {
        let (mut known, mut pending) = {
            let mut inner = self.inner.lock();
            let mut known = std::mem::take(&mut inner.texts);
            if std::mem::take(&mut inner.purge_failures) {
                known.retain(|_, b| !matches!(**b, Body::Unavailable(_)));
            }
            (known, inner.pending.take())
        };
        let mut kept: HashMap<PathBuf, Arc<Body>> = HashMap::new();
        let mut all: HashMap<PathBuf, Arc<Body>> = HashMap::new();
        for a in &rows {
            let k = key(a);
            if all.contains_key(&k) {
                continue;
            }
            let body = match known.remove(&k) {
                Some(b) => b,
                None => match extract(a, &mut pending).await {
                    Ok(b) => Arc::new(b),
                    Err(reason) => {
                        all.insert(k, Arc::new(Body::Unavailable(reason)));
                        continue;
                    }
                },
            };
            kept.insert(k.clone(), body.clone());
            all.insert(k, body);
        }
        {
            let mut inner = self.inner.lock();
            inner.texts = kept;
            inner.pending = pending;
        }
        Index::build(rows, &all)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn art(dir: &Path, call_id: &str, name: &str, version: u32, created_at: i64) -> Artifact {
        let id = format!("{name}-blob");
        Artifact {
            path: dir.join(format!("{id}.{}", kind::ext(name).unwrap())),
            id,
            session: SessionId::new(),
            call_id: call_id.into(),
            name: name.into(),
            title: name.into(),
            kind: kind::of(name),
            mime: kind::mime(kind::of(name), name),
            bytes: 0,
            version,
            source: None,
            preview: None,
            pages: None,
            origin: None,
            created_at,
        }
    }

    fn rows_fetch(rows: Arc<Mutex<Vec<Artifact>>>) -> impl Fn() -> Fetch + Clone + Send + 'static {
        move || -> Fetch {
            let rows = rows.lock().clone();
            Box::pin(async move { Ok(rows) })
        }
    }

    async fn ready<F: Fn() -> Fetch + Send + Clone + 'static>(
        cache: &Arc<Cache>,
        fetch: F,
    ) -> Arc<Index> {
        for _ in 0..500 {
            if let Some(r) = cache.index(fetch.clone()) {
                return r.expect("build ok");
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("index never published");
    }

    #[tokio::test]
    async fn same_blob_is_two_occurrences_and_late_content_arrives() {
        let dir = tempfile::tempdir().unwrap();
        let a = art(dir.path(), "c1", "plan.md", 1, 10);
        std::fs::write(&a.path, "# Plan\n\nship the deployment on friday\n").unwrap();
        let mut b = a.clone();
        b.call_id = "c2".into();
        b.version = 2;
        b.created_at = 20;
        let rows = Arc::new(Mutex::new(vec![a.clone(), b]));
        let cache = Arc::new(Cache::default());
        let fetch = rows_fetch(rows.clone());

        assert!(
            cache.index(fetch.clone()).is_none(),
            "cold build reports indexing"
        );
        let index = ready(&cache, fetch.clone()).await;
        let page = index.search("deployment friday", 10);
        let ids: Vec<&str> = page
            .hits
            .iter()
            .map(|h| h.artifact.call_id.as_str())
            .collect();
        assert_eq!(ids, ["c2", "c1"], "equal blobs stay separate, newest first");
        assert_eq!(page.hits[0].r#match.field, "content");
        assert_eq!(page.hits[0].content_coverage, ContentCoverage::Complete);
        assert!(!page.hits[0].r#match.ranges.is_empty());
        assert!(!page.has_more);
        assert!(index.search("nonexistent-zzz", 10).hits.is_empty());

        // Same generation: the same Arc, no rebuild.
        assert!(Arc::ptr_eq(&index, &ready(&cache, fetch.clone()).await));

        let mut c = art(dir.path(), "c3", "notes.txt", 1, 30);
        std::fs::write(&c.path, "the quarterly budget review\n").unwrap();
        c.title = "Budget".into();
        rows.lock().push(c);
        assert!(index.search("quarterly budget", 10).hits.is_empty());
        cache.invalidate();
        assert!(
            cache.index(fetch.clone()).is_none(),
            "stale publication is not served"
        );
        let index = ready(&cache, fetch).await;
        assert_eq!(index.len(), 3);
        let page = index.search("quarterly budget", 10);
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].artifact.call_id, "c3");
        let page = index.search("Budget", 10);
        assert_eq!(page.hits[0].r#match.field, "title", "title outranks body");
    }

    #[tokio::test]
    async fn coverage_names_failures_and_deleted_rows_vanish() {
        let dir = tempfile::tempdir().unwrap();
        let ok = art(dir.path(), "ok", "readme.md", 1, 3);
        std::fs::write(&ok.path, "alpha beta gamma\n").unwrap();
        let bad = art(dir.path(), "bad", "dump.txt", 1, 2);
        std::fs::write(&bad.path, [0xff, 0xfe, b'a']).unwrap();
        let img = art(dir.path(), "img", "shot.png", 1, 1);
        std::fs::write(&img.path, b"not a png").unwrap();
        let rows = Arc::new(Mutex::new(vec![ok, bad, img]));
        let cache = Arc::new(Cache::default());
        let fetch = rows_fetch(rows.clone());

        let index = ready(&cache, fetch.clone()).await;
        let page = index.search("dump", 10);
        assert!(matches!(page.coverage, Coverage::Partial));
        let codes: Vec<(&str, &str, Option<u32>)> = page
            .notices
            .iter()
            .map(|n| (n.code.as_str(), n.message.as_str(), n.affected))
            .collect();
        assert_eq!(
            codes,
            [
                ("content_not_searchable", "not valid UTF-8", Some(1)),
                ("metadata_only", "image: metadata only, not OCR'd", Some(1)),
            ],
            "failure and by-design metadata-only are both reported"
        );
        assert_eq!(page.hits[0].r#match.field, "name");
        assert_eq!(page.hits[0].content_coverage, ContentCoverage::MetadataOnly);
        let shot = index.search("shot", 10);
        assert_eq!(shot.hits[0].content_coverage, ContentCoverage::MetadataOnly);
        assert!(
            index.read_match("bad", "alpha").is_err(),
            "metadata-only row has no body match"
        );

        rows.lock().retain(|a| a.call_id != "bad");
        cache.invalidate();
        let index = ready(&cache, fetch.clone()).await;
        let page = index.search("x", 10);
        assert!(
            matches!(page.coverage, Coverage::Partial),
            "an image alone still means metadata-only coverage"
        );
        assert_eq!(page.notices.len(), 1);
        assert_eq!(page.notices[0].code, "metadata_only");
        assert!(index.search("dump", 10).hits.is_empty());
        assert!(
            index
                .read_match("bad", "dump")
                .unwrap_err()
                .contains("not indexed")
        );

        rows.lock().retain(|a| a.call_id != "img");
        cache.invalidate();
        let index = ready(&cache, fetch).await;
        assert!(matches!(index.search("x", 10).coverage, Coverage::Complete));
        assert!(index.search("x", 10).notices.is_empty());
    }

    #[tokio::test]
    async fn failed_bodies_retry_only_on_refresh_and_text_survives_it() {
        let dir = tempfile::tempdir().unwrap();
        let ok = art(dir.path(), "ok", "readme.md", 1, 2);
        std::fs::write(&ok.path, "alpha beta gamma\n").unwrap();
        let bad = art(dir.path(), "bad", "dump.txt", 1, 1);
        std::fs::write(&bad.path, [0xff, 0xfe, b'a']).unwrap();
        let rows = Arc::new(Mutex::new(vec![ok, bad.clone()]));
        let cache = Arc::new(Cache::default());
        let fetch = rows_fetch(rows.clone());

        let first = ready(&cache, fetch.clone()).await;
        assert_eq!(
            first.search("dump", 10).notices[0].message,
            "not valid UTF-8"
        );
        let ok_body = first.entries[0].body.clone();

        // The bytes become readable, but a plain mutation reuses the failure.
        std::fs::write(&bad.path, "now valid delta text\n").unwrap();
        cache.invalidate();
        let second = ready(&cache, fetch.clone()).await;
        assert!(
            !Arc::ptr_eq(&first, &second),
            "rebuilt for the new generation"
        );
        assert!(
            second.search("delta", 10).hits.is_empty(),
            "query/rebuild is not an extraction retry"
        );
        assert_eq!(
            second.search("dump", 10).notices[0].message,
            "not valid UTF-8"
        );
        assert!(Arc::ptr_eq(&ok_body, &second.entries[0].body));

        cache.refresh();
        assert!(
            cache.index(fetch.clone()).is_none(),
            "refresh discards the publication"
        );
        let third = ready(&cache, fetch).await;
        let page = third.search("delta", 10);
        assert_eq!(page.hits.len(), 1);
        assert_eq!(page.hits[0].artifact.call_id, "bad");
        assert_eq!(page.hits[0].content_coverage, ContentCoverage::Complete);
        assert!(matches!(page.coverage, Coverage::Complete));
        assert!(
            Arc::ptr_eq(&ok_body, &third.entries[0].body),
            "refresh keeps successfully extracted text"
        );
    }

    #[tokio::test]
    async fn read_match_returns_bounded_rebased_context() {
        let dir = tempfile::tempdir().unwrap();
        let a = art(dir.path(), "c1", "log.txt", 1, 1);
        let mut text = "x\n".repeat(3000);
        text.push_str("needle in the haystack\n");
        text.push_str(&"y\n".repeat(3000));
        std::fs::write(&a.path, &text).unwrap();
        let rows = Arc::new(Mutex::new(vec![a]));
        let cache = Arc::new(Cache::default());
        let index = ready(&cache, rows_fetch(rows)).await;

        let ctx = index.read_match("c1", "needle haystack").unwrap();
        assert_eq!(ctx.call_id, "c1");
        assert_eq!(ctx.r#match.field, "content");
        let chars = ctx.r#match.snippet.chars().count();
        assert!(
            chars > SNIPPET_CHARS && chars <= CONTEXT_CHARS + 2,
            "{chars}"
        );
        assert!(ctx.r#match.snippet.contains("needle"));
        let matched: String = ctx
            .r#match
            .ranges
            .iter()
            .flat_map(|r| {
                ctx.r#match
                    .snippet
                    .chars()
                    .skip(r.start as usize)
                    .take((r.end - r.start) as usize)
            })
            .collect();
        assert!(matched.contains('n') && matched.contains('k'), "{matched}");
        assert!(ctx.r#match.ranges.iter().all(|r| (r.end as usize) <= chars));

        let hit = index.search("needle haystack", 5);
        assert!(hit.hits[0].r#match.snippet.chars().count() <= SNIPPET_CHARS + 2);
        assert!(
            index
                .read_match("c1", "zzzqqq")
                .unwrap_err()
                .contains("nothing")
        );
    }

    #[tokio::test]
    async fn oversized_malformed_and_unrecognised_are_explicit() {
        let dir = tempfile::tempdir().unwrap();
        let mut big = art(dir.path(), "big", "deck.pptx", 1, 1);
        big.bytes = 30 * 1024 * 1024;
        let pdf = art(dir.path(), "pdf", "scan.pdf", 1, 2);
        std::fs::write(&pdf.path, b"%PDF-1.7\n%garbage").unwrap();
        let bin = art(dir.path(), "bin", "blob.dat", 1, 3);
        std::fs::write(&bin.path, b"\x00\x01\x02").unwrap();
        let fake = art(dir.path(), "fake", "report.docx", 1, 4);
        std::fs::write(&fake.path, b"this is not a zip package").unwrap();
        let rows = Arc::new(Mutex::new(vec![big, pdf, bin, fake]));
        let cache = Arc::new(Cache::default());
        let index = ready(&cache, rows_fetch(rows)).await;
        let page = index.search("deck", 10);
        assert!(matches!(page.coverage, Coverage::Partial));
        let msgs: Vec<(&str, &str)> = page
            .notices
            .iter()
            .map(|n| (n.code.as_str(), n.message.as_str()))
            .collect();
        assert!(
            msgs.iter()
                .any(|(c, m)| *c == "content_not_searchable" && m.contains("25 MB conversion cap")),
            "{msgs:?}"
        );
        assert!(
            msgs.iter().any(|(c, m)| *c == "content_not_searchable"
                && m.contains("malformed or unsupported Docx")),
            "a .docx without a document inside is a failure, not an empty body: {msgs:?}"
        );
        assert!(
            msgs.iter()
                .any(|(c, m)| *c == "metadata_only" && m.starts_with("unrecognised binary")),
            "{msgs:?}"
        );
        assert_eq!(
            page.notices.len(),
            4,
            "cap, pdf, malformed docx, binary: {msgs:?}"
        );
        assert_eq!(
            index.search("blob", 10).hits[0].content_coverage,
            ContentCoverage::MetadataOnly
        );
        assert_eq!(
            index.search("report", 10).hits[0].content_coverage,
            ContentCoverage::MetadataOnly
        );
    }
}
