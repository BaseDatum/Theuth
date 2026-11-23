//! 201 evaluation E2 (service level), written by the evaluator, not the
//! builders: the scenarios of `docs/design/201-eval-E2.md` that run on the
//! [`Endless`] service over a real store with fake compactors. Also the one
//! test seam the loop-level eval tests need ([`set_compactor`]).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use futures::future::BoxFuture;
use genai::chat::{ChatMessage, ContentPart, MessageContent, ToolResponse};
use parking_lot::Mutex;
use runtime::{Runtime, SessionId};

use super::compactor::{Compactor, Reply};
use super::tree::{Kind, Quality};
use super::{Attached, Endless, EndlessConfig, MemoryMode};
use crate::store::Store;
use crate::store::endless::UnitRow;

/// Test seam for the loop-level eval tests (`loop_::endless_eval_tests`):
/// `factory` serves every compactor of the fresh service `e` (the loop
/// tests cannot reach the private `inner` the service tests set directly).
pub(crate) fn set_compactor(e: &mut Endless, factory: super::pump::CompactorFactory) {
    Arc::get_mut(&mut e.inner)
        .expect("a fresh Endless")
        .compactor_for_tests = Some(factory);
}

/// What a fake compactor does with a call.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Act {
    /// Answer a grounded line (the input's head), batches line by line.
    Echo,
    /// Never answer.
    Hang,
}

/// A scripted compactor: echoes or hangs; optionally sees images (a
/// describer) or sits on a local endpoint; records every `small` call.
pub(crate) struct EvalCompactor {
    pub(crate) model: models::Resolved,
    pub(crate) act: Act,
    pub(crate) vision: bool,
    pub(crate) local: Option<String>,
    pub(crate) calls: AtomicU32,
    pub(crate) described: AtomicU32,
    /// `(ctx, step)` of every `compress_small` call.
    pub(crate) small: Mutex<Vec<(String, String)>>,
}

impl EvalCompactor {
    pub(crate) fn new(act: Act) -> Arc<Self> {
        Self::with(
            act,
            false,
            None,
            super::pump::resolved_for_tests("fake/eval"),
        )
    }

