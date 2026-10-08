// 144-W2 owns this file.
//! The block index (design 144 §7): `nb_blocks` (FTS5) + `nb_vec` (vec0) + `nb_index_state` in
//! `documents/index.sqlite`, fed by `DocumentChanged`, searched by `nb_search` and by the `notes`
//! / `sources` / `notebooks` categories of `search()`. Blocks come from the local mirror through
//! the actor's `Outline` + annotated `Markdown` (any engine); vectors from the `embed` actor
//! (`decision::embed`), lexical-only when no checkpoint is installed.

mod chunk;
mod db;

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;
use std::time::{SystemTime, UNIX_EPOCH};

use decision::embed::{EMBED_ACTOR, Embed, EmbedActor, EmbedInfo, Info, Tokens};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use search::{MatchRange, SearchPage, TextMatch};
use serde::{Deserialize, Serialize};
use surface::anchor::{Locator, TheuthAnchor, TheuthKind};

use crate::DOCUMENT_ACTOR;
use crate::actor::{DocumentActor, Markdown, Outline};
use crate::meta::{DocId, DocKind, DocumentChanged};
use crate::notebook::ItemKind;
use crate::store::{Store, sha256};
use chunk::Block;
use db::{BlockRow, Index, Lexical, State, VecRow};

/// Kinds the index walks (§7.1).
pub const INDEXABLE: [DocKind; 5] = [
    DocKind::Note,
    DocKind::Board,
    DocKind::MeetingNote,
    DocKind::Source,
    DocKind::Notebook,
];

/// `nb_vec` dims when no model is installed (gte-modernbert-base's).
const DEFAULT_DIMS: usize = 768;
/// Reciprocal-rank constant (§7.4).
const RRF_K: f32 = 60.0;
/// Snippet of a vector-only hit (§7.4).
const SNIPPET_CHARS: usize = 160;
/// `store.list` cap per kind on a rebuild.
const WALK_LIMIT: u32 = 10_000;
/// Texts per `Embed` message (the actor's forward batch, §7.2), so a query waits ≤ one batch.
const EMBED_BATCH: usize = 32;

/// The `notebooks:` keys this module reads (§14); other workers read theirs from the same
/// section — unknown keys are ignored on every side.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexConfig {
    /// A checkpoint directory name under `<data_dir>/models/` (D9).
    pub embedding_model: String,
    /// Chunk budget in model tokens (§7.3).
    pub chunk_tokens: usize,
    /// Overlap as a share of `chunk_tokens`, in whole tail blocks.
    pub chunk_overlap: f32,
    /// Chunks under this many tokens are dropped.
    pub chunk_min_tokens: usize,
}

impl Default for IndexConfig {
    fn default() -> Self {
        Self {
            embedding_model: decision::embed::DEFAULT_MODEL.to_owned(),
            chunk_tokens: 400,
            chunk_overlap: 0.15,
            chunk_min_tokens: 5,
        }
    }
}

impl IndexConfig {
    fn params(&self) -> chunk::Params {
        chunk::Params {
            max_tokens: self.chunk_tokens.max(1),
            overlap: self.chunk_overlap.clamp(0.0, 0.9),
            min_tokens: self.chunk_min_tokens,
        }
    }
}

/// The §13.2 `nb_search` arguments.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NbSearchArgs {
    /// The query (lexical and embedded alike).
    pub query: String,
    /// Restrict to these notebooks' items; default every indexed document.
    #[serde(default)]
    pub notebooks: Option<Vec<DocId>>,
    /// Restrict to these documents.
    #[serde(default)]
    pub docs: Option<Vec<DocId>>,
    /// Restrict to block kinds: `note | board | meeting_note | source | insight | notebook`.
    #[serde(default)]
    pub kinds: Option<Vec<String>>,
    /// Hits; default 10.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// One block-level hit (§7.4): what `nb_search` returns and what the `sources` / `notebooks` search
