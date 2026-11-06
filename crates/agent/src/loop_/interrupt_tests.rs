//! 76 §7: a cancel that names the heard prefix. A raw OpenAI-compatible
//! endpoint streams two sentences and the naming chunk of a tool call, then
//! holds the stream open; the real [`Session`] actor is cancelled mid-stream.
//! `CancelTurn { keep: Some(..) }` persists exactly the prefix and finishes
//! with `interrupted`; `keep: None` on the same session is today's cancel.
//! The post-turn case: a request whose text contains [`FINISH`] gets three
//! sentences and a closed stream, and [`TrimTurn`] rewrites the persisted
//! reply to the heard prefix. One tokio test per scenario set: the tool
//! registry is a process-global actor bound to the runtime that spawned it.

use std::sync::Arc;
use std::time::Duration;

use genai::Client;
use genai::chat::ChatRole;
use kameo::actor::Spawn as _;
use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc::{UnboundedReceiver, unbounded_channel};

use super::vision_fallback_tests::Sink;
use super::*;

const FIRST: &str = "The build is green. ";
const SECOND: &str = "Two tests were skipped. ";
/// What the caller says was heard: the two sentences, as `talk` would join them.
const KEEP: &str = "The build is green. Two tests were skipped.";
/// A prompt containing this word gets a reply that finishes: `THREE`.
const FINISH: &str = "reply in full";
const THREE: &str = "One. Two. Three.";

/// One SSE line as one HTTP chunk.
fn sse(line: &str) -> String {
    let line = format!("data: {line}\n\n");
    format!("{:x}\r\n{line}\r\n", line.len())
}

/// One delta as one HTTP chunk.
fn chunk(delta: serde_json::Value) -> String {
    sse(&json!({"choices": [{"index": 0, "delta": delta}]}).to_string())
}

/// Read one request: headers, then its `Content-Length` body; the body.
async fn drain_request(sock: &mut tokio::net::TcpStream) -> String {
    let mut buf = Vec::new();
    let mut piece = [0u8; 4096];
    let mut want = usize::MAX;
    let mut body_at = 0;
    while buf.len() < want {
        let Ok(n) = sock.read(&mut piece).await else {
            break;
        };
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&piece[..n]);
        if want == usize::MAX
            && let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n")
        {
            let head = String::from_utf8_lossy(&buf[..pos]).to_ascii_lowercase();
            let len = head
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse::<usize>().ok())
                .unwrap_or(0);
            body_at = pos + 4;
            want = body_at + len;
        }
    }
    String::from_utf8_lossy(&buf[body_at.min(buf.len())..]).into_owned()
}

/// A local OpenAI-compatible endpoint: every request gets `FIRST`, `SECOND`
/// and a chunk naming a `shell` call, then the stream is held until the
/// client hangs up (the cancel) or 30 s pass. The stream never ends, so the
/// call is never in a `StreamEnd`. A request mentioning `FINISH` instead
/// gets `THREE` and a closed stream: a short reply that finished streaming.
async fn holding_openai() -> (String, Requests) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base_url = format!("http://{}/v1/", listener.local_addr().unwrap());
    let requests: Requests = Arc::default();
    let log = requests.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let request = drain_request(&mut sock).await;
                log.lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .push(request.clone());
                let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
                if request.contains(FINISH) {
                    let body = format!(
                        "{head}{}{}{}0\r\n\r\n",
                        chunk(json!({"role": "assistant", "content": THREE})),
                        sse(&json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}]})
                            .to_string()),
                        sse("[DONE]"),
                    );
                    let _ = sock.write_all(body.as_bytes()).await;
                    return;
                }
                let body = format!(
                    "{head}{}{}{}",
                    chunk(json!({"role": "assistant", "content": FIRST})),
                    chunk(json!({"content": SECOND})),
                    chunk(
                        json!({"tool_calls": [{"index": 0, "id": "call_1", "type": "function",
                        "function": {"name": "shell", "arguments": ""}}]})
                    ),
                );
                let _ = sock.write_all(body.as_bytes()).await;
                let mut sink = [0u8; 64];
                let _ = tokio::time::timeout(Duration::from_secs(30), async {
                    while matches!(sock.read(&mut sink).await, Ok(n) if n > 0) {}
                })
                .await;
            });
        }
    });
    (base_url, requests)
}

/// Request bodies the endpoint received, in order.
type Requests = Arc<std::sync::Mutex<Vec<String>>>;

struct Harness {
    store: Store,
    id: SessionId,
    events: UnboundedReceiver<AgentEvent>,
    actor: ActorRef<Session>,
    /// Dropped last: the scratch dir outlives the actor that uses it.
    _dir: crate::TestDir,
}

