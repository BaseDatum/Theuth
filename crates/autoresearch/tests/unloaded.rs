//! 194 R1: `autoresearch.settings.set` with the plugin not running settles each role effort
//! against its model before `config.yaml` is written, against the section saved there (D4c).
//! Its own binary: it registers the agent actor, which the process-wide kameo registry allows
//! once.

use autoresearch::AutoresearchConfig;
use rpc_core::Server;
use runtime::Runtime;

/// One reasoning model (`low|high`) and one plain model, under a provider with no key.
const MODELS: &str = r#"{"testp":{"id":"testp","name":"T","models":{
    "thinker":{"id":"thinker","name":"thinker","limit":{"context":100000,"output":4096},
        "reasoning":true,"reasoning_options":[{"type":"effort","values":["low","high"]}]},
    "plain":{"id":"plain","name":"plain","limit":{"context":100000,"output":4096}}}}}"#;

#[tokio::test]
async fn the_disabled_plugin_path_settles_efforts_too() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("models.json"), MODELS).unwrap();
    let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    agent::spawn_test_agents(rt.clone(), |_: &str| false)
        .await
        .unwrap();
    let api: jsonrpsee::Methods = autoresearch::rpc_module(Server::new(rt)).module.into();
    let set = |model: Option<&str>, effort: Option<&str>| {
        let mut cfg = AutoresearchConfig::default();
        cfg.models.executor = model.map(Into::into);
        cfg.models.executor_effort = effort.map(Into::into);
        let api = api.clone();
        async move {
            api.call::<_, AutoresearchConfig>("autoresearch.settings.set", (cfg,))
                .await
                .map(|c| c.models.executor_effort)
                .map_err(|e| e.to_string())
        }
    };
    let saved = || std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
    assert_eq!(
        set(Some("testp/thinker"), Some("high")).await,
        Ok(Some("high".into()))
    );
    assert!(saved().contains("executor_effort: high"), "{}", saved());
    let e = set(Some("testp/thinker"), Some("medium"))
        .await
        .unwrap_err();
    assert!(
        e.contains("models.executor_effort: testp/thinker accepts off|low|high"),
        "D4b: {e}"
    );
    assert!(
        saved().contains("executor_effort: high"),
        "refused: unchanged"
    );
    assert_eq!(
        set(Some("testp/plain"), Some("high")).await,
        Ok(None),
        "D4c against the saved section"
    );
    assert!(!saved().contains("executor_effort: high"), "{}", saved());
    assert_eq!(set(None, Some("high")).await, Ok(None), "D4d");
}