/// categories page.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct BlockHit {
    /// The document.
    pub doc: DocId,
    /// Its kind.
    pub kind: DocKind,
    /// Its title.
    pub title: String,
    /// The block id (an outline id; opens the editor at the block). Empty when the title matched.
    pub block: String,
    /// The nearest heading above the block.
    pub heading: Option<String>,
    /// Fused rank score (reciprocal rank, higher is better).
    pub score: f32,
    /// Snippet with highlight ranges; `field` is `title`, `heading` or `text`.
    pub r#match: TextMatch,
    /// The citation href for this block: `theuth://notes/<id>#<block>?s=<sha8>` or `sources/…`.
    pub href: String,
    /// Live notebooks holding the document.
    pub notebooks: Vec<DocId>,
}

// ---------------------------------------------------------------- entry points

/// Hybrid retrieval (§7.4): FTS5 bm25 ∪ vec0 cosine, reciprocal-rank fused. Any engine.
pub async fn nb_search(
    actor: &ActorRef<DocumentActor>,
    args: NbSearchArgs,
) -> anyhow::Result<Vec<BlockHit>> {
    let limit = search::limit(args.limit) as usize;
    let mut hits = actor
        .ask(NbSearch(args))
        .await
        .map_err(anyhow::Error::msg)?;
    hits.truncate(limit);
    Ok(hits)
}

/// Walk `docs` where `kind ∈ (note, board, meeting_note, source, notebook)` and re-index every stale
/// document (§7.1 first run, §7.5 model change).
pub async fn reindex(actor: &ActorRef<DocumentActor>) -> anyhow::Result<()> {
    actor.ask(Reindex).await.map_err(anyhow::Error::msg)
}

/// The indexer's trigger (§7.1): called by the document actor for every `DocumentChanged` it publishes;
/// re-indexes the changed blocks of `ev.doc`, drops the rows of a deleted document.
pub fn on_document_changed(ev: &DocumentChanged) {
    let Ok(Some(actor)) = ActorRef::<DocumentActor>::lookup(DOCUMENT_ACTOR) else {
        return;
    };
    let ev = ev.clone();
    tokio::spawn(async move {
        let _ = actor.tell(Changed(ev)).await;
    });
}

// ---------------------------------------------------------------- actor messages

/// `tell`: index (or drop) one changed document.
#[derive(Clone, Debug)]
pub struct Changed(pub DocumentChanged);

impl Message<Changed> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, Changed(ev): Changed, ctx: &mut Context<Self, Self::Reply>) {
        let ix = self.indexer(ctx.actor_ref().clone());
        tokio::spawn(async move {
            let r = if ev.deleted {
                ix.remove(&ev.doc).await
            } else {
                ix.index_doc(&ev.doc).await
            };
            if let Err(e) = r {
                tracing::warn!("index: {}: {e:#}", ev.doc);
            }
        });
    }
}

/// The bus route: the document actor subscribes to its own `DocumentChanged` (`lib.rs`), so every
/// publish site in `actor.rs`/`notebook.rs` reaches the index without a call beside it.
impl Message<DocumentChanged> for DocumentActor {
    type Reply = ();

    async fn handle(&mut self, ev: DocumentChanged, ctx: &mut Context<Self, Self::Reply>) {
        Message::<Changed>::handle(self, Changed(ev), ctx).await
    }
}

/// `ask` → `Result<Vec<BlockHit>, String>`: the hybrid search, `limit + 1` hits for paging.
#[derive(Clone, Debug)]
pub struct NbSearch(pub NbSearchArgs);

impl Message<NbSearch> for DocumentActor {
    type Reply = DelegatedReply<Result<Vec<BlockHit>, String>>;

    async fn handle(
        &mut self,
        NbSearch(args): NbSearch,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let ix = self.indexer(ctx.actor_ref().clone());
        tokio::spawn(async move { tx.send(ix.search(args).await.map_err(|e| format!("{e:#}"))) });
        reply
    }
}