impl Harness {
    /// A session actor on a fresh store whose model is the gateway at `base_url`.
    async fn new(base_url: String) -> Self {
        let scratch = crate::TestDir::new("interrupt");
        let dir = scratch.0.clone();
        let store = Store::open_memory().await.unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "fake/m", None, None, None)
            .await
            .unwrap();
        let auth = Auth::stub(&dir);
        auth.gateways
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .insert(
                "fake".into(),
                crate::providers::Gateway {
                    base_url,
                    key: "k".into(),
                    runtime: None,
                },
            );
        let model = Resolved {
            id: "fake/m".into(),
            genai: "fake::m".into(),
            provider: Some("fake".into()),
            model: None,
        };
        let rt = Runtime::new(figment::Figment::new(), dir.clone());
        let (tx, events) = unbounded_channel();
        let sink = Sink::spawn_with_mailbox(Sink(tx), runtime::subscriber_mailbox());
        rt.subscribe::<Sink, AgentEvent>(&sink).await;
        let actor = Session::spawn(Session::new(
            id,
            0,
            (model, None),
            None,
            SessionState::resume(0, None, vec![]),
            Arc::new(AgentConfig::default()),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            store.clone(),
            Client::new().unwrap(),
            auth,
            rt,
            Arc::new(models::Catalog::load(&dir)),
            None,
            Arc::new(Semaphore::new(1)),
            SessionResources::durable_arc(&dir),
        ));
        Self {
            store,
            id,
            events,
            actor,
            _dir: scratch,
        }
    }

    /// Bus events up to and including the first `done` one.
    async fn drain_until(&mut self, done: impl Fn(&AgentEvent) -> bool) -> Vec<AgentEvent> {
        let mut out = Vec::new();
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), self.events.recv())
                .await
                .expect("event within 10s")
                .expect("bus alive");
            let stop = done(&ev);
            out.push(ev);
            if stop {
                return out;
            }
        }
    }

    /// Open a turn.
    async fn tell(&self, text: &str) {
        self.actor
            .tell(UserInput {
                channel: None,
                id: None,
                text: text.into(),
                files: Vec::new(),
                ticket: None,
                scope: None,
                defer: false,
            })
            .await
            .unwrap();
    }

    /// Open a turn the endpoint completes (`FINISH` in `text`) and wait for
    /// its handback; returns its `turn_seq`.
    async fn finished_turn(&mut self, text: &str) -> i64 {
        assert!(text.contains(FINISH));
        self.tell(text).await;
        let events = self.settled().await;
        assert_ne!(stop_reason(&events), "error", "{events:#?}");
        events
            .iter()
            .find_map(|e| match e {
                AgentEvent::TurnStarted { turn_seq, .. } => Some(*turn_seq),
                _ => None,
            })
            .expect("TurnStarted")
    }

    /// Events through the turn's handback (`TurnFinished`, then the settled
    /// `ContextChanged`): only then is the actor idle for an edit.
    async fn settled(&mut self) -> Vec<AgentEvent> {
        let mut events = self
            .drain_until(|e| matches!(e, AgentEvent::TurnFinished { .. }))
            .await;
        events.extend(
            self.drain_until(
                |e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"),
            )
            .await,
        );
        events
    }

    /// Open a turn, wait for the tool call to be named (both sentences have
    /// streamed by then), cancel with `keep`, and return every event through
    /// `TurnFinished`.
    async fn interrupted_turn(&mut self, text: &str, keep: Option<String>) -> Vec<AgentEvent> {
        self.tell(text).await;
        let mut events = self
            .drain_until(|e| matches!(e, AgentEvent::ToolCallPending { .. }))
            .await;
        assert_eq!(deltas(&events), [FIRST, SECOND]);
        self.actor.tell(CancelTurn { keep }).await.unwrap();
        events.extend(
            self.drain_until(|e| matches!(e, AgentEvent::TurnFinished { .. }))
                .await,
        );
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, AgentEvent::ToolCallStarted { .. })),
            "the tool named in the dropped stream never dispatches: {events:#?}"
        );
        events
    }

    /// `(role, last line)` of every stored row, in order — the user rows
    /// carry the `<time/>` tag on their first line.
    async fn rows(&self) -> Vec<(ChatRole, String)> {
        self.store
            .messages(self.id)
            .await
            .unwrap()
            .into_iter()
            .map(|(_, m)| {
                let text = text_of(&m.content);
                (m.role, text.lines().last().unwrap_or_default().to_owned())
            })
            .collect()
    }
}

fn deltas(events: &[AgentEvent]) -> Vec<&str> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::TextDelta { text, .. } => Some(text.as_ref()),
            _ => None,
        })
        .collect()
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

#[test]
fn cancel_with_keep_persists_the_heard_prefix_then_a_plain_cancel_drops() {
    crate::with_test_registry(scenarios());
}

