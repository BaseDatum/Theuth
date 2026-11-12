//! 194 R1: every autoresearch effort setter settles against its model's catalog row (D4b–d)
//! through a live keyless engine — the settings, the panel's `draft_edit` and the
//! `autoresearch_draft` tool. Its own binary: it registers the agent actor, which the
//! process-wide kameo registry allows once.

use std::path::Path;

use autoresearch::config::settle_settings;
use autoresearch::setup::{edit, settle_draft};
use autoresearch::tools::{read_draft, write_draft};
use autoresearch::{AutoresearchConfig, StudySpec};
use runtime::{Runtime, SessionId};

/// One reasoning model (`low|high`) and one plain model, under a provider with no key.
const MODELS: &str = r#"{"testp":{"id":"testp","name":"T","models":{
    "thinker":{"id":"thinker","name":"thinker","limit":{"context":100000,"output":4096},
        "reasoning":true,"reasoning_options":[{"type":"effort","values":["low","high"]}]},
    "plain":{"id":"plain","name":"plain","limit":{"context":100000,"output":4096}}}}}"#;

const THINKER: &str = "testp/thinker accepts off|low|high";

fn s(v: &str) -> Option<String> {
    Some(v.to_owned())
}

#[tokio::test]
async fn efforts_settle_against_their_models() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models.json"), MODELS).unwrap();
    let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    agent::spawn_test_agents(rt, |_: &str| false).await.unwrap();
    settings().await;
    draft_edit(&dir.path().join("ar")).await;
    draft_tool(&dir.path().join("ar")).await;
}

/// `autoresearch.settings.set` (running or not): refused on the same model, dropped by a model
/// change, cleared with the model.
async fn settings() {
    let mut was = AutoresearchConfig::default();
    was.models.executor = s("testp/thinker");
    was.review.model = s("testp/thinker");
    let mut cfg = was.clone();
    cfg.models.executor_effort = s("medium");
    let e = settle_settings(&mut cfg, &was).await.unwrap_err();
    assert_eq!(e, format!("models.executor_effort: {THINKER}"));
    let mut cfg = was.clone();
    cfg.review.effort = s("medium");
    let e = settle_settings(&mut cfg, &was).await.unwrap_err();
    assert_eq!(e, format!("review.effort: {THINKER}"));
    let mut cfg = was.clone();
    cfg.models.executor_effort = s("high");
    settle_settings(&mut cfg, &was).await.unwrap();
    assert_eq!(cfg.models.executor_effort, s("high"));
    cfg.models.executor = s("testp/plain");
    settle_settings(&mut cfg, &was).await.unwrap();
    assert_eq!(cfg.models.executor_effort, None, "D4c: plain takes none");
    cfg.models.executor = None;
    cfg.models.executor_effort = s("high");
    settle_settings(&mut cfg, &was).await.unwrap();
    assert_eq!(cfg.models.executor_effort, None, "D4d: no model, no effort");
}

/// The panel: an effort the model does not take is refused and nothing is saved; a model change
/// drops it; emptying the model clears it.
async fn draft_edit(root: &Path) {
    let spec = StudySpec {
        slug: "toy".into(),
        repo: "/x/repo".into(),
        ..StudySpec::default()
    };
    write_draft(root, &spec, SessionId::new(), true, vec![]).unwrap();
    let models = || read_draft(root, "toy").unwrap().spec.models;
    edit(root, "toy", "models.executor", "testp/thinker")
        .await
        .unwrap();
    let e = edit(root, "toy", "models.executor_effort", "medium")
        .await
        .unwrap_err()
        .to_string();
    assert_eq!(e, format!("models.executor_effort: {THINKER}"));
    assert_eq!(models().executor_effort, None, "refused: not saved");
    edit(root, "toy", "models.executor_effort", "high")
        .await
        .unwrap();
    assert_eq!(models().executor_effort, s("high"));
    edit(root, "toy", "models.executor", "testp/plain")
        .await
        .unwrap();
    assert_eq!(
        (models().executor, models().executor_effort),
        (s("testp/plain"), None)
    );
    let e = edit(root, "toy", "models.executor_effort", "high")
        .await
        .unwrap_err()
        .to_string();
    assert!(e.contains("testp/plain accepts no reasoning effort"), "{e}");
    edit(root, "toy", "models.advisor", "testp/thinker")
        .await
        .unwrap();
    edit(root, "toy", "models.advisor_effort", "low")
        .await
        .unwrap();
    edit(root, "toy", "models.advisor", "").await.unwrap();
    assert_eq!((models().advisor, models().advisor_effort), (None, None));
}

/// `autoresearch_draft`: a new draft's bad pick is refused (every role, the target and judge
/// too); a re-draft that changes the model drops it; a re-draft without the model clears it.
async fn draft_tool(root: &Path) {
    let mut spec = StudySpec {
        slug: "tool".into(),
        repo: "/x/repo".into(),
        ..StudySpec::default()
    };
    spec.models.target = s("testp/plain");
    spec.models.target_effort = s("low");
    let e = settle_draft(root, &mut spec.clone()).await.unwrap_err();
    assert_eq!(
        e,
        "models.target_effort: testp/plain accepts no reasoning effort"
    );
    spec.models.target_effort = None;
    spec.models.judge = s("testp/thinker");
    spec.models.judge_effort = s("medium");
    let e = settle_draft(root, &mut spec.clone()).await.unwrap_err();
    assert_eq!(e, format!("models.judge_effort: {THINKER}"));
    spec.models.judge_effort = s("high");
    settle_draft(root, &mut spec).await.unwrap();
    assert_eq!(spec.models.judge_effort, s("high"));
    write_draft(root, &spec, SessionId::new(), true, vec![]).unwrap();
    let mut again = spec.clone();
    again.models.judge = s("testp/plain");
    settle_draft(root, &mut again).await.unwrap();
    assert_eq!(
        again.models.judge_effort, None,
        "D4c against the saved draft"
    );
    let mut again = spec.clone();
    again.models.judge = None;
    settle_draft(root, &mut again).await.unwrap();
    assert_eq!(again.models.judge_effort, None, "D4d");
}
