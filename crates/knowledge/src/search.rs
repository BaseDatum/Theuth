//! Human search over current concepts and their complete history (design doc
//! 49 §3.3–3.4, §4.3), plus reading a concept's text at an exact commit.
//!
//! Separate from the model's FTS retrieval in [`store`](crate::store): the
//! FTS rows are read as stored data (ids, metadata, whole bodies) and matched
//! with the shared fuzzy matcher; `knowledge_search`/hints are untouched.
//!
//! Snapshots are lazy owner-local caches. Current concepts are keyed by an
//! index-mutation generation (bumped at every row upsert/remove and sync);
//! history is keyed by the captured full HEAD id, so an external ref move is
//! simply a new key. A cold key starts one build off the mailbox and answers
//! `indexing`; matching runs on a blocking thread under a one-permit bound.

use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ::search::{Coverage, FuzzyMatcher, SearchNotice, SearchPage, SearchText, TextMatch};
use anyhow::{Context as _, bail};
use gix::ObjectId;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use okf::Document;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use sqlx::sqlite::SqlitePool;
use tokio::sync::Semaphore;

use crate::KnowledgeActor;
use crate::repo::Repo;
use crate::store::{Knowledge, MetaRow, concept_id};
use crate::types::{ChangeStatus, Meta};

/// Characters of context around a matched span.
const SNIPPET_CHARS: usize = 160;

const BUSY: &str = "knowledge search is busy; retry";

/// One current concept that matched.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ConceptSearchHit {
    /// The concept's current frontmatter.
    pub meta: Meta,
    /// `field` is `metadata` (id, title, description, tags, type) or `body`.
    pub r#match: TextMatch,
}

/// Which candidate of a historical change matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum HistorySide {
    /// The complete text in the first parent.
    Before,
    /// The complete text in the change commit.
    After,
    /// Concept id, frontmatter and commit subject.
    Metadata,
}

/// One `(change commit, concept)` record that matched (49 §3.4).
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HistorySearchHit {
    /// Concept id.
    pub id: String,
    /// Title in the change commit, else in the parent, else the id.
    pub title: String,
    /// Full change commit id.
    pub commit: String,
    /// Full first-parent id; `None` for a root commit.
    pub parent: Option<String>,
    /// Commit time, RFC 3339 UTC.
    pub time: String,
    /// Commit subject line.
    pub message: String,
    /// How the commit touched the concept file.
    pub status: ChangeStatus,
    /// Which candidate matched.
    pub side: HistorySide,
    /// The commit whose tree holds the matched body (`commit` for After, the
    /// parent for Before); `None` for a metadata hit.
    pub snapshot_commit: Option<String>,
    /// `field` is `metadata` or `body`.
    pub r#match: TextMatch,
}

/// A concept's complete, immutable text at one commit.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HistoricalConcept {
    /// Concept id.
    pub id: String,
    /// Full commit id.
    pub commit: String,
    /// Commit time, RFC 3339 UTC.
    pub time: String,
    /// The file verbatim as committed.
    pub text: String,
}

/// `ask` → `Result<SearchPage<ConceptSearchHit>, String>`: current concepts (49 §3.3).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchConcepts {
    /// Fuzzy query (fzf syntax); blank → ready and empty.
    pub query: String,
    /// Hits per page; defaults and caps per the shared search limits.
    pub limit: Option<u32>,
}

/// `ask` → `Result<SearchPage<HistorySearchHit>, String>`: recorded changes (49 §3.4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchHistory {
    /// Fuzzy query (fzf syntax); blank → ready and empty.
    pub query: String,
    /// Hits per page; defaults and caps per the shared search limits.
    pub limit: Option<u32>,
}

/// `ask` → `Result<HistoricalConcept, String>`: read, never restore. `commit`
/// is a full object id or a unique prefix.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadRevision {
    /// Concept id (normalised like every other command).
    pub id: String,
    /// Commit object id, full or a unique prefix.
    pub commit: String,
}

/// `ask` → `Result<(), String>`: explicit refresh. `current` runs a sync
/// (external file edits, then a rebuild on the next query); `history` forgets
/// a failed build so the next query retries. A good snapshot is kept until a
/// fresh one is published.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct RefreshSearch {
    /// Sync current concepts from disk; the next query rebuilds.
    pub current: bool,
    /// Forget a failed history snapshot so the next query retries.
    pub history: bool,
}

impl Message<SearchConcepts> for KnowledgeActor {
    type Reply = DelegatedReply<Result<SearchPage<ConceptSearchHit>, String>>;

    async fn handle(
        &mut self,
        m: SearchConcepts,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let k = self.k.clone();
        // Off the mailbox: a cold build and the match run on blocking threads.
        tokio::spawn(async move { tx.send(Ok(k.search_concepts(&m.query, m.limit).await)) });
        reply
    }
}

