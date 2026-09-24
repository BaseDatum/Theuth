//! The `meetings` search category (66 §10, phase 5): titles nucleo-ranked
//! like the other owners, transcript segments through the FTS index that
//! `meeting_search` already uses, one hit per meeting as a `SearchPage`.

use std::collections::HashMap;
use std::sync::Arc;

use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use search::{FuzzyMatcher, SearchPage, SearchText, TextMatch};
use serde::{Deserialize, Serialize};

use crate::store::{MeetingSummary, Store};
use crate::{Meeting, MeetingId};

/// Meetings whose titles are matched per query.
const MAX_MEETINGS: u32 = 500;
const SNIPPET_CHARS: usize = 160;

/// One meeting that matched.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MeetingSearchHit {
    /// The meeting as `meeting_list` shows it.
    pub meeting: MeetingSummary,
    /// `field` is `title` or `transcript` (the best segment; `ranges` may be empty).
    pub r#match: TextMatch,
}

/// `ask` → `Result<SearchPage<MeetingSearchHit>, String>`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SearchMeetings {
    /// Fuzzy query (fzf syntax) for titles; whole words for the transcript index. Blank → ready and empty.
    pub query: String,
    /// Hits per page; defaults and caps per the shared search limits.
    pub limit: Option<u32>,
}

impl Message<SearchMeetings> for Meeting {
    type Reply = DelegatedReply<Result<SearchPage<MeetingSearchHit>, String>>;

    async fn handle(&mut self, m: SearchMeetings, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let store = self.store.clone();
        tokio::spawn(async move { tx.send(Ok(search_meetings(&store, &m.query, m.limit).await)) });
        reply
    }
}

/// Human title: `title`, else the process name (§8).
pub fn title_of(m: &MeetingSummary) -> &str {
    m.title.as_deref().filter(|t| !t.trim().is_empty()).unwrap_or(&m.process_name)
}

/// Title hits (nucleo) and transcript hits (FTS, best rank first, one per
/// meeting); score desc, started desc; `limit + 1` rows set `has_more`.
pub async fn search_meetings(store: &Arc<Store>, query: &str, limit: Option<u32>) -> SearchPage<MeetingSearchHit> {
    let limit = search::limit(limit);
    if let Err(e) = search::validate_query(query) {
        return SearchPage::error(e);
    }
    let query = query.trim().to_owned();
    if query.is_empty() {
        return SearchPage::ready(Vec::new(), limit);
    }
    let meetings = match store.list(MAX_MEETINGS).await {
        Ok(m) => m,
        Err(e) => return SearchPage::error(format!("{e:#}")),
    };
    let segments = store.search(&query, None).await.unwrap_or_default();
    let matched = tokio::task::spawn_blocking(move || {
        let mut m = FuzzyMatcher::new(&query);
        let by_id: HashMap<MeetingId, &MeetingSummary> = meetings.iter().map(|s| (s.id, s)).collect();
        // (score, meeting, match)
        let mut hits: Vec<(u32, &MeetingSummary, TextMatch)> = Vec::new();
        for s in &meetings {
            let title = SearchText::new(title_of(s).to_owned());
            if let Some((score, r#match)) = m.matched(&title, "title", SNIPPET_CHARS) {
                hits.push((score, s, r#match));
            }
        }
        for seg in &segments {
            if hits.iter().any(|(_, s, _)| s.id == seg.meeting) {
                continue;
            }
            let Some(summary) = by_id.get(&seg.meeting) else { continue };
            // The FTS snippet marks matches with `[…]`; nucleo re-finds them for ranges.
            let plain = SearchText::new(seg.snippet.replace(['[', ']'], ""));
            let (score, r#match) = m.matched(&plain, "transcript", SNIPPET_CHARS).unwrap_or((
                1,
                TextMatch {
                    field: "transcript".into(),
                    snippet: plain.as_str().to_owned(),
                    ranges: Vec::new(),
                },
            ));
            hits.push((score, summary, r#match));
        }
        hits.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.started_at.cmp(&a.1.started_at)));
        hits.truncate(limit as usize + 1);
        let hits = hits
            .into_iter()
            .map(|(_, s, r#match)| MeetingSearchHit {
                meeting: s.clone(),
                r#match,
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
    use crate::store::Manifest;
    use crate::transcript::{Segment, Transcript};
    use crate::{MeetingState, ProcessInfo, Track};
    use search::PageState;

    async fn meeting(store: &Store, title: Option<&str>, started: &str) -> MeetingId {
        let mut m = Manifest::new(
            MeetingId::new(),
            ProcessInfo {
                pid: 1,
                name: "Zoom".into(),
                bundle_id: None,
                active: true,
            },
            started.into(),
        );
        m.title = title.map(str::to_owned);
        m.state = MeetingState::Done;
        store.create(&m).await.unwrap();
        m.id
    }

    /// Phase 5 row: a title hit, a segment hit, `has_more`.
    #[tokio::test]
    async fn title_and_segment_hits_page() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(Store::open(dir.path()).await.unwrap());
        let planning = meeting(&store, Some("Weekly planning"), "2026-09-23T10:00:00Z").await;
        let standup = meeting(&store, None, "2026-09-22T10:00:00Z").await;
        let _other = meeting(&store, Some("Retro"), "2026-09-21T10:00:00Z").await;
        store
            .index_transcript(&Transcript {
                meeting: standup,
                speakers: vec![],
                segments: vec![Segment {
                    id: 0,
                    track: Track::Mic,
                    speaker: "me".into(),
                    t0_ms: 0,
                    t1_ms: 4_000,
                    text: "let us move planning to thursday".into(),
                    words: vec![],
                }],
                reviewed_at: None,
                summarized_at: None,
                diarized: false,
                note: None,
            })
            .await
            .unwrap();

        let page = search_meetings(&store, "planning", None).await;
        assert_eq!(page.state, PageState::Ready, "{page:?}");
        let ids: Vec<MeetingId> = page.hits.iter().map(|h| h.meeting.id).collect();
        assert_eq!(ids, [planning, standup], "{page:?}");
        assert_eq!(page.hits[0].r#match.field, "title");
        assert_eq!(page.hits[1].r#match.field, "transcript");
        assert_eq!(title_of(&page.hits[1].meeting), "Zoom");
        assert!(page.hits[1].r#match.snippet.contains("planning"), "{page:?}");
        assert!(!page.has_more);

        let page = search_meetings(&store, "planning", Some(1)).await;
        assert_eq!(page.hits.len(), 1);
        assert!(page.has_more);
        assert!(search_meetings(&store, "", None).await.hits.is_empty());
    }
}
