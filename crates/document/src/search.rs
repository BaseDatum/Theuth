//! The `notes` search category (66 §10, phase 5; block-level since 144 §7.4): titles and blocks
//! of free-standing notes and boards out of `nb_blocks` / `nb_vec` — no file is read per query.
//! One kameo message on the document actor, answered off the mailbox.

use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use search::{SearchPage, TextMatch};
use serde::{Deserialize, Serialize};

use crate::actor::DocumentActor;
use crate::index::{Indexer, NbSearchArgs};
use crate::meta::DocSummary;

/// One block of a note that matched.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NoteSearchHit {
    /// The note as `doc_list` shows it.
    pub doc: DocSummary,
    /// `field` is `title`, `heading` or `text`.
    pub r#match: TextMatch,
    /// The block that matched (an outline id; opens the editor there); empty when the title did.
    pub block: String,
    /// The nearest heading above the block.
    pub heading: Option<String>,
    /// `theuth://notes/<id>#<block>?s=<sha8>` (144 §6.1); no locator for a title hit.
    pub href: String,
}

/// `ask` → `Result<SearchPage<NoteSearchHit>, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchNotes {
    /// The query (lexical and embedded alike); blank → ready and empty.
    pub query: String,
    /// Hits per page; defaults and caps per the shared search limits.
    pub limit: Option<u32>,
}

impl Message<SearchNotes> for DocumentActor {
    type Reply = DelegatedReply<Result<SearchPage<NoteSearchHit>, String>>;

    async fn handle(
        &mut self,
        m: SearchNotes,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let ix = self.indexer(ctx.actor_ref().clone());
        tokio::spawn(async move { tx.send(Ok(search_notes(&ix, &m.query, m.limit).await)) });
        reply
    }
}

/// Live notes and boards (144 §7.1 kinds `note | board`) matching `query` at block level:
/// hybrid rank (§7.4); `limit + 1` fused rows set `has_more`.
pub async fn search_notes(
    ix: &Indexer,
    query: &str,
    limit: Option<u32>,
) -> SearchPage<NoteSearchHit> {
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
        kinds: Some(vec!["note".into(), "board".into()]),
        limit: Some(limit),
    };
    let hits = match ix.search(args).await {
        Ok(h) => h,
        Err(e) => return SearchPage::error(format!("{e:#}")),
    };
    let mut out = Vec::with_capacity(hits.len());
    for h in hits {
        let Ok(meta) = ix.store().read_meta(&h.doc) else {
            continue;
        };
        out.push(NoteSearchHit {
            doc: DocSummary::from(&meta),
            r#match: h.r#match,
            block: h.block,
            heading: h.heading,
            href: h.href,
        });
    }
    SearchPage::ready(out, limit)
}
