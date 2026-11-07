//! Design doc 52 §10 gates driven end to end through a real [`Turn`] against
//! a fake OpenAI-compatible provider (wiremock): the captured request bodies
//! are the evidence, never the final answer. One tokio test runs every
//! scenario in sequence because the tool registry is a process-global actor
//! bound to the runtime that spawned it.

use std::borrow::Cow;
use std::sync::Arc;
use std::time::Duration;

use genai::resolver::{AuthData, Endpoint, ServiceTargetResolver};
use genai::{Client, ServiceTarget};
use kameo::actor::Spawn as _;
use serde_json::{Value, json};
use tokio::sync::Notify;
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use super::*;

/// What the text-only main asks (the fake's fixed question reply).
const QUESTION: &str = "What colour is the square in m0.p1 and roughly how large is it?";
/// The image-only fact: it appears in no user text, file name or tool text.
const ANSWER: &str = "A solid crimson-red square filling the whole 4x4 image.";
/// The second image's fact (two-image scenario).
const ANSWER2: &str = "A solid navy-blue square filling the whole 4x4 image.";
/// The main model's final reply once it has the observation.
pub(super) const FINAL: &str = "The square is red.";
/// The reply of a main call the fake cuts off (`finish_reason: length`).
const TRUNCATED: &str = "Working on it";
/// A main request carrying this text gets an empty reply (`finish_reason:
/// content_filter`, no content chunk): a refused / filtered round.
const EMPTY_REPLY_TRIGGER: &str = "EMPTY-REPLY";
/// A vault value the main model leaks into its question (52 §5.3).
const SECRET: &str = "sk-live-deadbeef42";
const PLACEHOLDER: &str = "{{secret:TOKEN}}";
/// A question that quotes the secret.
const LEAKY_QUESTION: &str = "Is the token sk-live-deadbeef42 visible in m0.p1?";
/// Per-route usage `(prompt, completion)` so the turn's totals are attributable.
const QUESTION_USAGE: (u32, u32) = (11, 2);
const VISION_USAGE: (u32, u32) = (23, 5);
pub(super) const MAIN_USAGE: (u32, u32) = (37, 7);

/// The summariser's reply: long enough not to count as degenerate.
fn summary() -> String {
    format!(
        "Summary of the folded turns (SUMMARY-EPOCH-HEAD). {}",
        "The user greeted the assistant and shared one screenshot; the assistant replied. "
            .repeat(8)
    )
}

// region:    --- Fake provider

