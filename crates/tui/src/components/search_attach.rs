//! Live search and reference picking, including previews and historical inspection.
use super::*;
use iocraft::KeyCode;
use rpc_compose::api::clients::{CompositionApiClient, AgentSessionsApiClient, ArtifactArtifactsApiClient, KnowledgeConceptsApiClient};
use runtime::SessionId;
use serde::Deserialize;

pub mod state;
pub mod attach;
mod ui;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode { Search, Attach }
#[derive(Default)]
pub(crate) struct SearchAttach {
    pub(crate) state: Box<state::State>,
    pub(crate) mode: Option<Mode>,
    pub(crate) inspect: Option<state::Inspect>,
    pane: u64,
}
#[expect(clippy::large_enum_variant, reason = "One Loaded page per already boxed reply, matching state::Reply.")]
pub(crate) enum Msg {
    Reply { generation: u64, reply: state::Reply },
    Edited { pane: u64, value: String },
    KnowledgeChanged, ArtifactsChanged, SessionsChanged,
    Presented(state::Inspect), Restore,
    Action(Result<Option<String>, String>),
}
fn requests(cx: &mut Cx<'_, '_, Msg>, requests: Vec<state::Request>) {
    for request in requests {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            Msg::Reply { generation: request.generation, reply: search_request(&api, request.kind).await }
        });
    }
}
fn open_path(cx: &mut Cx<'_, '_, Msg>, path: std::path::PathBuf) {
    cx.spawn(Lane::Concurrent, move |_| async move { Msg::Action(artifacts_apps::open_path(&path).map(|()| None)) });
}
fn export(cx: &mut Cx<'_, '_, Msg>, id: String, name: String) {
    cx.spawn(Lane::Concurrent, move |api| async move { Msg::Action(artifacts_apps::export_artifact(&api, id, name, None).await.map(Some)) });
}
impl SearchAttach {
    fn open(&mut self, mode: Mode, query: &str, cx: &mut Cx<'_, '_, Msg>) {
        self.inspect = None;
        self.mode = Some(mode);
        self.pane += 1;
        cx.core(CoreAction::SetReadOnly(false));
        requests(cx, self.state.open(query));
        cx.focus(FocusClass::Search);
    }
    fn close(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        self.mode = None;
        self.pane += 1;
        self.state.close();
        cx.close();
    }
    fn restore(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        self.mode = Some(Mode::Search);
        self.pane += 1;
        requests(cx, self.state.restore());
        cx.focus(FocusClass::Search);
    }
    fn jump_latest(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        if let Some(inspect) = self.inspect.take() { cx.core(CoreAction::ResumeSession(inspect.session)); }
    }
    fn back_to_results(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        self.jump_latest(cx);
        cx.post(Msg::Restore);
    }
    fn inspect_page(&mut self, session: SessionId, seq: i64, mut loaded: agent::Loaded, cx: &mut Cx<'_, '_, Msg>) {
        self.mode = None;
        self.pane += 1;
        let opts = cx.core.markdown_options();
        cx.present_transcript(move |registry| {
            let mut target = None;
            let mut entries = Vec::new();
            for history in std::mem::take(&mut loaded.history) {
                if history.context().is_some_and(|context| context.target == agent::ContextTarget::Message { seq }) {
                    target = Some(entries.len());
                    entries.push(crate::model::Entry::Info(format!("▸ matched message · seq {seq}")));
                }
                entries.extend(crate::model::from_history(history, &opts, registry));
            }
            if target.is_none() {
                entries.insert(0, crate::model::Entry::Info(format!("message {seq} is not on this page (removed or hidden); showing the messages around it")));
            }
            (TranscriptPresentation { session, loaded, entries, anchor: Some((target.unwrap_or(0), 0)), read_only: true },
             Msg::Presented(state::Inspect { session, seq, found: target.is_some() }))
        });
    }
    fn search_key(
        &mut self,
        code: KeyCode,
        width: usize,
        area: usize,
        tx: &mut Cx<'_, '_, Msg>,
    ) {
        use state::{Act, FOOTER, Focus};
        let compact = area < 4;
        // A preview owns the keys while up; Esc pops it.
        if let Some(act) = self.state.preview_key(code, width, area) {
            match code {
                KeyCode::Char('o') => {
                    if let Some(a) = self.state.preview_artifact() {
                        open_path(tx, a.path.clone());
                    }
                }
                KeyCode::Char('e') => {
                    if let Some(a) = self.state.preview_artifact() {
                        export(tx, a.id.clone(), a.name.clone());
                    }
                }
                _ => {}
            }
            self.search_act(act, tx);
            return;
        }
        let focus = self.state.focus;
        let act = match (code, focus) {
            (KeyCode::Esc, _) => {
                self.close(tx);
                return;
            }
            (KeyCode::Tab, Focus::Query) => {
                self.state.focus = Focus::Filters(0);
                Act::None
            }
            (KeyCode::Tab, Focus::Filters(_)) => {
                self.state.focus = Focus::Results;
                Act::None
            }
            (KeyCode::Tab, Focus::Results) => {
                self.state.focus = Focus::Footer(0);
                Act::None
            }
            (KeyCode::Tab, Focus::Footer(_)) => {
                self.state.focus = Focus::Query;
                Act::None
            }
            (KeyCode::BackTab, Focus::Query) => {
                self.state.focus = Focus::Footer(0);
                Act::None
            }
            (KeyCode::BackTab, Focus::Filters(_)) => {
                self.state.focus = Focus::Query;
                Act::None
            }
            (KeyCode::BackTab, Focus::Results) => {
                self.state.focus = Focus::Filters(0);
                Act::None
            }
            (KeyCode::BackTab, Focus::Footer(_)) => {
                self.state.focus = Focus::Results;
                Act::None
            }
            (KeyCode::Up, _) => {
                self.state.move_cursor(-1);
                Act::None
            }
            (KeyCode::Down, _) => {
                self.state.move_cursor(1);
                Act::None
            }
            (KeyCode::PageUp, _) => {
                self.state.move_cursor(-(area.max(1) as isize));
                Act::None
            }
            (KeyCode::PageDown, _) => {
                self.state.move_cursor(area.max(1) as isize);
                Act::None
            }
            (KeyCode::Left, Focus::Filters(i)) => {
                self.state.focus = Focus::Filters(i.saturating_sub(1));
                Act::None
            }
            (KeyCode::Right, Focus::Filters(i)) => {
                self.state.focus = Focus::Filters((i + 1).min(state::N));
                Act::None
            }
            (KeyCode::Char(' ') | KeyCode::Enter, Focus::Filters(i)) => {
                Act::Send(self.state.toggle(i))
            }
            (KeyCode::Left, Focus::Footer(i)) => {
                self.state.focus = Focus::Footer(i.saturating_sub(1));
                Act::None
            }
            (KeyCode::Right, Focus::Footer(i)) => {
                self.state.focus = Focus::Footer((i + 1).min(FOOTER.len() - 1));
                Act::None
            }
            (KeyCode::Char(' ') | KeyCode::Enter, Focus::Footer(0)) => {
                Act::Send(self.state.refresh())
            }
            (KeyCode::Char(' ') | KeyCode::Enter, Focus::Footer(_)) => {
                self.state.previews.push(state::Preview::Help);
                Act::None
            }
            (KeyCode::Enter, Focus::Query | Focus::Results) if self.mode == Some(Mode::Attach) => {
                match self.state.pick() {
                    Some(r) => {
                        tx.core(CoreAction::HoldReference(r));
                        self.close(tx);
                        Act::None
                    }
                    // A `show more` row or nothing selectable: the ordinary key.
                    None => self.state.enter(),
                }
            }
            (KeyCode::Enter, Focus::Query | Focus::Results) => self.state.enter(),
            _ => Act::None,
        };
        self.state.ensure_visible(area, compact);
        self.search_act(act, tx);
    }

