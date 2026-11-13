//! Descendant activity presentation and /agents picker; membership stays core.
use super::*;
use agent::{AgentEvent, AgentInfo, AgentState, ObservedSource, ObservedState};
use crate::model::{ago, describe, elapsed, observed_word, short};
use iocraft::prelude::{element, BorderStyle, FlexDirection, Text, TextWrap, View as Layout};
use rpc_compose::api::clients::*;
use serde::Deserialize;
use std::collections::HashMap;

const BOARD_ROWS: usize = 6;
const WATCH_GLYPH: &str = "◎";

#[derive(Default)]
pub(crate) struct AgentsBoard {
    pub(crate) rows: HashMap<SessionId, RowState>,
    pub(crate) menu: Option<(Vec<MenuRow>, usize)>,
    pub(crate) picker_pending: bool,
}
#[derive(Clone, Default)]
pub(crate) struct RowState {
    tool: Option<String>,
    question: Option<String>,
    pub(crate) error: Option<String>,
    fresh: bool,
    live: bool,
    approval: bool,
}
static EMPTY_ROW_STATE: RowState = RowState { tool: None, question: None, error: None, fresh: false, live: false, approval: false };
pub(crate) struct MenuRow {
    pub(crate) info: AgentInfo,
    state: RowState,
}
impl MenuRow {
    pub(crate) fn view(&self) -> AgentRow<'_> { AgentRow { info: &self.info, state: &self.state } }
}
#[derive(Clone, Copy)]
pub(crate) struct AgentRow<'a> {
    pub(crate) info: &'a AgentInfo,
    state: &'a RowState,
}
impl AgentRow<'_> {
    fn live(&self) -> bool { live(self.info) }
    fn on_board(&self) -> bool { self.live() || self.state.fresh }
}
fn live(info: &AgentInfo) -> bool {
    matches!(info.state, AgentState::Running | AgentState::Awaiting { .. } | AgentState::Approval { .. })
}
#[derive(Clone, Debug)]
pub(crate) enum Request { ListAgents(SessionId) }
pub(crate) enum Msg {
    Listed(Result<Vec<AgentInfo>, String>),
    Reseed,
    Observe(Observation),
    Applied(Observation),
}
pub(crate) enum Observation {
    Spawn { parent: SessionId, child: SessionId },
    Observed { child: SessionId, fresh: bool },
    Started(SessionId),
    Pending { session: SessionId, tool: String },
    Call { session: SessionId, tool: String, question: Option<String>, awaiting: bool },
    ToolFinished(SessionId),
    Finished { session: SessionId, cancelled: bool, fresh: bool },
    Error { session: SessionId, message: String },
    Approval { session: SessionId, tool: String },
}
impl AgentsBoard {
    fn request(&self, session: SessionId, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, Request::ListAgents(session), |api, request| async move {
            let Request::ListAgents(session) = request;
            Msg::Listed(api.list_agents(session).await.map_err(crate::reason))
        });
    }
    pub(crate) fn row<'a>(&'a self, info: &'a AgentInfo) -> AgentRow<'a> {
        AgentRow { info, state: self.rows.get(&info.id).unwrap_or(&EMPTY_ROW_STATE) }
    }
    pub(crate) fn board<'a>(&'a self, members: &'a dyn SessionMembers) -> Vec<AgentRow<'a>> {
        let mut rows = Vec::new();
        members.visit(&mut |info| {
            let row = self.row(info);
            if row.on_board() { rows.push(row); }
        });
        rows.sort_by_key(|row| rank(row.info));
        rows
    }
    fn refresh(&mut self, members: &dyn SessionMembers) {
        members.visit(&mut |info| {
            let state = self.rows.entry(info.id).or_default();
            let approval = matches!(info.state, AgentState::Approval { .. });
            if state.approval && !approval { state.tool = None; }
            state.approval = approval;
            state.live = live(info);
        });
    }
    fn reseed(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        self.rows.retain(|id, _| cx.core.members.agent_info(*id).is_some());
        let pending = std::mem::take(&mut self.picker_pending);
        let mut items = Vec::new();
        cx.core.members.visit(&mut |info| {
            let state = self.rows.entry(info.id).or_default();
            state.live = live(info);
            state.approval = matches!(info.state, AgentState::Approval { .. });
            if pending {
                state.fresh = false;
                items.push(MenuRow { info: info.clone(), state: state.clone() });
            }
        });
        if !pending { return; }
        if items.is_empty() { cx.core(CoreAction::Notice("no subagents".into())); }
        else {
            items.sort_by_key(|row| rank(&row.info));
            self.menu = Some((items, 0));
            cx.focus(FocusClass::Modal);
        }
    }
    fn applied(&mut self, observation: Observation, cx: &mut Cx<'_, '_, Msg>) {
        let session = match &observation {
            Observation::Spawn { child, .. } | Observation::Observed { child, .. } => *child,
            Observation::Started(session) | Observation::ToolFinished(session) => *session,
            Observation::Pending { session, .. } | Observation::Call { session, .. } |
            Observation::Finished { session, .. } | Observation::Error { session, .. } |
            Observation::Approval { session, .. } => *session,
        };
        let Some(info) = cx.core.members.agent_info(session) else { return; };
        let direct = cx.core.session == Some(info.parent);
        let state = self.rows.entry(session).or_default();
        match observation {
            Observation::Spawn { parent, .. } => {
                *state = RowState::default();
                if cx.core.session == Some(parent) { cx.interject(format!("↳ spawned {}", info.name)); }
            }
            Observation::Observed { fresh, .. } => { state.fresh = fresh; state.tool = None; }
            Observation::Started(_) => { state.tool = None; state.fresh = false; }
            Observation::Pending { tool, .. } | Observation::Approval { tool, .. } => state.tool = Some(tool),
            Observation::Call { tool, question, awaiting, .. } => {
                if awaiting { state.question = question; }
                state.tool = Some(tool);
            }
            Observation::ToolFinished(_) => state.tool = None,
            Observation::Finished { cancelled, fresh, .. } => {
                state.fresh = fresh; state.tool = None;
                if direct && cancelled { cx.interject(format!("↳ {} cancelled", info.name)); }
            }
            Observation::Error { message, .. } => {
                if direct { cx.core(CoreAction::InterjectError(format!("↳ {} error: {message}", info.name))); }
                state.error = Some(message);
            }
        }
        state.live = live(info);
        state.approval = matches!(info.state, AgentState::Approval { .. });
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        let Some((items, cursor)) = &mut self.menu else { return; };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => { self.menu = None; cx.close(); }
            KeyCode::Enter => {
                let picked = items.get(*cursor).map(|row| row.info.id);
                self.menu = None; cx.close();
                if let Some(id) = picked { cx.core(CoreAction::ResumeSession(id)); }
            }
            _ => {}
        }
    }
}
impl Component for AgentsBoard {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "agents-board", interests: &[
            Interest { topic: "agent.events", version: 1, variants: &["SubagentSpawned", "ObservedChildChanged", "TurnStarted", "ToolCallPending", "ToolCallStarted", "ToolCallFinished", "TurnFinished", "Error"] },
            Interest { topic: "tools.approval-requested", version: 1, variants: &["ApprovalRequested"] },
        ], slash: &[("/agents", "", "subagents of this session (picker: ⏎ opens one)")], bindings: &[] }
    }
    fn decode(&self, event: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && event.version() != Some(1) { return None; }
        if matches!(event.topic(), Some("tools.approval-requested") | None) && event.event()["type"] == "ApprovalRequested" {
            let request = tools::ApprovalRequested::deserialize(event.event()).ok()?;
            return Some(Msg::Observe(Observation::Approval { session: request.session, tool: format!("{} {}", request.tool, describe(&request.tool, &request.args).join(" ")) }));
        }
        if !matches!(event.topic(), Some("agent.events") | None) || !core.descendant_event { return None; }
        let observation = match core.agent? {
            AgentEvent::SubagentSpawned { parent, child, .. } => Observation::Spawn { parent: *parent, child: *child },
            AgentEvent::ObservedChildChanged { child, observed, .. } => Observation::Observed { child: *child, fresh: matches!(observed.state, ObservedState::Failed | ObservedState::Cancelled | ObservedState::Unknown) },
            AgentEvent::TurnStarted { session, .. } => Observation::Started(*session),
            AgentEvent::ToolCallPending { session, name, .. } => Observation::Pending { session: *session, tool: format!("preparing {name}") },
            AgentEvent::ToolCallStarted { session, name, args, .. } => {
                let awaiting = name == "send_message" && args["await_reply"] == true;
                Observation::Call { session: *session, tool: match first_arg(args) {
                    Some(arg) => format!("running {name} {arg}"), None => format!("running {name}"),
                }, question: if awaiting { args["message"].as_str().and_then(|m| m.lines().next()).map(str::to_owned) } else { None }, awaiting }
            }
            AgentEvent::ToolCallFinished { session, .. } => Observation::ToolFinished(*session),
            AgentEvent::TurnFinished { session, stop_reason, .. } => Observation::Finished { session: *session, cancelled: stop_reason == "cancelled", fresh: matches!(stop_reason.as_str(), "error" | "cancelled") },
            AgentEvent::Error { session, message, .. } => Observation::Error { session: *session, message: message.clone() },
            _ => return None,
        };
        Some(Msg::Observe(observation))
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Lifecycle(Lifecycle::SessionRefreshRequested) => {
                if let Some(session) = cx.core.session { self.request(session, cx); }
            }
            Input::Lifecycle(Lifecycle::SessionReset) => {
                self.rows.clear(); self.menu = None; self.picker_pending = false; cx.close();
            }
            Input::Lifecycle(Lifecycle::FocusLost) => self.menu = None,
            Input::Lifecycle(Lifecycle::PromptChanged { .. }) => self.refresh(cx.core.members),
            Input::Lifecycle(Lifecycle::Prompts(value)) => {
                if let Some(requests) = value["approvals"].as_array() {
                    for request in requests {
                        if let Ok(request) = tools::ApprovalRequested::deserialize(request) {
                            self.applied(Observation::Approval { session: request.session, tool: format!("{} {}", request.tool, describe(&request.tool, &request.args).join(" ")) }, cx);
                        }
                    }
                }
            }
            Input::Slash { name: "/agents", .. } => match cx.core.session {
                Some(session) => { self.picker_pending = true; self.request(session, cx); }
                None => cx.core(CoreAction::Notice("no session".into())),
            },
            Input::Message(Msg::Observe(observation)) => cx.post(Msg::Applied(observation)),
            Input::Message(Msg::Applied(observation)) => self.applied(observation, cx),
            Input::Message(Msg::Listed(Ok(list))) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                for info in &list { cx.plain(Plain::Line(format!("{}  {}  {}  {}", info.name, info.state.word(), info.last, short(info.id)))); }
                cx.core(CoreAction::Descendants(list)); cx.post(Msg::Reseed);
            }
            Input::Message(Msg::Listed(Err(error))) => cx.error(error),
            Input::Message(Msg::Reseed) => self.reseed(cx),
            Input::Key(key) if self.menu.is_some() => { self.key(key.code, cx); cx.consume_key(); }
            _ => {}
        }
    }
    fn animating(&self) -> bool { self.rows.values().any(|state| state.live) }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Status(StatusSlot::BeforeQueue) => {
                let (mut count, mut visible) = (0, false);
                cx.core.members.visit(&mut |info| { count += 1; visible |= self.row(info).on_board(); });
                Some(View::Text(if count == 0 || visible { String::new() } else { format!("  agents: {count} idle") }))
            }
            Surface::BeforePinned => {
                let rows = self.board(cx.core.members);
                if rows.is_empty() { return None; }
                let height = 3 + rows.len().min(BOARD_ROWS) + usize::from(rows.len() > BOARD_ROWS);
                Some(View::IntrinsicElement { element: board_view(rows, cx), rows: height, replace: false })
            }
            Surface::Modal => {
                let (items, cursor) = self.menu.as_ref()?;
                let rows = items.iter().map(|row| board_row(row.view(), cx.spinner(), cx.width as u16, cx.session_badge(row.info.id))).collect();
                Some(View::IntrinsicElement { element: crate::ui::menu_view("agents  ⏎ open  esc close  (the one you leave keeps running)".into(), rows, *cursor, cx.width as u16, cx.height as u16, false), rows: 0, replace: true })
            }
            _ => None,
        }
    }
}
pub(crate) fn board_row(row: AgentRow<'_>, spinner: &str, width: u16, tag: String) -> String {
    let fit = width.saturating_sub(u16::try_from(super::search_attach::state::cells(&tag)).unwrap_or(u16::MAX));
    format!("{}{tag}", agent_row(row, spinner, fit))
}
fn board_view(rows: Vec<AgentRow<'_>>, cx: &ViewCx<'_>) -> iocraft::AnyElement<'static> {
    let more = rows.len().saturating_sub(BOARD_ROWS);
    let fit = cx.width.saturating_sub(5);
    element! {
        Layout(border_style: BorderStyle::Round, border_color: crate::ui::SAND, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1) {
            Text(content: "agents", color: crate::ui::SAND)
            #(rows.iter().take(BOARD_ROWS).map(|row| {
                let row = board_row(*row, cx.spinner(), cx.width as u16, cx.session_badge(row.info.id));
                let (glyph, rest) = split_chars(&row, 2);
                let (name, rest) = split_chars(rest, 15);
                element! {
                    Layout(flex_direction: FlexDirection::Row) {
                        Text(content: glyph.to_owned(), color: crate::ui::BONE_DIM, wrap: TextWrap::NoWrap)
                        Text(content: name.to_owned(), color: crate::ui::FAIENCE, wrap: TextWrap::NoWrap)
                        Text(content: rest.chars().take(fit.saturating_sub(17)).collect::<String>(), color: crate::ui::BONE_DIM, wrap: TextWrap::NoWrap)
                    }
                }
            }))
            #((more > 0).then(|| element!(Text(content: format!("… {more} more (/agents)"), color: crate::ui::SAND))))
        }
    }.into_any()
}
fn split_chars(s: &str, n: usize) -> (&str, &str) {
    s.split_at(s.char_indices().nth(n).map_or(s.len(), |(i, _)| i))
}

