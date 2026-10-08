// 144-W2 owns this file.
//! The block index (design 144 §7): `nb_blocks` (FTS5) + `nb_vec` (vec0) in `documents/index.sqlite`,
//! fed by `DocumentChanged`, searched by `nb_search` and by the `sources` / `notebooks` categories of
//! `search()`. The skeleton leaves every engine unimplemented.

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use search::{SearchPage, TextMatch};
use serde::{Deserialize, Serialize};

use crate::actor::DocumentActor;
use crate::meta::{DocId, DocKind, DocumentChanged};

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
    /// The block id (an outline id; opens the editor at the block).
    pub block: String,
    /// The nearest heading above the block.
    pub heading: Option<String>,
    /// Fused rank score (reciprocal rank, higher is better).
    pub score: f32,
    /// Snippet with highlight ranges; `field` is `heading` or `text`.
    pub r#match: TextMatch,
    /// The citation href for this block: `theuth://notes/<id>#<block>?s=<sha8>` or `sources/…`.
    pub href: String,
    /// Live notebooks holding the document.
    pub notebooks: Vec<DocId>,
}

const NOT_IMPLEMENTED: &str = "144: not implemented yet";

/// Hybrid retrieval (§7.4): FTS5 bm25 ∪ vec0 cosine, reciprocal-rank fused. Any engine.
pub async fn nb_search(
    _actor: &ActorRef<DocumentActor>,
    _args: NbSearchArgs,
) -> anyhow::Result<Vec<BlockHit>> {
    Err(anyhow::anyhow!(NOT_IMPLEMENTED))
}

/// Walk `docs` where `kind ∈ (note, board, meeting_note, source, notebook)` and re-index every stale
/// document (§7.1 first run, §7.5 model change).
pub async fn reindex(_actor: &ActorRef<DocumentActor>) -> anyhow::Result<()> {
    Err(anyhow::anyhow!(NOT_IMPLEMENTED))
}

/// The indexer's trigger (§7.1): called by the document actor for every `DocumentChanged` it publishes;
/// re-indexes the changed blocks of `ev.doc`, drops the rows of a deleted document.
pub fn on_document_changed(_ev: &DocumentChanged) {}

/// `ask` → `Result<SearchPage<BlockHit>, String>`: the `sources` category of `search()` (§7.4).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchSources {
    /// The query.
    pub query: String,
    /// Hits per page.
    pub limit: Option<u32>,
}

impl Message<SearchSources> for DocumentActor {
    type Reply = Result<SearchPage<BlockHit>, String>;

    async fn handle(
        &mut self,
        _: SearchSources,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(SearchPage::unavailable(NOT_IMPLEMENTED.to_owned()))
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
    type Reply = Result<SearchPage<BlockHit>, String>;

    async fn handle(
        &mut self,
        _: SearchNotebooks,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(SearchPage::unavailable(NOT_IMPLEMENTED.to_owned()))
    }
}