/// `ask` → `Result<(), String>`: the §7.1 first run / §7.5 rebuild.
#[derive(Clone, Copy, Debug)]
pub struct Reindex;

impl Message<Reindex> for DocumentActor {
    type Reply = DelegatedReply<Result<(), String>>;

    async fn handle(&mut self, _: Reindex, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let ix = self.indexer(ctx.actor_ref().clone());
        tokio::spawn(async move { tx.send(ix.reindex().await.map_err(|e| format!("{e:#}"))) });
        reply
    }
}

/// `ask` → `Result<SearchPage<BlockHit>, String>`: the `sources` category of `search()` (§7.4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchSources {
    /// The query.
    pub query: String,
    /// Hits per page.
    pub limit: Option<u32>,
}

impl Message<SearchSources> for DocumentActor {
    type Reply = DelegatedReply<Result<SearchPage<BlockHit>, String>>;

    async fn handle(
        &mut self,
        m: SearchSources,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let ix = self.indexer(ctx.actor_ref().clone());
        tokio::spawn(async move {
            tx.send(Ok(ix.page(&m.query, m.limit, &["source", "insight"]).await))
        });
        reply
    }
}

/// `ask` → `Result<SearchPage<BlockHit>, String>`: the `notebooks` category of `search()` (§7.4) —
/// notebook titles and descriptions.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchNotebooks {
    /// The query.
    pub query: String,
    /// Hits per page.
    pub limit: Option<u32>,
}

impl Message<SearchNotebooks> for DocumentActor {
    type Reply = DelegatedReply<Result<SearchPage<BlockHit>, String>>;

    async fn handle(
        &mut self,
        m: SearchNotebooks,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let ix = self.indexer(ctx.actor_ref().clone());
        tokio::spawn(async move { tx.send(Ok(ix.page(&m.query, m.limit, &["notebook"]).await)) });
        reply
    }
}

impl DocumentActor {
    /// The indexer over this actor's store and `notebooks` config.
    pub(crate) fn indexer(&self, me: ActorRef<DocumentActor>) -> Indexer {
        let cfg: IndexConfig = self.rt.section("notebooks").unwrap_or_default();
        Indexer {
            actor: me,
            store: self.store.clone(),
            cfg,
        }
    }
}

// ---------------------------------------------------------------- the indexer

/// Index writes run one at a time: a document changed twice in a row reads its blocks after the
/// earlier write landed, so the index never ends on the older text.
// ponytail: one global lock (writes are local and small); per-document locks if ingest throughput matters.
static WRITE: LazyLock<tokio::sync::Mutex<()>> = LazyLock::new(|| tokio::sync::Mutex::new(()));

/// Stateless: the actor (blocks), the store (titles, membership), the config (chunking, model
/// name). The embed actor is looked up per use.
#[derive(Clone)]
pub struct Indexer {
    actor: ActorRef<DocumentActor>,
    store: Store,
    cfg: IndexConfig,
}

impl Indexer {
    /// The store behind the index.
    pub fn store(&self) -> &Store {
        &self.store
    }

    fn embed_actor(&self) -> Option<ActorRef<EmbedActor>> {
        ActorRef::<EmbedActor>::lookup(EMBED_ACTOR).ok().flatten()
    }

    /// The installed model, `None` when the actor or its checkpoint directory is absent.
    async fn model(&self) -> Option<EmbedInfo> {
        if let Err(e) = decision::embed::check_name(&self.cfg.embedding_model) {
            static WARNED: std::sync::Once = std::sync::Once::new();
            WARNED.call_once(|| tracing::warn!("index: {e}; nb_search runs FTS5-only"));
            return None;
        }
        self.embed_actor()?.ask(Info).await.ok().flatten()
    }