fn rank(a: &AgentInfo) -> (u8, i64, u128) {
    let ms = |t: Option<chrono::DateTime<chrono::Utc>>| t.map_or(0, |t| t.timestamp_millis());
    let id = a.id.0.as_u128();
    match a.state {
        AgentState::Running => (0, ms(a.started_at), id),
        AgentState::Awaiting { .. } | AgentState::Approval { .. } => (1, ms(a.started_at), id),
        _ => (2, -ms(a.finished_at), id),
    }
}

pub(crate) fn agent_row(a: AgentRow<'_>, spinner: &str, width: u16) -> String {
    // 141 §10.1: a watch-only child is `◎ name <observed state> watch · <src> [lastIntent]`.
    let (glyph, word) = match &a.info.observed {
        Some(o) => (WATCH_GLYPH, observed_word(o.state)),
        None => (
            crate::components::broadcast::state::glyph(&a.info.state, spinner),
            a.info.state.word(),
        ),
    };
    let name = match a.info.depth {
        0 | 1 => a.info.name.clone(),
        d => format!("{}↳ {}", " ".repeat(2 * usize::from(d - 1)), a.info.name),
    };
    let now = chrono::Utc::now();
    let age = match (a.info.started_at, a.info.finished_at) {
        (Some(t), _) if a.live() => elapsed(now - t),
        (_, Some(t)) => ago(now - t),
        _ => String::new(),
    };
    let active = match crate::components::broadcast::state::age(a.info.activity.last_active) {
        s if s.is_empty() => s,
        s => format!(" ·{s}"),
    };
    let tail = format!(
        "  {age}{}{active}",
        crate::components::broadcast::state::counts(&a.info.activity)
    );
    // Glyph, name, state and their gaps take 27 columns, the picker frame 7; the detail gets
    // what is left after the measured tail (141 R4 T13b: an estimate let long ages overflow).
    let room = usize::from(width)
        .saturating_sub(34 + crate::components::search_attach::state::cells(&tail))
        .max(16);
    let detail = match (&a.info.observed, &a.info.state) {
        (Some(o), _) => {
            let mut d = match o.detail["lastIntent"].as_str() {
                Some(intent) => format!("watch · {} · {intent}", source_tag(o.source)),
                None => format!("watch · {}", source_tag(o.source)),
            };
            // 141 R3 T13: the child's model when the column has room for it.
            let m = &a.info.model;
            if !m.is_empty() && d.chars().count() + 3 + m.chars().count() <= room {
                d = format!("{d} · {m}");
            }
            d
        }
        (None, AgentState::Running) => a.state.tool.clone().unwrap_or_else(|| "streaming".to_owned()),
        (None, AgentState::Awaiting { from }) => match &a.state.question {
            Some(q) => format!("{from}: \"{q}\""),
            None => from.clone(),
        },
        (None, AgentState::Approval { tool }) => a.state.tool.clone().unwrap_or_else(|| tool.clone()),
        (None, AgentState::Error) => a.state.error.clone().unwrap_or_else(|| a.info.last.clone()),
        (None, AgentState::Cancelled | AgentState::Idle) => a.info.last.clone(),
    };
    let detail = crate::components::search_attach::state::fit(&detail, room);
    format!("{glyph} {name:<14} {word:<9} {detail:<room$}{tail}")
}

fn source_tag(source: ObservedSource) -> &'static str {
    match source {
        ObservedSource::Rfd => "ACP",
        ObservedSource::OpencodeExt | ObservedSource::OpencodeTask => "opencode",
        ObservedSource::OmpTask => "omp task",
    }
}

fn first_arg(args: &serde_json::Value) -> Option<String> {
    args.as_object()?
        .values()
        .find_map(|v| v.as_str())
        .map(|s| {
            s.lines()
                .next()
                .unwrap_or_default()
                .chars()
                .take(40)
                .collect()
        })
}