    pub(crate) fn with(
        act: Act,
        vision: bool,
        local: Option<&str>,
        model: models::Resolved,
    ) -> Arc<Self> {
        Arc::new(Self {
            model,
            act,
            vision,
            local: local.map(str::to_owned),
            calls: AtomicU32::new(0),
            described: AtomicU32::new(0),
            small: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn calls(&self) -> u32 {
        self.calls.load(Ordering::SeqCst)
    }

    pub(crate) fn factory(self: &Arc<Self>) -> super::pump::CompactorFactory {
        let me = self.clone();
        Arc::new(move |_: &str| -> Arc<dyn Compactor> { me.clone() })
    }
}

/// The first text of the first message of a compactor conversation.
fn step_of(convo: &[ChatMessage]) -> String {
    convo
        .first()
        .and_then(|m| m.content.iter().next())
        .and_then(|p| match p {
            ContentPart::Text(t) => Some(t.clone()),
            _ => None,
        })
        .unwrap_or_default()
}

/// A grounded reply for a standard step (unit, merge or batch).
fn echo_standard(step: &str) -> String {
    if step.contains("Compress each of these") {
        return step
            .split("\n\n[")
            .skip(1)
            .filter_map(|seg| {
                let (k, rest) = seg.split_once("] ")?;
                let body = rest.split_once(": ").map_or(rest, |(_, b)| b);
                Some(format!(
                    "{k}| {}",
                    super::guards::cut_utf8(body, 300).replace('\n', " ")
                ))
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
    let input = step.rsplit_once("bytes:\n").map_or(step, |(_, s)| s);
    super::guards::cut_utf8(input, 400).replace('\n', " ")
}

/// A grounded one-line reply for a `small` step; ungrounded for a `BAD` unit.
fn echo_small(step: &str) -> String {
    if step.contains("BAD") {
        return "zebra crossing painted blue".into();
    }
    let body = step.split_once('\n').map_or(step, |(_, b)| b);
    let body = body.lines().next().unwrap_or_default();
    let body = body.split_once(": ").map_or(body, |(_, b)| b);
    super::guards::cut_utf8(body, 200).to_owned()
}

fn reply(text: String) -> Reply {
    Reply {
        text,
        usage: serde_json::from_str(r#"{"prompt_tokens":1000,"completion_tokens":50}"#).ok(),
        latency: Duration::from_millis(1),
        ..Default::default()
    }
}

impl Compactor for EvalCompactor {
    fn compress<'a>(
        &'a self,
        _ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.act == Act::Hang {
            return Box::pin(futures::future::pending());
        }
        let text = echo_standard(&step_of(convo));
        Box::pin(async move { Ok(reply(text)) })
    }

    fn compress_small<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        let step = step_of(convo);
        self.small.lock().push((ctx.to_owned(), step.clone()));
        if self.act == Act::Hang {
            return Box::pin(futures::future::pending());
        }
        let text = echo_small(&step);
        Box::pin(async move { Ok(reply(text)) })
    }

    fn model(&self) -> &models::Resolved {
        &self.model
    }

    fn local(&self) -> Option<String> {
        self.local.clone()
    }

    fn accepts_images(&self) -> bool {
        self.vision
    }

    fn describe_image<'a>(
        &'a self,
        _image: &'a genai::chat::Binary,
    ) -> BoxFuture<'a, Result<Reply, String>> {
        self.described.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(reply("a red square on white\n".into())) })
    }
}

async fn harness(
    cfg: EndlessConfig,
    c: Arc<EvalCompactor>,
) -> (crate::TestDir, Endless, Store, SessionId) {
    let scratch = crate::test_dir("endless-eval");
    let dir = scratch.path().to_path_buf();
    let store = Store::open(&dir.join("sessions.sqlite"), None)
        .await
        .unwrap();
    let mut endless = Endless::new(
        store.clone(),
        Runtime::new(figment::Figment::new(), dir.clone()),
        genai::Client::builder().build().unwrap(),
        crate::supervisor::Auth::stub(&dir),
        Arc::new(models::Catalog::load(&dir)),
        cfg,
        None,
    );
    set_compactor(&mut endless, c.factory());
    let id = SessionId::new();
    store
        .create_session(id, "fake/session", None, None, None)
        .await
        .unwrap();
    store
        .set_session_memory(id, MemoryMode::Endless)
        .await
        .unwrap();
    (scratch, endless, store, id)
}

fn attached() -> Attached {
    Attached {
        model: super::pump::resolved_for_tests("fake/session"),
        effort: None,
        agent: "theuth".into(),
        store: None,
    }
}

async fn eventually<F, Fut>(what: &str, secs: u64, mut f: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let end = std::time::Instant::now() + Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        if f().await {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out waiting for {what}");
}

/// Whether `text` shows unit `i` on a line of its own (`i+1|…`).
fn alone(text: &str, i: u64) -> bool {
    let head = format!("{i}+1|");
    text.lines().any(|l| l.starts_with(&head))
}

// region:    --- S5: rows stored by the pre-201 derivation

/// S5 (last item): a session whose units were stored by the derivation at
/// `5b88548ff` (`git show 5b88548ff:crates/agent/src/endless/units.rs`:
/// every text part a unit, an empty caption included; a binary `[image]`;
/// a tool response one `echo`, its images none; a vision note `user`)
/// loads without an index shift: the stored rows stay as they were, the
/// next row continues at the next index, and each old index still opens
/// its own message.
#[tokio::test]
async fn eval_s5_pre_201_unit_rows_load_without_index_shifts() {
    let (_dir, endless, store, id) =
        harness(EndlessConfig::default(), EvalCompactor::new(Act::Hang)).await;
    let png = |n: &str| genai::chat::Binary::from_base64("image/png", "iVBORw0K", Some(n.into()));
    let note = "<vision_observation>\nm2.p1: a red square\n</vision_observation>";
    let rows = [
        ChatMessage::user("hello before the images"),
        ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text(String::new()),
            ContentPart::Binary(png("old.png")),
        ])),
        ChatMessage::from(vec![
            ToolResponse::new("c1", "shot taken").with_images(vec![png("/tmp/oldshot.png")]),
        ]),
        ChatMessage::user(note),
        ChatMessage::assistant("after the images, the deploy went fine"),
    ];
    let mut seqs = Vec::new();
    for m in &rows {
        seqs.push(store.append(id, m).await.unwrap());
    }
    // The base derivation of those rows, as it stored them.
    let old: Vec<(i64, u16, Kind, &str)> = vec![
        (seqs[0], 0, Kind::User, "hello before the images"),
        (seqs[1], 0, Kind::User, ""),
        (seqs[1], 1, Kind::User, "[image]"),
        (seqs[2], 0, Kind::Echo, "shot taken"),
        (seqs[3], 0, Kind::User, note),
        (
            seqs[4],
            0,
            Kind::Talk,
            "after the images, the deploy went fine",
        ),
    ];
    let stored: Vec<UnitRow> = old
        .iter()
        .enumerate()
        .map(|(i, (seq, part, kind, text))| UnitRow {
            i: i as u64,
            seq: *seq,
            part: *part,
            kind: *kind,
            size: (kind.as_str().len() + 2 + text.len()) as u32,
            turn: 0,
        })
        .collect();
    store.endless_units_put(id, &stored).await.unwrap();

    endless.attach(id, attached()).await.unwrap();
    let after = store
        .append(id, &ChatMessage::user("new message after the upgrade"))
        .await
        .unwrap();
    endless.render_for_turn(id).await.unwrap();

    let units = store.endless_units(id).await.unwrap();
    assert_eq!(&units[..6], &stored[..], "the stored rows are untouched");
    assert_eq!(units.len(), 7, "{units:#?}");
    assert_eq!(
        (units[6].i, units[6].seq, units[6].part, units[6].kind),
        (6, after, 0, Kind::User),
        "the next row continues at the next index"
    );
    for (i, want) in [
        (0, "hello before the images"),
        (2, "[image: old.png]"),
        (3, "shot taken"),
        (5, "after the images"),
        (6, "new message after the upgrade"),
    ] {
        let lines = endless.zoom(id, i, 1).await.unwrap();
        let text: String = lines.iter().map(|l| l.text.clone()).collect();
        assert!(text.contains(want), "zoom({i},1) = {text:?}, want {want:?}");
    }
    // Every unit has its line (drafted at load for the node-less old ones).
    let l0: Vec<u64> = store
        .endless_nodes(id)
        .await
        .unwrap()
        .iter()
        .filter(|n| n.l == 0)
        .map(|n| n.i)
        .collect();
    for u in &units {
        assert!(l0.contains(&u.i), "unit {} has no line: {l0:?}", u.i);
    }
}

