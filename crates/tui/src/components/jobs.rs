//! Background jobs own active ids and shell-card tails, including local kernels.
use std::collections::{BTreeSet, HashMap, VecDeque};
use std::sync::{Arc, atomic::{AtomicBool, Ordering}};
use kameo::{Actor, message::{Context, Message}};
use kameo::actor::Spawn;
use runtime::{Runtime, SessionId};
use serde::Deserialize;
use tools::{JobEvent, JobView};
use rpc::api::clients::JobsApiClient;
use super::*;

#[derive(Default)]
pub(crate) struct Jobs {
    active: BTreeSet<String>,
    tails: HashMap<String, JobTail>,
    snapshot: u64,
    changes: Option<HashMap<String, bool>>,
}
pub(crate) enum Msg {
    Event(JobEvent),
    Listed(Result<Vec<JobView>, String>),
    Cancelled(Result<String, String>),
    Snapshot { session: SessionId, snapshot: u64, result: Result<Vec<String>, String> },
}
#[derive(Clone, Debug)]
pub(crate) enum Request {
    ListJobs(SessionId),
    CancelJob { session: SessionId, id: String },
    Snapshot(SessionId),
}

impl Jobs {
    #[cfg(test)]
    pub(crate) fn tail(&self, call: &str) -> Option<&JobTail> { self.tails.get(call) }
    fn refresh(&mut self, cx: &mut Cx<'_, '_, Msg>) {
        if let Some(session) = cx.core.session && self.changes.is_none() {
            self.snapshot += 1;
            self.changes = Some(HashMap::new());
            let snapshot = self.snapshot;
            cx.spawn_request(Lane::Concurrent, Request::Snapshot(session), move |api, request| async move {
                let Request::Snapshot(session) = request else { unreachable!() };
                let actor = if matches!(api.as_ref(), rpc::client::Engine::Local(_)) {
                    kameo::actor::ActorRef::<tools::jobs::Jobs>::lookup(tools::jobs::JOBS_ACTOR).ok().flatten()
                } else { None };
                let result = match actor {
                    Some(actor) => actor.ask(tools::jobs::LocalJobIds(session)).await.map(|ids| ids.into_iter().map(|id| id.0).collect()).map_err(|e| e.to_string()),
                    None => api.list_jobs(session).await.map(|rows| rows.into_iter().filter(|row| row.state == "running").map(|row| row.id.0).collect()).map_err(crate::reason),
                };
                Msg::Snapshot { session, snapshot, result }
            });
        }
    }
}