async fn scenarios() {
    let scratch = crate::TestDir::new("interrupt-reg");
    let reg_dir = scratch.0.clone();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();

    let (base_url, _) = holding_openai().await;
    let mut h = Harness::new(base_url).await;

    // `keep: Some` — exactly the heard prefix, as one assistant message; no
    // recovery note; the turn finishes `interrupted`.
    let events = h.interrupted_turn("Report.", Some(KEEP.into())).await;
    assert_eq!(stop_reason(&events), "interrupted");
    assert_eq!(
        h.rows().await,
        [
            (ChatRole::User, "Report.".to_owned()),
            (ChatRole::Assistant, KEEP.to_owned()),
        ]
    );

    // `keep: None` on the same session — today's cancel: nothing of the
    // stream is persisted, and the earlier prefix did not leak into this turn.
    let events = h.interrupted_turn("Again.", None).await;
    assert_eq!(stop_reason(&events), "cancelled");
    assert_eq!(
        h.rows().await,
        [
            (ChatRole::User, "Report.".to_owned()),
            (ChatRole::Assistant, KEEP.to_owned()),
            (ChatRole::User, "Again.".to_owned()),
        ]
    );

    // An empty prefix persists nothing but still stops as `interrupted`.
    let events = h.interrupted_turn("Once more.", Some(String::new())).await;
    assert_eq!(stop_reason(&events), "interrupted");
    assert_eq!(h.rows().await.len(), 4, "{:#?}", h.rows().await);
}

#[test]
fn trim_rewrites_the_finished_reply_to_the_heard_prefix() {
    crate::with_test_registry(trim_scenarios());
}

/// 76 §7, the post-turn case: `TrimTurn` on a reply that finished streaming.
async fn trim_scenarios() {
    let scratch = crate::TestDir::new("trim-reg");
    let reg_dir = scratch.0.clone();
    let reg_rt = Runtime::new(figment::Figment::new(), reg_dir.clone());
    tools::registry::start(&reg_rt).await.unwrap();

    let (base_url, requests) = holding_openai().await;
    let mut h = Harness::new(base_url).await;

    // Refused while a turn runs: the held stream is in flight.
    h.tell("Report.").await;
    h.drain_until(|e| matches!(e, AgentEvent::ToolCallPending { .. }))
        .await;
    assert_eq!(
        h.actor
            .ask(TrimTurn {
                turn: 0,
                keep: "x".into()
            })
            .await
            .map_err(|e| e.to_string()),
        Err("turn in flight".to_owned())
    );
    h.actor.tell(CancelTurn { keep: None }).await.unwrap();
    h.settled().await;

    // A short reply finishes streaming in full; the user heard two sentences.
    // Its user row follows the cancelled turn's, so it is no turn *start* in
    // the store's sense — `turn_seq` is still what `talk` has.
    let turn = h.finished_turn("Please reply in full.").await;
    assert_eq!(
        h.rows().await,
        [
            (ChatRole::User, "Report.".to_owned()),
            (ChatRole::User, "Please reply in full.".to_owned()),
            (ChatRole::Assistant, THREE.to_owned()),
        ]
    );
    h.actor
        .ask(TrimTurn {
            turn,
            keep: "One. Two.".into(),
        })
        .await
        .unwrap();
    let events = h
        .drain_until(|e| matches!(e, AgentEvent::ContextChanged { .. }))
        .await;
    assert!(matches!(
        events.last(),
        Some(AgentEvent::ContextChanged { reason, .. }) if reason == "trim"
    ));
    assert_eq!(
        h.rows().await[2],
        (ChatRole::Assistant, "One. Two.".to_owned())
    );

    // The in-memory history was replaced too: the next request carries the
    // heard prefix and not the third sentence.
    let turn = h.finished_turn("Please reply in full again.").await;
    let last = requests
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .last()
        .cloned()
        .unwrap();
    assert!(last.contains("One. Two."), "{last}");
    assert!(!last.contains("Three."), "{last}");

    // Empty `keep` deletes the reply; a second trim finds nothing.
    h.actor
        .ask(TrimTurn {
            turn,
            keep: String::new(),
        })
        .await
        .unwrap();
    h.drain_until(|e| matches!(e, AgentEvent::ContextChanged { .. }))
        .await;
    assert_eq!(
        h.rows()
            .await
            .into_iter()
            .map(|(r, _)| r)
            .collect::<Vec<_>>(),
        [
            ChatRole::User,
            ChatRole::User,
            ChatRole::Assistant,
            ChatRole::User
        ]
    );
    let breakdown = h.actor.ask(ContextBreakdownNow).await.unwrap().unwrap();
    assert_eq!(breakdown.tail_messages, 4);
    assert_eq!(
        h.actor
            .ask(TrimTurn {
                turn,
                keep: "z".into()
            })
            .await
            .map_err(|e| e.to_string()),
        Err("no assistant row".to_owned())
    );
}