impl Message<SearchHistory> for KnowledgeActor {
    type Reply = DelegatedReply<Result<SearchPage<HistorySearchHit>, String>>;

    async fn handle(
        &mut self,
        m: SearchHistory,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let k = self.k.clone();
        tokio::spawn(async move { tx.send(Ok(k.search_history(&m.query, m.limit).await)) });
        reply
    }
}

impl Message<ReadRevision> for KnowledgeActor {
    type Reply = DelegatedReply<Result<HistoricalConcept, String>>;

    async fn handle(
        &mut self,
        m: ReadRevision,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let k = self.k.clone();
        tokio::spawn(async move {
            tx.send(
                k.read_revision(&m.id, &m.commit)
                    .await
                    .map_err(|e| format!("{e:#}")),
            );
        });
        reply
    }
}

impl Message<RefreshSearch> for KnowledgeActor {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: RefreshSearch,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        if m.history {
            self.k.search.history.retry();
        }
        if m.current {
            self.k.sync().await.map_err(|e| format!("{e:#}"))?;
        }
        Ok(())
    }
}

/// Lazy snapshot of one owner: at most one build at a time, published only
/// when complete and still for the requested key; a failure stays visible
/// for that key instead of reading as zero matches.
struct Slot<K, T> {
    state: Mutex<State<K, T>>,
}

enum State<K, T> {
    Empty,
    Building(K),
    Failed(K, String),
    Ready(K, Arc<T>),
}

enum Lookup<T> {
    Ready(Arc<T>),
    Building,
    Failed(String),
    /// The caller now owns starting the build for this key.
    Start,
}

impl<K, T> Default for Slot<K, T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(State::Empty),
        }
    }
}

impl<K: PartialEq + Clone, T> Slot<K, T> {
    fn lookup(&self, key: &K) -> Lookup<T> {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        match &*s {
            State::Ready(k, index) if k == key => Lookup::Ready(index.clone()),
            State::Failed(k, e) if k == key => Lookup::Failed(e.clone()),
            // A build for an older key finishes first; its publication is
            // then superseded and the next lookup restarts for this key.
            State::Building(_) => Lookup::Building,
            _ => {
                *s = State::Building(key.clone());
                Lookup::Start
            }
        }
    }

    fn publish(&self, key: &K, built: Result<T, String>) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if !matches!(&*s, State::Building(k) if k == key) {
            return;
        }
        *s = match built {
            Ok(index) => State::Ready(key.clone(), Arc::new(index)),
            Err(e) => State::Failed(key.clone(), e),
        };
    }

    /// Forget a failure so the next lookup rebuilds (explicit refresh).
    fn retry(&self) {
        let mut s = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if matches!(&*s, State::Failed(..)) {
            *s = State::Empty;
        }
    }
}

/// The store's human-search state (49 §4.3).
pub(crate) struct Caches {
    generation: AtomicU64,
    current: Arc<Slot<u64, CurrentIndex>>,
    history: Arc<Slot<ObjectId, HistoryIndex>>,
    /// One running match per owner; excess work is refused, not queued.
    current_permit: Arc<Semaphore>,
    history_permit: Arc<Semaphore>,
}

impl Default for Caches {
    fn default() -> Self {
        Self {
            generation: AtomicU64::new(0),
            current: Arc::default(),
            history: Arc::default(),
            current_permit: Arc::new(Semaphore::new(1)),
            history_permit: Arc::new(Semaphore::new(1)),
        }
    }
}

impl Caches {
    /// The index rows or the stale set changed; the next current search
    /// rebuilds. A failed history build gets another try as well.
    pub(crate) fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        self.history.retry();
    }
}

impl Knowledge {
    /// Current concepts matching `query` (49 §3.3). Cold snapshot → `indexing`.
    pub async fn search_concepts(
        &self,
        query: &str,
        limit: Option<u32>,
    ) -> SearchPage<ConceptSearchHit> {
        let limit = ::search::limit(limit);
        if let Err(e) = ::search::validate_query(query) {
            return SearchPage::error(e);
        }
        let query = query.trim();
        if query.is_empty() {
            return SearchPage::ready(Vec::new(), limit);
        }
        let generation = self.search.generation.load(Ordering::Acquire);
        let index = match self.search.current.lookup(&generation) {
            Lookup::Ready(index) => index,
            Lookup::Building => return SearchPage::indexing(),
            Lookup::Failed(e) => return SearchPage::error(e),
            Lookup::Start => {
                let slot = self.search.current.clone();
                let db = self.db.clone();
                let stale = self.stale.lock().unwrap_or_else(|p| p.into_inner()).clone();
                tokio::spawn(async move {
                    let built = CurrentIndex::build(&db, stale).await;
                    slot.publish(&generation, built.map_err(|e| format!("{e:#}")));
                });
                return SearchPage::indexing();
            }
        };
        let permit = self.search.current_permit.clone();
        let query = query.to_owned();
        let matched = tokio::task::spawn_blocking(move || {
            let Ok(_p) = permit.try_acquire() else {
                return SearchPage::error(BUSY);
            };
            index.search(&query, limit)
        })
        .await;
        matched.unwrap_or_else(|e| SearchPage::error(e.to_string()))
    }

