//! Inline web progress keyed by the current transcript's stable call identities.
use std::collections::HashMap;
use web::progress::ScrapeProgress;
use crate::model::Entry;
use super::*;

/// Live web summaries of the accepted transcript.
#[derive(Default)]
pub struct WebProgress {
    /// Current running-call summary, removed when the result lands.
    pub progress: HashMap<String, String>,
}
/// Owner progress or core tool completion.
pub enum Msg {
    /// Latest live web progress.
    Progress(ScrapeProgress),
    /// Current-session tool result landed.
    Finished(String),
}
impl Component for WebProgress {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration {
            id: "web-progress", slash: &[], bindings: &[],
            interests: &[
                Interest { topic: "web.scrape-progress", version: 1, variants: &["ScrapeProgress"] },
                Interest { topic: "agent.events", version: 1, variants: &["ToolCallFinished"] },
            ],
        }
    }
    fn decode(&self, event: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        match event.topic() {
            Some("web.scrape-progress") if event.version() == Some(1) => serde::Deserialize::deserialize(event.event()).ok().map(Msg::Progress),
            None if event.event()["type"] == "ScrapeProgress" => serde::Deserialize::deserialize(event.event()).ok().map(Msg::Progress),
            Some("agent.events") | None => match core.agent? {
                agent::AgentEvent::ToolCallFinished { session, call_id, .. } if core.session == Some(*session) => Some(Msg::Finished(call_id.clone())),
                _ => None,
            },
            _ => None,
        }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Message(Msg::Progress(event)) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                if cx.core.session != Some(event.session) { return; }
                if let Some(Entry::Tool { name, .. }) = cx.core.entries.iter().rev().find(|entry|
                    matches!(entry, Entry::Tool { call_id, result: None, .. } if *call_id == event.call_id))
                {
                    self.progress.insert(event.call_id.clone(), scrape_line(name, &event));
                }
            }
            Input::Message(Msg::Finished(call)) => { self.progress.remove(&call); }
            Input::Lifecycle(Lifecycle::SessionReset | Lifecycle::TranscriptReplaced | Lifecycle::EngineChanged) => self.progress.clear(),
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, _cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::EntrySummary { key } => self.progress.get(key).cloned().map(View::Text),
            _ => None,
        }
    }
}

/// The spinner row from the latest owner progress, unchanged from the original renderer.
pub fn scrape_line(tool: &str, p: &ScrapeProgress) -> String {
    let at = p.inflight.first().or(p.done.last()).map_or("", |u| host(u));
    let n = p.done.len();
    match tool.strip_prefix("↳ ").unwrap_or(tool) {
        "crawl" if at.is_empty() => format!("crawling… {n}/{}", p.limit),
        "crawl" => format!("crawling… {n}/{} · {at}", p.limit),
        "map_site" => format!("mapping… {n}/{}", p.limit),
        "scrape_url" => format!("reading {at}…"),
        "search_web" if at.is_empty() => "searching…".to_owned(),
        "search_web" => format!("reading {n}/{} · {at}", p.limit),
        _ => format!("running {tool}…"),
    }
}
fn host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or(rest)
}