/// One OpenAI chat-completions SSE reply: a content chunk (none when `text` is
/// empty — a refused/filtered reply has no blocks), the finish chunk, a
/// usage-only chunk, `[DONE]`.
pub(super) fn sse(text: &str, finish: &str, (prompt, completion): (u32, u32)) -> ResponseTemplate {
    let chunk = |choices: Value, usage: Option<Value>| {
        let mut v =
            json!({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": choices});
        if let Some(u) = usage {
            v["usage"] = u;
        }
        v
    };
    let content = if text.is_empty() {
        String::new()
    } else {
        format!(
            "data: {}\n\n",
            chunk(
                json!([{"index": 0, "delta": {"role": "assistant", "content": text}, "finish_reason": null}]),
                None
            )
        )
    };
    let body = format!(
        "{content}data: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        chunk(
            json!([{"index": 0, "delta": {}, "finish_reason": finish}]),
            None
        ),
        chunk(
            json!([]),
            Some(
                json!({"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion})
            )
        ),
    );
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

/// One OpenAI SSE reply carrying a single tool call, `finish_reason: tool_calls`.
fn sse_tool_call(name: &str, arguments: &str) -> ResponseTemplate {
    let chunk = |choices: Value| json!({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": choices});
    let body = format!(
        "data: {}\n\ndata: {}\n\ndata: {}\n\ndata: [DONE]\n\n",
        chunk(
            json!([{"index": 0, "delta": {"role": "assistant", "tool_calls": [
            {"index": 0, "id": "call_bridge_1", "type": "function",
             "function": {"name": name, "arguments": arguments}}
        ]}, "finish_reason": null}])
        ),
        chunk(json!([{"index": 0, "delta": {}, "finish_reason": "tool_calls"}])),
        json!({"id": "c", "object": "chat.completion.chunk", "model": "m", "choices": [],
            "usage": {"prompt_tokens": MAIN_USAGE.0, "completion_tokens": MAIN_USAGE.1,
                      "total_tokens": MAIN_USAGE.0 + MAIN_USAGE.1}}),
    );
    ResponseTemplate::new(200).set_body_raw(body, "text/event-stream")
}

/// The fake provider: routes on the system prompt, so the same server serves
/// the question call, the vision call, the summariser and the ordinary main call.
pub(super) struct Fake {
    /// The main model's question (the question call's reply).
    question: &'static str,
    /// The vision reply is not the JSON array the harness requires.
    malformed: bool,
    /// Hold the question reply this long (cancellation scenario).
    question_delay: Option<Duration>,
    /// Fired when the question request has arrived.
    question_seen: Arc<Notify>,
    /// Main calls whose request lacks this text are cut off (`length`), so
    /// the turn runs another round — the steering injection point.
    truncate_until: Option<&'static str>,
    /// Hold the harvest reply ([`HARVEST_REPLY`]) this long (detached-harvest scenarios).
    harvest_delay: Option<Duration>,
    /// Hold the summariser reply this long (compaction-queue scenarios).
    compact_delay: Option<Duration>,
    /// Answer the summariser with this HTTP status instead of a summary.
    compact_status: Option<u16>,
    /// `(name, arguments JSON)`: the first main call of a turn (no `tool`
    /// message yet) replies with this one tool call (runtime-bridge scenarios).
    tool_call: Option<(&'static str, String)>,
    /// 137 D11: a main call carrying a `tool` message but no
    /// `<context_summary` head is refused as a context overflow (400); the
    /// retry after the cut, head in place, answers [`FINAL`].
    overflow_until_head: bool,
}

/// The harvest call's reply: one well-formed concept — a 136 `record` whose
/// title (≈ 400 chars) the agent must split (136 §0.14).
pub(super) const HARVEST_REPLY: &str = "<concept id=\"facts/detached-harvest\">\n---\ntype: record\ntitle: Detached harvest landed on 2026-10-08. The harvest instruction was answered by the fake provider while the compaction had already returned; the detached task kept the pre-fold frame, published Harvesting before Compacted and Harvested once the held reply arrived, and the knowledge store received exactly one put whose evidence names the session that ran the turn and whose status is fresh until dreaming tidies it, at which point the title is checked again.\ndescription: The harvest ran off the turn\ntags: [test]\n---\nThe fake provider answered the harvest instruction.\n</concept>";

impl Respond for Fake {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let b = body(req);
        let sys = system(&b);
        if sys == vision::QUESTION_SYSTEM {
            self.question_seen.notify_one();
            let t = sse(self.question, "stop", QUESTION_USAGE);
            return match self.question_delay {
                Some(d) => t.set_delay(d),
                None => t,
            };
        }
        if sys == vision::VISION_SYSTEM {
            if self.malformed {
                return sse("I see a square, but this is not JSON", "stop", VISION_USAGE);
            }
            let reply: Vec<Value> = texts(&b)
                .iter()
                .filter_map(|t| t.strip_prefix("Image ")?.strip_suffix(':'))
                .map(|r| {
                    let text = if r.ends_with(".p2") { ANSWER2 } else { ANSWER };
                    json!({"ref": r, "status": "answered", "text": text})
                })
                .collect();
            return sse(&Value::Array(reply).to_string(), "stop", VISION_USAGE);
        }
        if model(&b) == "broken" {
            return ResponseTemplate::new(500)
                .set_body_string("{\"error\":{\"message\":\"boom\"}}");
        }
        let text = all_text(&b);
        if text.contains(
            knowledge::harvest::HARVEST_INSTRUCTION
                .lines()
                .next()
                .unwrap_or_default(),
        ) {
            let t = sse(HARVEST_REPLY, "stop", MAIN_USAGE);
            return match self.harvest_delay {
                Some(d) => t.set_delay(d),
                None => t,
            };
        }
        if text.contains(compact::COMPACT_INSTRUCTION) {
            if let Some(status) = self.compact_status {
                return ResponseTemplate::new(status)
                    .set_body_string("{\"error\":{\"message\":\"summariser refused\"}}");
            }
            let t = sse(&summary(), "stop", MAIN_USAGE);
            return match self.compact_delay {
                Some(d) => t.set_delay(d),
                None => t,
            };
        }
        // The trigger stays in history: only the newest message decides, so the next turn answers.
        if texts(&b)
            .last()
            .is_some_and(|t| t.contains(EMPTY_REPLY_TRIGGER))
        {
            return sse("", "content_filter", MAIN_USAGE);
        }
        if self.truncate_until.is_some_and(|t| !text.contains(t)) {
            return sse(TRUNCATED, "length", MAIN_USAGE);
        }
        if let Some((name, arguments)) = &self.tool_call
            && !b["messages"]
                .as_array()
                .is_some_and(|m| m.iter().any(|m| m["role"] == "tool"))
        {
            return sse_tool_call(name, arguments);
        }
        if self.overflow_until_head && !text.contains("<context_summary") {
            return ResponseTemplate::new(400).set_body_string(
                "{\"error\":{\"message\":\"This model's maximum context length is 1000 tokens (context_length_exceeded)\"}}",
            );
        }
        sse(FINAL, "stop", MAIN_USAGE)
    }
}

/// An in-process vault: one known value ↔ one placeholder.
struct Vault;

impl tools::secrets::Secrets for Vault {
    fn expand(&self, _: &mut Value) -> Result<usize, String> {
        Ok(0)
    }
    fn redact<'a>(&self, text: &'a str) -> Cow<'a, str> {
        if text.contains(SECRET) {
            Cow::Owned(text.replace(SECRET, PLACEHOLDER))
        } else {
            Cow::Borrowed(text)
        }
    }
    fn redact_value(&self, v: &mut Value) {
        if let Value::String(s) = v
            && let Cow::Owned(clean) = self.redact(s)
        {
            *s = clean;
        }
    }
}

// endregion: --- Fake provider

// region:    --- Request readers

pub(super) fn body(r: &Request) -> Value {
    r.body_json().expect("provider request is JSON")
}

fn model(b: &Value) -> &str {
    b["model"].as_str().unwrap_or_default()
}

/// The leading system message's text, `""` without one.
pub(super) fn system(b: &Value) -> &str {
    let m = &b["messages"][0];
    if m["role"] == "system" {
        m["content"].as_str().unwrap_or_default()
    } else {
        ""
    }
}

/// Every text content part of every message, in order (string contents included).
pub(super) fn texts(b: &Value) -> Vec<String> {
    let mut out = Vec::new();
    for m in b["messages"].as_array().into_iter().flatten() {
        match &m["content"] {
            Value::String(s) => out.push(s.clone()),
            Value::Array(parts) => out.extend(
                parts
                    .iter()
                    .filter(|p| p["type"] == "text")
                    .filter_map(|p| p["text"].as_str().map(str::to_owned)),
            ),
            _ => {}
        }
    }
    out
}

/// Every `image_url` part's URL, in order.
fn image_urls(b: &Value) -> Vec<String> {
    b["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["content"].as_array())
        .flatten()
        .filter(|p| p["type"] == "image_url")
        .filter_map(|p| p["image_url"]["url"].as_str().map(str::to_owned))
        .collect()
}

fn all_text(b: &Value) -> String {
    texts(b).join("\n")
}

// endregion: --- Request readers

// region:    --- Harness

/// A bus subscriber that forwards every `AgentEvent` to a channel.
#[derive(kameo::Actor)]
pub(super) struct Sink(pub(super) tokio::sync::mpsc::UnboundedSender<AgentEvent>);

impl Message<AgentEvent> for Sink {
    type Reply = ();
    async fn handle(&mut self, ev: AgentEvent, _: &mut Context<Self, Self::Reply>) {
        let _ = self.0.send(ev);
    }
}

/// A fresh store, session, runtime bus and fake provider per scenario.
pub(super) struct Harness {
    pub(super) dir: PathBuf,
    pub(super) store: Store,
    pub(super) rt: Runtime,
    /// `None` for a harness attached to another process's provider (109 crash tests).
    server: Option<MockServer>,
    pub(super) client: Client,
    pub(super) catalog: Arc<models::Catalog>,
    vision_model: Arc<RwLock<Option<Resolved>>>,
    pub(super) id: SessionId,
    events: UnboundedReceiver<AgentEvent>,
    /// The synthetic attachments (red, blue) and their base64 as the provider must receive them.
    png: PathBuf,
    png_b64: String,
    png2: PathBuf,
    png2_b64: String,
    /// The python worker pool a turn gets (`None`: no runtime, every tool direct).
    pub(super) pool: Option<Arc<Pool>>,
    /// Removes `dir` on drop (`None` for an attached harness); last so it drops last.
    _scratch: Option<crate::TestDir>,
}

impl Harness {
    pub(super) async fn new(fake: impl Respond + 'static) -> Self {
        let scratch = crate::TestDir::new("vision-e2e");
        let dir = scratch.0.clone();
        let model = |id: &str, image: bool| {
            json!({
                "id": id, "name": id,
                "limit": {"context": 128_000, "output": 4_096},
                "tool_call": true, "image": image
            })
        };
        // `tiny`: a 9k window (81 COMPACT-FLOOR: trigger under `MIN_HEADROOM`).
        let mut tiny = model("tiny", false);
        tiny["limit"]["context"] = json!(9_000);
        std::fs::write(
            dir.join("models.json"),
            json!({"openai": {"id": "openai", "name": "OpenAI", "models": {
                "textonly": model("textonly", false),
                "eyes": model("eyes", true),
                "broken": model("broken", true),
                "tiny": tiny,
                // 137G R1-10: the endless turn-budget test's own model id, so no
                // other scenario's learned bytes/token reaches it.
                "bounded": model("bounded", false),
            }}})
            .to_string(),
        )
        .unwrap();
        // A red and a blue 4x4 PNG: the facts live in pixels only.
        let square = |rgb: [u8; 3]| {
            let mut bytes = Vec::new();
            let mut enc = png::Encoder::new(&mut bytes, 4, 4);
            enc.set_color(png::ColorType::Rgb);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().unwrap();
            w.write_image_data(&rgb.repeat(16)).unwrap();
            drop(w);
            bytes
        };
        let png = dir.join("shot.png");
        let bytes = square([255, 0, 0]);
        std::fs::write(&png, &bytes).unwrap();
        let png_b64 = BASE64.encode(&bytes);
        let png2 = dir.join("second.png");
        let bytes = square([0, 0, 128]);
        std::fs::write(&png2, &bytes).unwrap();
        let png2_b64 = BASE64.encode(&bytes);

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(fake)
            .mount(&server)
            .await;
        let endpoint = format!("{}/v1/", server.uri());
        let client = client_for(&endpoint);

        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "openai/textonly", None, None, None)
            .await
            .unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        let (tx, events) = unbounded_channel();
        let sink = Sink::spawn_with_mailbox(Sink(tx), runtime::subscriber_mailbox());
        rt.subscribe::<Sink, AgentEvent>(&sink).await;
        Self {
            catalog: Arc::new(models::Catalog::load(&dir)),
            dir,
            store,
            rt,
            server: Some(server),
            client,
            vision_model: Arc::default(),
            id,
            events,
            png,
            png_b64,
            png2,
            png2_b64,
            pool: None,
            _scratch: Some(scratch),
        }
    }

    /// A harness over another harness's `dir` (its `models.json`, store and
    /// session) and provider `endpoint`, for a second process or a second
    /// life of the same one (109 §11.1): nothing is created, nothing removed.
    pub(super) async fn attach(dir: PathBuf, endpoint: &str, id: SessionId) -> Self {
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        let (tx, events) = unbounded_channel();
        let sink = Sink::spawn_with_mailbox(Sink(tx), runtime::subscriber_mailbox());
        rt.subscribe::<Sink, AgentEvent>(&sink).await;
        Self {
            catalog: Arc::new(models::Catalog::load(&dir)),
            png: dir.join("shot.png"),
            png2: dir.join("second.png"),
            dir,
            store,
            rt,
            server: None,
            client: client_for(endpoint),
            vision_model: Arc::default(),
            id,
            events,
            png_b64: String::new(),
            png2_b64: String::new(),
            pool: None,
            _scratch: None,
        }
    }

    /// The fake provider's `…/v1/` endpoint.
    pub(super) fn endpoint(&self) -> String {
        format!("{}/v1/", self.server.as_ref().expect("own server").uri())
    }

    pub(super) fn resolve(&self, id: &str) -> Resolved {
        self.catalog.resolve(id, &|_| true).unwrap()
    }

    fn set_fallback(&self, id: Option<&str>) {
        *self.vision_model.write().unwrap_or_else(|p| p.into_inner()) = id.map(|i| self.resolve(i));
    }

    /// A turn over the session's current frame, as `Session::start_turn` builds one.
    pub(super) async fn turn(&self, main: &str, cancel: CancellationToken) -> (Turn, TurnId) {
        let (epoch, head, history) = self.store.frame(self.id).await.unwrap();
        let mut session = Session::new(
            self.id,
            0,
            (self.resolve(main), None),
            None,
            SessionState::resume(epoch, head, history),
            Arc::new(AgentConfig::default()),
            Arc::default(),
            Arc::default(),
            self.vision_model.clone(),
            Arc::default(),
            Arc::default(),
            self.store.clone(),
            self.client.clone(),
            Auth::stub(&self.dir),
            self.rt.clone(),
            self.catalog.clone(),
            self.pool.clone(),
            Arc::new(Semaphore::new(1)),
            SessionResources::durable_arc(&self.dir),
        );
        let tid = TurnId::new();
        let turn = Turn::new(
            session.shared.clone(),
            Some(tid),
            cancel,
            session.state.take().unwrap(),
        );
        (turn, tid)
    }

    /// The session actor itself, as `Agents::session` spawns one over the
    /// stored frame: the `UserInput`/`CompactNow`/`TurnDone` seam.
    pub(super) async fn actor(&self, main: &str) -> ActorRef<Session> {
        self.actor_styled(main, CompactStyle::default()).await
    }

    /// [`Harness::actor`] with a configured default compaction style (`agent.compact_style`).
    pub(super) async fn actor_styled(&self, main: &str, style: CompactStyle) -> ActorRef<Session> {
        self.actor_with(main, style, AgentConfig::default()).await
    }

    /// [`Harness::actor_styled`] under `cfg`.
    pub(super) async fn actor_with(
        &self,
        main: &str,
        style: CompactStyle,
        cfg: AgentConfig,
    ) -> ActorRef<Session> {
        let (epoch, head, history) = self.store.frame(self.id).await.unwrap();
        Session::spawn(Session::new(
            self.id,
            0,
            (self.resolve(main), None),
            None,
            SessionState::resume(epoch, head, history),
            Arc::new(cfg),
            Arc::new(RwLock::new(style)),
            Arc::default(),
            self.vision_model.clone(),
            Arc::default(),
            Arc::default(),
            self.store.clone(),
            self.client.clone(),
            Auth::stub(&self.dir),
            self.rt.clone(),
            self.catalog.clone(),
            self.pool.clone(),
            Arc::new(Semaphore::new(1)),
            SessionResources::durable_arc(&self.dir),
        ))
    }

    pub(super) fn input(&self, text: &str, with_image: bool) -> Input {
        self.input_files(
            text,
            if with_image {
                vec![self.png.clone()]
            } else {
                vec![]
            },
        )
    }

    fn input_files(&self, text: &str, files: Vec<PathBuf>) -> Input {
        Input {
            channel: None,
            text: text.into(),
            files,
            context: Vec::new(),
            replay: None,
            ticket: None,
            scope: None,
            peer: None,
            defer: false,
            advisory: None,
            passive: false,
        }
    }

    /// Run one full turn opened by `text` (+ the PNG) and return the bus
    /// events up to and including its `TurnFinished`.
    async fn run(&mut self, main: &str, text: &str, with_image: bool) -> Vec<AgentEvent> {
        let (turn, tid) = self.turn(main, CancellationToken::new()).await;
        turn.run(tid, vec![self.input(text, with_image)]).await;
        self.drain().await
    }

    pub(super) async fn drain(&mut self) -> Vec<AgentEvent> {
        self.drain_until(|e| matches!(e, AgentEvent::TurnFinished { .. }))
            .await
    }

    /// Bus events up to and including the first `done` one.
    pub(super) async fn drain_until(
        &mut self,
        done: impl Fn(&AgentEvent) -> bool,
    ) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), self.events.recv())
                .await
                .expect("terminal event within 10s")
                .expect("bus alive");
            let stop = done(&ev);
            out.push(ev);
            if stop {
                return out;
            }
        }
    }

    /// `/compact` between turns as [`CompactNow`] runs it: a `Turn`-less
    /// host over the stored frame. Returns the new epoch and the bus events
    /// through `Compacted`.
    async fn compact(&mut self, style: CompactStyle) -> (u32, Vec<AgentEvent>) {
        self.compact_with(style, None).await
    }

    /// [`Harness::compact`] with a ready-made head (`Compact::head`, 53 §0.7).
    async fn compact_with(
        &mut self,
        style: CompactStyle,
        head: Option<String>,
    ) -> (u32, Vec<AgentEvent>) {
        let (mut turn, _) = self.turn("openai/textonly", CancellationToken::new()).await;
        let epoch = turn.compact_now(Some(style), head).await.unwrap();
        let events = self
            .drain_until(|e| matches!(e, AgentEvent::Compacted { .. }))
            .await;
        (epoch, events)
    }

    pub(super) async fn requests(&self) -> Vec<Value> {
        self.server
            .as_ref()
            .expect("own server")
            .received_requests()
            .await
            .unwrap()
            .iter()
            .map(body)
            .collect()
    }

    /// `(seq, note)` rows of the session.
    async fn notes(&self) -> Vec<(i64, vision::VisionNote)> {
        self.store
            .vision_notes(self.id)
            .await
            .unwrap()
            .into_iter()
            .collect()
    }

    async fn pending(&self) -> usize {
        let vf = self.store.vision_frame(self.id).await.unwrap();
        vision::pending(&vf.images, &vf.notes).len()
    }
}

