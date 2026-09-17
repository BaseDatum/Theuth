//! Live page-fetch progress for one tool call (design doc 38 §6, 16 §2):
//! `search_web` reading its top hits and `scrape`'s crawl/map/scrape_url
//! publish the same event so every frontend renders one card.

use runtime::{Runtime, SessionId};
use serde::{Deserialize, Serialize};

/// Joins a [`ScrapeProgress`] to the tool call that is fetching; `None` in
/// library use (no events).
#[derive(Clone, Debug)]
pub struct ProgressCx {
    /// Owning session.
    pub session: SessionId,
    /// Provider call id (`ToolCallStarted.call_id`).
    pub call_id: String,
}

/// Live URLs for one search/scrape/crawl/map call. Wire `{ "type": "ScrapeProgress", … }`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type")]
pub struct ScrapeProgress {
    /// Owning session.
    pub session: SessionId,
    /// The tool call (joins the card).
    pub call_id: String,
    /// URLs being fetched right now.
    pub inflight: Vec<String>,
    /// Completed page URLs, oldest first.
    pub done: Vec<String>,
    /// Page budget for this run.
    pub limit: usize,
}

/// Publish one progress snapshot; a no-op without a bus or a call context.
pub async fn report(
    rt: Option<&Runtime>,
    cx: Option<&ProgressCx>,
    inflight: &[String],
    done: &[String],
    limit: usize,
) {
    let (Some(rt), Some(cx)) = (rt, cx) else {
        return;
    };
    rt.publish(ScrapeProgress {
        session: cx.session,
        call_id: cx.call_id.clone(),
        inflight: inflight.to_vec(),
        done: done.to_vec(),
        limit,
    })
    .await;
}