    /// Unit vectors for `texts`, `None` when the model is absent or failed (lexical-only then).
    async fn embed(&self, texts: &[String]) -> Option<Vec<Vec<f32>>> {
        let actor = self.embed_actor()?;
        let mut out = Vec::with_capacity(texts.len());
        for batch in texts.chunks(EMBED_BATCH) {
            match actor
                .ask(Embed {
                    texts: batch.to_vec(),
                })
                .await
            {
                Ok(v) => out.extend(v),
                Err(e) => {
                    tracing::debug!("index: embed unavailable: {e}");
                    return None;
                }
            }
        }
        Some(out)
    }

    /// Tokens per text: the model's tokenizer, else `chars / 4`.
    async fn tokens(&self, texts: &[String]) -> Vec<usize> {
        if let Some(actor) = self.embed_actor()
            && let Ok(n) = actor
                .ask(Tokens {
                    texts: texts.to_vec(),
                })
                .await
            && n.len() == texts.len()
        {
            return n;
        }
        texts.iter().map(|t| chunk::estimate_tokens(t)).collect()
    }

    /// The top-level blocks of `doc` in order: ids and heading levels from the outline, full
    /// text from the annotated markdown.
    async fn blocks(&self, doc: &DocId) -> anyhow::Result<Vec<Block>> {
        let outline = self
            .actor
            .ask(Outline {
                doc: doc.clone(),
                depth: Some(1),
            })
            .await
            .map_err(anyhow::Error::msg)?;
        let md = self
            .actor
            .ask(Markdown {
                doc: doc.clone(),
                blocks: None,
                annotate: true,
            })
            .await
            .map_err(anyhow::Error::msg)?;
        Ok(assemble(&outline.blocks, &md))
    }

    /// Index `doc` if its text or the model changed since the last run.
    pub async fn index_doc(&self, doc: &DocId) -> anyhow::Result<()> {
        let _g = WRITE.lock().await;
        let Ok(meta) = self.store.read_meta(doc) else {
            return self.remove_unlocked(doc).await;
        };
        if !INDEXABLE.contains(&meta.kind) {
            return Ok(());
        }
        let model = self.model().await;
        let ix = Index::open(
            self.store.root(),
            model.as_ref().map_or(DEFAULT_DIMS, |m| m.dims),
        )
        .await?;
        let blocks = self.blocks(doc).await?;
        let texts: Vec<String> = blocks.iter().map(|b| b.text.clone()).collect();
        let shas: Vec<String> = texts.iter().map(|t| sha256(t.as_bytes())).collect();
        let content_sha = sha256(format!("{}\n{}", meta.title, shas.join("\n")).as_bytes());
        let model_name = model.as_ref().map(|m| m.model.clone());
        let state = ix.state(doc.as_str()).await?;
        let same_model = state.as_ref().is_some_and(|s| s.model == model_name);
        if same_model
            && state.as_ref().and_then(|s| s.content_sha.as_deref()) == Some(content_sha.as_str())
        {
            return Ok(());
        }
        let tokens = self.tokens(&texts).await;
        let kind = meta.kind.to_string();
        let kinds = chunk::kinds(&blocks, &kind);
        let mut rows = Vec::with_capacity(blocks.len() + 1);
        rows.push(BlockRow {
            block: String::new(),
            kind: kind.clone(),
            ord: 0,
            heading: None,
            text: meta.title.clone(),
            text_sha: sha256(meta.title.as_bytes()),
            tokens: chunk::estimate_tokens(&meta.title) as i64,
        });
        let mut heading: Option<String> = None;
        for (i, b) in blocks.iter().enumerate() {
            if b.heading.is_some() {
                heading = Some(b.text.clone());
            }
            rows.push(BlockRow {
                block: b.id.clone(),
                kind: kinds[i].to_owned(),
                ord: i as i64 + 1,
                heading: heading.clone(),
                text: b.text.clone(),
                text_sha: shas[i].clone(),
                tokens: tokens[i] as i64,
            });
        }
        let mut vectors = Vec::new();
        let mut embedded = None;
        if model.is_some() {
            let chunks = chunk::chunk(&blocks, &tokens, &self.cfg.params());
            // Unchanged chunks keep their vector — only when the same model made it (§7.5).
            let old = if same_model {
                ix.vectors(doc.as_str()).await?
            } else {
                HashMap::new()
            };
            let mut pending: Vec<(usize, String)> = Vec::new();
            let mut blobs: Vec<Option<Vec<u8>>> = Vec::with_capacity(chunks.len());
            for (i, c) in chunks.iter().enumerate() {
                let sha = sha256(c.text.as_bytes());
                match old.get(&sha) {
                    Some(blob) => blobs.push(Some(blob.clone())),
                    None => {
                        blobs.push(None);
                        pending.push((i, c.text.clone()));
                    }
                }
            }
            let fresh = if pending.is_empty() {
                Some(Vec::new())
            } else {
                self.embed(&pending.iter().map(|(_, t)| t.clone()).collect::<Vec<_>>())
                    .await
            };
            if let Some(fresh) = fresh {
                for ((i, _), v) in pending.iter().zip(fresh) {
                    blobs[*i] = Some(db::to_blob(&v));
                }
                vectors = chunks
                    .iter()
                    .zip(blobs)
                    .enumerate()
                    .filter_map(|(ord, (c, blob))| {
                        Some(VecRow {
                            block: blocks[c.first].id.clone(),
                            chunk: ord as i64,
                            sha: sha256(c.text.as_bytes()),
                            embedding: blob?,
                        })
                    })
                    .collect();
                embedded = model.clone();
            }
        }
        let state = State {
            doc: doc.as_str().to_owned(),
            content_sha: Some(content_sha),
            model: embedded.as_ref().map(|m| m.model.clone()),
            dims: embedded.as_ref().map(|m| m.dims as i64),
            indexed_ms: Some(now_ms()),
        };
        ix.replace(doc.as_str(), &rows, &vectors, &state).await
    }

