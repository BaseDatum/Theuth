use super::*;
use crate::{model::{Command, Entry, Model}, topics::TopicNotification};
use serde_json::json;

// One stateful consumer exercises the neutral shell, not a feature DTO mirror.
pub(crate) struct Counter {
    id: &'static str,
    value: i64,
    bindings: &'static [Binding],
    slash: &'static [SlashSpec],
}
pub(crate) enum Msg {
    Add(i64),
    Fail(String),
    Work(Lane, Box<dyn FnOnce(Arc<Engine>) -> BoxFuture<'static, Msg> + Send>),
}
impl Counter {
    pub(crate) fn new(id: &'static str) -> Self { Self { id, value: 0, bindings: &[], slash: &[] } }
}
impl Component for Counter {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration {
            id: self.id,
            interests: &[Interest { topic: "test.counter", version: 1, variants: &["Add"] }],
            slash: self.slash, bindings: self.bindings,
        }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        (event.event()["type"] == "Add").then(|| event.event()["value"].as_i64()).flatten().map(Msg::Add)
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Message(Msg::Add(n)) => {
                cx.plain(Plain::Line(format!("{}: {} -> {}", self.id, self.value, self.value + n)));
                self.value += n;
                cx.interject(format!("{}={}", self.id, self.value));
            }
            Input::Message(Msg::Fail(error)) => cx.error(error),
            Input::Message(Msg::Work(lane, work)) => cx.spawn(lane, work),
            Input::Key(key) if key.code == KeyCode::Esc => { cx.close(); cx.consume_key(); }
            Input::Key(_) => { cx.focus(self.bindings[0].class); cx.consume_key(); }
            Input::Paste(text) | Input::Submit(text) => { self.value += text.len() as i64; cx.consume_key(); }
            Input::Lifecycle(Lifecycle::Resync) => self.value = -1,
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, _: &ViewCx<'_>) -> Option<View> {
        matches!(at, Surface::Status(StatusSlot::Work)).then(|| View::Text(format!("{}={}", self.id, self.value)))
    }
}
fn core() -> CoreView<'static> {
    CoreView { session: None, project: None, inspecting: false, streaming: false, reasoning: false, agent: None, entries: &[] }
}
pub(crate) fn topic(name: &str, version: u32, event: serde_json::Value) -> TopicEvent {
    TopicEvent::Remote(Arc::new(TopicNotification { topic: name.into(), version, event }))
}
fn shown(registry: &Registry) -> Vec<String> {
    let cx = ViewCx { core: core(), width: 80, height: 24, frame: 0 };
    registry.views(Surface::Status(StatusSlot::Work), &cx).filter_map(|v| match v { View::Text(t) => Some(t), _ => None }).collect()
}
pub(crate) fn reply(to: ComponentToken, engine_epoch: u64, message: Msg) -> LocalReply {
    LocalReply { to, engine_epoch, body: Box::new(message) }
}

#[test]
fn topic_fanout_is_ordered_and_unknown_contracts_are_isolated() {
    let mut registry = Registry::empty();
    registry.register(Counter::new("a")).unwrap();
    registry.register(Counter::new("b")).unwrap();
    for event in [
        topic("unknown.counter", 1, json!({"type":"Add","value":100})),
        topic("test.counter", 2, json!({"type":"Add","value":100})),
        topic("test.counter", 1, json!({"type":"FutureAdd","value":100})),
    ] { registry.wire(&event, &core()); }
    let out = registry.wire(&topic("test.counter", 1, json!({"type":"Add","value":3})), &core());
    let actions: Vec<_> = out.actions.iter().map(|a| match a { CoreAction::Interject(t) => t.as_str(), _ => panic!("unexpected action") }).collect();
    assert_eq!(actions, ["a=3", "b=3"]);
    let lines: Vec<_> = out.plain.iter().map(|p| match p { Plain::Line(t) => t.as_str(), _ => panic!("unexpected output") }).collect();
    assert_eq!(lines, ["a: 0 -> 3", "b: 0 -> 3"]);
    registry.wire(&topic("test.counter", 1, json!({"type":"Add","value":4})), &core());
    assert_eq!(shown(&registry), ["a=7", "b=7"]);
}