/// A genai client whose every request goes to `endpoint` (the fake provider).
fn client_for(endpoint: &str) -> Client {
    let endpoint = endpoint.to_owned();
    Client::builder()
        .with_service_target_resolver(ServiceTargetResolver::from_resolver_fn(
            move |mut t: ServiceTarget| -> genai::resolver::Result<ServiceTarget> {
                t.endpoint = Endpoint::from_owned(endpoint.clone());
                t.auth = AuthData::Key("x".into());
                Ok(t)
            },
        ))
        .build()
        .unwrap()
}

fn stop_reason(events: &[AgentEvent]) -> &str {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TurnFinished { stop_reason, .. } => Some(stop_reason.as_str()),
            _ => None,
        })
        .expect("TurnFinished")
}

fn errors(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error { message, .. } => Some(message.to_string()),
            _ => None,
        })
        .collect()
}

/// `(stage, model, first source reference, seq)` of every progress event.
fn progress(events: &[AgentEvent]) -> Vec<(VisionStage, String, String, Option<i64>)> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::VisionProgress {
                stage,
                model,
                sources,
                seq,
                ..
            } => Some((
                *stage,
                model.clone(),
                sources
                    .iter()
                    .map(|s| s.reference())
                    .collect::<Vec<_>>()
                    .join(","),
                *seq,
            )),
            _ => None,
        })
        .collect()
}