    /// Drop `doc`'s rows.
    pub async fn remove(&self, doc: &DocId) -> anyhow::Result<()> {
        let _g = WRITE.lock().await;
        self.remove_unlocked(doc).await
    }

    async fn remove_unlocked(&self, doc: &DocId) -> anyhow::Result<()> {
        let model = self.model().await;
        let ix = Index::open(
            self.store.root(),
            model.as_ref().map_or(DEFAULT_DIMS, |m| m.dims),
        )
        .await?;
        ix.delete(doc.as_str()).await
    }

    /// §7.1 first run / §7.5 rebuild: every indexable live document whose text or model is
    /// stale, newest update first; rows of documents no longer live are dropped.
    pub async fn reindex(&self) -> anyhow::Result<()> {
        let mut docs = Vec::new();
        for kind in INDEXABLE {
            docs.extend(self.store.list(Some(kind), WALK_LIMIT).await?);
        }
        docs.sort_by_key(|d| std::cmp::Reverse(d.updated_ms));
        let live: BTreeSet<&str> = docs.iter().map(|d| d.id.as_str()).collect();
        let model = self.model().await;
        let ix = Index::open(
            self.store.root(),
            model.as_ref().map_or(DEFAULT_DIMS, |m| m.dims),
        )
        .await?;
        for gone in ix.indexed_docs().await? {
            if !live.contains(gone.as_str()) {
                ix.delete(&gone).await?;
            }
        }
        let (mut n, mut failed) = (0usize, 0usize);
        for d in &docs {
            match self.index_doc(&d.id).await {
                Ok(()) => n += 1,
                Err(e) => {
                    failed += 1;
                    tracing::warn!("index: {}: {e:#}", d.id);
                }
            }
        }
        tracing::info!(
            "index: walked {n} documents ({failed} failed), model {}",
            model.as_ref().map_or("none", |m| m.model.as_str())
        );
        Ok(())
    }