#[test]
fn registry_rejects_ambiguous_ids_commands_and_chords() {
    let mut registry = Registry::empty();
    registry.register(Counter::new("a")).unwrap();
    assert!(registry.register(Counter::new("a")).is_err());
    let mut counter = Counter::new("reserved");
    counter.slash = &[("/model", "", "reserved")];
    assert!(registry.register(counter).is_err());
    let mut counter = Counter::new("aliases");
    counter.slash = &[("/counter", "", "one"), ("/counter", "", "two")];
    assert!(registry.register(counter).is_err());
    let mut counter = Counter::new("key");
    counter.bindings = &[Binding { class: FocusClass::Pinned, code: KeyCode::Char('x'), modifiers: KeyModifiers::NONE }];
    let mut duplicate = Counter::new("duplicate-key");
    duplicate.bindings = counter.bindings;
    registry.register(counter).unwrap();
    assert!(registry.register(duplicate).is_err());
    assert_eq!(shown(&registry), ["a=0", "key=0"]);
}

#[test]
fn component_focus_is_exclusive_and_unmount_removes_interests() {
    let mut registry = Registry::empty();
    let mut a = Counter::new("a");
    a.bindings = &[Binding { class: FocusClass::Pinned, code: KeyCode::Char('x'), modifiers: KeyModifiers::NONE }];
    a.slash = &[("/counter", "", "counter")];
    let a = registry.register(a).unwrap();
    let mut b = Counter::new("b");
    b.bindings = &[Binding { class: FocusClass::Modal, code: KeyCode::Char('x'), modifiers: KeyModifiers::NONE }];
    let b = registry.register(b).unwrap();
    let key = KeyEvent::new(iocraft::KeyEventKind::Press, KeyCode::Char('x'));
    assert!(registry.key(FocusClass::Pinned, &key, &core()).consumed());
    registry.paste("one", &core());
    assert_eq!(shown(&registry), ["a=3", "b=0"]);
    assert!(registry.key(FocusClass::Modal, &key, &core()).consumed());
    registry.submit("four", &core());
    assert_eq!(shown(&registry), ["a=3", "b=4"]);
    registry.key(FocusClass::Modal, &KeyEvent::new(iocraft::KeyEventKind::Press, KeyCode::Esc), &core());
    assert!(!registry.focused());
    assert!(!registry.paste("ignored", &core()).consumed());
    registry.key(FocusClass::Modal, &key, &core());
    registry.unmount(b);
    assert!(!registry.focused());
    registry.unmount(a);
    assert!(registry.slash("/counter", "", &core()).is_none());
    registry.wire(&topic("test.counter", 1, json!({"type":"Add","value":100})), &core());
    assert!(shown(&registry).is_empty());
}

#[test]
fn late_errors_are_fenced_after_unmount_and_engine_replacement() {
    let mut registry = Registry::empty();
    let removed = registry.register(Counter::new("removed")).unwrap();
    let current = registry.register(Counter::new("current")).unwrap();
    let secret = reply(removed, 0, Msg::Fail("sensitive-reply-body".into()));
    assert!(!format!("{secret:?}").contains("sensitive-reply-body"));
    registry.unmount(removed);
    assert!(registry.reply(secret, &core()).actions.is_empty());
    registry.lifecycle(Lifecycle::EngineChanged, &core());
    assert!(registry.reply(reply(current, 0, Msg::Fail("old engine".into())), &core()).actions.is_empty());
    let wrong_type = LocalReply { to: current, engine_epoch: 1, body: Box::new("wrong owner type") };
    assert!(registry.reply(wrong_type, &core()).actions.is_empty());
    let out = registry.reply(reply(current, 1, Msg::Fail("current failure".into())), &core());
    assert!(matches!(&out.actions[..], [CoreAction::Error(error)] if error == "current failure"));
    assert_eq!(shown(&registry), ["current=0"]);
}

