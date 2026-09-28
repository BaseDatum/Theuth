//! 57 §4.1 parity: `browser::systemone::render` must produce, byte for byte, what
//! `cklxx/laya-browser`'s systemone server feeds laya for their recorded Wikipedia step
//! (`code/sample_request.json`, jev-ultrafast's request after the format transform: its page
//! text is already 1200 chars and no label exceeds 50 chars, so the v2 rendering recorded on
//! 2026-09-21 is also the v3 rendering v17s reads).
//!
//! `fixtures/laya-browser-sample_request.rendered.json` is upstream code's own rendering of that
//! request — `laya.common.serialize_state` / `render_options` and `Agent._to_internal`'s
//! `json.dumps(instructions)`, run in Python on 2026-09-21 — so the comparison is against the
//! bytes their tokenizer saw, not against our reading of their code.
//!
//! The model-gated test loads the browser checkpoint from `~/.theuth/models/decision-browser/`
//! (`$DECISION_BROWSER_MODEL_DIR` overrides; v10 or v17s) through the real engine (`AskMany`,
//! the browser families' gates) and expects the file's `expected`: `operation = TYPE_TEXT`, and
//! the type target `[2] Search Wikipedia (searchbox)`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use browser::systemone::{self, Element, Observation, RecentAction, Scroll};
use decision::engine::MODEL_FILES;
use decision::log::Log;
use decision::{AskMany, DecisionConfig, Disposition, Engine, FamilyGate, Mode, Question};
use kameo::actor::Spawn;
use serde_json::Value;

fn fixture(name: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name);
    serde_json::from_str(
        &std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
    )
    .unwrap()
}

/// The Wikipedia step as an [`Observation`], reconstructed from the request: page from
/// `state.page`, elements from `click_target`'s criteria (`[i] label (role)`; an element that
/// also appears in `type_text_target` is editable and its click label carries jev's `Open `).
fn wikipedia() -> (String, Observation) {
    let req = fixture("laya-browser-sample_request.json");
    let goal = req["questions"]["operation"]["instructions"]["goal"]
        .as_str()
        .unwrap()
        .to_owned();
    let typeable: BTreeMap<usize, String> = req["questions"]["type_text_target"]["criteria"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| (k.parse().unwrap(), v.as_str().unwrap().to_owned()))
        .collect();
    let mut elements: Vec<Element> = req["questions"]["click_target"]["criteria"]
        .as_object()
        .unwrap()
        .iter()
        .map(|(k, v)| {
            let index: usize = k.parse().unwrap();
            let text = v.as_str().unwrap();
            // "[i] label (role)" → label, role; the type list's label wins for an editable.
            let body = text.strip_prefix(&format!("[{index}] ")).unwrap();
            let (label, role) = body.rsplit_once(" (").unwrap();
            let role = role.strip_suffix(')').unwrap();
            let editable = typeable.contains_key(&index);
            let label = if editable {
                let t = &typeable[&index];
                t.strip_prefix(&format!("[{index}] "))
                    .unwrap()
                    .rsplit_once(" (")
                    .unwrap()
                    .0
                    .to_owned()
            } else {
                label.to_owned()
            };
            Element {
                index,
                ref_: format!("e{index}"),
                label,
                role: role.to_owned(),
                editable,
                ..Element::default()
            }
        })
        .collect();
    elements.sort_by_key(|e| e.index);
    let page = &req["state"]["page"];
    let obs = Observation {
        url: page["url"].as_str().unwrap().to_owned(),
        title: page["title"].as_str().unwrap().to_owned(),
        text: page["text"]
            .as_str()
            .unwrap()
            .chars()
            .take(systemone::TEXT_MAX)
            .collect(),
        elements,
        recent_actions: Vec::<RecentAction>::new(),
        // The recorded page is the top of Wikipedia's main page with more below: jev offered
        // SCROLL_DOWN and no SCROLL_UP.
        more_below: true,
        scrolled: false,
    };
    (goal, obs)
}