    /// One `search()` category page over `kinds`.
    async fn page(&self, query: &str, limit: Option<u32>, kinds: &[&str]) -> SearchPage<BlockHit> {
        let limit = search::limit(limit);
        if let Err(e) = search::validate_query(query) {
            return SearchPage::error(e);
        }
        if query.trim().is_empty() {
            return SearchPage::ready(Vec::new(), limit);
        }
        let args = NbSearchArgs {
            query: query.to_owned(),
            notebooks: None,
            docs: None,
            kinds: Some(kinds.iter().map(|k| (*k).to_owned()).collect()),
            limit: Some(limit),
        };
        match self.search(args).await {
            Ok(hits) => SearchPage::ready(hits, limit),
            Err(e) => SearchPage::error(format!("{e:#}")),
        }
    }

    /// §7.4: FTS5 top-`4×limit` ∪ vec0 top-`4×limit`, RRF-fused, de-duplicated by block; up to
    /// `limit + 1` hits (the callers page).
    pub async fn search(&self, args: NbSearchArgs) -> anyhow::Result<Vec<BlockHit>> {
        let limit = search::limit(args.limit);
        search::validate_query(&args.query).map_err(anyhow::Error::msg)?;
        let query = args.query.trim();
        if query.is_empty() {
            return Ok(Vec::new());
        }
        let docs = self.scope(&args).await?;
        if docs.as_ref().is_some_and(Vec::is_empty) {
            return Ok(Vec::new());
        }
        let kinds = args.kinds.filter(|k| !k.is_empty());
        let k = 4 * limit;
        let model = self.model().await;
        let ix = Index::open(
            self.store.root(),
            model.as_ref().map_or(DEFAULT_DIMS, |m| m.dims),
        )
        .await?;

        let fts = db::fts_query(query);
        let lexical = if fts.is_empty() {
            Vec::new()
        } else {
            ix.lexical(&fts, docs.as_deref(), kinds.as_deref(), k)
                .await?
        };

        let mut nearest = Vec::new();
        let qtext = [query.to_owned()];
        if let Some(m) = &model
            && let Some(q) = self.embed(&qtext).await
            && let Some(q) = q.first()
        {
            // §7.5: a document's vectors count only when its model is the current one.
            let models = ix.models().await?;
            let over = if kinds.is_some() { 4 * k } else { k };
            let near = ix.nearest(&db::to_blob(q), docs.as_deref(), over).await?;
            let keys: Vec<(String, String)> = near
                .into_iter()
                .filter(|n| {
                    models
                        .get(&n.doc)
                        .is_some_and(|mm| mm.as_deref() == Some(m.model.as_str()))
                })
                .map(|n| (n.doc, n.block))
                .collect();
            let rows = ix.rows(&keys).await?;
            let by_key: HashMap<(String, String), Lexical> = rows
                .into_iter()
                .map(|r| ((r.doc.clone(), r.block.clone()), r))
                .collect();
            for key in keys {
                if let Some(r) = by_key.get(&key)
                    && kinds.as_ref().is_none_or(|ks| ks.contains(&r.kind))
                {
                    nearest.push(r.clone());
                }
            }
        }

        // Fuse: 1/(60+rank) per list, summed per block; a lexical row keeps its snippet.
        let mut fused: HashMap<(String, String), (f32, Lexical, bool)> = HashMap::new();
        for (rank, l) in lexical.into_iter().enumerate() {
            let key = (l.doc.clone(), l.block.clone());
            fused.insert(key, (1.0 / (RRF_K + rank as f32), l, true));
        }
        for (rank, v) in nearest.into_iter().enumerate() {
            let s = 1.0 / (RRF_K + rank as f32);
            fused
                .entry((v.doc.clone(), v.block.clone()))
                .and_modify(|e| e.0 += s)
                .or_insert((s, v, false));
        }
        let mut ranked: Vec<(f32, Lexical, bool)> = fused.into_values().collect();
        ranked.sort_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then_with(|| a.1.doc.cmp(&b.1.doc))
                .then_with(|| a.1.block.cmp(&b.1.block))
        });
        ranked.truncate(limit as usize + 1);

        let mut heads: HashMap<String, (DocKind, String, Vec<DocId>)> = HashMap::new();
        let mut hits = Vec::with_capacity(ranked.len());
        for (score, row, lexical) in ranked {
            if !heads.contains_key(&row.doc) {
                let id = DocId(row.doc.clone());
                let Ok(meta) = self.store.read_meta(&id) else {
                    continue;
                };
                let notebooks = match meta.kind {
                    DocKind::Notebook => vec![id.clone()],
                    DocKind::Source => {
                        self.store
                            .notebooks_of(ItemKind::Source, id.as_str(), false)
                            .await?
                    }
                    _ => {
                        self.store
                            .notebooks_of(ItemKind::Note, id.as_str(), false)
                            .await?
                    }
                };
                heads.insert(row.doc.clone(), (meta.kind, meta.title, notebooks));
            }
            let (kind, title, notebooks) = &heads[&row.doc];
            hits.push(hit(score, &row, lexical, *kind, title, notebooks.clone()));
        }
        Ok(hits)
    }

    /// The document scope: `docs` ∪ the live items of `notebooks` (notes, sources, the notebook
    /// documents themselves); `None` = everything.
    async fn scope(&self, args: &NbSearchArgs) -> anyhow::Result<Option<Vec<String>>> {
        if args.docs.is_none() && args.notebooks.is_none() {
            return Ok(None);
        }
        let mut docs: BTreeSet<String> = args
            .docs
            .iter()
            .flatten()
            .map(|d| d.as_str().to_owned())
            .collect();
        for nb in args.notebooks.iter().flatten() {
            docs.insert(nb.as_str().to_owned());
            for item in self.store.items(nb).await? {
                if matches!(item.kind, ItemKind::Note | ItemKind::Source) {
                    docs.insert(item.item);
                }
            }
        }
        Ok(Some(docs.into_iter().collect()))
    }
}