    /// Carry out what a search key asked.
    fn search_act(&mut self, act: state::Act, tx: &mut Cx<'_, '_, Msg>) {
        match act {
            state::Act::None => {}
            state::Act::Send(reqs) => {
                requests(tx, reqs);
            }
            // A title hit or a recent row: an ordinary session switch (49 §7).
            // The current session needs no reload; the menu just closes.
            state::Act::Resume(id) => {
                self.close(tx);
                if Some(id) != tx.core.session || tx.core.inspecting {
                    tx.core(CoreAction::ResumeSession(id));
                }
            }
        }
    }

}
impl Component for SearchAttach {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "search-attach", interests: &[
            Interest { topic: "knowledge.changed", version: 1, variants: &[] },
            Interest { topic: "artifact.shown", version: 1, variants: &[] },
            Interest { topic: "agent.events", version: 1, variants: &["TitleChanged", "SessionDeleted", "TurnFinished"] },
        ], slash: &[
            ("/search", "[query]", "search sessions, artifacts, knowledge and history (live; fzf syntax)"),
            ("/attach", "[query]", "attach a note, concept, artifact, chat or meeting to the next message (66)"),
        ], bindings: &[] }
    }
    fn decode(&self, event: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && event.version() != Some(1) { return None; }
        match (event.topic(), event.event()["type"].as_str()) {
            (Some("knowledge.changed") | None, Some("KnowledgeChanged")) => {
                ::knowledge::KnowledgeChanged::deserialize(event.event()).ok()?; Some(Msg::KnowledgeChanged)
            }
            (Some("artifact.shown") | None, Some("ArtifactShown")) => {
                artifact::ArtifactShown::deserialize(event.event()).ok()?; Some(Msg::ArtifactsChanged)
            }
            (Some("agent.events") | None, _) => match core.agent? {
                agent::AgentEvent::TitleChanged { .. } | agent::AgentEvent::SessionDeleted { .. } | agent::AgentEvent::TurnFinished { .. } => Some(Msg::SessionsChanged),
                _ => None,
            },
            _ => None,
        }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { name, args } => self.open(if name == "/attach" { Mode::Attach } else { Mode::Search }, args, cx),
            Input::Message(Msg::Edited { pane, value }) if self.mode.is_some() && pane == self.pane => self.state.edit(value, crate::model::now_ms()),
            Input::Paste { text, caret } if self.mode.is_some() => {
                cx.consume_key();
                if self.state.focus == state::Focus::Query && self.state.previews.is_empty() {
                    let flat = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
                    let mut value = self.state.query.clone();
                    let at = caret.min(value.len());
                    let at = if value.is_char_boundary(at) { at } else { value.len() };
                    value.insert_str(at, &flat);
                    self.state.edit(value, crate::model::now_ms());
                }
            }
            Input::Key(key) if self.mode.is_some() => {
                cx.consume_key();
                let (width, area, _, _) = ui::layout(&self.state, cx.core.width as u16, cx.core.viewport);
                self.search_key(key.code, width, area, cx);
            }
            Input::Key(key) if self.inspect.is_some() => {
                cx.consume_key();
                let step = match key.code {
                    KeyCode::PageUp => Some(-(cx.core.viewport as isize)),
                    KeyCode::PageDown => Some(cx.core.viewport as isize),
                    KeyCode::Up => Some(-1), KeyCode::Down => Some(1), _ => None,
                };
                if let Some(delta) = step {
                    cx.core(CoreAction::Scroll { delta, rows: cx.core.viewport, width: cx.core.width });
                } else {
                    match key.code {
                        KeyCode::Enter | KeyCode::Char('j') => self.jump_latest(cx),
                        KeyCode::Esc | KeyCode::Char('b') | KeyCode::Backspace => self.back_to_results(cx),
                        _ => {}
                    }
                }
            }
            Input::Message(Msg::Reply { generation, reply }) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                for line in state::print_lines(&reply) { cx.plain(Plain::Line(line)); }
                if self.mode.is_some() {
                    let now = crate::model::now_ms();
                    match self.state.reply(generation, reply, now) {
                        state::Effect::None => {},
                        state::Effect::Send(reqs) => requests(cx, reqs),
                        state::Effect::Artifact(load) => {
                            let body = artifacts_apps::artifact_text(&load.artifact, load.bytes.as_deref().map_err(String::as_str), &cx.core.markdown_options());
                            self.state.previews.push(state::Preview::Artifact { artifact: Box::new(load.artifact), body: Box::new(body), matched: load.matched, panel: false, scroll: 0 });
                        }
                        state::Effect::Inspect { session, seq, loaded } => self.inspect_page(session, seq, loaded, cx),
                    }
                    if self.mode.is_some() { requests(cx, self.state.tick(now)); }
                }
                if self.wake().is_none() { cx.plain(Plain::Finished); }
            }
            Input::Message(Msg::Presented(inspect)) => { self.mode = None; self.inspect = Some(inspect); cx.close(); }
            Input::Message(Msg::Restore) => self.restore(cx),
            Input::Message(Msg::KnowledgeChanged) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                let now = crate::model::now_ms();
                self.state.invalidate(::search::SearchCategory::Knowledge, self.mode.is_some(), now);
                self.state.invalidate(::search::SearchCategory::KnowledgeHistory, self.mode.is_some(), now);
            }
            Input::Message(Msg::ArtifactsChanged) => self.state.invalidate(::search::SearchCategory::Artifacts, self.mode.is_some(), crate::model::now_ms()),
            Input::Message(Msg::SessionsChanged) => self.state.invalidate(::search::SearchCategory::Sessions, self.mode.is_some(), crate::model::now_ms()),
            Input::Message(Msg::Action(Ok(Some(text)))) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text));
            }
            Input::Message(Msg::Action(Ok(None))) => {}
            Input::Message(Msg::Action(Err(error))) => cx.error(error),
            Input::Tick { now_ms, .. } if self.mode.is_some() => requests(cx, self.state.tick(now_ms)),
            Input::Lifecycle(Lifecycle::FocusLost) => { self.mode = None; self.pane += 1; }
            Input::Lifecycle(Lifecycle::SessionReset) => { self.mode = None; self.inspect = None; self.pane += 1; cx.close(); }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Modal if self.mode.is_some() => {
                let pane = self.pane;
                let editor = if self.state.previews.is_empty() {
                    Some(EditorConfig { value: self.state.query.clone(), focused: self.state.focus == state::Focus::Query,
                        on_change: Box::new(cx.callback(move |value| Msg::Edited { pane, value })?) })
                } else { None };
                Some(ui::search_box(&self.state, self.mode == Some(Mode::Attach), editor, cx.width as u16, cx.height))
            }
            Surface::PromptBadge => self.inspect.as_ref().map(ui::inspect_hint),
            _ => None,
        }
    }
    fn captures(&self, class: FocusClass) -> bool { class == FocusClass::Content && self.inspect.is_some() }
    fn separate_editor(&self) -> bool { self.mode.is_some() }
    fn wake(&self) -> Option<u64> { self.mode.and_then(|_| self.state.wake()) }
}

