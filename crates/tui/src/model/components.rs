use super::*;
use crate::components::{CoreAction, CoreView, FocusClass, Lifecycle, Outbox, Plain, Surface, View, ViewCx};
use crate::topics::TopicEvent;

/// Printer receives original replies before reduction, not appended transcript rows.
pub(crate) enum Output<'a> { Event(&'a Model, &'a UiEvent), Plain(&'a Plain) }

impl Model {
    /// Fold a topic, component reply or residual core event into the UI state.
    pub fn apply(&mut self, event: UiEvent) { self.receive(event, &mut |_| {}); }

    pub(crate) fn receive(&mut self, event: UiEvent, emit: &mut impl FnMut(Output<'_>)) {
        let (residual, out) = match event {
            UiEvent::Wire(event) => {
                let residual = crate::topics::residual(&event);
                let core = CoreView { session: self.session, project: self.project.as_deref(),
                inspecting: self.inspect.is_some(), streaming: self.streaming,
                reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })),
                agent: match &residual { Some(UiEvent::Agent(event)) => Some(event), _ => None }, entries: &self.entries };
                let out = self.components.wire(&event, &core);
                self.stream_control(&event, emit);
                (residual, out)
            }
            UiEvent::Component(reply) => {
                let core = CoreView { session: self.session, project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
                (None, self.components.reply(reply, &core))
            }
            event => {
                let lifecycle = match &event {
                    UiEvent::Engine(_) => Some(Lifecycle::EngineChanged),
                    UiEvent::Error(error) => Some(Lifecycle::CommandFailed(error)),
                    _ => None,
                };
                let out = lifecycle.map_or_else(Outbox::default, |event| self.component_lifecycle(event));
                (Some(event), out)
            }
        };
        for plain in &out.plain { emit(Output::Plain(plain)); }
        if let Some(event) = residual {
            emit(Output::Event(self, &event));
            self.reduce(event);
        }
        self.component_finish(out, emit);
    }

    fn component_finish(&mut self, out: Outbox, emit: &mut impl FnMut(Output<'_>)) {
        for effect in out.effects {
            if let Some(commands) = &self.component_commands {
                let _ = commands.send(Command::Component(effect));
            }
        }
        for action in out.actions {
            match action {
                CoreAction::Notice(text) => self.entries.push(Entry::Info(text)),
                CoreAction::Interject(text) => self.interject(Entry::Info(text)),
                CoreAction::Error(text) => self.receive(UiEvent::Error(text), emit),
                CoreAction::Reject(text) => self.entries.push(Entry::Error(text)),
            }
        }
    }

    fn component_lifecycle(&mut self, event: Lifecycle<'_>) -> Outbox {
        let core = CoreView { session: self.session, project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        self.components.lifecycle(event, &core)
    }
    pub(crate) fn component_notify(&mut self, event: Lifecycle<'_>) {
        let out = self.component_lifecycle(event);
        self.component_finish(out, &mut |_| {});
    }
    pub(crate) fn mount_components(&mut self, commands: &UnboundedSender<Command>) {
        self.component_commands = Some(commands.clone());
        self.component_notify(Lifecycle::Mount);
    }
    pub(crate) fn component_tick(&mut self, now_ms: u64) {
        self.component_tick_with(now_ms, &mut |_| {});
    }
    pub(crate) fn component_tick_with(&mut self, now_ms: u64, emit: &mut impl FnMut(Output<'_>)) {
        let core = CoreView { session: self.session, project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        let out = self.components.tick(now_ms, self.frame, &core);
        for plain in &out.plain { emit(Output::Plain(plain)); }
        self.component_finish(out, emit);
    }
    pub(crate) fn component_key(&mut self, class: FocusClass, key: &iocraft::KeyEvent) -> bool {
        let core = CoreView { session: self.session, project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        let out = self.components.key(class, key, &core);
        let consumed = out.consumed();
        self.component_finish(out, &mut |_| {});
        consumed
    }
    pub(crate) fn component_paste(&mut self, text: &str) -> bool {
        let core = CoreView { session: self.session, project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        let out = self.components.paste(text, &core);
        let consumed = out.consumed();
        self.component_finish(out, &mut |_| {});
        consumed
    }
    pub(super) fn component_submit(&mut self, text: &str) -> bool {
        let core = CoreView { session: self.session, project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        let out = self.components.submit(text, &core);
        let consumed = out.consumed();
        self.component_finish(out, &mut |_| {});
        consumed
    }
    pub(super) fn component_slash(&mut self, name: &str, args: &str) -> bool {
        let core = CoreView { session: self.session, project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        let Some(out) = self.components.slash(name, args, &core) else { return false };
        self.component_finish(out, &mut |_| {});
        true
    }
    pub(crate) fn component_views(&self, surface: Surface<'_>, width: usize, height: usize) -> Vec<View> {
        let cx = ViewCx {
            core: CoreView { session: self.session, project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries },
            width, height, frame: self.frame,
        };
        self.components.views(surface, &cx).collect()
    }
    pub(crate) fn component_text(&self, surface: Surface<'_>) -> String {
        let cx = ViewCx {
            core: CoreView { session: self.session, project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries },
            width: self.width, height: 0, frame: self.frame,
        };
        let mut text = String::new();
        for view in self.components.views(surface, &cx) {
            if let View::Text(segment) = view { text.push_str(&segment); }
        }
        text
    }

    fn stream_control(&mut self, event: &TopicEvent, emit: &mut impl FnMut(Output<'_>)) {
        if !event.is_control() { return; }
        let value = event.event();
        match value["type"].as_str() {
            Some("PendingPrompts") => {
                // Replace even empty sets; repeat live Requested messages are idempotent.
                // Secrets remain in the residual owner until its own migration.
                if let Some(rows) = value["approvals"].as_array() {
                    self.approvals.clear();
                    for row in rows {
                        if let Ok(request) = <ApprovalRequested as serde::Deserialize>::deserialize(row) {
                            let event = UiEvent::Approval(request);
                            emit(Output::Event(self, &event));
                            self.reduce(event);
                        }
                    }
                }
                if let Some(rows) = value["asks"].as_array() {
                    self.asks.clear();
                    for row in rows {
                        if let Ok(request) = <AskRequested as serde::Deserialize>::deserialize(row) {
                            let event = UiEvent::Ask(request);
                            emit(Output::Event(self, &event));
                            self.reduce(event);
                        }
                    }
                }
                if let Some(rows) = value["secrets"].as_array() {
                    self.secret_requests.clear();
                    for row in rows {
                        if let Ok(request) = <secrets::SecretsEvent as serde::Deserialize>::deserialize(row) {
                            let event = UiEvent::SecretsEvent(request);
                            emit(Output::Event(self, &event));
                            self.reduce(event);
                        }
                    }
                }
                let approvals = &self.approvals;
                let asks = &self.asks;
                self.double.retain(|id, _| approvals.iter().any(|a| a.id.0.to_string() == *id) || asks.iter().any(|a| a.req.id.0.to_string() == *id));
                self.component_notify(Lifecycle::Prompts(value));
            }
            Some("Lagged") => {
                // Never overwrite an inspected historical page or clear the draft.
                if self.inspect.is_none() { self.refresh_context = self.session.is_some(); }
                if let (Some(session), Some(commands)) = (self.session, &self.component_commands) {
                    let _ = commands.send(Command::ListAgents(session));
                }
                self.refresh_cost = self.session.is_some();
                self.refresh_artifacts = self.session.is_some();
                self.refresh_advisor = self.session.is_some();
                self.refresh_communication = true;
                self.refresh_link = self.link.is_some();
                self.refresh_nodes = self.nodes.is_some();
                self.refresh_secrets = self.secrets.is_some();
                self.component_notify(Lifecycle::Resync);
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[test]
fn component_plain_output_precedes_core_actions_and_errors_keep_global_effects() {
    use crate::components::tests::{Counter, Msg, reply, topic};
    use serde_json::json;
    let mut model = Model::new("model".into(), true);
    let token = model.components.register(Counter::new("counter")).unwrap();
    let mut bytes = Vec::new();
    model.receive(UiEvent::Wire(topic("test.counter", 1, json!({"type":"Add","value":4}))), &mut |output| {
        assert_eq!(crate::print_output(&mut bytes, output), (false, false));
    });
    assert_eq!(bytes, b"counter: 0 -> 4\n");
    assert!(matches!(&model.entries[..], [Entry::Info(text)] if text == "counter=4"));
    model.compacting = true;
    model.awaiting_cost = true;
    model.awaiting_link = true;
    let mut completed = false;
    model.receive(UiEvent::Component(reply(token, 0, Msg::Fail("request failed".into()))), &mut |output| {
        completed |= crate::print_output(&mut bytes, output).0;
    });
    assert!(completed);
    assert!(!model.compacting && !model.awaiting_cost && !model.awaiting_link);
    assert!(matches!(model.entries.last(), Some(Entry::Error(text)) if text == "request failed"));
}