// endregion: --- Harness

fn fake() -> Fake {
    Fake {
        question: QUESTION,
        malformed: false,
        question_delay: None,
        question_seen: Arc::default(),
        truncate_until: None,
        harvest_delay: None,
        compact_delay: None,
        compact_status: None,
        tool_call: None,
        overflow_until_head: false,
    }
}

impl Fake {
    /// [`fake`] holding every harvest reply for `hold` (the detached-harvest scenarios).
    pub(super) fn harvest_held(hold: Duration) -> Self {
        Self {
            harvest_delay: Some(hold),
            ..fake()
        }
    }

    /// [`fake`] holding every summariser reply for `hold` (compaction-queue scenarios).
    pub(super) fn compact_held(hold: Duration) -> Self {
        Self {
            compact_delay: Some(hold),
            ..fake()
        }
    }

    /// [`fake`] holding both the harvest and the summariser reply for `hold`
    /// (87 HARVEST-DETACH: a compaction cancelled while its harvest runs).
    pub(super) fn harvest_and_compact_held(hold: Duration) -> Self {
        Self {
            harvest_delay: Some(hold),
            compact_delay: Some(hold),
            ..fake()
        }
    }

    /// [`fake`] refusing every summariser call with a deterministic 400
    /// (compaction-failure scenarios): no retry, no cut fallback.
    pub(super) fn compact_refused() -> Self {
        Self {
            compact_status: Some(400),
            ..fake()
        }
    }

    /// [`fake`] whose first main call of a turn replies with one direct tool
    /// call `name(arguments)` (runtime-bridge scenarios); the follow-up call
    /// (the one carrying the tool result) answers [`FINAL`].
    pub(super) fn tool_call(name: &'static str, arguments: &Value) -> Self {
        Self {
            tool_call: Some((name, arguments.to_string())),
            ..fake()
        }
    }

    /// [`Fake::tool_call`] whose follow-up call overflows until a
    /// `<context_summary` head is in the request (137 D11 scenarios).
    pub(super) fn tool_call_then_overflow(name: &'static str, arguments: &Value) -> Self {
        Self {
            overflow_until_head: true,
            ..Self::tool_call(name, arguments)
        }
    }
}

/// `(usage, context)` of the `TurnFinished` event.
fn finished(events: &[AgentEvent]) -> (Usage, u32) {
    events
        .iter()
        .find_map(|e| match e {
            AgentEvent::TurnFinished { usage, context, .. } => Some((*usage, *context)),
            _ => None,
        })
        .expect("TurnFinished")
}

/// 52 §10 gates: main-authored question, native bypass, no-fallback notice,
/// explicit failure without an in-turn retry loop, cancellation, batching,
/// steering, manual summary compaction, accounting, redaction.
#[test]
fn vision_fallback_contract_against_fake_provider() {
    crate::with_test_registry(vision_scenarios());
}