    /// Recorded concept changes matching `query` (49 §3.4): every commit
    /// reachable from the current HEAD, full before/after text.
    pub async fn search_history(
        &self,
        query: &str,
        limit: Option<u32>,
    ) -> SearchPage<HistorySearchHit> {
        let limit = ::search::limit(limit);
        if let Err(e) = ::search::validate_query(query) {
            return SearchPage::error(e);
        }
        let query = query.trim();
        if query.is_empty() {
            return SearchPage::ready(Vec::new(), limit);
        }
        let Some(repo) = self.repo.clone() else {
            return SearchPage::unavailable("history unavailable: no repository");
        };
        // The captured full HEAD is the key: an external ref move (no
        // `KnowledgeChanged`) is a different key and rebuilds from scratch.
        let head = {
            let repo = repo.clone();
            match tokio::task::spawn_blocking(move || repo.head_id()).await {
                Ok(Ok(Some(head))) => head,
                // Unborn: nothing has been recorded yet, and that is complete.
                Ok(Ok(None)) => return SearchPage::ready(Vec::new(), limit),
                Ok(Err(e)) => return SearchPage::error(format!("{e:#}")),
                Err(e) => return SearchPage::error(e.to_string()),
            }
        };
        let index = match self.search.history.lookup(&head) {
            Lookup::Ready(index) => index,
            Lookup::Building => return SearchPage::indexing(),
            Lookup::Failed(e) => return SearchPage::error(e),
            Lookup::Start => {
                let slot = self.search.history.clone();
                tokio::spawn(async move {
                    let built = tokio::task::spawn_blocking(move || {
                        HistoryIndex::build(&repo, head).map_err(|e| format!("{e:#}"))
                    })
                    .await
                    .unwrap_or_else(|e| Err(e.to_string()));
                    slot.publish(&head, built);
                });
                return SearchPage::indexing();
            }
        };
        let permit = self.search.history_permit.clone();
        let query = query.to_owned();
        let matched = tokio::task::spawn_blocking(move || {
            let Ok(_p) = permit.try_acquire() else {
                return SearchPage::error(BUSY);
            };
            index.search(&query, limit)
        })
        .await;
        matched.unwrap_or_else(|e| SearchPage::error(e.to_string()))
    }