/// One hit from a fused row.
fn hit(
    score: f32,
    row: &Lexical,
    lexical: bool,
    kind: DocKind,
    title: &str,
    notebooks: Vec<DocId>,
) -> BlockHit {
    let r#match = if row.block.is_empty() {
        let (snippet, ranges) = strip_marks(&row.snippet);
        TextMatch {
            field: "title".into(),
            snippet,
            ranges,
        }
    } else if lexical && row.snippet.contains(db::MARK_START) {
        let (snippet, ranges) = strip_marks(&row.snippet);
        TextMatch {
            field: "text".into(),
            snippet,
            ranges,
        }
    } else if lexical && row.hsnip.contains(db::MARK_START) {
        let (snippet, ranges) = strip_marks(&row.hsnip);
        TextMatch {
            field: "heading".into(),
            snippet,
            ranges,
        }
    } else {
        let (snippet, _) = search::snippet(&row.text, &[], SNIPPET_CHARS);
        TextMatch {
            field: "text".into(),
            snippet,
            ranges: Vec::new(),
        }
    };
    let href = TheuthAnchor {
        kind: if kind == DocKind::Source {
            TheuthKind::Sources
        } else {
            TheuthKind::Notes
        },
        id: row.doc.clone(),
        locator: (!row.block.is_empty()).then(|| Locator::Block(row.block.clone())),
        sha8: (!row.block.is_empty()).then(|| row.text_sha[..8].to_owned()),
    }
    .href();
    BlockHit {
        doc: DocId(row.doc.clone()),
        kind,
        title: title.to_owned(),
        block: row.block.clone(),
        heading: row.heading.clone(),
        score,
        r#match,
        href,
        notebooks,
    }
}