impl Component for Jobs {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "jobs", interests: &[Interest { topic: "tools.jobs", version: 1, variants: &[] }], slash: &[("/jobs", "[kill <id>]", "list this session's background jobs")], bindings: &[] }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        if matches!(event, TopicEvent::Local(_)) || !((event.topic() == Some("tools.jobs") && event.version() == Some(1)) || event.topic().is_none()) { return None; }
        JobEvent::deserialize(event.event()).ok().map(Msg::Event)
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Message(Msg::Event(event)) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                match event {
                    JobEvent::Started { session, id, call_id, .. } if cx.core.session == Some(session) => {
                        if cx.core.entries.iter().any(|e| matches!(e, crate::model::Entry::Tool { call_id: c, .. } if c == &call_id)) { self.tails.insert(call_id, JobTail::new(id.0.clone())); }
                        if let Some(changes) = &mut self.changes { changes.insert(id.0.clone(), true); }
                        self.active.insert(id.0);
                    }
                    JobEvent::Output { session, id, chunk } if cx.core.session == Some(session) => {
                        if let Some(tail) = self.tails.values_mut().find(|tail| tail.id == id.0) { tail.push(&chunk); }
                    }
                    JobEvent::Screen { session, id, rows, .. } if cx.core.session == Some(session) => {
                        if let Some(tail) = self.tails.values_mut().find(|tail| tail.id == id.0) { tail.set_screen(&rows); }
                    }
                    JobEvent::Finished { session, id, exit, secs, inline, .. } if cx.core.session == Some(session) => {
                        if inline { self.tails.retain(|_, tail| tail.id != id.0); }
                        else if let Some(tail) = self.tails.values_mut().find(|tail| tail.id == id.0) { tail.exit = Some(format!("{exit} · {secs}s")); }
                        self.active.remove(&id.0);
                        if let Some(changes) = &mut self.changes { changes.insert(id.0, false); }
                    }
                    _ => {}
                }
            }
            Input::Message(Msg::Listed(result)) => match result {
                Ok(rows) => { if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); } let text = tools::jobs::table(&rows); cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text)); }
                Err(error) => cx.error(error),
            },
            Input::Message(Msg::Cancelled(result)) => match result {
                Ok(text) => { if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); } cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text)); }
                Err(error) => cx.error(error),
            },
            Input::Message(Msg::Snapshot { session, snapshot, result }) if cx.core.session == Some(session) && self.snapshot == snapshot => {
                if let Ok(ids) = result {
                    self.active = ids.into_iter().collect();
                    for (id, running) in self.changes.take().unwrap_or_default() {
                        if running { self.active.insert(id); } else { self.active.remove(&id); }
                    }
                } else { self.changes = None; }
            }
            Input::Slash { args, .. } => match (cx.core.session, args.split_once(' ')) {
                (None, _) => cx.core(CoreAction::Notice("no session".into())),
                (Some(session), None) if args.is_empty() => cx.spawn_request(Lane::Concurrent, Request::ListJobs(session), |api, request| async move {
                    let Request::ListJobs(session) = request else { unreachable!() };
                    Msg::Listed(api.list_jobs(session).await.map_err(crate::reason))
                }),
                (Some(session), Some(("kill", id))) if !id.trim().is_empty() => {
                    let id = id.trim().to_owned();
                    cx.spawn_request(Lane::Concurrent, Request::CancelJob { session, id }, |api, request| async move {
                        let Request::CancelJob { session, id } = request else { unreachable!() };
                        Msg::Cancelled(api.cancel_job(session, tools::JobId(id.clone())).await.map(|_| format!("killed {id}")).map_err(crate::reason))
                    });
                }
                _ => cx.core(CoreAction::Reject("usage: /jobs [kill <id>]".into())),
            },
            Input::Lifecycle(Lifecycle::SessionReset) => { self.active.clear(); self.tails.clear(); self.changes = None; self.snapshot += 1; }
            Input::Lifecycle(Lifecycle::TranscriptReplaced) => { self.tails.clear(); }
            Input::Lifecycle(Lifecycle::EngineChanged) => { self.changes = None; self.snapshot += 1; self.refresh(cx); }
            Input::Lifecycle(Lifecycle::Resync) => self.refresh(cx),
            _ => {}
        }
    }
    fn entry_rows(&self, key: &str) -> usize { self.tails.get(key).map_or(0, |tail| 1 + tail.rows().count()) }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Status(StatusSlot::Work) if !self.active.is_empty() => Some(View::Text(format!("  ·  {} job(s)", self.active.len()))),
            Surface::Entry { key, .. } => {
                use iocraft::prelude::{View as BoxView, *};
                let tail = self.tails.get(key)?;
                let fit = cx.width.saturating_sub(5);
                Some(super::View::Element { rows: self.entry_rows(key), replace: false, element: element! {
                    BoxView(flex_direction: FlexDirection::Column, margin_top: 1) {
                        Text(content: tail.header(), color: crate::ui::SAND, italic: true, wrap: TextWrap::NoWrap)
                        #(tail.rows().map(|line| element!(Text(content: line.chars().take(fit).collect::<String>(), color: crate::ui::BONE_DIM, wrap: TextWrap::NoWrap))))
                    }
                }.into_any() })
            }
            _ => None,
        }
    }
}

#[derive(Actor)]
struct JobsBridge { tx: tokio::sync::mpsc::UnboundedSender<UiEvent>, local: Arc<AtomicBool> }
impl Message<JobEvent> for JobsBridge {
    type Reply = ();
    async fn handle(&mut self, event: JobEvent, _: &mut Context<Self, ()>) {
        if self.local.load(Ordering::Acquire) {
            let _ = self.tx.send(UiEvent::Wire(TopicEvent::Legacy(Arc::new(serde_json::to_value(event).expect("job event")))));
        }
    }
}
pub(crate) async fn subscribe(rt: &Runtime, tx: tokio::sync::mpsc::UnboundedSender<UiEvent>, local: Arc<AtomicBool>) {
    let bridge = JobsBridge::spawn_with_mailbox(JobsBridge { tx, local }, runtime::subscriber_mailbox());
    rt.subscribe::<JobsBridge, JobEvent>(&bridge).await;
}
/// Rows of live output a `shell` card shows for its background job.
pub const JOB_LINES: usize = 10;

/// The tail of a background job's output as it arrives (`JobEvent::Output`,
/// coalesced upstream to one chunk per half second per job): the last
/// [`JOB_LINES`] lines, frozen with how it ended. A PTY job (22 §5) instead
/// shows the last non-blank rows of its latest `JobEvent::Screen`. Bounded,
/// so any number of jobs costs the same; the log file is the whole story.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JobTail {
    /// Job id (`j3`).
    pub id: String,
    /// Complete lines, oldest first, at most [`JOB_LINES`].
    lines: VecDeque<String>,
    /// Bytes after the last newline (a progress bar's current state).
    partial: String,
    /// `exited 0 · 47s` and the like once finished.
    pub exit: Option<String>,
}