    /// The complete text of `id` in the commit `commit` (full id or a unique
    /// prefix). Read-only: nothing is written, committed or restored. Absent → error.
    pub async fn read_revision(&self, id: &str, commit: &str) -> anyhow::Result<HistoricalConcept> {
        let Some(repo) = self.repo.clone() else {
            bail!("history unavailable: no repository")
        };
        let cid = concept_id(id)?;
        let spec = commit.trim();
        if spec.len() < 4 || !spec.bytes().all(|b| b.is_ascii_hexdigit()) {
            bail!("commit must be a hex object id (≥ 4 chars), got {commit:?}");
        }
        let id = cid.to_string();
        let rel = cid.to_path(Path::new(""));
        let (oid, seconds, text) = {
            let id = id.clone();
            let spec = spec.to_owned();
            tokio::task::spawn_blocking(move || -> anyhow::Result<(ObjectId, i64, String)> {
                let (oid, seconds) = repo.commit_at(&spec)?;
                let bytes = repo
                    .read_at(&rel, oid)?
                    .with_context(|| format!("{id} is not in commit {oid}"))?;
                Ok((
                    oid,
                    seconds,
                    String::from_utf8(bytes).context("concept is not UTF-8")?,
                ))
            })
            .await??
        };
        let time = chrono::DateTime::from_timestamp(seconds, 0)
            .map(|t| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
            .unwrap_or_default();
        Ok(HistoricalConcept {
            id,
            commit: oid.to_string(),
            time,
            text,
        })
    }
}

/// `id`, title, description, tags and type on fixed lines: the metadata candidate.
fn meta_text(id: &str, title: &str, description: &str, tags: &[String], type_: &str) -> String {
    format!("{id}\n{title}\n{description}\n{}\n{type_}", tags.join(" "))
}

fn partial<T>(page: &mut SearchPage<T>, affected: u32, code: &str, message: String) {
    if affected > 0 {
        page.coverage = Coverage::Partial;
        page.notices.push(SearchNotice {
            code: code.to_owned(),
            message,
            affected: Some(affected),
        });
    }
}

/// An FTS row as stored, plus its file path for the stale check.
#[derive(FromRow)]
struct CurrentRow {
    #[sqlx(flatten)]
    meta: MetaRow,
    body: String,
    path: String,
}

struct Entry {
    meta: Meta,
    text: SearchText,
    body: SearchText,
}

/// Every valid current concept at one index generation.
pub(crate) struct CurrentIndex {
    entries: Vec<Entry>,
    /// Files the last sync could not read or parse: rows excluded or never indexed.
    stale: u32,
}

impl CurrentIndex {
    async fn build(db: &SqlitePool, stale: HashMap<String, String>) -> anyhow::Result<Self> {
        // Only `META_COLS` (a const) is interpolated.
        let rows: Vec<CurrentRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
            "SELECT {}, body, path FROM concepts",
            crate::store::META_COLS
        )))
        .fetch_all(db)
        .await?;
        Ok(tokio::task::spawn_blocking(move || {
            let entries = rows
                .into_iter()
                .filter(|r| !stale.contains_key(&r.path))
                .map(|r| {
                    let meta: Meta = r.meta.into();
                    Entry {
                        text: SearchText::new(meta_text(
                            &meta.id,
                            &meta.title,
                            &meta.description,
                            &meta.tags,
                            &meta.type_,
                        )),
                        body: SearchText::new(r.body),
                        meta,
                    }
                })
                .collect();
            Self {
                entries,
                stale: stale.len() as u32,
            }
        })
        .await?)
    }

    /// Best candidate per concept; score desc, timestamp desc, id asc; at
    /// most `limit + 1` scored rows get snippets (the lookahead sets `has_more`).
    fn search(&self, query: &str, limit: u32) -> SearchPage<ConceptSearchHit> {
        let mut m = FuzzyMatcher::new(query);
        let mut scored: Vec<(u32, bool, &Entry)> = Vec::new();
        for e in &self.entries {
            // Ties go to metadata: an id/title hit is the more precise label.
            let (score, is_body) = match (m.score(&e.text), m.score(&e.body)) {
                (None, None) => continue,
                (Some(a), Some(b)) if b > a => (b, true),
                (Some(a), _) => (a, false),
                (None, Some(b)) => (b, true),
            };
            scored.push((score, is_body, e));
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.2.meta.timestamp.cmp(&a.2.meta.timestamp))
                .then_with(|| a.2.meta.id.cmp(&b.2.meta.id))
        });
        scored.truncate(limit as usize + 1);
        let hits = scored
            .into_iter()
            .filter_map(|(_, is_body, e)| {
                let (text, field) = if is_body {
                    (&e.body, "body")
                } else {
                    (&e.text, "metadata")
                };
                let (_, r#match) = m.matched(text, field, SNIPPET_CHARS)?;
                Some(ConceptSearchHit {
                    meta: e.meta.clone(),
                    r#match,
                })
            })
            .collect();
        let mut page = SearchPage::ready(hits, limit);
        partial(
            &mut page,
            self.stale,
            "stale_files",
            format!(
                "{} concept file(s) could not be read or parsed and are not searched; fix them and refresh",
                self.stale
            ),
        );
        page
    }
}

/// One historical blob, decoded and parsed once, shared by object id.
struct Blob {
    title: Option<String>,
    /// Frontmatter title, description, tags, type (no id: it belongs to the record).
    meta: String,
    body: SearchText,
}

impl Blob {
    /// `None` when the text is not a valid concept file (an opened but
    /// unterminated or non-mapping frontmatter), the same rule the current
    /// index applies; a file without frontmatter is a valid minimal concept.
    fn parse(text: &str) -> Option<Self> {
        let doc = Document::parse(text).ok()?;
        let fm = &doc.frontmatter;
        let title = fm.title().filter(|t| !t.is_empty());
        let meta = format!(
            "{}\n{}\n{}\n{}",
            title.as_deref().unwrap_or_default(),
            fm.description().unwrap_or_default(),
            fm.tags().join(" "),
            fm.type_().unwrap_or_default()
        );
        Some(Self {
            title,
            meta,
            body: SearchText::new(doc.body),
        })
    }
}

/// One `(change commit, concept)` record with raw provenance.
struct Record {
    id: String,
    title: String,
    commit: ObjectId,
    parent: Option<ObjectId>,
    seconds: i64,
    message: String,
    status: ChangeStatus,
    before: Option<Arc<Blob>>,
    after: Option<Arc<Blob>>,
    /// id, the after (else before) frontmatter, and the commit subject.
    meta: SearchText,
}

/// Every concept change reachable from one captured HEAD.
pub(crate) struct HistoryIndex {
    records: Vec<Record>,
    /// Blobs that were missing, corrupt, not UTF-8 or not a parseable
    /// concept file: their text is absent and coverage is partial.
    unreadable: u32,
}