// endregion: --- S5

// region:    --- S7: ageing by use over real merges

/// 60 free user lines (≈ 420 B each, ≈ 25 KB) over the 16 KB view floor:
/// pairs of them do not fit one 512 B line, so every merge needs the
/// model; with `merge_at` 0.5 the merges are due at once. Generation 1 is
/// cut after the pump built level-1 lines; a unit merged there is zoomed;
/// `(alone in generation 2, alone in generation 3, generation 1 text)`.
async fn zoomed_across_real_merges(age_by_use: bool) -> (bool, bool, String) {
    let cfg = EndlessConfig {
        view_bytes: 16_000,
        merge_at: 0.5,
        batch_units: 1,
        profile: super::Profile::Standard,
        age_by_use,
        ..EndlessConfig::default()
    };
    let (_dir, endless, store, id) = harness(cfg, EvalCompactor::new(Act::Echo)).await;
    for k in 0..60 {
        let text = format!(
            "tag{k}: {}",
            "keep the nginx worker_processes line ".repeat(11)
        );
        store.append(id, &ChatMessage::user(text)).await.unwrap();
    }
    endless.attach(id, attached()).await.unwrap();
    // Generation 1 is cut once the view fits on built merges (no hard-cap
    // prefix run, at least one merged line).
    let end = std::time::Instant::now() + Duration::from_secs(40);
    let g1 = loop {
        let g = endless
            .start_generation(id, None, 1_000_000, false)
            .await
            .unwrap();
        let t = g.render.text();
        if !t.contains("older lines folded") && (0..60u64).any(|i| !alone(&t, i)) {
            break g;
        }
        assert!(
            std::time::Instant::now() < end,
            "the view never fit on merges; level ≥ 1 nodes: {:?}\n{t}",
            store
                .endless_nodes(id)
                .await
                .unwrap()
                .iter()
                .filter(|n| n.l >= 1)
                .map(|n| (n.l, n.i, n.quality()))
                .collect::<Vec<_>>()
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    let t1 = g1.render.text();
    let i = (0..60u64)
        .find(|i| !alone(&t1, *i))
        .unwrap_or_else(|| panic!("no merged line in generation 1:\n{t1}"));
    endless.zoom(id, i, 1).await.unwrap();
    let g2 = endless
        .start_generation(id, None, 1_000_000, false)
        .await
        .unwrap();
    let g3 = endless
        .start_generation(id, None, 1_000_000, false)
        .await
        .unwrap();
    endless.detach(id);
    (alone(&g2.render.text(), i), alone(&g3.render.text(), i), t1)
}

/// S7: 201 §10 — a line zoomed in generation N stays unmerged at the trim
/// into N+1 and merges again at N+2 (exactly one generation of grace);
/// `age_by_use: false` folds it at once.
#[tokio::test]
async fn eval_s7_a_zoomed_line_gets_exactly_one_generation_of_grace() {
    let (g2, g3, t1) = zoomed_across_real_merges(true).await;
    assert_eq!(
        (g2, g3),
        (true, false),
        "age_by_use on; generation 1:\n{t1}"
    );
    let (g2, g3, t1) = zoomed_across_real_merges(false).await;
    assert_eq!(
        (g2, g3),
        (false, false),
        "age_by_use off; generation 1:\n{t1}"
    );
}

// endregion: --- S7

// region:    --- S8: the small profile through the service

/// S8: 201 §7 — a local compactor with a 2,048-token window resolves to
/// `small`; every request is one unit (or one join), carries no `<chat>`
/// view and at most 2,000 bytes of context, the step fits the window's
/// chunk; a guard trip keeps the draft; a 12 KB unit is summarised in
/// window-sized chunks.
#[tokio::test]
async fn eval_s8_small_profile_requests_stay_small_and_keep_drafts_on_a_trip() {
    let model = models::Resolved {
        model: Some(
            serde_json::from_value(serde_json::json!({
                "id": "tiny", "name": "tiny", "limit": {"context": 2048, "output": 512}
            }))
            .unwrap(),
        ),
        ..super::pump::resolved_for_tests("local/tiny")
    };
    let chunk = super::compactor::small_chunk_bytes(crate::tokens::bytes_of(
        &model.id,
        crate::tokens::memory_window(&model),
    ));
    let c = EvalCompactor::with(Act::Echo, false, Some("ep-tiny"), model);
    let (_dir, endless, store, id) = harness(EndlessConfig::default(), c.clone()).await;
    for k in 0..5 {
        let text = format!(
            "message {k}: deploy step {k} touched /srv/app/config_{k}.yaml and {}",
            "the rollout notes repeat here ".repeat(19)
        );
        store
            .append(id, &ChatMessage::assistant(text))
            .await
            .unwrap();
    }
    let bad = format!(
        "BAD message: the cache warmer {}",
        "restarted the frontend pods ".repeat(22)
    );
    store
        .append(id, &ChatMessage::assistant(bad))
        .await
        .unwrap();
    let huge: String = (0..240)
        .map(|j| format!("huge part {j}: the migration of table t{j} copied rows."))
        .collect::<Vec<_>>()
        .join(" ");
    assert!(huge.len() > 12_000);
    store
        .append(id, &ChatMessage::assistant(huge.clone()))
        .await
        .unwrap();
    // 201-FIX R2-2: a unit under the 16-chunk cap is reduced in stages.
    let mid: String = (0..110)
        .map(|j| format!("mid part {j}: table m{j} moved."))
        .collect::<Vec<_>>()
        .join(" ");
    store
        .append(id, &ChatMessage::assistant(mid.clone()))
        .await
        .unwrap();
    endless.attach(id, attached()).await.unwrap();

    let mem = endless.memory(id).await.unwrap();
    assert_eq!(mem.profile, super::Profile::Small, "{}", mem.profile_reason);
    // 201-FIX R2-2: the huge unit is over `SMALL_MAX_CHUNKS` chunks here.
    let mid_chunks = super::compactor::chunks(&mid, chunk);
    assert!(
        super::compactor::chunks(&huge, chunk).len() > super::compactor::SMALL_MAX_CHUNKS,
        "chunk {chunk}"
    );
    assert!((3..=super::compactor::SMALL_MAX_CHUNKS).contains(&mid_chunks.len()));
    // Units 0–4 and the mid unit 7 refined; unit 5 (BAD) tried.
    eventually("the good units refined, the bad one tried", 30, || async {
        let nodes = store.endless_nodes(id).await.unwrap();
        let refined = |i: u64| {
            nodes
                .iter()
                .any(|n| n.l == 0 && n.i == i && n.quality() == Quality::Model)
        };
        [0, 1, 2, 3, 4, 7].iter().all(|i| refined(*i))
            && c.small.lock().iter().any(|(_, s)| s.contains("BAD"))
    })
    .await;

    let calls = c.small.lock().clone();
    assert!(!calls.is_empty());
    let mut huge_calls = 0;
    let mut mid_calls = 0;
    for (ctx, step) in &calls {
        assert!(
            ctx.len() <= super::compactor::SMALL_CTX_BYTES,
            "context {} B",
            ctx.len()
        );
        assert!(!ctx.contains("<chat>"), "no view in a small request: {ctx}");
        let units = step.matches("Write one line about this message").count();
        let joins = step.matches("Join these two lines").count();
        assert_eq!(units + joins, 1, "one unit per call: {step}");
        assert!(!step.contains("Compress each of these"), "no batch: {step}");
        let body = step.split_once('\n').map_or("", |(_, b)| b);
        assert!(
            body.len() <= chunk + 16,
            "step body {} B over the chunk {chunk}",
            body.len()
        );
        let total = super::compactor::SMALL.len() + ctx.len() + step.len();
        assert!(
            total <= super::compactor::SMALL.len() + 2_000 + chunk + 200,
            "request {total} B"
        );
        if step.contains("huge part") {
            huge_calls += 1;
        }
        if step.contains("mid part") {
            mid_calls += 1;
        }
    }
    // 201-FIX R2-2 (was: the huge unit went in window-sized chunks): over
    // the cap no call is sent for it, its draft stays, it is not retried.
    assert_eq!(huge_calls, 0, "no call for a unit over the chunk cap");
    assert!(
        super::pump::failed_for_tests(&endless.inner, id)
            .iter()
            .all(|(k, ..)| *k != (0, 6)),
        "not a failure, not retried"
    );
    // The mid unit: one call per chunk, then more than one reduce call
    // (stages), and a reduce input carries the last chunk's line.
    assert!(
        mid_calls >= mid_chunks.len() + 2,
        "{mid_calls} calls for {} chunks",
        mid_chunks.len()
    );
    let last = mid_chunks.last().unwrap();
    let tag = &last[last.find("mid part ").unwrap()..];
    let tag = &tag[..tag.find(':').unwrap()];
    assert!(
        calls.iter().any(|(_, s)| {
            let body = s.split_once('\n').map_or("", |(_, b)| b);
            body.lines().count() > 1 && body.contains(tag)
        }),
        "the last chunk ({tag}) reaches a reduce call"
    );
    let nodes = store.endless_nodes(id).await.unwrap();
    let bad = nodes.iter().find(|n| n.l == 0 && n.i == 5).unwrap();
    assert_eq!(
        bad.quality(),
        Quality::Draft,
        "the draft stays: {}",
        bad.text
    );
    assert!(!bad.text.contains("zebra"), "{}", bad.text);
    let huge_node = nodes.iter().find(|n| n.l == 0 && n.i == 6).unwrap();
    assert_eq!(huge_node.quality(), Quality::Draft, "{}", huge_node.text);
    assert!(
        nodes.iter().all(|n| !n.text.contains("zebra crossing")),
        "a guard-failing line is never stored"
    );
}

// endregion: --- S8