/// One registry and one vault for the process; every scenario's turn finds them.
async fn vision_scenarios() {
    let scratch = crate::TestDir::new("vision-reg");
    let reg_dir = scratch.0.clone();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();
    tools::secrets::install(Arc::new(Vault));

    text_only_main_asks_fallback_looks_main_answers().await;
    vision_capable_main_bypasses_a_broken_fallback().await;
    text_only_main_without_fallback_gets_one_untyped_notice().await;
    malformed_fallback_reply_fails_explicitly_and_retries_next_turn().await;
    cancellation_during_the_question_appends_nothing().await;
    two_images_in_one_input_share_one_consultation().await;
    steered_image_is_prepared_before_the_next_main_call().await;
    manual_summary_compaction_inspects_pending_images_first().await;
    leaked_secret_in_the_question_is_redacted_before_it_leaves().await;
    cut_compaction_budgets_the_tail_and_folds_tool_results().await;
    empty_reply_is_surfaced_as_an_error().await;
}

/// Gate "Main-authored question" / "Privacy and accounting" / "Stable prefix".
async fn text_only_main_asks_fallback_looks_main_answers() {
    let mut h = Harness::new(fake()).await;
    h.set_fallback(Some("openai/eyes"));
    let events = h
        .run(
            "openai/textonly",
            "Please look at the attached screenshot.",
            true,
        )
        .await;
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(stop_reason(&events), "stop");

    let reqs = h.requests().await;
    assert_eq!(reqs.len(), 3, "question, vision, main: {reqs:#?}");
    let [question, vision_req, main] = reqs.as_slice() else {
        unreachable!()
    };
    let marker = {
        let vf = h.store.vision_frame(h.id).await.unwrap();
        assert_eq!(vf.images.len(), 1);
        assert_eq!(vf.images[0].source.reference(), "m0.p1");
        vf.images[0].marker()
    };

    // 1. The question call: text-only main, its own model, the question
    //    system prompt, the marker in place of the image, no tools, no image.
    assert_eq!(model(question), "textonly");
    assert_eq!(system(question), vision::QUESTION_SYSTEM);
    assert!(question.get("tools").is_none(), "{question}");
    assert!(image_urls(question).is_empty());
    let qtext = all_text(question);
    assert!(qtext.contains(&marker), "{qtext}");
    assert!(qtext.contains("Please look at the attached screenshot."));
    assert!(!qtext.contains("base64") && !qtext.contains(&h.png_b64));
    assert!(
        !qtext.contains("red"),
        "the fact is not in any text: {qtext}"
    );

    // 2. The vision call: the fallback model, the vision system prompt, the
    //    exact question plus the one image and nothing of the conversation.
    assert_eq!(model(vision_req), "eyes");
    assert_eq!(system(vision_req), vision::VISION_SYSTEM);
    assert!(vision_req.get("tools").is_none(), "{vision_req}");
    let vparts = texts(vision_req);
    assert_eq!(vparts.len(), 3, "system, head, image label: {vparts:?}");
    assert_eq!(
        vparts[1],
        format!("Question:\n{QUESTION}\n\nImages, in order:\n- m0.p1: shot.png\n")
    );
    assert_eq!(vparts[2], "Image m0.p1:");
    assert_eq!(
        image_urls(vision_req),
        vec![format!("data:image/png;base64,{}", h.png_b64)]
    );
    assert!(!all_text(vision_req).contains("Please look at the attached screenshot."));
    assert!(!all_text(vision_req).contains("<conversation>"));

    // 3. The ordinary main call: tools back, no image, the same marker
    //    unchanged, the attributed observation carrying the answer.
    assert_eq!(model(main), "textonly");
    assert_ne!(system(main), vision::QUESTION_SYSTEM);
    assert!(
        main["tools"].as_array().is_some_and(|t| !t.is_empty()),
        "main call offers tools"
    );
    assert!(image_urls(main).is_empty());
    let mtext = all_text(main);
    assert!(mtext.contains(&marker), "{mtext}");
    assert!(mtext.contains("<vision_observation>"), "{mtext}");
    assert!(mtext.contains(&format!("Question from openai/textonly:\n{QUESTION}")));
    assert!(mtext.contains(&format!("m0.p1: {ANSWER}")), "{mtext}");
    assert!(mtext.contains("Vision model: openai/eyes"));
    assert!(!mtext.contains("NOT SEEN"));

    // Store: one typed note, the image no longer pending; the transcript is
    // user(image) → vision note → assistant.
    let notes = h.notes().await;
    assert_eq!(notes.len(), 1);
    let (seq, note) = &notes[0];
    assert_eq!(note.version, vision::NOTE_VERSION);
    assert_eq!(note.question_model, "openai/textonly");
    assert_eq!(note.vision_model, "openai/eyes");
    assert_eq!(note.observations.len(), 1);
    assert_eq!(note.observations[0].status, VisionStatus::Answered);
    assert_eq!(note.observations[0].source.reference(), "m0.p1");
    assert_eq!(h.pending().await, 0);
    let rows = h.store.messages(h.id).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[1].0, *seq);
    assert_eq!(text_of(&rows[2].1.content), FINAL);

    // Events: questioning → inspecting → completed with the note's seq.
    assert_eq!(
        progress(&events),
        vec![
            (
                VisionStage::Questioning,
                "openai/textonly".into(),
                "m0.p1".into(),
                None
            ),
            (
                VisionStage::Inspecting,
                "openai/eyes".into(),
                "m0.p1".into(),
                None
            ),
            (
                VisionStage::Completed,
                "openai/eyes".into(),
                "m0.p1".into(),
                Some(*seq)
            ),
        ]
    );

    // 9. Accounting (52 §7.2): every call's usage counts toward the turn,
    //    only the main call meters the context.
    let (usage, context) = finished(&events);
    assert_eq!(
        usage.prompt,
        QUESTION_USAGE.0 + VISION_USAGE.0 + MAIN_USAGE.0
    );
    assert_eq!(
        usage.completion,
        QUESTION_USAGE.1 + VISION_USAGE.1 + MAIN_USAGE.1
    );
    assert_eq!(context, MAIN_USAGE.0 + MAIN_USAGE.1);
}

/// Gate "Native bypass": an image-capable main gets the image itself; a
/// fallback that would fail is never consulted and nothing is appended.
async fn vision_capable_main_bypasses_a_broken_fallback() {
    let mut h = Harness::new(fake()).await;
    h.set_fallback(Some("openai/broken"));
    let events = h.run("openai/eyes", "What is in the picture?", true).await;
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(stop_reason(&events), "stop");

    let reqs = h.requests().await;
    assert_eq!(reqs.len(), 1, "{reqs:#?}");
    assert_eq!(model(&reqs[0]), "eyes");
    assert_eq!(
        image_urls(&reqs[0]),
        vec![format!("data:image/png;base64,{}", h.png_b64)]
    );
    let text = all_text(&reqs[0]);
    assert!(!text.contains("<vision"), "{text}");
    assert!(!text.contains("[image m0.p1"), "{text}");
    assert!(h.notes().await.is_empty());
    assert!(progress(&events).is_empty());
    assert_eq!(h.store.messages(h.id).await.unwrap().len(), 2);
}

