//! Read-only agent roster, distinct from the subagent picker and activity board.
use rpc_compose::api::clients::BoardsApiClient;
use super::*;

/// Read-only agent roster listing.
#[derive(Default)]
pub struct Roster;
/// Typed roster owner request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// List all roster agents.
    ListRoster,
}
/// Roster request completion.
pub enum Msg {
    /// Returned summaries or owner error.
    Loaded(Result<Vec<::roster::AgentSummary>, String>),
}
impl Component for Roster {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "roster", interests: &[], slash: &[("/roster", "", "the agent roster: name, status, open tasks (67)")], bindings: &[] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { .. } => cx.spawn_request(Lane::Concurrent, Request::ListRoster, |api, _| async move {
                Msg::Loaded(api.agents_list().await.map_err(crate::reason))
            }),
            Input::Message(Msg::Loaded(result)) => match result {
                Ok(rows) => {
                    let text = agents_table(&rows);
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    cx.plain(Plain::Line(text.clone()));
                    cx.core(CoreAction::Notice(text));
                }
                Err(error) => cx.error(error),
            },
            _ => {}
        }
    }
    fn view(&self, _: Surface<'_>, _: &ViewCx<'_>) -> Option<View> { None }
}
/// `/roster`: `name · status · open` per roster agent (67 §12; `/agents` is the subagent picker).
pub fn agents_table(rows: &[::roster::AgentSummary]) -> String {
    if rows.is_empty() {
        return "no agents".into();
    }
    let mut out = vec!["name · status · open".to_owned()];
    out.extend(rows.iter().map(|a| {
        let status = a
            .paused_label()
            .or_else(|| a.waiting_label())
            .unwrap_or_else(|| {
                serde_json::to_value(a.status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default()
            });
        format!("{} · {status} · {}", a.file.name.as_str(), a.open_tasks)
    }));
    out.join("\n")
}
