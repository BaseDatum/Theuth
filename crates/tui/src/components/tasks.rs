//! Project-scoped task inventory; no live subscription or modal state.
use rpc::api::clients::BoardsApiClient;
use super::*;

/// Project-scoped task listing.
#[derive(Default)]
pub struct Tasks;
/// Typed task owner request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// List the selected root project, including ungrouped.
    List(Option<String>),
}
/// Completion of a task owner request.
pub enum Msg {
    /// Project and its returned rows or owner error.
    Loaded(Option<String>, Result<Vec<::tasks::Task>, String>),
}
impl Component for Tasks {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "tasks", interests: &[], slash: &[("/tasks", "", "this project's tasks: id, state, owner, title (67)")], bindings: &[] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { .. } => {
                let project = cx.core.project.map(str::to_owned);
                cx.spawn_request(Lane::Concurrent, Request::List(project), move |api, Request::List(project)| async move {
                    let filter = ::tasks::TaskFilter { project: project.clone(), ..Default::default() };
                    Msg::Loaded(project, api.tasks_list(filter).await.map_err(crate::reason))
                });
            }
            Input::Message(Msg::Loaded(project, result)) => match result {
                Ok(rows) => {
                    let text = tasks_table(project.as_deref(), &rows);
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
/// `/tasks`: `id · state · owner · title`, oldest first as the engine lists them (67 §12).
/// The heading names the project, or `all projects` when the session has none.
pub fn tasks_table(project: Option<&str>, rows: &[::tasks::Task]) -> String {
    let scope = project.unwrap_or("all projects");
    if rows.is_empty() {
        return format!("no tasks ({scope})");
    }
    let mut out = vec![
        format!("tasks · {scope}"),
        "id · state · owner · title".to_owned(),
    ];
    out.extend(rows.iter().map(|t| {
        let id = &t.id.0;
        let short = &id[id.len().saturating_sub(6)..];
        format!("{short} · {} · {} · {}", t.state.as_str(), t.owner, t.title)
    }));
    out.join("\n")
}