/// An empty reply (no text, no tool calls) ends the turn with an error note in
/// the transcript instead of silently — the stop reason is the only clue the
/// provider gives (2026-09-28: one session lost ten turns this way).
async fn empty_reply_is_surfaced_as_an_error() {
    let mut h = Harness::new(fake()).await;
    h.set_fallback(None);
    let events = h.run("openai/textonly", EMPTY_REPLY_TRIGGER, false).await;
    assert_eq!(
        errors(&events),
        ["model returned an empty reply (stop_reason=content_filter)"]
    );
    assert_eq!(stop_reason(&events), "content_filter");
    // Nothing to replay: no empty assistant row lands in history.
    assert_eq!(h.store.messages(h.id).await.unwrap().len(), 1);
    // The next turn runs normally.
    let events = h.run("openai/textonly", "Say the colour.", false).await;
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(stop_reason(&events), "stop");
}

/// 52 §4.1: text-only main, no fallback — one main request with the stable
/// marker and one untyped `<vision>` notice; no typed note.
async fn text_only_main_without_fallback_gets_one_untyped_notice() {
    let mut h = Harness::new(fake()).await;
    h.set_fallback(None);
    let events = h.run("openai/textonly", "Read the label.", true).await;
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(stop_reason(&events), "stop");

    let reqs = h.requests().await;
    assert_eq!(reqs.len(), 1, "{reqs:#?}");
    assert_eq!(model(&reqs[0]), "textonly");
    assert!(image_urls(&reqs[0]).is_empty());
    let text = all_text(&reqs[0]);
    assert!(
        text.contains("[image m0.p1: shot.png, image/png, fp "),
        "{text}"
    );
    assert!(
        text.contains("<vision>") && text.contains("no vision fallback model is configured"),
        "{text}"
    );
    assert!(!text.contains("<vision_observation>"));
    assert!(h.notes().await.is_empty());
    assert_eq!(h.pending().await, 1);
    assert!(progress(&events).is_empty());
}

/// Gate "Error/cancel": an unparseable fallback reply commits a Failed note
/// telling the main it has NOT SEEN the image; no second attempt this turn;
/// the next user turn retries exactly once.
async fn malformed_fallback_reply_fails_explicitly_and_retries_next_turn() {
    let mut h = Harness::new(Fake {
        malformed: true,
        ..fake()
    })
    .await;
    h.set_fallback(Some("openai/eyes"));
    let events = h.run("openai/textonly", "Which button is lit?", true).await;
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(stop_reason(&events), "stop");

    let reqs = h.requests().await;
    assert_eq!(
        reqs.len(),
        3,
        "question, vision, main — no retry: {reqs:#?}"
    );
    assert_eq!(system(&reqs[0]), vision::QUESTION_SYSTEM);
    assert_eq!(system(&reqs[1]), vision::VISION_SYSTEM);
    let main = all_text(&reqs[2]);
    assert!(main.contains("<vision_observation>"), "{main}");
    assert!(
        main.contains("m0.p1: NOT SEEN (vision model openai/eyes:"),
        "{main}"
    );
    assert!(main.contains("You have not seen this image"), "{main}");
    assert!(image_urls(&reqs[2]).is_empty());
    let notes = h.notes().await;
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0].1.observations[0].status, VisionStatus::Failed);
    assert_eq!(notes[0].1.vision_model, "openai/eyes");
    assert_eq!(h.pending().await, 1, "a failure keeps the image pending");
    let stages: Vec<VisionStage> = progress(&events).into_iter().map(|p| p.0).collect();
    assert_eq!(
        stages,
        [
            VisionStage::Questioning,
            VisionStage::Inspecting,
            VisionStage::Failed
        ]
    );

    // A later explicit user turn retries once (and fails once more).
    let events = h.run("openai/textonly", "Try again please.", false).await;
    assert_eq!(errors(&events), Vec::<String>::new());
    let reqs = h.requests().await;
    assert_eq!(reqs.len(), 6, "{reqs:#?}");
    assert_eq!(
        reqs.iter()
            .filter(|r| system(r) == vision::VISION_SYSTEM)
            .count(),
        2
    );
    assert_eq!(h.notes().await.len(), 2);
    assert!(all_text(&reqs[5]).contains("Try again please."));
}

/// Gate "Error/cancel": cancelling while the question call is in flight
/// stops preparation — no note row, no vision request, `cancelled`.
async fn cancellation_during_the_question_appends_nothing() {
    let seen = Arc::new(Notify::new());
    let mut h = Harness::new(Fake {
        question_delay: Some(Duration::from_secs(5)),
        question_seen: seen.clone(),
        ..fake()
    })
    .await;
    h.set_fallback(Some("openai/eyes"));
    let cancel = CancellationToken::new();
    let (turn, tid) = h.turn("openai/textonly", cancel.clone()).await;
    let input = h.input("Describe the chart.", true);
    let running = tokio::spawn(turn.run(tid, vec![input]));
    tokio::time::timeout(Duration::from_secs(10), seen.notified())
        .await
        .expect("the question request reaches the provider");
    cancel.cancel();
    tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("the turn ends promptly after cancel")
        .unwrap();
    let events = h.drain().await;
    assert_eq!(stop_reason(&events), "cancelled");

    let reqs = h.requests().await;
    assert_eq!(reqs.len(), 1, "{reqs:#?}");
    assert_eq!(system(&reqs[0]), vision::QUESTION_SYSTEM);
    assert!(h.notes().await.is_empty());
    assert_eq!(h.pending().await, 1);
    let rows = h.store.messages(h.id).await.unwrap();
    assert_eq!(rows.len(), 1, "only the user input: {rows:?}");
    assert_eq!(
        progress(&events)
            .into_iter()
            .map(|p| p.0)
            .collect::<Vec<_>>(),
        [VisionStage::Questioning]
    );
}

