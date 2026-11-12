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
                let core = CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(),
                inspecting: self.inspect.is_some(), streaming: self.streaming,
                reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })),
                agent: match &residual { Some(UiEvent::Agent(event)) => Some(event), _ => None }, entries: &self.entries };
                let out = self.components.wire(&event, &core);
                self.stream_control(&event, emit);
                (residual, out)
            }
            UiEvent::Component(reply) => {
                let core = CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
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
            let previous_session = self.session;
            self.reduce(event);
            if self.session != previous_session {
                let changed = self.component_lifecycle(Lifecycle::SessionChanged);
                self.component_finish(changed, emit);
            }
        }
        self.component_finish(out, emit);
        self.component_reconcile_core_focus();
    }

    fn component_finish(&mut self, out: Outbox, emit: &mut impl FnMut(Output<'_>)) {
        if out.claimed.is_some_and(|class| matches!(class,
            FocusClass::Modal | FocusClass::Secret | FocusClass::Search))
        {
            self.menu = None;
            self.prompt = None;
        }
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
                CoreAction::UseSkill { name, args } => {
                    if let Some(commands) = self.component_commands.clone() {
                        self.use_skill(name, args, &commands);
                    }
                }
                CoreAction::Reject(text) => self.entries.push(Entry::Error(text)),
                CoreAction::AppendCard(card) => self.entries.push(Entry::Card {
                    kind: CardKind::Component { group: card.group, key: card.key, glyph: card.glyph, color: card.color },
                    title: card.title, text: Box::new(markdown::Text::complete(&card.body)), expanded: card.expanded,
                }),
                CoreAction::ReplaceCard { group, key, body } => {
                    if let Some(Entry::Card { kind: CardKind::Component { key: current, .. }, text, .. }) =
                        self.entries.iter_mut().rev().find(|entry| matches!(entry,
                            Entry::Card { kind: CardKind::Component { group: old, .. }, .. } if *old == group))
                        && *current == key
                    {
                        **text = markdown::Text::complete(&body);
                    }
                }
            }
        }
    }

    fn component_lifecycle(&mut self, event: Lifecycle<'_>) -> Outbox {
        let core = CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        self.components.lifecycle(event, &core)
    }
    pub(crate) fn component_notify(&mut self, event: Lifecycle<'_>) {
        let out = self.component_lifecycle(event);
        self.component_finish(out, &mut |_| {});
    }
    /// Residual modal acquisition displaces, rather than hides, a component surface.
    pub(super) fn component_reconcile_core_focus(&mut self) {
        if self.components.focused()
            && (self.prompt.is_some() || self.menu.as_ref().is_some_and(|menu| !matches!(menu, Menu::Slash { .. })))
        {
            let core = CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
            let out = self.components.release_focus(&core);
            self.component_finish(out, &mut |_| {});
        }
    }
    pub(crate) fn mount_components(&mut self, commands: &UnboundedSender<Command>) {
        self.component_commands = Some(commands.clone());
        self.component_notify(Lifecycle::Mount);
    }
    pub(crate) fn component_tick(&mut self, now_ms: u64) {
        self.component_tick_with(now_ms, &mut |_| {});
    }
    pub(crate) fn component_tick_with(&mut self, now_ms: u64, emit: &mut impl FnMut(Output<'_>)) {
        self.component_reconcile_core_focus();
        let core = CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        let out = self.components.tick(now_ms, self.frame, &core);
        for plain in &out.plain { emit(Output::Plain(plain)); }
        self.component_finish(out, emit);
    }
    pub(crate) fn component_key(&mut self, class: FocusClass, key: &iocraft::KeyEvent) -> bool {
        self.component_reconcile_core_focus();
        let core = CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        let out = self.components.key(class, key, &core);
        let consumed = out.consumed();
        self.component_finish(out, &mut |_| {});
        consumed
    }
    pub(crate) fn component_paste(&mut self, text: &str) -> bool {
        self.component_reconcile_core_focus();
        let core = CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        let out = self.components.paste(text, &core);
        let consumed = out.consumed();
        self.component_finish(out, &mut |_| {});
        consumed
    }
    pub(crate) fn component_submit(&mut self, text: &str) -> bool {
        self.component_reconcile_core_focus();
        let core = CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        let out = self.components.submit(text, &core);
        let consumed = out.consumed();
        self.component_finish(out, &mut |_| {});
        consumed
    }
    pub(super) fn component_slash(&mut self, name: &str, args: &str) -> bool {
        let core = CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries };
        let Some(out) = self.components.slash(name, args, &core) else { return false };
        self.component_finish(out, &mut |_| {});
        true
    }
    pub(crate) fn component_views(&self, surface: Surface<'_>, width: usize, height: usize) -> Vec<View> {
        let cx = ViewCx {
            core: CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries },
            width, height, frame: self.frame,
        };
        self.components.views(surface, &cx).collect()
    }
    pub(crate) fn component_text(&self, surface: Surface<'_>) -> String {
        let cx = ViewCx {
            core: CoreView { interactive: self.components.interactive(), session: self.session, parent: self.session_parent, private_root: self.private.as_ref().map(|p| p.root), project: self.project.as_deref(), inspecting: self.inspect.is_some(), streaming: self.streaming, reasoning: matches!(self.entries.last(), Some(Entry::Reasoning { done: false, .. })), agent: None, entries: &self.entries },
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
    model.components.get_mut::<crate::components::link::Link>().unwrap().awaiting = true;
    let mut completed = false;
    model.receive(UiEvent::Component(reply(token, 0, Msg::Fail("request failed".into()))), &mut |output| {
        completed |= crate::print_output(&mut bytes, output).0;
    });
    assert!(completed);
    assert!(!model.compacting && !model.awaiting_cost && !model.components.get::<crate::components::link::Link>().unwrap().awaiting);
    assert!(matches!(model.entries.last(), Some(Entry::Error(text)) if text == "request failed"));
}

#[cfg(test)]
#[test]
fn header_slots_preserve_order_across_core_and_reverse_registered_owners() {
    use crate::components::{Component, Cx, Declaration, HeaderSlot, Input, Registry, ViewCx};
    struct Header { slot: HeaderSlot, id: &'static str }
    impl Component for Header {
        type Msg = ();
        fn declaration(&self) -> Declaration {
            Declaration { id: self.id, interests: &[], slash: &[], bindings: &[] }
        }
        fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<()> { None }
        fn update(&mut self, _: Input<'_, ()>, _: &mut Cx<'_, '_, ()>) {}
        fn view(&self, at: Surface<'_>, _: &ViewCx<'_>) -> Option<View> {
            matches!(at, Surface::Header(slot) if slot == self.slot)
                .then(|| View::Text(format!("  ·  {}", self.id)))
        }
    }
    let mut model = Model::new("nobody/nothing".into(), true);
    model.approvals_off = true;
    model.components = Registry::empty();
    for (slot, id) in [
        (HeaderSlot::Update, "update"), (HeaderSlot::Nodes, "nodes"),
        (HeaderSlot::Link, "link"), (HeaderSlot::Communication, "communication"),
        (HeaderSlot::Advisor, "advisor"), (HeaderSlot::Costs, "costs"),
    ] {
        model.components.register(Header { slot, id }).unwrap();
    }
    let catalog = Catalog::load(std::env::temp_dir().join("theuth-tui-no-user-file").as_path());
    assert_eq!(model.header(&catalog).0,
        "nobody/nothing  ·  ctx 0  ·  costs  ·  advisor  ·  approvals off  ·  communication  ·  link  ·  nodes  ·  update");
}

#[cfg(test)]
#[test]
fn modal_replacement_closes_displaced_surface_without_losing_cache() {
    use crate::components::{Component, Cx, Declaration, Input, Registry};
    struct Modal { id: &'static str, open: bool, cached: usize }
    impl Component for Modal {
        type Msg = ();
        fn declaration(&self) -> Declaration {
            Declaration { id: self.id, interests: &[], bindings: &[],
                slash: if self.id == "first" { &[("/first", "", "")] } else { &[("/second", "", "")] } }
        }
        fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<()> { None }
        fn update(&mut self, input: Input<'_, ()>, cx: &mut Cx<'_, '_, ()>) {
            match input {
                Input::Slash { .. } => { self.open = true; self.cached += 1; cx.focus(FocusClass::Modal); }
                Input::Lifecycle(Lifecycle::FocusLost) => self.open = false,
                Input::Key(key) if key.code == KeyCode::Esc => {
                    self.open = false; cx.close(); cx.consume_key();
                }
                _ => {}
            }
        }
        fn view(&self, at: Surface<'_>, _: &ViewCx<'_>) -> Option<View> {
            (self.open && matches!(at, Surface::Modal)).then(|| View::Text(self.id.into()))
        }
    }
    let mut model = Model::default();
    model.components = Registry::empty();
    for id in ["first", "second"] {
        model.components.register(Modal { id, open: false, cached: 0 }).unwrap();
    }
    model.menu = Some(Menu::Sessions { items: Vec::new(), cursor: 0 });
    model.prompt = Some(Prompt::ProviderName);
    model.component_slash("/first", "");
    assert!(model.menu.is_none() && model.prompt.is_none());
    model.component_slash("/second", "");
    assert!(!model.components.get::<Modal>().unwrap().open);
    assert_eq!(model.components.get::<Modal>().unwrap().cached, 1);
    let visible: Vec<_> = model.component_views(Surface::Modal, 80, 24).into_iter()
        .filter_map(|view| match view { View::Text(text) => Some(text), _ => None }).collect();
    assert_eq!(visible, ["second"]);
    model.component_key(FocusClass::Modal, &iocraft::KeyEvent::new(iocraft::KeyEventKind::Press, KeyCode::Esc));
    assert!(model.component_views(Surface::Modal, 80, 24).is_empty());
    model.component_slash("/first", "");
    model.menu = Some(Menu::Sessions { items: Vec::new(), cursor: 0 });
    model.component_tick(0);
    assert!(!model.components.focused() && !model.components.get::<Modal>().unwrap().open);
    assert_eq!(model.components.get::<Modal>().unwrap().cached, 2);
    assert!(matches!(model.menu, Some(Menu::Sessions { .. })));
}
