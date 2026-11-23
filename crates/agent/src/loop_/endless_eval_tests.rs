//! 201 evaluation E2 (turn loop), written by the evaluator, not the
//! builders: the scenarios of `docs/design/201-eval-E2.md` that need the
//! real [`Session`] actor, the real endless service (fake compactors from
//! `endless::eval_tests`) and the fake provider of
//! [`super::vision_fallback_tests`]. The provider is scripted per main call
//! ([`Script`]); every other call (title, summary, harvest) goes to [`Fake`].

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use kameo::actor::Spawn;
use serde_json::{Value, json};
use wiremock::Respond;

use super::vision_fallback_tests::{
    Fake, Harness, MAIN_USAGE, body, sse, sse_tool_call, system, texts,
};
use super::*;
use crate::endless::eval_tests::{Act, EvalCompactor};
use crate::endless::tree::Quality;
use crate::endless::{Attached, EndlessConfig, Profile};

// region:    --- Harness pieces

/// A function of the request that picks a tool call `(name, args)`.
type Pick = Box<dyn Fn(&Value) -> Option<(String, Value)> + Send + Sync>;

/// One scripted main-call reply.
enum Step {
    Text(String),
    Call(String, Value),
    Pick(Pick),
}

/// The scripted provider: each main call (a request with `tools`) takes the
/// next [`Step`] (none left: a short text); tool call ids are
/// `ecall_<main call index>`. Records when each main call arrived.
struct Script {
    inner: Fake,
    steps: parking_lot::Mutex<VecDeque<Step>>,
    n: AtomicUsize,
    at: parking_lot::Mutex<Vec<Instant>>,
}

impl Script {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Fake::compact_held(Duration::ZERO),
            steps: parking_lot::Mutex::new(VecDeque::new()),
            n: AtomicUsize::new(0),
            at: parking_lot::Mutex::new(Vec::new()),
        })
    }

    fn text(&self, t: impl Into<String>) {
        self.steps.lock().push_back(Step::Text(t.into()));
    }

    fn call(&self, name: &str, args: Value) {
        self.steps
            .lock()
            .push_back(Step::Call(name.to_owned(), args));
    }

    fn pick(&self, f: impl Fn(&Value) -> Option<(String, Value)> + Send + Sync + 'static) {
        self.steps.lock().push_back(Step::Pick(Box::new(f)));
    }

    fn main_calls(&self) -> usize {
        self.n.load(Ordering::SeqCst)
    }
}

struct ScriptRef(Arc<Script>);

impl Respond for ScriptRef {
    fn respond(&self, r: &wiremock::Request) -> wiremock::ResponseTemplate {
        let b = body(r);
        if !b["tools"].is_array() {
            return self.0.inner.respond(r);
        }
        let k = self.0.n.fetch_add(1, Ordering::SeqCst);
        self.0.at.lock().push(Instant::now());
        let step = self.0.steps.lock().pop_front();
        let call = |name: &str, args: &Value| {
            sse_tool_call(&format!("ecall_{k}"), name, &args.to_string())
        };
        match step {
            Some(Step::Text(t)) => sse(&t, "stop", MAIN_USAGE),
            Some(Step::Call(name, args)) => call(&name, &args),
            Some(Step::Pick(f)) => match f(&b) {
                Some((name, args)) => call(&name, &args),
                None => sse("Nothing to pick.", "stop", MAIN_USAGE),
            },
            None => sse("Done.", "stop", MAIN_USAGE),
        }
    }
}

/// A registry for `rebase`'s `ListTools`; process-global, so one per scenario.
async fn registry() -> (crate::TestDir, ActorRef<ToolRegistry>) {
    let scratch = crate::test_dir("endless-eval-reg");
    let reg_rt = Runtime::new(figment::Figment::new(), scratch.path().to_path_buf());
    let reg = tools::registry::start(&reg_rt).await.unwrap();
    (scratch, reg)
}

/// Adds `(id, image, context tokens)` models to the harness catalog (each
/// scenario its own id: learned bytes/token stay apart).
fn add_models(h: &mut Harness, ids: &[(&str, bool, u64)]) {
    let path = h.dir.join("models.json");
    let mut v: Value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    for (id, image, ctx) in ids {
        v["openai"]["models"][*id] = json!({
            "id": id, "name": id,
            "limit": {"context": ctx, "output": 4_096},
            "tool_call": true, "image": image
        });
        crate::tokens::forget_for_tests(&format!("openai/{id}"));
    }
    std::fs::write(&path, v.to_string()).unwrap();
    h.catalog = Arc::new(models::Catalog::load(&h.dir));
}

/// A fresh endless service over the harness store, `c` its compactor.
fn service(h: &Harness, cfg: EndlessConfig, c: &Arc<EvalCompactor>) -> Endless {
    let mut e = Endless::new(
        h.store.clone(),
        h.rt.clone(),
        h.client.clone(),
        Auth::stub(&h.dir),
        h.catalog.clone(),
        cfg,
        None,
    );
    crate::endless::eval_tests::set_compactor(&mut e, c.factory());
    e
}

/// Memory `endless`, attached as the engine attaches a chat.
async fn attach(h: &Harness, e: &Endless, model: &str) {
    h.store
        .set_session_memory(h.id, MemoryMode::Endless)
        .await
        .unwrap();
    e.attach(
        h.id,
        Attached {
            model: h.resolve(model),
            effort: None,
            agent: "theuth".into(),
            store: None,
        },
    )
    .await
    .unwrap();
}

/// The session actor over the stored frame (as an engine start builds it).
async fn session(
    h: &Harness,
    model: &str,
    e: Option<&Endless>,
    pool: Option<Arc<Pool>>,
) -> ActorRef<Session> {
    let (epoch, head, history) = h.store.frame(h.id).await.unwrap();
    let mut s = Session::new(
        h.id,
        0,
        (h.resolve(model), None),
        None,
        SessionState::resume(epoch, head, history),
        Arc::new(AgentConfig {
            max_tool_rounds: 250,
            ..AgentConfig::default()
        }),
        Arc::default(),
        Arc::default(),
        Arc::default(),
        Arc::default(),
        Arc::default(),
        h.store.clone(),
        h.client.clone(),
        Auth::stub(&h.dir),
        h.rt.clone(),
        h.catalog.clone(),
        pool,
        Arc::new(Semaphore::new(1)),
        SessionResources::durable_arc(&h.dir),
    );
    s.shared.endless = e.cloned();
    Session::spawn(s)
}

async fn turn_with(
    h: &mut Harness,
    a: &ActorRef<Session>,
    text: &str,
    files: Vec<PathBuf>,
) -> Vec<AgentEvent> {
    a.tell(UserInput {
        channel: None,
        id: None,
        text: text.into(),
        files,
        ticket: None,
        scope: None,
        defer: false,
    })
    .await
    .unwrap();
    h.drain_until(|e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"))
        .await
}

async fn turn(h: &mut Harness, a: &ActorRef<Session>, text: &str) -> Vec<AgentEvent> {
    turn_with(h, a, text, Vec::new()).await
}

fn compactions(events: &[AgentEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, AgentEvent::Compacted { .. }))
        .count()
}

fn errors(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|e| match e {
            AgentEvent::Error { message, .. } => Some(message.clone()),
            _ => None,
        })
        .collect()
}

/// The main (round) calls: title, summary and harvest frames carry no `tools`.
async fn main_calls(h: &Harness) -> Vec<Value> {
    h.requests()
        .await
        .into_iter()
        .filter(|r| r["tools"].is_array())
        .collect()
}