/// 52 §5.1 step 2: two images of one row form one group — one question,
/// one vision request carrying both, one note with two answered observations.
async fn two_images_in_one_input_share_one_consultation() {
    let mut h = Harness::new(fake()).await;
    h.set_fallback(Some("openai/eyes"));
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    let input = h.input_files(
        "Compare the two squares.",
        vec![h.png.clone(), h.png2.clone()],
    );
    turn.run(tid, vec![input]).await;
    let events = h.drain().await;
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(stop_reason(&events), "stop");

    let reqs = h.requests().await;
    assert_eq!(reqs.len(), 3, "question, vision, main: {reqs:#?}");
    assert_eq!(system(&reqs[0]), vision::QUESTION_SYSTEM);
    assert_eq!(system(&reqs[1]), vision::VISION_SYSTEM);
    let vparts = texts(&reqs[1]);
    assert!(vparts.contains(&"Image m0.p1:".to_owned()), "{vparts:?}");
    assert!(vparts.contains(&"Image m0.p2:".to_owned()), "{vparts:?}");
    assert_eq!(
        image_urls(&reqs[1]),
        vec![
            format!("data:image/png;base64,{}", h.png_b64),
            format!("data:image/png;base64,{}", h.png2_b64),
        ]
    );

    let vf = h.store.vision_frame(h.id).await.unwrap();
    let refs: Vec<String> = vf.images.iter().map(|im| im.source.reference()).collect();
    assert_eq!(refs, ["m0.p1", "m0.p2"]);
    let main = all_text(&reqs[2]);
    assert!(image_urls(&reqs[2]).is_empty());
    for im in &vf.images {
        assert!(main.contains(&im.marker()), "{main}");
    }
    assert!(main.contains(&format!("m0.p1: {ANSWER}")), "{main}");
    assert!(main.contains(&format!("m0.p2: {ANSWER2}")), "{main}");

    let notes = h.notes().await;
    assert_eq!(notes.len(), 1);
    let obs = &notes[0].1.observations;
    assert_eq!(obs.len(), 2);
    assert!(obs.iter().all(|o| o.status == VisionStatus::Answered));
    assert_eq!(h.pending().await, 0);
    assert_eq!(
        progress(&events)
            .iter()
            .map(|p| p.2.as_str())
            .collect::<Vec<_>>(),
        ["m0.p1,m0.p2"; 3]
    );
}

/// 52 §5.1: input steered into a running turn has its image prepared before
/// the next ordinary call. The steer is queued in `Shared::steer` before the
/// turn starts; the fake cuts the first main reply off (`length`), so the
/// loop runs a second round and injects the queue at that boundary — the
/// real mid-turn path, no tool call needed.
async fn steered_image_is_prepared_before_the_next_main_call() {
    const STEER: &str = "Also, what is in this one?";
    let mut h = Harness::new(Fake {
        truncate_until: Some(STEER),
        ..fake()
    })
    .await;
    h.set_fallback(Some("openai/eyes"));
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    let steer_id = InputId::new();
    turn.shared
        .steer
        .lock()
        .unwrap()
        .push((Some(steer_id), h.input(STEER, true)));
    turn.run(tid, vec![h.input("Start with a plan.", false)])
        .await;
    let events = h.drain().await;
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(stop_reason(&events), "stop");
    assert!(
        events
            .iter()
            .any(|e| matches!(e, AgentEvent::InputInjected { id, .. } if *id == steer_id)),
        "the steer went through the injection path"
    );

    let reqs = h.requests().await;
    let systems: Vec<&str> = reqs.iter().map(system).collect();
    assert_eq!(
        reqs.len(),
        4,
        "main (cut off), question, vision, main: {systems:?}"
    );
    assert_eq!(systems[1], vision::QUESTION_SYSTEM);
    assert_eq!(systems[2], vision::VISION_SYSTEM);
    let first = all_text(&reqs[0]);
    assert!(
        !first.contains(STEER) && !first.contains("[image "),
        "{first}"
    );
    let vf = h.store.vision_frame(h.id).await.unwrap();
    assert_eq!(vf.images.len(), 1);
    let marker = vf.images[0].marker();
    assert!(marker.starts_with("[image m3.p1: shot.png"), "{marker}");
    assert!(texts(&reqs[2]).contains(&"Image m3.p1:".to_owned()));
    let last = all_text(&reqs[3]);
    assert!(last.contains(STEER), "{last}");
    assert!(last.contains(&marker), "{last}");
    assert!(last.contains(&format!("m3.p1: {ANSWER}")), "{last}");
    assert_eq!(h.pending().await, 0);
}

/// 52 §7.2: a manual `/compact` as summary inspects pending images before
/// the summariser reads the frame. Turn A has no image so the image row is
/// not the first message: the first compaction (no fallback) folds turn A and
/// leaves the image pending; the second (fallback set) consults and folds the
/// next row; the third folds the image row itself, which drops it from the
/// active index while its note stays visible.
async fn manual_summary_compaction_inspects_pending_images_first() {
    let mut h = Harness::new(fake()).await;
    h.set_fallback(None);
    let events = h.run("openai/textonly", "Hello there.", false).await;
    assert_eq!(errors(&events), Vec::<String>::new());
    let events = h.run("openai/textonly", "Look at this.", true).await;
    assert_eq!(errors(&events), Vec::<String>::new());
    assert_eq!(h.requests().await.len(), 2);
    let marker = {
        let vf = h.store.vision_frame(h.id).await.unwrap();
        assert_eq!(vf.images.len(), 1);
        vf.images[0].marker()
    };
    assert!(marker.starts_with("[image m2.p1: "), "{marker}");

    // No fallback: the summariser sees the marker and the untyped notice only.
    let (epoch, _) = h.compact(CompactStyle::Summary).await;
    assert_eq!(epoch, 1);
    let reqs = h.requests().await;
    assert_eq!(reqs.len(), 3, "{reqs:#?}");
    let s = all_text(&reqs[2]);
    assert!(
        s.contains(compact::COMPACT_INSTRUCTION) && s.contains(&marker),
        "{s}"
    );
    assert!(
        s.contains("<vision>") && !s.contains("<vision_observation>"),
        "{s}"
    );
    assert_eq!(h.pending().await, 1);
    assert!(h.notes().await.is_empty());

    // Fallback set: question → vision → summariser, then the fold.
    h.set_fallback(Some("openai/eyes"));
    let (epoch, events) = h.compact(CompactStyle::Summary).await;
    assert_eq!(epoch, 2);
    let reqs = h.requests().await;
    assert_eq!(reqs.len(), 6, "{reqs:#?}");
    assert_eq!(system(&reqs[3]), vision::QUESTION_SYSTEM);
    assert_eq!(system(&reqs[4]), vision::VISION_SYSTEM);
    assert_eq!(
        image_urls(&reqs[4]),
        vec![format!("data:image/png;base64,{}", h.png_b64)]
    );
    let s = all_text(&reqs[5]);
    assert_eq!(model(&reqs[5]), "textonly");
    assert!(s.contains(compact::COMPACT_INSTRUCTION), "{s}");
    assert!(
        s.contains("<vision_observation>") && s.contains(&marker),
        "{s}"
    );
    assert!(s.contains(&format!("m2.p1: {ANSWER}")), "{s}");
    assert!(image_urls(&reqs[5]).is_empty());
    let stages: Vec<VisionStage> = progress(&events).into_iter().map(|p| p.0).collect();
    assert_eq!(
        stages,
        [
            VisionStage::Questioning,
            VisionStage::Inspecting,
            VisionStage::Completed
        ]
    );
    assert!(matches!(
        events.last(),
        Some(AgentEvent::Compacted {
            epoch: 2,
            style: CompactStyle::Summary,
            folded: 1,
            ..
        })
    ));
    let (n, head, _) = h.store.frame(h.id).await.unwrap();
    assert_eq!(n, 2);
    let head = text_of(&head.expect("epoch head").content);
    assert!(head.contains("SUMMARY-EPOCH-HEAD"), "{head}");
    let vf = h.store.vision_frame(h.id).await.unwrap();
    assert_eq!(vf.images.len(), 1, "the image row is still in the tail");
    assert_eq!(vf.notes.len(), 1);
    assert_eq!(vision::pending(&vf.images, &vf.notes).len(), 0);

    // Nothing pending: no vision call; the fold takes the image row itself.
    let (epoch, _) = h.compact(CompactStyle::Summary).await;
    assert_eq!(epoch, 3);
    assert_eq!(h.requests().await.len(), 7);
    let vf = h.store.vision_frame(h.id).await.unwrap();
    assert!(vf.images.is_empty(), "a folded image is not active");
    assert_eq!(vf.notes.len(), 1, "its note is still in the tail");
    assert_eq!(vision::pending(&vf.images, &vf.notes).len(), 0);

    // 53 §0.7 / meetings 04 §0.6: a given head is installed verbatim as the
    // epoch's summary body — no summariser, no harvest, no vision call.
    let events = h.run("openai/textonly", "One more turn.", false).await;
    assert_eq!(errors(&events), Vec::<String>::new());
    let before = h.requests().await.len();
    let given = "# Weekly sync\n\n**Decisions**\n- Ship the MEETING-HEAD-MARKER fix".to_owned();
    let (epoch, _) = h
        .compact_with(CompactStyle::Summary, Some(given.clone()))
        .await;
    assert_eq!(epoch, 4);
    assert_eq!(
        h.requests().await.len(),
        before,
        "no model call for a given head"
    );
    let (n, head, _) = h.store.frame(h.id).await.unwrap();
    assert_eq!(n, 4);
    let head = text_of(&head.expect("epoch head").content);
    assert!(
        head.contains("MEETING-HEAD-MARKER") && head.contains("Weekly sync"),
        "{head}"
    );
    assert!(!head.contains("SUMMARY-EPOCH-HEAD"), "{head}");
}