impl HistoryIndex {
    fn build(repo: &Repo, head: ObjectId) -> anyhow::Result<Self> {
        let mut blobs: HashMap<ObjectId, Option<Arc<Blob>>> = HashMap::new();
        let mut unreadable = 0u32;
        let mut load = |id: Option<ObjectId>| -> Option<Arc<Blob>> {
            let id = id?;
            blobs
                .entry(id)
                .or_insert_with(|| {
                    match repo
                        .blob(id)
                        .ok()
                        .and_then(|b| String::from_utf8(b).ok())
                        .and_then(|t| Blob::parse(&t))
                    {
                        Some(blob) => Some(Arc::new(blob)),
                        None => {
                            unreadable += 1;
                            None
                        }
                    }
                })
                .clone()
        };
        let mut records = Vec::new();
        for c in repo.changes_from(head)? {
            for f in c.files {
                let before = load(f.before);
                let after = load(f.after);
                let id = f.id.to_string();
                let primary = after.as_ref().or(before.as_ref());
                let title = primary
                    .and_then(|b| b.title.clone())
                    .unwrap_or_else(|| id.clone());
                let meta = SearchText::new(format!(
                    "{id}\n{}\n{}",
                    primary.map_or("", |b| b.meta.as_str()),
                    c.message
                ));
                records.push(Record {
                    id,
                    title,
                    commit: c.commit,
                    parent: c.parent,
                    seconds: c.seconds,
                    message: c.message.clone(),
                    status: f.status,
                    before,
                    after,
                    meta,
                });
            }
        }
        Ok(Self {
            records,
            unreadable,
        })
    }