fn msgs(b: &Value) -> Vec<String> {
    b["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|m| serde_json::to_string(m).unwrap())
        .collect()
}

/// Index of the first message where `b` departs from `a` (`a.len()` when
/// `a`'s messages are a prefix of `b`'s).
fn first_diff(a: &Value, b: &Value) -> usize {
    let (x, y) = (msgs(a), msgs(b));
    x.iter().zip(&y).take_while(|(p, q)| p == q).count()
}

/// `a`'s messages are a byte prefix of `b`'s.
fn is_prefix(a: &Value, b: &Value) -> bool {
    let (x, y) = (msgs(a), msgs(b));
    x.len() <= y.len() && first_diff(a, b) == x.len()
}

fn tool_names(b: &Value) -> Vec<String> {
    b["tools"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t["function"]["name"].as_str().map(str::to_owned))
        .collect()
}

/// `(tool call ids of assistant messages, tool_call_ids of tool messages)`.
fn call_ids(b: &Value) -> (Vec<String>, Vec<String>) {
    let all = b["messages"].as_array().cloned().unwrap_or_default();
    let calls = all
        .iter()
        .filter(|m| m["role"] == "assistant")
        .flat_map(|m| m["tool_calls"].as_array().cloned().unwrap_or_default())
        .filter_map(|c| c["id"].as_str().map(str::to_owned))
        .collect();
    let results = all
        .iter()
        .filter(|m| m["role"] == "tool")
        .filter_map(|m| m["tool_call_id"].as_str().map(str::to_owned))
        .collect();
    (calls, results)
}

/// The content of the tool message answering `id`.
fn tool_result(b: &Value, id: &str) -> Option<String> {
    b["messages"]
        .as_array()?
        .iter()
        .find(|m| m["role"] == "tool" && m["tool_call_id"] == id)
        .map(|m| match &m["content"] {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        })
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

/// The request's ledger snapshot block.
fn snapshot(b: &Value) -> Option<String> {
    texts(b)
        .into_iter()
        .find(|t| t.contains("<ledger_snapshot>"))
}

/// A 4×4 PNG of one colour.
fn png_bytes(rgb: [u8; 3]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut enc = png::Encoder::new(&mut bytes, 4, 4);
    enc.set_color(png::ColorType::Rgb);
    enc.set_depth(png::BitDepth::Eight);
    let mut w = enc.write_header().unwrap();
    w.write_image_data(&rgb.repeat(16)).unwrap();
    drop(w);
    bytes
}

/// A long assistant reply (a `talk` unit over 512 B: a draft the model refines).
fn long_reply(k: usize) -> String {
    let body: Vec<String> = (0..30)
        .map(|j| format!("step {j} of turn {k} verified the walrus{j} service;"))
        .collect();
    format!("REPLY-{k}: {}", body.join(" "))
}

async fn generation(e: &Endless, h: &Harness) -> crate::endless::pump::Generation {
    e.generation(h.id).await.unwrap().expect("a generation")
}

// endregion: --- Harness pieces

// region:    --- S1: cache stability over a long session

#[test]
fn eval_s1_thirty_turns_stay_byte_prefixes_within_each_generation() {
    crate::with_test_registry(s1());
}

/// S1: 201 D4 / §3. 30 turns, each a tool round (2.4 KB of arguments) and
/// a 1.5 KB reply (a draft the fake compactor refines in the background),
/// tail marks 8 KB / 20 KB so the tail trims every few turns. Within a
/// generation every request's messages are a byte prefix of the next
/// request's; system and tools never change; a trim changes nothing before
/// the ledger/view; every call travels with its result; the previous turn
/// is always sent word for word. Inputs name an older turn's rare word, so
/// the `<recall>` prefetch fires and is covered by the same prefix check.
async fn s1() {
    let (_reg_dir, _reg) = registry().await;
    let script = Script::new();
    let mut h = Harness::new(ScriptRef(script.clone())).await;
    add_models(&mut h, &[("evals1", false, 128_000)]);
    let model = "openai/evals1";
    let echo = EvalCompactor::new(Act::Echo);
    let cfg = EndlessConfig {
        tail_low_bytes: 8_000,
        tail_high_bytes: 20_000,
        batch_units: 1,
        profile: Profile::Standard,
        ..EndlessConfig::default()
    };
    let e = service(&h, cfg, &echo);
    attach(&h, &e, model).await;
    let actor = session(&h, model, Some(&e), None).await;
    let input = |k: usize| {
        format!(
            "Turn {k}: check the quokka{k} rollout; what did we say about quokka{}?",
            k.saturating_sub(5)
        )
    };
    let mut gens = Vec::new();
    for k in 0..30 {
        script.call(
            "todo",
            json!({"op": "view", "pad": format!("turn {k} {}", "pad ".repeat(600))}),
        );
        script.text(long_reply(k));
        let ev = turn(&mut h, &actor, &input(k)).await;
        assert_eq!(compactions(&ev), 0, "turn {k}: no classic cut");
        assert_eq!(errors(&ev), Vec::<String>::new(), "turn {k}");
        gens.push(generation(&e, &h).await.n);
    }
    let calls = main_calls(&h).await;
    assert_eq!(calls.len(), 60, "two main calls per turn");
    let mut trims = 0;
    let mut recall_notes = 0;
    for k in 0..30 {
        let (a, b) = (&calls[2 * k], &calls[2 * k + 1]);
        assert!(
            is_prefix(a, b),
            "turn {k}: the round call extends the first (diff at message {})",
            first_diff(a, b)
        );
        for r in [a, b] {
            let (c, res) = call_ids(r);
            assert_eq!(c, res, "turn {k}: every call with its result, in order");
            assert_eq!(system(r), system(&calls[0]), "turn {k}: system unchanged");
            assert_eq!(r["tools"], calls[0]["tools"], "turn {k}: tools unchanged");
        }
        if texts(a).iter().any(|t| t.starts_with("<recall>")) {
            recall_notes += 1;
        }
        if k == 0 {
            continue;
        }
        let prev = &calls[2 * k - 1];
        if gens[k] == gens[k - 1] {
            assert!(
                is_prefix(prev, a),
                "turn {k} (generation {}): not a byte extension of the previous request; first differing message {} of {}:\nprev: {}\nnow:  {}",
                gens[k],
                first_diff(prev, a),
                msgs(prev).len(),
                msgs(prev)
                    .get(first_diff(prev, a))
                    .cloned()
                    .unwrap_or_default()
                    .chars()
                    .take(400)
                    .collect::<String>(),
                msgs(a)
                    .get(first_diff(prev, a))
                    .cloned()
                    .unwrap_or_default()
                    .chars()
                    .take(400)
                    .collect::<String>(),
            );
        } else {
            trims += 1;
            assert!(
                first_diff(prev, a) >= 1,
                "turn {k}: a trim keeps the system message"
            );
        }
        let sent = texts(a).concat();
        assert!(
            sent.contains(&input(k - 1)),
            "turn {k}: the last input is sent"
        );
        assert!(
            sent.contains(&long_reply(k - 1)),
            "turn {k}: the last reply is sent"
        );
        assert!(
            call_ids(a).0.contains(&format!("ecall_{}", 2 * (k - 1))),
            "turn {k}: the last tool round is sent"
        );
    }
    assert!(trims >= 2, "at least two trims: generations {gens:?}");
    assert!(recall_notes >= 1, "the prefetch ran at least once");
    // The drafts were refined in the background, and refined lines reach a later view.
    assert!(echo.calls() > 0, "the compactor ran");
    let nodes = h.store.endless_nodes(h.id).await.unwrap();
    let refined: Vec<&crate::endless::tree::Node> = nodes
        .iter()
        .filter(|n| n.l == 0 && n.quality() == Quality::Model)
        .collect();
    assert!(!refined.is_empty(), "drafts were refined");
    let view = generation(&e, &h).await.render.text();
    assert!(
        refined
            .iter()
            .any(|n| view.contains(&crate::endless::fold::line(
                crate::endless::fold::Part { l: 0, i: n.i },
                &n.text
            ))),
        "a refined line is in the last generation's view:\n{view}"
    );
}

// endregion: --- S1

// region:    --- S2: restart

#[test]
fn eval_s2_a_restart_resends_the_same_prefix() {
    crate::with_test_registry(s2());
}

/// S2: 201 §4.3 "Restart". Three turns on a compactor that never answers
/// (drafts stay drafts), the tail under the low mark; then the session
/// actor and the service are dropped, a new service over the same store
/// attaches (as `resume_all` does) with a compactor that answers, a draft
/// is refined, and a fresh actor is built from the stored frame. The next
/// request extends the last one before the restart byte for byte: the same
/// generation, the same frozen view (the refined line waits), the same tail.
async fn s2() {
    let (_reg_dir, _reg) = registry().await;
    let script = Script::new();
    let mut h = Harness::new(ScriptRef(script.clone())).await;
    add_models(&mut h, &[("evals2", false, 128_000)]);
    let model = "openai/evals2";
    let cfg = EndlessConfig {
        batch_units: 1,
        profile: Profile::Standard,
        ..EndlessConfig::default()
    };
    let hang = EvalCompactor::new(Act::Hang);
    let e1 = service(&h, cfg.clone(), &hang);
    attach(&h, &e1, model).await;
    let a1 = session(&h, model, Some(&e1), None).await;
    for k in 0..3 {
        script.call("todo", json!({"op": "view", "pad": "x".repeat(1_000)}));
        script.text(long_reply(k));
        let ev = turn(&mut h, &a1, &format!("Before the restart, turn {k}.")).await;
        assert_eq!(errors(&ev), Vec::<String>::new());
    }
    let g_before = generation(&e1, &h).await;
    let before = main_calls(&h).await.pop().unwrap();
    crate::supervisor::transfer::stop_actor(&a1).await;
    e1.detach(h.id);
    drop(e1);

    let echo = EvalCompactor::new(Act::Echo);
    let e2 = service(&h, cfg, &echo);
    attach(&h, &e2, model).await;
    let store = h.store.clone();
    let id = h.id;
    let deadline = Instant::now() + Duration::from_secs(20);
    while !store
        .endless_nodes(id)
        .await
        .unwrap()
        .iter()
        .any(|n| n.l == 0 && n.quality() == Quality::Model)
    {
        assert!(
            Instant::now() < deadline,
            "no draft refined after the restart"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let a2 = session(&h, model, Some(&e2), None).await;
    script.text("After the restart.");
    let ev = turn(&mut h, &a2, "After the restart, turn 3.").await;
    assert_eq!(errors(&ev), Vec::<String>::new());
    let after = main_calls(&h).await.pop().unwrap();
    let g_after = generation(&e2, &h).await;
    assert_eq!(
        g_after.n, g_before.n,
        "no trim: the tail is under the low mark"
    );
    assert_eq!(
        g_after.render.text(),
        g_before.render.text(),
        "the frozen view"
    );
    assert_eq!(system(&before), system(&after), "system");
    assert_eq!(before["tools"], after["tools"], "tools");
    let d = first_diff(&before, &after);
    assert!(
        is_prefix(&before, &after),
        "the request after the restart departs at message {d} of {}:\nbefore: {}\nafter:  {}",
        msgs(&before).len(),
        msgs(&before)
            .get(d)
            .cloned()
            .unwrap_or_default()
            .chars()
            .take(400)
            .collect::<String>(),
        msgs(&after)
            .get(d)
            .cloned()
            .unwrap_or_default()
            .chars()
            .take(400)
            .collect::<String>(),
    );
}

// endregion: --- S2

// region:    --- S3: nothing waits on the summary model

#[test]
fn eval_s3_a_hung_compactor_delays_no_turn() {
    crate::with_test_registry(s3());
}

/// S3: 201 D5. A compactor that never answers; 10 turns of four tool
/// rounds and a long reply each (tail marks 8 KB / 16 KB: several trims).
/// Every turn's first main call goes out well under a second after the
/// input; every unit has a line; every generation's view is within the
/// fold's budget (`view_bytes` × (1 + slack)).
async fn s3() {
    let (_reg_dir, _reg) = registry().await;
    let script = Script::new();
    let mut h = Harness::new(ScriptRef(script.clone())).await;
    add_models(&mut h, &[("evals3", false, 128_000)]);
    let model = "openai/evals3";
    let cfg = EndlessConfig {
        view_bytes: 16_000,
        tail_low_bytes: 8_000,
        tail_high_bytes: 16_000,
        batch_units: 1,
        profile: Profile::Standard,
        ..EndlessConfig::default()
    };
    let budget = cfg.fold_cfg().high();
    let hang = EvalCompactor::new(Act::Hang);
    let e = service(&h, cfg, &hang);
    attach(&h, &e, model).await;
    let actor = session(&h, model, Some(&e), None).await;
    let mut waits = Vec::new();
    let mut gens = Vec::new();
    for k in 0..10 {
        for r in 0..4 {
            script.call(
                "todo",
                json!({"op": "view", "pad": format!("turn {k} round {r} {}", "q".repeat(1_200))}),
            );
        }
        script.text(long_reply(k));
        let n0 = script.main_calls();
        let t0 = Instant::now();
        let ev = turn(&mut h, &actor, &format!("Turn {k}: next batch, please.")).await;
        assert_eq!(errors(&ev), Vec::<String>::new(), "turn {k}");
        let first = script.at.lock()[n0];
        waits.push(first.saturating_duration_since(t0));
        let g = generation(&e, &h).await;
        assert!(
            g.render.text().len() <= budget,
            "turn {k}: generation {} view {} B over the budget {budget}",
            g.n,
            g.render.text().len()
        );
        gens.push(g.n);
    }
    for (k, w) in waits.iter().enumerate() {
        assert!(
            *w < Duration::from_millis(500),
            "turn {k} started after {w:?}: {waits:?}"
        );
    }
    assert!(
        gens.last().copied().unwrap_or(0) >= 3,
        "trims happened: {gens:?}"
    );
    assert!(hang.calls() > 0, "the compactor was asked");
    // Every unit has a line (drafts: nothing was refined).
    e.render_for_turn(h.id).await.unwrap();
    let units = h.store.endless_units(h.id).await.unwrap();
    let nodes = h.store.endless_nodes(h.id).await.unwrap();
    assert!(units.len() >= 60, "{} units", units.len());
    for u in &units {
        assert!(
            nodes.iter().any(|n| n.l == 0 && n.i == u.i),
            "unit {} has no line",
            u.i
        );
    }
    assert!(
        !nodes
            .iter()
            .any(|n| n.l == 0 && n.quality() == Quality::Model),
        "nothing refined by a hung compactor"
    );
}

// endregion: --- S3

// region:    --- S5: images

#[test]
fn eval_s5_images_ride_the_tail_then_leave_as_named_lines_and_zoom_back() {
    crate::with_test_registry(s5());
}

/// S5: 201 §8 / D9. A user photo and a `read_file` screenshot are sent as
/// real image parts while their turns are in the tail; once trims move them
/// out no request carries them, the view names them `[image: …]` with the
/// fake describer's line; two generations later `zoom(id, 1)` through the
/// loop sends the screenshot again; units before the images keep their rows.
async fn s5() {
    let (_reg_dir, _reg) = registry().await;
    let pics = crate::test_dir("eval-pics");
    let photo = pics.path().join("evalphoto.png");
    let shot = pics.path().join("evalshot.png");
    let (red, blue) = (png_bytes([255, 0, 0]), png_bytes([0, 0, 255]));
    std::fs::write(&photo, &red).unwrap();
    std::fs::write(&shot, &blue).unwrap();
    let (red64, blue64) = (BASE64.encode(&red), BASE64.encode(&blue));
    let script = Script::new();
    let mut h = Harness::new(ScriptRef(script.clone())).await;
    add_models(&mut h, &[("evalimg", true, 128_000)]);
    let model = "openai/evalimg";
    let seer = EvalCompactor::with(
        Act::Echo,
        true,
        None,
        crate::endless::pump::resolved_for_tests("fake/seer"),
    );
    let cfg = EndlessConfig {
        tail_low_bytes: 8_000,
        tail_high_bytes: 16_000,
        batch_units: 1,
        profile: Profile::Standard,
        ..EndlessConfig::default()
    };
    let e = service(&h, cfg, &seer);
    attach(&h, &e, model).await;
    let actor = session(&h, model, Some(&e), None).await;

    script.text("Noted.");
    turn(
        &mut h,
        &actor,
        "Hello, a plain first message about the walrus.",
    )
    .await;
    e.render_for_turn(h.id).await.unwrap();
    let units_before = h.store.endless_units(h.id).await.unwrap();
    assert!(!units_before.is_empty());

    script.text("I see a red square.");
    turn_with(&mut h, &actor, "Here is a photo.", vec![photo.clone()]).await;
    let r = main_calls(&h).await.pop().unwrap();
    assert!(
        image_urls(&r).iter().any(|u| u.ends_with(&red64)),
        "the photo is sent as an image"
    );
    script.call("read_file", json!({"path": shot.to_string_lossy()}));
    script.text("The screenshot is blue.");
    turn(&mut h, &actor, "Read the screenshot file.").await;
    let r = main_calls(&h).await.pop().unwrap();
    let urls = image_urls(&r);
    assert!(
        urls.iter().any(|u| u.ends_with(&blue64)),
        "the screenshot is sent"
    );
    assert!(
        urls.iter().any(|u| u.ends_with(&red64)),
        "the photo still rides the tail"
    );

    // Push both turns out of the tail, then two more generations.
    let big = |k: usize| format!("FILLER-{k} {}", "words for the tail ".repeat(400));
    let mut out_at = None;
    for k in 0..30 {
        script.text("Ok.");
        turn(&mut h, &actor, &big(k)).await;
        let r = main_calls(&h).await.pop().unwrap();
        let n = generation(&e, &h).await.n;
        if out_at.is_none() && image_urls(&r).is_empty() {
            out_at = Some(n);
        }
        if out_at.is_some_and(|o| n >= o + 2) {
            break;
        }
    }
    let out_at = out_at.expect("the images left the tail");
    let g = generation(&e, &h).await;
    assert!(g.n >= out_at + 2, "two generations after the images left");
    let view = g.render.text();
    let photo_line = view
        .lines()
        .find(|l| l.contains("user: [image: evalphoto.png]"))
        .unwrap_or_else(|| panic!("the photo named in the view:\n{view}"));
    assert!(
        photo_line.contains("a red square on white"),
        "described: {photo_line}"
    );
    // The tool picture's unit is named by the binary's own name (read_file
    // names it by a content hash, not the file name; the echo before it
    // carries the path).
    let shot_line = view
        .lines()
        .find(|l| l.contains("echo: [image: "))
        .unwrap_or_else(|| panic!("the screenshot named in the view:\n{view}"));
    assert!(
        shot_line.contains("a red square on white"),
        "described: {shot_line}"
    );
    assert!(seer.described.load(Ordering::SeqCst) >= 2);

    // zoom(id, 1) through the loop re-attaches the screenshot.
    let units = h.store.endless_units(h.id).await.unwrap();
    let shot_unit = units
        .iter()
        .find(|u| u.part & crate::endless::units::IMAGE_PART != 0)
        .expect("a tool-picture unit")
        .i;
    script.call("zoom", json!({"id": shot_unit, "n": 1}));
    script.text("That was the screenshot.");
    turn(&mut h, &actor, "Show me the screenshot again.").await;
    let r = main_calls(&h).await.pop().unwrap();
    let k = script.main_calls() - 2;
    let out = tool_result(&r, &format!("ecall_{k}")).unwrap_or_default();
    assert!(
        out.contains(&format!("{shot_unit}+1|")),
        "zoom answered: {out}"
    );
    assert!(
        image_urls(&r).iter().any(|u| u.ends_with(&blue64)),
        "zoom(id, 1) sends the screenshot again: {out}"
    );
    assert_eq!(
        &units[..units_before.len()],
        &units_before[..],
        "earlier units unchanged"
    );
}

// endregion: --- S5

// region:    --- S6: tool-list pinning through the loop

/// A registry tool that answers its own name.
struct EvalTool {
    name: &'static str,
    direct: bool,
}

impl tools::ErasedTool for EvalTool {
    fn spec(&self) -> tools::ToolSpec {
        tools::ToolSpec {
            name: self.name.into(),
            description: format!("eval tool {}", self.name),
            schema: json!({"type": "object", "properties": {}}),
            risk: tools::Risk::ReadOnly,
            direct: self.direct,
            returns: "str".into(),
        }
    }

    fn call(
        &self,
        _: Value,
        _: tools::ToolCx,
    ) -> futures::future::BoxFuture<'_, anyhow::Result<tools::ToolOutput>> {
        let name = self.name;
        Box::pin(async move { Ok(tools::ToolOutput::ok(format!("{name} ran"))) })
    }
}

async fn register(reg: &ActorRef<ToolRegistry>, name: &'static str, direct: bool) {
    reg.ask(tools::registry::RegisterTool(Arc::new(EvalTool {
        name,
        direct,
    })))
    .await
    .unwrap();
}

/// The python worker pool, as [`crate::repl::test_pool`] finds it; `None`
/// when there is none.
async fn worker_pool() -> Option<Arc<Pool>> {
    crate::repl::test_pool(1).await
}

#[test]
fn eval_s6_tool_list_changes_wait_for_the_trim_through_the_loop() {
    crate::with_test_registry(s6());
}

/// S6: 201 §11 / D12 through the loop with a real python worker pool (the
/// `## Runtime` index exists only with one; no product seam was needed).
/// A tool registered mid-generation is announced after the input and joins
/// the list at the next trim; a tool unregistered mid-generation stays
/// defined and refuses; a per-chat hide applies at once; an epochs session
/// sees every change at once.
async fn s6() {
    let (_reg_dir, reg) = registry().await;
    let Some(pool) = worker_pool().await else {
        eprintln!("S6: no python worker (monty/t) on PATH; scenario not run");
        return;
    };
    eprintln!("S6: running with a python worker pool");
    register(&reg, "eval_alpha", true).await;
    register(&reg, "eval_delta", true).await;
    let script = Script::new();
    let mut h = Harness::new(ScriptRef(script.clone())).await;
    add_models(&mut h, &[("evals6", false, 128_000)]);
    let model = "openai/evals6";
    let hang = EvalCompactor::new(Act::Hang);
    let e = service(&h, EndlessConfig::default(), &hang);
    attach(&h, &e, model).await;
    let actor = session(&h, model, Some(&e), Some(pool.clone())).await;

    script.text("One.");
    turn(&mut h, &actor, "First turn.").await;
    let r1 = main_calls(&h).await.pop().unwrap();
    let t1 = tool_names(&r1);
    assert!(
        system(&r1).contains("## Runtime"),
        "the runtime index is on"
    );
    for n in ["eval_alpha", "eval_delta"] {
        assert!(t1.iter().any(|t| t == n), "{n} in {t1:?}");
    }
    let g1 = generation(&e, &h).await.n;

    // Added mid-generation: announced, not listed.
    register(&reg, "eval_beta", true).await;
    register(&reg, "eval_gamma", false).await;
    script.text("Two.");
    turn(&mut h, &actor, "Second turn.").await;
    let r2 = main_calls(&h).await.pop().unwrap();
    assert_eq!(r2["tools"], r1["tools"], "the direct list is held");
    assert_eq!(system(&r2), system(&r1), "the index is held");
    assert!(
        is_prefix(&r1, &r2),
        "diff at message {}",
        first_diff(&r1, &r2)
    );
    let note = texts(&r2)
        .into_iter()
        .find(|t| t.starts_with("<tools>"))
        .expect("a <tools> note");
    assert!(
        note.contains("eval_beta") && note.contains("eval_gamma"),
        "{note}"
    );
    let sent = texts(&r2);
    let input = sent
        .iter()
        .position(|t| t.contains("Second turn."))
        .unwrap();
    let at = sent.iter().position(|t| t.starts_with("<tools>")).unwrap();
    assert!(at > input, "the note follows the input");

    // Removed mid-generation: still defined, the call refuses.
    reg.ask(tools::registry::UnregisterTool("eval_alpha".into()))
        .await
        .unwrap();
    script.call("eval_alpha", json!({}));
    script.text("Three.");
    turn(&mut h, &actor, "Third turn: call eval_alpha.").await;
    let calls = main_calls(&h).await;
    let (r3a, r3b) = (&calls[calls.len() - 2], &calls[calls.len() - 1]);
    assert_eq!(r3a["tools"], r1["tools"], "a removed tool stays defined");
    let k = script.main_calls() - 2;
    let out = tool_result(r3b, &format!("ecall_{k}")).unwrap_or_default();
    assert!(
        out.contains("no longer available"),
        "the call refuses: {out}"
    );
    assert!(is_prefix(&r2, r3a));

    // A per-chat hide applies now.
    reg.ask(tools::registry::SetHidden {
        session: h.id,
        tools: vec!["eval_delta".into()],
        hidden: true,
    })
    .await
    .unwrap();
    script.text("Four.");
    turn(&mut h, &actor, "Fourth turn.").await;
    let r4 = main_calls(&h).await.pop().unwrap();
    let t4 = tool_names(&r4);
    assert!(
        !t4.iter().any(|t| t == "eval_delta"),
        "hidden at once: {t4:?}"
    );
    assert!(
        t4.iter().any(|t| t == "eval_alpha"),
        "the removal still waits: {t4:?}"
    );
    assert!(
        !t4.iter().any(|t| t == "eval_beta"),
        "the addition still waits: {t4:?}"
    );
    assert_eq!(
        generation(&e, &h).await.n,
        g1,
        "all of it in one generation"
    );

    // A trim (the tail passes the high mark): the list is rebuilt.
    script.text("Five.");
    turn(
        &mut h,
        &actor,
        &format!("BIG {}", "filler words ".repeat(6_000)),
    )
    .await;
    script.text("Six.");
    turn(&mut h, &actor, "Sixth turn.").await;
    assert!(generation(&e, &h).await.n > g1, "a trim happened");
    let r6 = main_calls(&h).await.pop().unwrap();
    let t6 = tool_names(&r6);
    assert!(
        t6.iter().any(|t| t == "eval_beta"),
        "joined at the trim: {t6:?}"
    );
    assert!(
        !t6.iter().any(|t| t == "eval_alpha"),
        "left at the trim: {t6:?}"
    );
    assert!(
        system(&r6).contains("eval_gamma"),
        "the index has eval_gamma"
    );
    drop(actor);

    // Epochs: every change at once, no note.
    let script2 = Script::new();
    let mut h2 = Harness::new(ScriptRef(script2.clone())).await;
    add_models(&mut h2, &[("evals6e", false, 128_000)]);
    let actor = session(&h2, "openai/evals6e", None, Some(pool)).await;
    script2.text("One.");
    turn(&mut h2, &actor, "Epochs one.").await;
    register(&reg, "eval_epsilon", true).await;
    reg.ask(tools::registry::UnregisterTool("eval_beta".into()))
        .await
        .unwrap();
    script2.text("Two.");
    turn(&mut h2, &actor, "Epochs two.").await;
    let r = main_calls(&h2).await.pop().unwrap();
    let t = tool_names(&r);
    assert!(t.iter().any(|n| n == "eval_epsilon"), "{t:?}");
    assert!(!t.iter().any(|n| n == "eval_beta"), "{t:?}");
    assert!(!texts(&r).iter().any(|s| s.starts_with("<tools>")));
}

#[test]
fn eval_s7_builtin_switches_through_the_loop() {
    crate::with_test_registry(s7());
}

/// The last `<tools>` note of request `b`.
fn last_tools_note(b: &Value) -> Option<String> {
    texts(b)
        .into_iter()
        .rev()
        .find(|t| t.starts_with("<tools>"))
}

/// S7: 206 through the loop with a real python worker pool. Two builtins of
/// the `sandbox` category (one direct, one runtime-only) are switched off
/// harness-wide and per chat by category mid-generation: each applies at the
/// next call, is told once by the `<tools>` note and its call refuses as not
/// available (a hide, never a removal); switched back on they are additions
/// (announced, callable from python, direct again at the trim). The endless
/// tools stay throughout; another endless chat sees the harness-wide switch.
async fn s7() {
    let (_reg_dir, reg) = registry().await;
    let Some(pool) = worker_pool().await else {
        eprintln!("S7: no python worker (THEUTH_WORKER, monty/t on PATH); scenario skipped");
        return;
    };
    eprintln!("S7: running with a python worker pool");
    const DIRECT: &str = "sandbox_evaldirect";
    const RUNTIME: &str = "sandbox_evalrun";
    assert_eq!(tools::builtins::category_of(DIRECT), "sandbox");
    assert_eq!(tools::builtins::category_of(RUNTIME), "sandbox");
    register(&reg, DIRECT, true).await;
    register(&reg, RUNTIME, false).await;
    let has = |b: &Value, n: &str| tool_names(b).iter().any(|t| t == n);
    let endless_tools = |b: &Value| {
        for n in ["zoom", "recall", "date"] {
            assert!(has(b, n), "{n} stays: {:?}", tool_names(b));
        }
    };

    let script = Script::new();
    let mut h = Harness::new(ScriptRef(script.clone())).await;
    add_models(&mut h, &[("evals7", false, 128_000)]);
    let model = "openai/evals7";
    let hang = EvalCompactor::new(Act::Hang);
    let e = service(&h, EndlessConfig::default(), &hang);
    attach(&h, &e, model).await;
    let actor = session(&h, model, Some(&e), Some(pool.clone())).await;

    // A second endless chat on the same registry.
    let script2 = Script::new();
    let mut h2 = Harness::new(ScriptRef(script2.clone())).await;
    add_models(&mut h2, &[("evals7b", false, 128_000)]);
    let model2 = "openai/evals7b";
    let hang2 = EvalCompactor::new(Act::Hang);
    let e2 = service(&h2, EndlessConfig::default(), &hang2);
    attach(&h2, &e2, model2).await;
    let actor2 = session(&h2, model2, Some(&e2), Some(pool.clone())).await;

    script.text("One.");
    turn(&mut h, &actor, "First turn.").await;
    let r1 = main_calls(&h).await.pop().unwrap();
    assert!(has(&r1, DIRECT), "{:?}", tool_names(&r1));
    assert!(system(&r1).contains(RUNTIME), "the index has {RUNTIME}");
    endless_tools(&r1);
    let g1 = generation(&e, &h).await.n;
    script2.text("Other one.");
    turn(&mut h2, &actor2, "Other chat, first turn.").await;
    assert!(has(&main_calls(&h2).await.pop().unwrap(), DIRECT));

    // (1) Off always: gone at the next call, same generation, told once.
    reg.ask(tools::SetBuiltinsDisabled {
        names: vec![DIRECT.into()],
        disabled: true,
    })
    .await
    .unwrap();
    script.call(DIRECT, json!({}));
    script.text("Two.");
    turn(&mut h, &actor, "Second turn: call it.").await;
    let calls = main_calls(&h).await;
    let (r2a, r2b) = (&calls[calls.len() - 2], &calls[calls.len() - 1]);
    assert!(!has(r2a, DIRECT), "off at once: {:?}", tool_names(r2a));
    endless_tools(r2a);
    assert_eq!(generation(&e, &h).await.n, g1, "no trim");
    assert!(
        is_prefix(&r1, r2a),
        "diff at message {}",
        first_diff(&r1, r2a)
    );
    let note = last_tools_note(r2a).expect("a <tools> note");
    assert!(
        note.contains("Switched off for this chat") && note.contains(DIRECT),
        "{note}"
    );
    assert!(!note.contains("builtin"), "{note}");
    let sent = texts(r2a);
    let input = sent
        .iter()
        .position(|t| t.contains("Second turn: call it."))
        .unwrap();
    let at = sent.iter().rposition(|t| t.starts_with("<tools>")).unwrap();
    assert!(at > input, "the note follows the input");

    // (2) Its call refuses as not available: a hide, not a removal.
    let k = script.main_calls() - 2;
    let out = tool_result(r2b, &format!("ecall_{k}")).unwrap_or_default();
    assert!(
        out.contains("is not available in this session") && !out.contains("no longer available"),
        "the call refuses: {out}"
    );
    // (3) Every call keeps its result.
    let (mut ids, mut results) = call_ids(r2b);
    ids.sort();
    results.sort();
    assert_eq!(ids, results);

    // (8) The other chat sees the harness-wide switch at its next call.
    script2.text("Other two.");
    turn(&mut h2, &actor2, "Other chat, second turn.").await;
    let q2 = main_calls(&h2).await.pop().unwrap();
    assert!(!has(&q2, DIRECT), "{:?}", tool_names(&q2));
    endless_tools(&q2);

    // (4) The whole category off for this chat: the runtime-only one leaves
    // the index; only it is told (the direct one was told already).
    reg.ask(tools::registry::SetHidden {
        session: h.id,
        tools: vec![tools::category_key("sandbox")],
        hidden: true,
    })
    .await
    .unwrap();
    script.text("Three.");
    turn(&mut h, &actor, "Third turn.").await;
    let r3 = main_calls(&h).await.pop().unwrap();
    assert!(!system(&r3).contains(RUNTIME), "left the index");
    assert!(!has(&r3, DIRECT));
    endless_tools(&r3);
    let note = last_tools_note(&r3).expect("a <tools> note");
    assert!(note.contains(RUNTIME) && !note.contains(DIRECT), "{note}");
    assert_eq!(generation(&e, &h).await.n, g1, "no trim");

    // (5) Both back on: additions — the direct list holds, the note names
    // both, python calls the runtime-only one now.
    reg.ask(tools::SetBuiltinsDisabled {
        names: vec![DIRECT.into()],
        disabled: false,
    })
    .await
    .unwrap();
    reg.ask(tools::registry::SetHidden {
        session: h.id,
        tools: vec![tools::category_key("sandbox")],
        hidden: false,
    })
    .await
    .unwrap();
    script.call(
        repl::NAME,
        json!({"code": format!("print(await {RUNTIME}())")}),
    );
    script.text("Four.");
    turn(&mut h, &actor, "Fourth turn.").await;
    let calls = main_calls(&h).await;
    let (r4a, r4b) = (&calls[calls.len() - 2], &calls[calls.len() - 1]);
    assert_eq!(r4a["tools"], r3["tools"], "the direct list is held");
    assert_eq!(generation(&e, &h).await.n, g1, "no trim");
    let note = last_tools_note(r4a).expect("a <tools> note");
    assert!(
        note.contains("New tools") && note.contains(DIRECT) && note.contains(RUNTIME),
        "{note}"
    );
    assert!(!note.contains("Switched off"), "{note}");
    let k = script.main_calls() - 2;
    let out = tool_result(r4b, &format!("ecall_{k}")).unwrap_or_default();
    assert!(
        out.contains(&format!("{RUNTIME} ran")),
        "python calls it: {out}"
    );

    // (6) A trim: both are back in the request's tool part.
    script.text("Five.");
    turn(
        &mut h,
        &actor,
        &format!("BIG {}", "filler words ".repeat(6_000)),
    )
    .await;
    script.text("Six.");
    turn(&mut h, &actor, "Sixth turn.").await;
    assert!(generation(&e, &h).await.n > g1, "a trim happened");
    let r6 = main_calls(&h).await.pop().unwrap();
    assert!(has(&r6, DIRECT), "direct again: {:?}", tool_names(&r6));
    assert!(system(&r6).contains(RUNTIME), "the index has {RUNTIME}");
    endless_tools(&r6);
    drop(actor2);
    drop(actor);
}

// endregion: --- S6

// region:    --- S9: the ledger through turns

/// The id of the suggestion whose line mentions `word` in the request's snapshot.
fn suggestion_id(b: &Value, word: &str) -> Option<String> {
    let snap = snapshot(b)?;
    let (_, rest) = snap.split_once("Suggested (unconfirmed)")?;
    let line = rest.lines().find(|l| l.contains(word))?;
    let (_, after) = line.split_once('[')?;
    Some(after.split_once(']')?.0.to_owned())
}

#[test]
fn eval_s9_the_ledger_through_turns_and_trims() {
    crate::with_test_registry(s9());
}

/// S9: 201 §9 / §10 through the loop. Three user lines with cues become
/// suggestions at the first trim; the model accepts one and drops one with
/// the `ledger` tool; the third lapses two trims after it appeared; the
/// `todo` list is mirrored into Open tasks and closed items follow; over
/// `ledger_bytes` (1,000) closed tasks leave the snapshot first and stay
/// reachable by `recall`; within each generation every request extends the
/// previous one (the `<ledger>` and `<recall>` notes come after the input).
async fn s9() {
    let (_reg_dir, _reg) = registry().await;
    let script = Script::new();
    let mut h = Harness::new(ScriptRef(script.clone())).await;
    add_models(&mut h, &[("evals9", false, 128_000)]);
    let model = "openai/evals9";
    let cfg = EndlessConfig {
        tail_low_bytes: 8_000,
        tail_high_bytes: 16_000,
        ledger_bytes: 1_000,
        batch_units: 1,
        profile: Profile::Standard,
        ..EndlessConfig::default()
    };
    let hang = EvalCompactor::new(Act::Hang);
    let e = service(&h, cfg, &hang);
    attach(&h, &e, model).await;
    let actor = session(&h, model, Some(&e), None).await;
    let mut gens: Vec<u32> = Vec::new();
    let mut firsts: Vec<usize> = Vec::new();
    // Each generation's frozen snapshot, as its first turn saw it.
    let mut snaps: std::collections::BTreeMap<u32, String> = std::collections::BTreeMap::new();
    let big = |k: usize| format!("FILLER-{k} {}", "padding words here ".repeat(400));
    macro_rules! go {
        ($text:expr) => {{
            firsts.push(script.main_calls());
            turn(&mut h, &actor, $text).await;
            let g = generation(&e, &h).await;
            gens.push(g.n);
            snaps
                .entry(g.n)
                .or_insert_with(|| g.ledger.clone().unwrap_or_default());
        }};
    }

    script.call(
        "todo",
        json!({"op": "add", "items": ["migrate the schema", "write the runbook", "rotate the keys"]}),
    );
    for text in [
        "We need the gateway done. You must never deploy on fridays.",
        "Let's go with postgres for the storage layer.",
        "Make sure the backups are encrypted at rest.",
    ] {
        script.text("Noted.");
        go!(text);
    }
    // Trim into a new generation: the cues become suggestions.
    let mut k = 0;
    while generation(&e, &h).await.n < 2 {
        script.text("Ok.");
        go!(&big(k));
        k += 1;
        assert!(k < 10, "no trim");
    }
    let g_sug = generation(&e, &h).await.n;
    let snap = snaps[&g_sug].clone();
    for w in ["fridays", "postgres", "backups"] {
        assert!(snap.contains(w), "suggested {w}:\n{snap}");
    }
    assert!(
        snap.contains("migrate the schema"),
        "todo mirrored:\n{snap}"
    );

    // Accept, drop, close two tasks, add goals past the budget.
    script.pick(|b| {
        suggestion_id(b, "postgres").map(|id| {
            (
                "ledger".into(),
                json!({"section": "decisions", "op": "accept", "id": id}),
            )
        })
    });
    script.pick(|b| {
        suggestion_id(b, "fridays").map(|id| {
            (
                "ledger".into(),
                json!({"section": "constraints", "op": "drop", "id": id}),
            )
        })
    });
    script.call("todo", json!({"op": "done", "task": "migrate the schema"}));
    script.call("todo", json!({"op": "done", "task": "write the runbook"}));
    // Seven goals: with the two closed tasks the snapshot passes 1,000 B;
    // without them it fits (open entries are never hidden).
    for g in 0..7 {
        let tail = if g == 6 {
            " in two phases, reviewed weekly"
        } else {
            ""
        };
        script.call(
            "ledger",
            json!({"section": "goals", "op": "add", "text": format!("goal {g}: ship gateway part {g} with tests and a written rollback plan for every region{tail}")}),
        );
    }
    script.text("Recorded.");
    go!("Please record the decisions.");
    let g_rec = *gens.last().unwrap();
    while generation(&e, &h).await.n < (g_rec + 1).max(g_sug + 2) {
        script.text("Ok.");
        go!(&big(k));
        k += 1;
        assert!(k < 30, "no further trims");
    }
    let stored = h
        .store
        .endless_ledger(h.id)
        .await
        .unwrap()
        .unwrap_or_default();
    let ctx = format!(
        "suggestions at {g_sug}, record turn in {g_rec}, per turn {gens:?}\nstored: {stored}"
    );
    // Lapse: still suggested one trim later, gone two trims later.
    let next = &snaps[&(g_sug + 1)];
    let suggested = next
        .split_once("Suggested (unconfirmed)")
        .map_or("", |s| s.1);
    assert!(
        suggested.contains("backups"),
        "still suggested:\n{next}\n{ctx}"
    );
    let later = &snaps[&(g_sug + 2)];
    assert!(
        !later.contains("backups"),
        "lapsed after two trims:\n{later}\n{ctx}"
    );
    // Accept and drop, todo mirror, budget: the snapshot after the record turn.
    let s = &snaps[&(g_rec + 1)];
    let (body, _) = s
        .split_once("Suggested (unconfirmed)")
        .unwrap_or((s.as_str(), ""));
    let decisions = body.split_once("Decisions").map_or("", |d| d.1);
    assert!(decisions.contains("postgres"), "accepted:\n{s}\n{ctx}");
    assert!(!s.contains("fridays"), "dropped:\n{s}\n{ctx}");
    for g in 0..7 {
        assert!(s.contains(&format!("goal {g}:")), "open goal {g}:\n{s}");
    }
    assert!(s.contains("rotate the keys"), "open task:\n{s}");
    assert!(s.len() <= 1_000, "{} B over the budget:\n{s}", s.len());
    assert!(
        !s.contains("migrate the schema") && !s.contains("write the runbook"),
        "closed tasks leave the snapshot first over the budget:\n{s}"
    );
    let hits = e.recall(h.id, "migrate schema", 5).await.unwrap();
    assert!(
        hits.iter().any(|l| l.text.contains("migrate")),
        "recall reaches the closed task: {hits:?}"
    );

    // Byte stability: within a generation each turn's first request extends
    // the previous turn's last request.
    let calls = main_calls(&h).await;
    for t in 1..firsts.len() {
        if gens[t] != gens[t - 1] {
            continue;
        }
        let (prev, now) = (&calls[firsts[t] - 1], &calls[firsts[t]]);
        assert!(
            is_prefix(prev, now),
            "turn {t} (generation {}): departs at message {}",
            gens[t],
            first_diff(prev, now)
        );
    }
}

// endregion: --- S9

// region:    --- S10: one round larger than the window

/// What the user sees of a turn whose one tool round is larger than the
/// model's window, then of a plain next turn: `(errors, compactions, main
/// calls, stop reasons)` per turn.
async fn one_huge_round(
    endless: bool,
    model_id: &str,
) -> Vec<(Vec<String>, usize, usize, Vec<String>)> {
    let script = Script::new();
    let mut h = Harness::new(ScriptRef(script.clone())).await;
    add_models(&mut h, &[(model_id, false, 128_000)]);
    let model = format!("openai/{model_id}");
    let hang = EvalCompactor::new(Act::Hang);
    let e = service(&h, EndlessConfig::default(), &hang);
    if endless {
        attach(&h, &e, &model).await;
    }
    let actor = session(&h, &model, Some(&e), None).await;
    script.call("todo", json!({"op": "view", "pad": "p".repeat(600_000)}));
    script.text("Read it.");
    let mut out = Vec::new();
    for text in ["Read the huge file.", "Hello again, a small question."] {
        let n0 = script.main_calls();
        let ev = turn(&mut h, &actor, text).await;
        let stops = ev
            .iter()
            .filter_map(|e| match e {
                AgentEvent::TurnFinished { stop_reason, .. } => Some(stop_reason.clone()),
                _ => None,
            })
            .collect();
        out.push((
            errors(&ev),
            compactions(&ev),
            script.main_calls() - n0,
            stops,
        ));
        script.text("Fine.");
    }
    out
}

#[test]
fn eval_s10_one_round_over_the_window_endless_vs_epochs() {
    crate::with_test_registry(async {
        let (_reg_dir, _reg) = registry().await;
        let endless = one_huge_round(true, "evals10e").await;
        let epochs = one_huge_round(false, "evals10c").await;
        eprintln!("S10 endless: {endless:#?}");
        eprintln!("S10 epochs:  {epochs:#?}");
        // Characterisation, observed at head 0bdf0ebb9 (201-eval-E2.md §S10):
        // `(errors, compactions, main calls that reached the provider, stop)`.
        let shape = |r: &[(Vec<String>, usize, usize, Vec<String>)]| {
            r.iter()
                .map(|(e, c, n, s)| (e.len(), *c, *n, s.clone()))
                .collect::<Vec<_>>()
        };
        let stop = |s: &str| vec![s.to_owned()];
        assert_eq!(
            shape(&endless),
            [(1, 1, 1, stop("error")), (0, 1, 1, stop("stop"))],
            "endless"
        );
        assert_eq!(
            shape(&epochs),
            [(1, 1, 3, stop("error")), (0, 1, 1, stop("stop"))],
            "epochs"
        );
        for r in [&endless, &epochs] {
            let e = &r[0].0[0];
            assert!(
                e.starts_with("model: prompt is too long")
                    && e.contains("theuth preflight, not sent"),
                "{e}"
            );
        }
    });
}

// endregion: --- S10

// region:    --- S4: epochs and meeting frames against the base

/// A request body with the run's variable bytes replaced: the scratch dir,
/// the session id, uuids, timestamps, `<time now=…>` stamps, the dated line.
fn normalise(h: &Harness, b: &Value) -> String {
    let mut s = serde_json::to_string_pretty(b).unwrap();
    s = s.replace(&h.dir.to_string_lossy().to_string(), "<DIR>");
    s = s.replace(&h.id.to_string(), "<SID>");
    for (re, to) in [
        (
            r"[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}",
            "<UUID>",
        ),
        (r#"now=\\"[^\\]*\\""#, "now=<T>"),
        (
            r"\d{4}-\d{2}-\d{2}[T ][0-9:.]+(Z|[+-]\d{2}:?\d{2})?",
            "<TS>",
        ),
        (r"Date: [^\\]*", "Date: <D>"),
        (r"Working directory: [^\\]*", "Working directory: <CWD>"),
        (r"theuth-[a-z-]+-<UUID>", "<SCRATCH>"),
    ] {
        s = regex::Regex::new(re)
            .unwrap()
            .replace_all(&s, to)
            .into_owned();
    }
    s
}

/// The epochs / meeting script: a plain turn, a turn with one `todo` round,
/// a `/compact` cut, a plain turn. Returns every main request, normalised.
async fn frames_of(kind: &str) -> Vec<String> {
    eprintln!("S4: running {kind}");
    let script = Script::new();
    let mut h = Harness::new(ScriptRef(script.clone())).await;
    let model = "openai/textonly";
    let hang = EvalCompactor::new(Act::Hang);
    let e = service(&h, EndlessConfig::default(), &hang);
    let with_service = kind != "epochs-bare";
    if kind == "meeting" {
        // A meeting desk's tree: memory `endless`, filter `meeting`, attached.
        h.store
            .set_endless_meta(h.id, UnitFilter::Meeting, 0)
            .await
            .unwrap();
        attach(&h, &e, model).await;
    }
    let (epoch, head, history) = h.store.frame(h.id).await.unwrap();
    let mut s = Session::new(
        h.id,
        0,
        (h.resolve(model), None),
        None,
        SessionState::resume(epoch, head, history),
        Arc::new(AgentConfig::default()),
        Arc::default(),
        Arc::default(),
        Arc::default(),
        Arc::default(),
        Arc::default(),
        h.store.clone(),
        h.client.clone(),
        Auth::stub(&h.dir),
        h.rt.clone(),
        h.catalog.clone(),
        None,
        Arc::new(Semaphore::new(1)),
        SessionResources::durable_arc(&h.dir),
    );
    if with_service {
        s.shared.endless = Some(e.clone());
    }
    if kind == "meeting" {
        s.shared
            .meeting_filter
            .store(true, std::sync::atomic::Ordering::Relaxed);
    }
    let actor = Session::spawn(s);
    script.text("First answer.");
    turn(&mut h, &actor, "First question.").await;
    script.call("todo", json!({"op": "add", "items": ["check the frames"]}));
    script.text("Second answer.");
    turn(&mut h, &actor, "Second question.").await;
    actor
        .ask(CompactNow {
            style: Some(CompactStyle::Cut),
            head: None,
        })
        .await
        .expect("a cut");
    // The mini-turn hands the state back with its own `settled`.
    let ev = h
        .drain_until(
            |e| matches!(e, AgentEvent::ContextChanged { reason, .. } if reason == "settled"),
        )
        .await;
    let cut = format!("compacted events: {}", compactions(&ev));
    script.text("Third answer.");
    turn(&mut h, &actor, "Third question.").await;
    std::iter::once(cut)
        .chain(main_calls(&h).await.iter().map(|b| normalise(&h, b)))
        .collect()
}

/// S4: 201 §12 "Legacy". The frames an `epochs` session (with and without
/// the endless service in the process) and a meeting-filtered session send
/// for the same scripted input, compared with the frames the same script
/// produced at the base `5b88548ff` (captured there into
/// `eval_golden_s4.json` by running this module with
/// `EVAL_S4_CAPTURE=<path>`).
#[test]
fn eval_s4_epochs_and_meeting_frames_match_the_base() {
    crate::with_test_registry(async {
        let (_reg_dir, _reg) = registry().await;
        let mut got = serde_json::Map::new();
        for kind in ["epochs-bare", "epochs-service", "meeting"] {
            got.insert(kind.to_owned(), json!(frames_of(kind).await));
        }
        let got = Value::Object(got);
        if let Some(path) = std::env::var_os("EVAL_S4_CAPTURE") {
            std::fs::write(path, serde_json::to_string_pretty(&got).unwrap()).unwrap();
            return;
        }
        let want: Value = serde_json::from_str(include_str!("eval_golden_s4.json")).unwrap();
        // The golden was captured in the base worktree: its working
        // directory line is masked like this run's.
        let cwd = regex::Regex::new(r"Working directory: [^\\]*").unwrap();
        let mut diffs = Vec::new();
        for kind in ["epochs-bare", "epochs-service", "meeting"] {
            let (g, w) = (&got[kind], &want[kind]);
            assert_eq!(
                g.as_array().map(Vec::len),
                w.as_array().map(Vec::len),
                "{kind}: request count"
            );
            for (i, (a, b)) in g
                .as_array()
                .unwrap()
                .iter()
                .zip(w.as_array().unwrap())
                .enumerate()
            {
                let base = cwd
                    .replace_all(b.as_str().unwrap(), "Working directory: <CWD>")
                    .into_owned();
                let (a, b): (Vec<&str>, Vec<&str>) = (
                    a.as_str().unwrap().lines().collect(),
                    base.lines().collect(),
                );
                let (sa, sb): (HashSet<&str>, HashSet<&str>) =
                    (a.iter().copied().collect(), b.iter().copied().collect());
                let removed: Vec<&&str> = b.iter().filter(|l| !sa.contains(*l)).collect();
                let added: Vec<&&str> = a.iter().filter(|l| !sb.contains(*l)).collect();
                if !removed.is_empty() || !added.is_empty() || a.len() != b.len() {
                    diffs.push(format!(
                        "{kind} request {i}: {} lines at base, {} now\n  base only: {removed:?}\n  head only: {added:?}",
                        b.len(),
                        a.len()
                    ));
                }
            }
        }
        assert!(
            diffs.is_empty(),
            "frames differ from the base:\n{}",
            diffs.join("\n")
        );
    });
}

// endregion: --- S4