/// 70 COMPACT-SIZE: an uncalibrated session (no provider usage yet) keeps at
/// most `TAIL_MESSAGES_UNCALIBRATED` rows, folds oversize tool results in the
/// kept tail (store rows stay whole), and `Compacted.after` is the whole-frame
/// estimate — below `before`, above the bare tail.
async fn cut_compaction_budgets_the_tail_and_folds_tool_results() {
    let mut h = Harness::new(fake()).await;
    let big = "y".repeat(compact::FOLD_CHARS * 2);
    for i in 0..60 {
        for m in [
            ChatMessage::user(format!("u{i}")),
            ChatMessage::from(vec![ToolCall {
                call_id: format!("c{i}"),
                fn_name: "shell".into(),
                fn_arguments: serde_json::json!({ "cmd": "ls" }),
                thought_signatures: None,
            }]),
            ChatMessage::from(vec![ToolResponse::new(format!("c{i}"), big.clone())]),
            ChatMessage::assistant(MessageContent::from_text(format!("a{i}"))),
        ] {
            h.store.append(h.id, &m).await.unwrap();
        }
    }
    let (epoch, events) = h.compact(CompactStyle::Cut).await;
    assert_eq!(epoch, 1);
    let (before, after) = events
        .iter()
        .find_map(|e| match e {
            AgentEvent::Compacted { before, after, .. } => Some((*before, *after)),
            _ => None,
        })
        .expect("Compacted");
    let (_, head, tail) = h.store.frame(h.id).await.unwrap();
    assert!(head.is_some());
    assert!(
        tail.len() <= compact::TAIL_MESSAGES_UNCALIBRATED,
        "tail {} rows",
        tail.len()
    );
    assert_ne!(tail[0].1.role, genai::chat::ChatRole::Tool);
    let folded = tail
        .iter()
        .flat_map(|(_, m)| m.content.tool_responses())
        .filter(|r| r.content.contains("chars folded — read the archive]"))
        .count();
    assert!(folded > 0, "no tool result folded");
    let tail_est: u32 = tail.iter().map(|(_, m)| compact::est(m)).sum();
    let budgets = AgentConfig::default().compaction;
    assert!(
        tail_est <= budgets.tail_tokens,
        "tail {tail_est} over {}",
        budgets.tail_tokens
    );
    // `before` is 0 for a manual `/compact` (no projection); `after` is the
    // whole frame — head and system on top of the tail.
    let head_est = compact::est(head.as_ref().unwrap());
    assert!(
        after >= tail_est + head_est,
        "after {after} tail {tail_est} head {head_est} (before {before})"
    );
    let raw = h.store.messages(h.id).await.unwrap();
    assert!(raw.iter().all(|(_, m)| {
        m.content
            .tool_responses()
            .iter()
            .all(|r| !r.content.contains("folded"))
    }));
}

/// 52 §5.3: a known secret the main quotes in its question is a placeholder
/// in the vision request and in the stored note — the value never leaves.
async fn leaked_secret_in_the_question_is_redacted_before_it_leaves() {
    let mut h = Harness::new(Fake {
        question: LEAKY_QUESTION,
        ..fake()
    })
    .await;
    h.set_fallback(Some("openai/eyes"));
    let events = h.run("openai/textonly", "Check the token.", true).await;
    assert_eq!(errors(&events), Vec::<String>::new());

    let reqs = h.requests().await;
    assert_eq!(reqs.len(), 3, "{reqs:#?}");
    let vision_req = all_text(&reqs[1]);
    assert!(vision_req.contains(PLACEHOLDER), "{vision_req}");
    assert!(!vision_req.contains(SECRET), "{vision_req}");
    let main = all_text(&reqs[2]);
    assert!(
        main.contains(PLACEHOLDER) && !main.contains(SECRET),
        "{main}"
    );
    let rows = h.store.messages(h.id).await.unwrap();
    let note = text_of(&rows[1].1.content);
    assert!(note.starts_with("<vision_observation>"), "{note}");
    assert!(
        note.contains(PLACEHOLDER) && !note.contains(SECRET),
        "{note}"
    );
}