/// Remove the `snippet()` markers, returning the clean text and the marked spans in scalars.
fn strip_marks(s: &str) -> (String, Vec<MatchRange>) {
    let mut out = String::with_capacity(s.len());
    let mut ranges = Vec::new();
    let (mut pos, mut start) = (0u32, None);
    for c in s.chars() {
        match c {
            db::MARK_START => start = Some(pos),
            db::MARK_END => {
                if let Some(st) = start.take()
                    && pos > st
                {
                    ranges.push(MatchRange {
                        start: st,
                        end: pos,
                    });
                }
            }
            _ => {
                out.push(c);
                pos += 1;
            }
        }
    }
    (out, ranges)
}

/// Join the outline's top-level blocks with the annotated markdown (`<!-- id -->` or
/// `<!-- id by name -->` before each block). Blocks without text are skipped; headings keep
/// their text without the `#`s.
fn assemble(outline: &[serde_json::Value], md: &str) -> Vec<Block> {
    let mut texts: HashMap<&str, String> = HashMap::new();
    let mut cur: Option<(&str, String)> = None;
    for line in md.lines() {
        if let Some(id) = marker_id(line) {
            if let Some((id, text)) = cur.take() {
                texts.insert(id, text);
            }
            cur = Some((id, String::new()));
        } else if let Some((_, text)) = &mut cur {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(line);
        }
    }
    if let Some((id, text)) = cur.take() {
        texts.insert(id, text);
    }
    outline
        .iter()
        .filter_map(|b| {
            let id = b["id"].as_str()?;
            let ty = b["type"].as_str().unwrap_or("");
            if ty == "frontmatter" {
                return None;
            }
            let raw = texts.get(id)?.trim();
            let heading = (ty == "heading")
                .then(|| {
                    b["text"]
                        .as_str()
                        .and_then(|t| t.strip_prefix("[h"))
                        .and_then(|t| t.split_once(']'))
                        .and_then(|(l, _)| l.parse::<u8>().ok())
                })
                .flatten();
            let text = if heading.is_some() {
                raw.trim_start_matches('#').trim().to_owned()
            } else {
                raw.to_owned()
            };
            (!text.is_empty()).then(|| Block {
                id: id.to_owned(),
                heading,
                text,
            })
        })
        .collect()
}

/// `<!-- <id> -->` / `<!-- <id> by <name> -->` → the id.
fn marker_id(line: &str) -> Option<&str> {
    let inner = line.trim().strip_prefix("<!-- ")?.strip_suffix(" -->")?;
    let id = inner.split(' ').next()?;
    (!id.is_empty()).then_some(id)
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assemble_joins_outline_ids_with_annotated_markdown() {
        let outline = vec![
            serde_json::json!({"id": "fm", "type": "frontmatter", "text": "type=note"}),
            serde_json::json!({"id": "h", "type": "heading", "text": "[h2] Failover", "children": []}),
            serde_json::json!({"id": "p", "type": "paragraph", "text": "Promote the…", "children": []}),
            serde_json::json!({"id": "e", "type": "paragraph", "text": "", "children": []}),
        ];
        let md = "---\ntype: note\n---\n<!-- h -->\n## Failover\n\n<!-- p by Ann -->\nPromote the standby.\nThen retry.\n\n<!-- e -->\n\n";
        let blocks = assemble(&outline, md);
        assert_eq!(
            blocks,
            [
                Block {
                    id: "h".into(),
                    heading: Some(2),
                    text: "Failover".into()
                },
                Block {
                    id: "p".into(),
                    heading: None,
                    text: "Promote the standby.\nThen retry.".into()
                },
            ]
        );
    }

    #[test]
    fn strip_marks_yields_scalar_ranges() {
        let (s, r) = strip_marks("é \u{1}fail\u{2}over \u{1}x\u{2}");
        assert_eq!(s, "é failover x");
        assert_eq!(
            r,
            [
                MatchRange { start: 2, end: 6 },
                MatchRange { start: 11, end: 12 }
            ]
        );
    }
}