#[tokio::test]
async fn ordered_effects_keep_fifo_and_resolve_the_current_engine() {
    use parking_lot::{Mutex, RwLock};
    use tokio::sync::{mpsc::unbounded_channel, oneshot};
    let rt = runtime::Runtime::new(Default::default(), std::env::temp_dir());
    let engine = || Arc::new(Engine::Local(rpc::client::Local::new(rt.clone(), jsonrpsee::RpcModule::new)));
    let old = engine();
    let new = engine();
    let api = Arc::new(RwLock::new(old.clone()));
    let (tx, rx) = unbounded_channel();
    let (events, mut replies) = unbounded_channel();
    let runner = tokio::spawn(crate::run_commands(api.clone(), rx, events));
    let mut registry = Registry::empty();
    let token = registry.register(Counter::new("counter")).unwrap();
    let observed = Arc::new(Mutex::new(Vec::new()));
    let (started, started_rx) = oneshot::channel();
    let (release, release_rx) = oneshot::channel();
    let work = {
        let observed = observed.clone();
        Box::new(move |engine| Box::pin(async move {
            started.send(()).unwrap();
            release_rx.await.unwrap();
            observed.lock().push((1, Arc::ptr_eq(&engine, &old)));
            Msg::Add(1)
        }) as BoxFuture<'static, Msg>)
    };
    let mut out = registry.reply(reply(token, 0, Msg::Work(Lane::OrderedInput, work)), &core());
    tx.send(Command::Component(out.effects.pop().unwrap())).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), started_rx).await.unwrap().unwrap();
    *api.write() = new.clone();
    let work = {
        let observed = observed.clone(); let new = new.clone();
        Box::new(move |engine| Box::pin(async move {
            observed.lock().push((2, Arc::ptr_eq(&engine, &new)));
            Msg::Add(2)
        }) as BoxFuture<'static, Msg>)
    };
    let mut out = registry.reply(reply(token, 0, Msg::Work(Lane::OrderedInput, work)), &core());
    tx.send(Command::Component(out.effects.pop().unwrap())).unwrap();
    let work = {
        let observed = observed.clone();
        Box::new(move |engine| Box::pin(async move {
            observed.lock().push((3, Arc::ptr_eq(&engine, &new)));
            release.send(()).unwrap();
            Msg::Add(3)
        }) as BoxFuture<'static, Msg>)
    };
    let mut out = registry.reply(reply(token, 0, Msg::Work(Lane::Concurrent, work)), &core());
    tx.send(Command::Component(out.effects.pop().unwrap())).unwrap();
    for _ in 0..3 {
        let event = tokio::time::timeout(std::time::Duration::from_secs(5), replies.recv()).await.unwrap().unwrap();
        let UiEvent::Component(reply) = event else { panic!("unexpected executor reply: {event:?}") };
        registry.reply(reply, &core());
    }
    assert_eq!(*observed.lock(), [(3, true), (1, true), (2, true)]);
    assert_eq!(shown(&registry), ["counter=6"]);
    drop(tx);
    runner.await.unwrap();
    rt.shutdown.cancel();
}

#[test]
fn empty_pending_replaces_categories_and_loss_resync_preserves_inspection() {
    let session = SessionId::new();
    let mut model = Model::new("model".into(), true);
    model.session = Some(session);
    model.components.register(Counter::new("counter")).unwrap();
    let request = tools::ApprovalRequested { id: tools::ApprovalId(uuid::Uuid::now_v7()), session, tool: "shell".into(), args: json!({"command":"echo hi"}), risk: tools::Risk::ReadOnly };
    let ask = tools::AskRequested { id: tools::AskId(uuid::Uuid::now_v7()), session, questions: vec![tools::Question { id: "q".into(), question: "Which?".into(), options: vec![], recommended: None }] };
    let secret = json!({"type":"SecretRequested","id":uuid::Uuid::now_v7(),"session":session,"name":"token","note":"API","current":null});
    model.apply(UiEvent::Wire(TopicEvent::control(json!({"type":"PendingPrompts","approvals":[request],"asks":[ask],"secrets":[secret]}))));
    // The cursor was taken before snapshot reads: queued duplicate live requests
    // must not produce two cards after that authoritative first snapshot.
    model.apply(UiEvent::Approval(request.clone()));
    model.apply(UiEvent::Ask(ask.clone()));
    assert_eq!(model.approvals.iter().map(|r| r.id).collect::<Vec<_>>(), [request.id]);
    assert_eq!(model.asks.iter().map(|r| r.req.id).collect::<Vec<_>>(), [ask.id]);
    assert_eq!(model.secret_requests.front().unwrap().name, "token");
    model.apply(UiEvent::Wire(TopicEvent::control(json!({"type":"PendingPrompts","approvals":[]}))));
    assert!(model.approvals.is_empty());
    assert_eq!(model.asks.front().unwrap().req.id, ask.id);
    model.apply(UiEvent::Wire(TopicEvent::control(json!({"type":"PendingPrompts","approvals":[],"asks":[],"secrets":[]}))));
    assert!(model.asks.is_empty() && model.secret_requests.is_empty());
    model.inspect = Some(crate::search::Inspect { session, seq: 12, found: true });
    model.pending_input = Some(("unsent draft".into(), 12));
    model.entries.push(Entry::Info("historical page".into()));
    model.apply(UiEvent::Wire(TopicEvent::control(rpc::topics::lagged(5))));
    assert!(!model.refresh_context);
    assert_eq!(model.inspect.as_ref().unwrap().seq, 12);
    assert_eq!(model.pending_input, Some(("unsent draft".into(), 12)));
    assert!(matches!(&model.entries[..], [Entry::Info(text)] if text == "historical page"));
    assert!(shown(&model.components).contains(&"counter=-1".into()));
    model.inspect = None;
    model.apply(UiEvent::Wire(TopicEvent::control(rpc::topics::lagged(1))));
    assert!(model.refresh_context);
}


#[test]
fn component_prompt_keeps_composer_focus_and_accepts_empty_answer() {
    struct PromptOwner;
    impl Component for PromptOwner {
        type Msg = ();
        fn declaration(&self) -> Declaration {
            Declaration { id: "prompt-test", interests: &[], bindings: &[], slash: &[("/prompt-test", "", "test")] }
        }
        fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<()> { None }
        fn update(&mut self, input: Input<'_, ()>, cx: &mut Cx<'_, '_, ()>) {
            match input {
                Input::Slash { .. } => cx.focus(FocusClass::Modal),
                Input::Submit(text) => {
                    cx.core(CoreAction::Notice(format!("answer:{text}")));
                    cx.consume_key();
                    cx.close();
                }
                _ => {}
            }
        }
        fn editor(&self) -> Option<Editor> {
            Some(Editor { hint: "secret note".into(), masked: true, submit_empty: true })
        }
        fn view(&self, _: Surface<'_>, _: &ViewCx<'_>) -> Option<View> { None }
    }
    let mut model = Model::default();
    model.components.register(PromptOwner).unwrap();
    let (tx, _) = tokio::sync::mpsc::unbounded_channel();
    model.submit("/prompt-test", &tx);
    assert!(model.input_focused());
    assert!(!model.modal_menu());
    assert!(model.components.editor().unwrap().1.masked);
    model.submit("", &tx);
    assert!(matches!(model.entries.last(), Some(Entry::Info(text)) if text == "answer:"));
    assert!(model.components.editor().is_none());
}