#[test]
fn renders_cklxx_sample_request_byte_for_byte() {
    let (goal, obs) = wikipedia();
    let rendered = fixture("laya-browser-sample_request.rendered.json");
    let asks = systemone::render(&goal, &obs);
    assert_eq!(asks.len(), 3);
    let names = ["operation", "click_target", "type_text_target"];
    for (ask, name) in asks.iter().zip(names) {
        let want = &rendered["questions"][name];
        let Question::Choice {
            instructions,
            candidates,
            state,
        } = &ask.question
        else {
            panic!("{name}: not a choice")
        };
        assert_eq!(
            instructions,
            want["instructions"].as_str().unwrap(),
            "{name} instructions"
        );
        // laya's `render_options`: "id: text" per criterion, in the request's order.
        let ours: Vec<String> = candidates
            .iter()
            .map(|c| format!("{}: {}", c.id, c.text))
            .collect();
        let theirs: Vec<&str> = want["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(ours.len(), theirs.len(), "{name} option count");
        for (i, (o, t)) in ours.iter().zip(&theirs).enumerate() {
            assert_eq!(o, t, "{name} option {i}");
        }
        assert_eq!(state, rendered["state"].as_str().unwrap(), "{name} state");
    }
    assert_eq!(asks[0].family.0, "browser.operation");
    assert_eq!(asks[1].family.0, "browser.target");
    assert_eq!(asks[2].family.0, "browser.type_target");
    // The whole head laya tokenizes: "<type> question: <instructions>".
    let Question::Choice { instructions, .. } = &asks[0].question else {
        unreachable!()
    };
    assert_eq!(
        format!("choice question: {instructions}"),
        rendered["questions"]["operation"]["head"].as_str().unwrap()
    );
}

/// The browser checkpoint dir, or `None` with a printed reason.
fn model_dir() -> Option<PathBuf> {
    let data = std::env::var_os("THEUTH_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".theuth")))?;
    let dir = std::env::var_os("DECISION_BROWSER_MODEL_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| data.join("models/decision-browser"));
    if MODEL_FILES.iter().all(|f| dir.join(f).is_file()) {
        Some(dir)
    } else {
        eprintln!(
            "skipped: no laya-browser checkpoint in {} (scripts/fetch-laya-browser.sh)",
            dir.display()
        );
        None
    }
}

#[tokio::test]
async fn browser_checkpoint_answers_type_text_into_the_search_box() {
    let Some(dir) = model_dir() else { return };
    let (goal, obs) = wikipedia();
    let tmp = tempfile::tempdir().unwrap();
    let mut cfg = DecisionConfig::default();
    cfg.checkpoints.insert("browser".into(), dir.clone().into());
    for f in [
        systemone::FAMILY_OPERATION,
        systemone::FAMILY_TARGET,
        systemone::FAMILY_TYPE_TARGET,
    ] {
        let g: &mut FamilyGate = cfg.families.get_mut(f).unwrap();
        g.mode = Mode::Active;
        g.budget = Duration::from_secs(120); // cold load + Metal shader compile on the first run
    }
    let engine = Engine::spawn(
        Engine::new(
            cfg,
            tmp.path().join("decision"),
            Log::new(tmp.path().join("decisions")),
        )
        .with_checkpoint("browser", dir),
    );
    let mut asks = systemone::render(&goal, &obs);
    for a in &mut asks {
        a.budget = Duration::from_secs(120);
    }
    let t = std::time::Instant::now();
    let ds = engine.ask(AskMany { asks }).await.unwrap();
    let first = t.elapsed();
    for d in &ds {
        eprintln!(
            "{} → {} {:?} (top: {:?})",
            d.family,
            d.choice,
            d.disposition.kind(),
            {
                let mut p = d.probabilities.clone();
                p.sort_by(|a, b| b.1.total_cmp(&a.1));
                p.truncate(3);
                p
            }
        );
    }
    assert_eq!(ds[0].choice, "TYPE_TEXT", "{:?}", ds[0].probabilities);
    assert_eq!(ds[2].choice, "2", "{:?}", ds[2].probabilities);
    assert!(matches!(
        ds[0].disposition,
        Disposition::Accept | Disposition::Escalate(_)
    ));
    assert_eq!(ds[1].probabilities.len(), 58, "no `none` on the closed set");
    // Warm step latency, the §4.1 ≤ 300 ms target, printed (not asserted: it is the bench's job).
    let asks = systemone::render(&goal, &obs);
    let t = std::time::Instant::now();
    let _ = engine.ask(AskMany { asks }).await.unwrap();
    eprintln!("first step {first:?}, warm step {:?}", t.elapsed());
    let _ = Scroll::default();
}