    /// Best candidate per record (ties: metadata, then After, then Before);
    /// score desc, time desc, commit asc, id asc; lookahead of one.
    fn search(&self, query: &str, limit: u32) -> SearchPage<HistorySearchHit> {
        let mut m = FuzzyMatcher::new(query);
        let mut scored: Vec<(u32, HistorySide, &Record)> = Vec::new();
        for r in &self.records {
            let mut best: Option<(u32, HistorySide)> = None;
            let mut consider = |score: Option<u32>, side: HistorySide| {
                if let Some(s) = score
                    && best.is_none_or(|(b, _)| s > b)
                {
                    best = Some((s, side));
                }
            };
            consider(m.score(&r.meta), HistorySide::Metadata);
            if let Some(a) = &r.after {
                consider(m.score(&a.body), HistorySide::After);
            }
            if let Some(b) = &r.before {
                consider(m.score(&b.body), HistorySide::Before);
            }
            if let Some((score, side)) = best {
                scored.push((score, side, r));
            }
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.2.seconds.cmp(&a.2.seconds))
                .then_with(|| a.2.commit.cmp(&b.2.commit))
                .then_with(|| a.2.id.cmp(&b.2.id))
        });
        scored.truncate(limit as usize + 1);
        let hits = scored
            .into_iter()
            .filter_map(|(_, side, r)| {
                let (text, field, snapshot) = match side {
                    HistorySide::Metadata => (&r.meta, "metadata", None),
                    HistorySide::After => (&r.after.as_ref()?.body, "body", Some(r.commit)),
                    HistorySide::Before => (&r.before.as_ref()?.body, "body", r.parent),
                };
                let (_, r#match) = m.matched(text, field, SNIPPET_CHARS)?;
                Some(HistorySearchHit {
                    id: r.id.clone(),
                    title: r.title.clone(),
                    commit: r.commit.to_string(),
                    parent: r.parent.map(|p| p.to_string()),
                    time: chrono::DateTime::from_timestamp(r.seconds, 0).map_or_else(
                        || r.seconds.to_string(),
                        |t| t.format("%Y-%m-%dT%H:%M:%SZ").to_string(),
                    ),
                    message: r.message.clone(),
                    status: r.status,
                    side,
                    snapshot_commit: snapshot.map(|c| c.to_string()),
                    r#match,
                })
            })
            .collect();
        let mut page = SearchPage::ready(hits, limit);
        partial(
            &mut page,
            self.unreadable,
            "unreadable_blobs",
            format!(
                "{} historical concept file(s) could not be read or parsed; their text is not searched",
                self.unreadable
            ),
        );
        page
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::time::Duration;

    use ::search::PageState;
    use runtime::Runtime;

    use super::*;
    use crate::types::{Put, Source};

    async fn open() -> Arc<Knowledge> {
        let dir = std::env::temp_dir().join(format!("theuth-ksearch-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        Knowledge::open(&dir, rt).await.unwrap()
    }

    fn put(id: &str, title: &str, body: &str) -> Put {
        Put {
            id: id.into(),
            type_: "fact".into(),
            title: title.into(),
            description: None,
            tags: vec!["rust".into()],
            scope: None,
            body: body.into(),
            visibility: None,
        }
    }

    /// A cold key answers `indexing` first; poll the way a client would.
    async fn current(k: &Knowledge, q: &str, limit: Option<u32>) -> SearchPage<ConceptSearchHit> {
        for _ in 0..500 {
            let page = k.search_concepts(q, limit).await;
            if !matches!(page.state, PageState::Indexing) {
                return page;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("current index never became ready")
    }

    async fn history(k: &Knowledge, q: &str, limit: Option<u32>) -> SearchPage<HistorySearchHit> {
        for _ in 0..500 {
            let page = k.search_history(q, limit).await;
            if !matches!(page.state, PageState::Indexing) {
                return page;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("history index never became ready")
    }

    fn ids(page: &SearchPage<ConceptSearchHit>) -> Vec<&str> {
        page.hits.iter().map(|h| h.meta.id.as_str()).collect()
    }

    #[tokio::test]
    async fn current_search_reads_ids_and_whole_bodies_and_excludes_stale_rows() {
        let k = open().await;
        let filler = "unrelated line\n".repeat(40);
        k.put(
            put(
                "facts/zeta-deploy",
                "Zeta rollout",
                &format!("{filler}quorum handshake at the end\n"),
            ),
            Source::Model,
        )
        .await
        .unwrap();
        k.put(put("ab/xy", "Tiny", "nothing"), Source::Model)
            .await
            .unwrap();

        // Cold: indexing, then ready without any FTS MATCH involvement.
        assert!(matches!(
            k.search_concepts("xy", None).await.state,
            PageState::Indexing
        ));
        // A two-character id fragment: below the FTS term floor, found here.
        let page = current(&k, "xy", None).await;
        assert!(matches!(page.state, PageState::Ready));
        assert_eq!(ids(&page), ["ab/xy"]);
        assert_eq!(page.hits[0].r#match.field, "metadata");
        assert!(matches!(page.coverage, Coverage::Complete));
        assert!(!page.has_more);
        // The FTS path is unchanged: `xy` is too short to be a term there.
        let cwd = std::env::current_dir().unwrap();
        assert!(k.search("xy", &cwd, 10).await.unwrap().is_empty());
        assert_eq!(
            k.search("zeta", &cwd, 10).await.unwrap()[0].meta.id,
            "facts/zeta-deploy"
        );

        // A phrase deep in the body, with a snippet around it.
        let page = current(&k, "quorum handshake", None).await;
        assert_eq!(ids(&page), ["facts/zeta-deploy"]);
        let m = &page.hits[0].r#match;
        assert_eq!(m.field, "body");
        assert!(m.snippet.contains("quorum"), "{}", m.snippet);
        assert!(!m.ranges.is_empty());

        // Lookahead: three matches, limit two → has_more.
        for n in ["one", "two", "three"] {
            k.put(
                put(&format!("common/{n}"), "Common", "shared pebble"),
                Source::Model,
            )
            .await
            .unwrap();
        }
        let page = current(&k, "pebble", Some(2)).await;
        assert_eq!(page.hits.len(), 2);
        assert!(page.has_more);
        let page = current(&k, "pebble", Some(3)).await;
        assert_eq!(page.hits.len(), 3);
        assert!(!page.has_more);

        // The file breaks on disk: sync keeps the old row but human search
        // excludes it and says so, until the file parses again.
        let file = k.root().join("facts/zeta-deploy.md");
        std::fs::write(&file, "---\ntype: fact\nnever terminated\n").unwrap();
        k.sync().await.unwrap();
        let page = current(&k, "quorum", None).await;
        assert!(page.hits.is_empty(), "{:?}", ids(&page));
        assert!(matches!(page.coverage, Coverage::Partial));
        assert_eq!(page.notices.len(), 1);
        assert_eq!(page.notices[0].code, "stale_files");
        assert_eq!(page.notices[0].affected, Some(1));
        // The row itself is still there for the model's FTS retrieval.
        assert_eq!(k.search("zeta", &cwd, 10).await.unwrap().len(), 1);

        std::fs::write(
            &file,
            "---\ntype: fact\ntitle: Zeta rollout\n---\nquorum restored\n",
        )
        .unwrap();
        k.sync().await.unwrap();
        let page = current(&k, "quorum", None).await;
        assert_eq!(ids(&page), ["facts/zeta-deploy"]);
        assert!(matches!(page.coverage, Coverage::Complete));

        // Blank query: ready and empty without touching the cache.
        let page = k.search_concepts("   ", None).await;
        assert!(matches!(page.state, PageState::Ready) && page.hits.is_empty());
    }

    #[tokio::test]
    async fn get_backfill_invalidates_current_snapshot() {
        let k = open().await;
        k.put(put("facts/a", "A", "alpha"), Source::Model)
            .await
            .unwrap();
        assert_eq!(ids(&current(&k, "alpha", None).await), ["facts/a"]);
        // An external file `sync` has not seen: `get` indexes it, and the
        // snapshot must follow that mutation without a sync or event.
        let file = k.root().join("facts/b.md");
        std::fs::write(&file, "---\ntype: fact\ntitle: B\n---\nbravo\n").unwrap();
        k.get("facts/b").await.unwrap();
        assert_eq!(ids(&current(&k, "bravo", None).await), ["facts/b"]);
    }

    #[tokio::test]
    async fn history_covers_full_text_root_deletion_merge_and_head_rewind() {
        let k = open().await;
        let repo = k.repo.clone().unwrap();
        let far = "filler line\n".repeat(12);
        k.put(
            put(
                "facts/a",
                "Kumquat notes",
                &format!("first line\n{far}far phrase pelican\n"),
            ),
            Source::Model,
        )
        .await
        .unwrap();
        // Change only the first line: the diff hunk (3 context lines) never
        // shows the pelican line, but the complete After/Before text does.
        k.put(
            put(
                "facts/a",
                "Kumquat notes",
                &format!("FIRST line edited\n{far}far phrase pelican\n"),
            ),
            Source::Model,
        )
        .await
        .unwrap();
        let changes = k.history(None, 10).await.unwrap().changes;
        assert_eq!(changes.len(), 2);
        let (edit, root) = (&changes[0].commit, &changes[1].commit);

        let page = history(&k, "far phrase pelican", None).await;
        assert!(matches!(page.state, PageState::Ready), "{:?}", page.state);
        assert!(matches!(page.coverage, Coverage::Complete));
        assert_eq!(page.hits.len(), 2, "{page:?}");
        for h in &page.hits {
            assert_eq!(h.id, "facts/a");
            assert_eq!(h.title, "Kumquat notes");
            assert_eq!(h.commit.len(), 40);
            assert!(h.time.ends_with('Z'), "{}", h.time);
            assert_eq!(h.r#match.field, "body");
            assert!(
                h.r#match.snippet.contains("pelican"),
                "{}",
                h.r#match.snippet
            );
        }
        let edited = page
            .hits
            .iter()
            .find(|h| h.commit.starts_with(edit))
            .unwrap();
        assert_eq!(edited.status, ChangeStatus::Modified);
        // Equal Before/After scores prefer After; the snapshot is the change commit.
        assert_eq!(edited.side, HistorySide::After);
        assert_eq!(
            edited.snapshot_commit.as_deref(),
            Some(edited.commit.as_str())
        );
        let parent = edited.parent.as_deref().unwrap();
        assert!(parent.len() == 40 && parent.starts_with(root), "{parent}");
        let rooted = page
            .hits
            .iter()
            .find(|h| h.commit.starts_with(root))
            .unwrap();
        assert_eq!(rooted.status, ChangeStatus::Added);
        assert_eq!(rooted.parent, None);
        assert_eq!(rooted.side, HistorySide::After);

        // Metadata hit: title and subject, no snapshot side.
        let page = history(&k, "kumquat", None).await;
        assert!(!page.hits.is_empty());
        assert!(page.hits.iter().all(|h| h.side == HistorySide::Metadata
            && h.snapshot_commit.is_none()
            && h.r#match.field == "metadata"));

        // Deletion: Before points at the first parent; the text reads there
        // and is absent at the deletion commit, and reading restores nothing.
        k.delete("facts/a").await.unwrap();
        let page = history(&k, "far phrase pelican", None).await;
        let deleted = page
            .hits
            .iter()
            .find(|h| h.status == ChangeStatus::Deleted)
            .expect("deleted record");
        assert_eq!(deleted.side, HistorySide::Before);
        assert_eq!(
            deleted.snapshot_commit.as_deref(),
            deleted.parent.as_deref()
        );
        assert_eq!(
            deleted.snapshot_commit.as_deref(),
            Some(edited.commit.as_str())
        );
        let before = k
            .read_revision("facts/a", deleted.snapshot_commit.as_deref().unwrap())
            .await
            .unwrap();
        assert!(before.text.contains("FIRST line edited") && before.text.contains("pelican"));
        assert_eq!(before.commit, edited.commit);
        assert!(k.read_revision("facts/a", &deleted.commit).await.is_err());
        assert!(
            k.get("facts/a").await.is_err(),
            "read-at-commit must not restore"
        );
        let n = k.history(None, 10).await.unwrap().changes.len();
        assert_eq!(n, 3, "read-at-commit must not commit");

        // A side branch is not searchable until merged; after the merge the
        // walk covers its ancestry although the merge itself changes nothing
        // against its first parent.
        let head_before_merge = repo.head_id().unwrap().unwrap();
        let root_id = ObjectId::from_hex(rooted.commit.as_bytes()).unwrap();
        std::fs::create_dir_all(k.root().join("facts")).unwrap();
        std::fs::write(
            k.root().join("facts/side.md"),
            "---\ntype: fact\ntitle: Side\n---\nwalrus on a branch\n",
        )
        .unwrap();
        let side = repo
            .commit_on(
                "refs/heads/side",
                &[root_id],
                &[PathBuf::from("facts/side.md")],
                "side add",
            )
            .unwrap();
        std::fs::remove_file(k.root().join("facts/side.md")).unwrap();
        assert!(
            history(&k, "walrus", None).await.hits.is_empty(),
            "unmerged tip excluded"
        );
        let merge = repo
            .commit_on("HEAD", &[head_before_merge, side], &[], "merge side")
            .unwrap();
        let page = history(&k, "walrus", None).await;
        assert_eq!(page.hits.len(), 1, "{page:?}");
        assert_eq!(page.hits[0].commit, side.to_string());
        assert_eq!(page.hits[0].id, "facts/side");
        assert_eq!(page.hits[0].status, ChangeStatus::Added);
        assert!(history(&k, "far phrase pelican", None).await.hits.len() >= 3);
        assert!(
            !history(&k, "merge side", None)
                .await
                .hits
                .iter()
                .any(|h| h.commit == merge),
            "a merge with no first-parent change has no record"
        );

        // External rewind with no KnowledgeChanged: the next query sees the
        // new reachable set only.
        repo.rewind(head_before_merge).unwrap();
        assert!(history(&k, "walrus", None).await.hits.is_empty());
        assert_eq!(history(&k, "far phrase pelican", None).await.hits.len(), 3);

        // A malformed concept file in history is not silently complete: its
        // record stays findable by id/subject, its text is not searched and
        // coverage says so. A frontmatter-less file is a valid minimal concept.
        std::fs::write(
            k.root().join("facts/broken.md"),
            "---\ntype: fact\nnever terminated ibis\n",
        )
        .unwrap();
        std::fs::write(k.root().join("facts/minimal.md"), "plain minimal ostrich\n").unwrap();
        repo.commit_on(
            "HEAD",
            &[head_before_merge],
            &[
                PathBuf::from("facts/broken.md"),
                PathBuf::from("facts/minimal.md"),
            ],
            "external add",
        )
        .unwrap();
        std::fs::remove_file(k.root().join("facts/broken.md")).unwrap();
        std::fs::remove_file(k.root().join("facts/minimal.md")).unwrap();
        let page = history(&k, "ibis", None).await;
        assert!(page.hits.is_empty(), "{page:?}");
        assert!(matches!(page.coverage, Coverage::Partial));
        assert_eq!(page.notices.len(), 1);
        assert_eq!(page.notices[0].code, "unreadable_blobs");
        assert_eq!(page.notices[0].affected, Some(1));
        let page = history(&k, "facts/broken", None).await;
        assert_eq!(page.hits.len(), 1, "{page:?}");
        assert_eq!(page.hits[0].side, HistorySide::Metadata);
        let page = history(&k, "ostrich", None).await;
        assert_eq!(page.hits.len(), 1, "{page:?}");
        assert_eq!(page.hits[0].id, "facts/minimal");
        assert_eq!(page.hits[0].side, HistorySide::After);
    }

    #[tokio::test]
    async fn read_revision_accepts_prefix_and_rejects_unknown_or_invalid() {
        let k = open().await;
        k.put(put("facts/a", "A", "alpha"), Source::Model)
            .await
            .unwrap();
        let full = k
            .repo
            .as_ref()
            .unwrap()
            .head_id()
            .unwrap()
            .unwrap()
            .to_string();
        let short = k.read_revision("facts/a", &full[..7]).await.unwrap();
        assert_eq!(short.commit, full, "prefix resolves to the full id");
        assert!(!short.time.is_empty(), "commit time reported");
        assert!(k.read_revision("facts/a", "zz").await.is_err(), "not hex");
        assert!(
            k.read_revision("facts/a", &"0".repeat(40)).await.is_err(),
            "unknown"
        );
        assert!(
            k.read_revision("index", &full).await.is_err(),
            "reserved id"
        );
        assert!(
            k.read_revision("facts/nope", &full).await.is_err(),
            "absent path"
        );
        let c = k.read_revision("Facts/A.md", &full).await.unwrap();
        assert_eq!(
            (c.id.as_str(), c.commit.as_str()),
            ("facts/a", full.as_str())
        );
        assert!(c.text.contains("alpha"));
    }
}
