//! The `notes` search category (66 §10, phase 5): titles and derived
//! markdown of free-standing notes, nucleo-ranked like the other owners.
//! One kameo message on the document actor, answered off the mailbox.

use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use search::{FuzzyMatcher, SearchPage, SearchText, TextMatch};
use serde::{Deserialize, Serialize};

use crate::actor::DocumentActor;
use crate::meta::{DocKind, DocSummary};
use crate::store::Store;

/// Notes the owner reads per query (the `doc_list` cap).
const MAX_NOTES: u32 = 500;
const SNIPPET_CHARS: usize = 160;

/// One note that matched.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct NoteSearchHit {
    /// The note as `doc_list` shows it.
    pub doc: DocSummary,
    /// `field` is `title` or `body`.
    pub r#match: TextMatch,
}

/// `ask` → `Result<SearchPage<NoteSearchHit>, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchNotes {
    /// Fuzzy query (fzf syntax); blank → ready and empty.
    pub query: String,
    /// Hits per page; defaults and caps per the shared search limits.
    pub limit: Option<u32>,
}

impl Message<SearchNotes> for DocumentActor {
    type Reply = DelegatedReply<Result<SearchPage<NoteSearchHit>, String>>;

    async fn handle(&mut self, m: SearchNotes, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let store = self.store.clone();
        tokio::spawn(async move { tx.send(Ok(search_notes(&store, &m.query, m.limit).await)) });
        reply
    }
}

/// Live notes (`deleted_ms IS NULL`) matching `query`: title first, else the
/// derived `note.md`; score desc, updated desc, id asc; `limit + 1` scored
/// rows set `has_more`.
// ponytail: reads every note file per query (≤ 500 small files); a cached
// index keyed on DocumentChanged is the upgrade if a measured harness is slow.
pub async fn search_notes(store: &Store, query: &str, limit: Option<u32>) -> SearchPage<NoteSearchHit> {
    let limit = search::limit(limit);
    if let Err(e) = search::validate_query(query) {
        return SearchPage::error(e);
    }
    let query = query.trim().to_owned();
    if query.is_empty() {
        return SearchPage::ready(Vec::new(), limit);
    }
    let notes = match store.list(Some(DocKind::Note), MAX_NOTES).await {
        Ok(n) => n,
        Err(e) => return SearchPage::error(format!("{e:#}")),
    };
    let root = store.root().to_path_buf();
    let matched = tokio::task::spawn_blocking(move || {
        let mut m = FuzzyMatcher::new(&query);
        let mut scored: Vec<(u32, bool, DocSummary, SearchText, SearchText)> = Vec::new();
        for doc in notes {
            let title = SearchText::new(doc.title.clone());
            let body = SearchText::new(
                std::fs::read_to_string(root.join(doc.id.as_str()).join("note.md")).unwrap_or_default(),
            );
            let (score, is_body) = match (m.score(&title), m.score(&body)) {
                (None, None) => continue,
                (Some(a), Some(b)) if b > a => (b, true),
                (Some(a), _) => (a, false),
                (None, Some(b)) => (b, true),
            };
            scored.push((score, is_body, doc, title, body));
        }
        scored.sort_by(|a, b| {
            b.0.cmp(&a.0)
                .then_with(|| b.2.updated_ms.cmp(&a.2.updated_ms))
                .then_with(|| a.2.id.as_str().cmp(b.2.id.as_str()))
        });
        scored.truncate(limit as usize + 1);
        let hits = scored
            .into_iter()
            .filter_map(|(_, is_body, doc, title, body)| {
                let (text, field) = if is_body { (&body, "body") } else { (&title, "title") };
                let (_, r#match) = m.matched(text, field, SNIPPET_CHARS)?;
                Some(NoteSearchHit { doc, r#match })
            })
            .collect();
        SearchPage::ready(hits, limit)
    })
    .await;
    matched.unwrap_or_else(|e| SearchPage::error(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::meta::{AgentStyle, Binding, DocId, DocMeta};
    use search::PageState;

    async fn note(store: &Store, title: &str, body: &str, updated_ms: i64) -> DocId {
        let id = DocId::new();
        let binding = Binding::Note;
        store
            .create(&DocMeta {
                id: id.clone(),
                kind: DocKind::Note,
                title: title.into(),
                derived: binding.derived(&id),
                binding,
                fragment: "blocks".into(),
                members: vec![],
                links: vec![],
                agent: AgentStyle::default(),
                last_export: None,
                authors: Default::default(),
                created_ms: updated_ms,
                updated_ms,
            })
            .await
            .unwrap();
        std::fs::write(store.root().join(id.as_str()).join("note.md"), body).unwrap();
        id
    }

    /// Phase 5 row: a title hit, a body hit, a deleted note excluded, `has_more`.
    #[tokio::test]
    async fn title_and_body_hits_skip_deleted_and_page() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(dir.path()).await.unwrap();
        let groceries = note(&store, "Groceries", "milk\neggs\n", 3).await;
        let plan = note(&store, "Plan", "buy groceries on the way home\n", 2).await;
        let gone = note(&store, "Groceries archive", "old groceries\n", 1).await;
        store.delete(&gone).await.unwrap();

        let page = search_notes(&store, "groceries", None).await;
        assert_eq!(page.state, PageState::Ready, "{page:?}");
        let ids: Vec<&str> = page.hits.iter().map(|h| h.doc.id.as_str()).collect();
        assert_eq!(ids, [groceries.as_str(), plan.as_str()], "{page:?}");
        assert_eq!(page.hits[0].r#match.field, "title");
        assert_eq!(page.hits[1].r#match.field, "body");
        assert!(page.hits[1].r#match.snippet.contains("groceries"), "{page:?}");
        assert!(!page.has_more);

        let page = search_notes(&store, "groceries", Some(1)).await;
        assert_eq!(page.hits.len(), 1);
        assert!(page.has_more);

        let blank = search_notes(&store, "  ", None).await;
        assert_eq!(blank.state, PageState::Ready);
        assert!(blank.hits.is_empty());
    }
}