async fn search_request(api: &Engine, kind: state::RequestKind) -> state::Reply {
    use state::{Reply, RequestKind};
    match kind {
        RequestKind::Results {
            query,
            categories,
            limit,
        } => Reply::Results {
            result: api
                .search(query, Some(categories.clone()), Some(limit))
                .await
                .map_err(crate::reason),
            categories,
            limit,
        },
        RequestKind::Recent => Reply::Recent(api.list_sessions().await.map_err(crate::reason)),
        RequestKind::Refresh(categories) => {
            Reply::Refreshed(api.search_refresh(Some(categories)).await.map_err(crate::reason))
        }
        RequestKind::Artifact { call_id, query } => {
            // A search hit is durable: a private tree is never indexed.
            let artifact = match api.get_artifact(call_id.clone(), None).await {
                Ok(a) => a,
                Err(e) => return Reply::Artifact(Err(crate::reason(e))),
            };
            // Text-like kinds (and an archive's listing sidecar) are read here,
            // off the UI thread (49 §6.2).
            let bytes = match artifacts_apps::artifact_read_path(&artifact) {
                Some(p) => tokio::fs::read(p).await.map_err(|e| e.to_string()),
                None => Ok(Vec::new()),
            };
            let matched = match query {
                Some(q) => Some(api.artifact_search_match(call_id, q).await.map_err(crate::reason)),
                None => None,
            };
            Reply::Artifact(Ok(state::ArtifactLoad {
                artifact,
                bytes,
                matched,
            }))
        }
        RequestKind::Concept(id) => Reply::Concept(api.knowledge_get(id).await.map_err(crate::reason)),
        RequestKind::Historical { id, commit } => Reply::Historical(
            api.knowledge_read_revision(id, commit)
                .await
                .map_err(crate::reason),
        ),
        RequestKind::Revision { commit, id } => Reply::Revision(
            api.knowledge_revision(commit, Some(id))
                .await
                .map_err(crate::reason),
        ),
        RequestKind::Page { session, seq } => Reply::Page {
            session,
            seq,
            result: api
                .load_session(
                    session,
                    Some(seq.saturating_add(1)),
                    Some(state::PAGE),
                    Some(false),
                )
                .await
                .map_err(crate::reason),
        },
    }
}