impl JobTail {
    /// An empty tail for job `id`.
    pub fn new(id: String) -> Self {
        Self {
            id,
            ..Self::default()
        }
    }

    /// Append raw output. Escape sequences are dropped, `\r` rewinds the
    /// current line (progress bars), tabs become spaces.
    pub fn push(&mut self, chunk: &str) {
        let mut chars = chunk.chars();
        while let Some(c) = chars.next() {
            match c {
                '\n' => {
                    if self.lines.len() == JOB_LINES {
                        self.lines.pop_front();
                    }
                    self.lines.push_back(std::mem::take(&mut self.partial));
                }
                '\r' => self.partial.clear(),
                '\t' => self.partial.push_str("    "),
                '\x1b' => match chars.next() {
                    // CSI: parameters, then one final byte in `@`..`~`.
                    Some('[') => {
                        for c in chars.by_ref() {
                            if ('@'..='~').contains(&c) {
                                break;
                            }
                        }
                    }
                    // OSC: to BEL or ST.
                    Some(']') => {
                        let mut prev = '\0';
                        for c in chars.by_ref() {
                            if c == '\x07' || (prev == '\x1b' && c == '\\') {
                                break;
                            }
                            prev = c;
                        }
                    }
                    _ => {}
                },
                c if c.is_control() => {}
                c => self.partial.push(c),
            }
        }
    }

    /// Replace the tail with the last [`JOB_LINES`] non-blank rows of a PTY
    /// job's screen (22 §5). The screen is a whole, not a stream: nothing
    /// carried over from before.
    pub fn set_screen(&mut self, rows: &[String]) {
        self.partial.clear();
        self.lines.clear();
        for r in rows
            .iter()
            .rev()
            .filter(|r| !r.trim().is_empty())
            .take(JOB_LINES)
        {
            self.lines.push_front(r.clone());
        }
    }

    /// The rows to draw, oldest first: complete lines and the partial one.
    pub fn rows(&self) -> impl Iterator<Item = &str> {
        let extra = usize::from(!self.partial.is_empty());
        self.lines
            .iter()
            .map(String::as_str)
            .skip((self.lines.len() + extra).saturating_sub(JOB_LINES))
            .chain((!self.partial.is_empty()).then_some(self.partial.as_str()))
    }

    /// Header row: `j3 · running` until it ends.
    pub fn header(&self) -> String {
        match &self.exit {
            Some(e) => format!("{} · {e}", self.id),
            None => format!("{} · running", self.id),
        }
    }
}

#[cfg(test)]
#[test]
fn ring_loss_resnapshot_reconciles_active_jobs_with_inflight_events() {
    let session = SessionId::new();
    let core = CoreView { session: Some(session), parent: None, private_root: None, project: None, inspecting: false, streaming: false, reasoning: false, agent: None, entries: &[] };
    let mut registry = Registry::empty();
    let token = registry.register(Jobs::default()).unwrap();
    let message = |body| LocalReply { to: token, engine_epoch: 0, body: Box::new(body) };
    let started = |id: &str| Msg::Event(JobEvent::Started {
        session, id: tools::JobId(id.into()), command: "sleep 9".into(), call_id: String::new(),
        log: "/tmp/job.log".into(), name: None, pty: false, kind: tools::JobKind::Shell,
    });
    registry.reply(message(started("j1")), &core);
    registry.lifecycle(Lifecycle::Resync, &core);
    let snapshot = registry.get::<Jobs>().unwrap().snapshot;
    registry.reply(message(Msg::Event(JobEvent::Finished {
        session, id: tools::JobId("j1".into()), command: "sleep 9".into(), exit: tools::JobExit::Exited(0),
        secs: 1, output: String::new(), inline: false, pty: false, kind: tools::JobKind::Shell,
    })), &core);
    registry.reply(message(started("j2")), &core);
    registry.reply(message(Msg::Snapshot { session, snapshot, result: Ok(vec!["j1".into(), "kernel".into()]) }), &core);
    assert_eq!(registry.get::<Jobs>().unwrap().active, BTreeSet::from(["j2".into(), "kernel".into()]));
    registry.lifecycle(Lifecycle::SessionReset, &core);
    registry.reply(message(Msg::Snapshot { session, snapshot, result: Ok(vec!["j1".into()]) }), &core);
    assert!(registry.get::<Jobs>().unwrap().active.is_empty());
}
