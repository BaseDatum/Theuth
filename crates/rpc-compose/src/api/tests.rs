use std::collections::HashSet;
use std::fmt::Display;
use std::sync::Arc;
use agent::rpc_forward as forward;
use agent::SpendReport;
use agent::rpc_tooling::SESSION_TOOLS;
use jsonrpsee::Extensions;
use jsonrpsee::types::ErrorObjectOwned;
use kameo::message::Message;
use link::LinkStatus;
use runtime::{Runtime, SessionId};
use tokio::sync::broadcast;
use tools::{Answer, ApprovalId, AskId, Decision};
use uuid::Uuid;
use rpc_core::{failed, NOT_RUNNING, ASK_FAILED, UNAUTHORIZED, FocusMap, BACKLOG, PrivateMap, Server};
use wire::{EventPolicy, WireFrame};
use agent::rpc_support::session_draining;
use tasks::rpc_support::{question_owner, route_question_via};
use a2a::rpc_support::A2A_FORWARD_MESSAGING_OFF;
use agent::rpc_attachments::decode_attachment;
use agent::rpc_document::named;
use document::rpc_support::join_outcome;
use link::rpc_sharing::{link_ids, link_origin};
use base64::Engine as _;

/// Unit-test composition only; export and production assembly belong to main.
pub(crate) fn test_rpc_module(server: Server) -> jsonrpsee::RpcModule<Server> {
    rpc_core::merge_described([
        rpc::rpc_module(server.clone()),
        crate::rpc_module(server.clone()),
        agent::rpc_module(server.clone()),
        a2a::rpc_module(server.clone()),
        app::rpc_module(server.clone()),
        artifact::rpc_module(server.clone()),
        document::rpc_module(server.clone()),
        knowledge::rpc_module(server.clone()),
        link::rpc_module(server.clone()),
        roster::rpc_module(server.clone()),
        search_providers::rpc_module(server.clone()),
        sessionsync::rpc_module(server.clone()),
        signal::rpc_module(server.clone()),
        slack::rpc_module(server.clone()),
        talk::rpc_module(server.clone()),
        tasks::rpc_module(server.clone()),
        telegram::rpc_module(server.clone()),
        tools::rpc_module(server.clone()),
        update::rpc_module(server.clone()),
        web::rpc_module(server.clone()),
        computer::rpc_module(server.clone()),
        mcp_server::rpc_module(server.clone()),
        codeindex::rpc_module(server.clone()),
        nodes::rpc_module(server.clone()),
        schedule::rpc_module(server.clone()),
        secrets::rpc_module(server.clone()),
        meeting::rpc_module(server.clone()),
        dreaming::rpc_module(server.clone()),
        autoresearch::rpc_module(server.clone()),
        cells::rpc_module(server.clone()),
        share::rpc_module(server.clone()),
        plugins::rpc_module(server.clone()),
        cloudnodes::rpc_module(server.clone()),
        sandbox::rpc_module(server),
    ]).expect("test RPC descriptors").module
}

    use super::*;

    // Bind legacy JSON fixtures to owner policies without changing assertions.
    fn policy(ev: &serde_json::Value) -> EventPolicy {
        if ev["type"] == "CommunicationChanged" {
            agent::CommunicationChanged::policy(ev)
        } else {
            agent::AgentEvent::policy(ev)
        }
    }

    fn admitted(ev: &serde_json::Value, peer: u64, private: &PrivateMap) -> bool {
        super::admitted(&policy(ev), peer, private)
    }

    fn wanted(ev: &serde_json::Value, session: Option<&str>, tree: Option<&mut HashSet<String>>) -> bool {
        super::wanted(&policy(ev), session, tree)
    }

    fn is_delta(ev: &serde_json::Value) -> bool {
        policy(ev).delta != wire::Delta::Control
    }

    /// A diagram's "Save SVG…" goes through `put_attachment` on the desktop shell:
    /// `.svg` must be accepted (image cap), unknown extensions still refused.
    #[test]
    fn decode_attachment_accepts_svg_and_refuses_unknown() {
        let b64 = base64::engine::general_purpose::STANDARD.encode(b"<svg/>");
        assert_eq!(decode_attachment(b64.clone(), "svg").unwrap(), b"<svg/>");
        let err = decode_attachment(b64, "exe").unwrap_err();
        assert!(
            err.message().contains("unsupported attachment type .exe"),
            "{err}"
        );
    }

    /// 109 §10.3 wire shapes the desktop's bindings rely on: the interrupted
    /// history row, `Loaded.resume` (absent → `None`), the `resume` attention
    /// kind, and `resume_turn` registered on the module.
    #[tokio::test]
    async fn durability_wire_shapes() {
        let at = chrono::DateTime::parse_from_rfc3339("2026-10-06T12:05:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc);
        let row = agent::HistoryEntry::Interrupted {
            text: "half a re".into(),
            at,
            context: None,
        };
        assert_eq!(
            serde_json::to_value(&row).unwrap(),
            serde_json::json!({ "role": "interrupted", "text": "half a re", "at": "2026-10-06T12:05:00Z", "context": null })
        );
        let turn = runtime::TurnId::new();
        let loaded = agent::Loaded {
            resume: Some(agent::ResumeState {
                turn,
                kind: agent::durability::ResumeKind::Suspended,
                interrupted_at: at,
                resumed: false,
                report: None,
                owner: Some("engine-a".into()),
                here: false,
                cause: Some(agent::durability::ResumeCause::Transfer),
                from: None,
            }),
            ..Default::default()
        };
        let v = serde_json::to_value(&loaded).unwrap();
        assert_eq!(v["resume"]["kind"], "suspended");
        assert_eq!(v["resume"]["turn"], turn.to_string());
        // 139 §9.1: the replica's banner fields, snake-case cause.
        assert_eq!(v["resume"]["owner"], "engine-a");
        assert_eq!(v["resume"]["here"], false);
        assert_eq!(v["resume"]["cause"], "transfer");
        let back: agent::Loaded = serde_json::from_value(v).unwrap();
        assert_eq!(back.resume, loaded.resume);
        let mut pre109 = serde_json::to_value(agent::Loaded::default()).unwrap();
        pre109.as_object_mut().unwrap().remove("resume");
        let old: agent::Loaded = serde_json::from_value(pre109).unwrap();
        assert_eq!(old.resume, None, "a pre-109 engine's page still parses");
        // 139 §5.2: a pre-139 engine's `resume` reads as owned here, cause unknown.
        let pre139: agent::ResumeState = serde_json::from_value(serde_json::json!({
            "turn": turn, "kind": "interrupted", "interrupted_at": at, "resumed": true
        }))
        .unwrap();
        assert_eq!(
            (pre139.owner, pre139.here, pre139.cause),
            (None, true, None)
        );
        // 139 §9.1: `TurnResumed` carries `cause` + `from`; an old engine's omits both.
        let ev = agent::AgentEvent::TurnResumed {
            session: SessionId::new(),
            turn,
            kind: agent::durability::ResumeKind::Interrupted,
            interrupted_at: at,
            resumed_at: at,
            partial_dropped: false,
            cause: agent::durability::ResumeCause::Takeover,
            from: Some("engine-a".into()),
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(
            (&v["cause"], &v["from"]),
            (
                &serde_json::json!("takeover"),
                &serde_json::json!("engine-a")
            )
        );
        let mut old = v.clone();
        let o = old.as_object_mut().unwrap();
        o.remove("cause");
        o.remove("from");
        match serde_json::from_value::<agent::AgentEvent>(old).unwrap() {
            agent::AgentEvent::TurnResumed { cause, from, .. } => {
                assert_eq!((cause, from), (agent::durability::ResumeCause::Crash, None));
            }
            other => panic!("{other:?}"),
        }
        // 139 §6.1: the SESSION_DRAINING payload in the external enum form the composer keys on.
        let session = SessionId::new();
        let e = session_draining(
            session,
            ErrorObjectOwned::owned(ASK_FAILED, "SessionDraining:release:engine-b", None::<()>),
        );
        assert_eq!(e.code(), forward::SESSION_DRAINING);
        assert_eq!(
            e.message(),
            format!("{session} is moving to engine-b — retry in a moment")
        );
        let data: serde_json::Value = serde_json::from_str(e.data().unwrap().get()).unwrap();
        assert_eq!(
            data,
            serde_json::json!({ "SessionDraining": { "session": session, "reason": "release", "next_owner": "engine-b" } })
        );
        let e = session_draining(
            session,
            ErrorObjectOwned::owned(ASK_FAILED, "SessionDraining:shutdown", None::<()>),
        );
        assert_eq!(e.code(), forward::SESSION_DRAINING);
        assert_eq!(
            e.message(),
            format!("{session} is pausing for shutdown — resumes on the next start")
        );
        let other = ErrorObjectOwned::owned(ASK_FAILED, "runner unavailable", None::<()>);
        assert_eq!(
            session_draining(session, other.clone()),
            other,
            "anything else passes through"
        );
        assert_eq!(
            serde_json::to_value(roster::attention::AttentionKind::Resume).unwrap(),
            "resume"
        );
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let names: Vec<&str> = test_rpc_module(crate::new_server(rt)).method_names().collect();
        assert!(names.contains(&"resume_turn"), "{names:?}");
    }

    /// 136 §2.7: every `dreaming.*` method is registered under its dotted name.
    #[tokio::test]
    async fn dreaming_methods_are_registered() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let names: Vec<&str> = test_rpc_module(crate::new_server(rt)).method_names().collect();
        for m in dreaming::rpc::METHODS {
            assert!(names.contains(&m), "{m} missing from {names:?}");
        }
    }

    /// 140 §12.5: every `autoresearch.*` method is registered under its dotted name.
    #[tokio::test]
    async fn autoresearch_methods_are_registered() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let names: Vec<&str> = test_rpc_module(crate::new_server(rt)).method_names().collect();
        for m in autoresearch::rpc_impl::METHODS {
            assert!(names.contains(&m), "{m} missing from {names:?}");
        }
    }

    /// 170 D1: `slack_manifest` answers `slack::MANIFEST` verbatim with no Slack
    /// actor running (unconfigured), and the text is the manifest the setup uses.
    #[tokio::test]
    async fn slack_manifest_answers_the_compiled_manifest_without_slack() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
        let req = r#"{"jsonrpc":"2.0","method":"slack_manifest","params":[],"id":1}"#;
        let (resp, _) = methods.raw_json_request(req, 1).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
        assert_eq!(v["result"].as_str(), Some(slack::MANIFEST), "{v}");
        let m: serde_yaml::Value = serde_yaml::from_str(slack::MANIFEST).unwrap();
        assert_eq!(m["features"]["slash_commands"][0]["command"], "/theuth");
    }

    /// 188: `config_flag_*` over the wire against a boot config (`running`) and a
    /// different config.yaml in the data dir (`enabled`).
    async fn config_flag_call(
        boot: &str,
        file: Option<&str>,
        method: &str,
        params: &str,
    ) -> (serde_json::Value, tempfile::TempDir) {
        use figment::providers::{Format, Yaml};
        let dir = tempfile::tempdir().unwrap();
        if let Some(f) = file {
            std::fs::write(dir.path().join("config.yaml"), f).unwrap();
        }
        let rt = Runtime::new(
            figment::Figment::from(Yaml::string(boot)),
            dir.path().to_path_buf(),
        );
        let methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
        let req = format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params},"id":1}}"#);
        let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
        (serde_json::from_str(resp.get()).unwrap(), dir)
    }

    /// 188: `get` reports the file value, the boot value and their difference for each
    /// key; an absent key or section is that section's default (decision and computer
    /// on, a2a off, double `DoubleConfig::default()`, both `allow_private_network` on).
    #[tokio::test]
    async fn config_flag_get_reports_file_boot_and_restart() {
        let flag = |key: &str, enabled: bool, running: bool| serde_json::json!({ "key": key, "enabled": enabled, "running": running, "restart_required": enabled != running });
        let boot = "decision:\n  enabled: false\na2a:\n  enabled: true\n";
        let file = "decision:\n  enabled: true\ncomputer:\n  enabled: false\ndouble:\n  enabled: true\nweb:\n  allow_private_network: false\n";
        for (key, want) in [
            ("decision.enabled", flag("decision.enabled", true, false)),
            ("a2a.enabled", flag("a2a.enabled", false, true)),
            ("computer.enabled", flag("computer.enabled", false, true)),
            ("double.enabled", flag("double.enabled", true, false)),
            (
                "web.allow_private_network",
                flag("web.allow_private_network", false, true),
            ),
            (
                "browser.allow_private_network",
                flag("browser.allow_private_network", true, true),
            ),
        ] {
            let (v, _dir) = config_flag_call(
                boot,
                Some(file),
                "config_flag_get",
                &format!(r#"["{key}"]"#),
            )
            .await;
            assert_eq!(v["result"], want, "{key}: {v}");
        }
        for (key, on) in [
            ("decision.enabled", true),
            ("a2a.enabled", false),
            ("computer.enabled", true),
            ("double.enabled", double::DoubleConfig::default().enabled),
            ("web.allow_private_network", true),
            ("browser.allow_private_network", true),
        ] {
            let (v, _dir) =
                config_flag_call("", None, "config_flag_get", &format!(r#"["{key}"]"#)).await;
            assert_eq!(v["result"], flag(key, on, on), "{key} defaults: {v}");
        }
    }

    /// 188: `set` rewrites only the section's `enabled` line (comments and every other
    /// key byte-for-byte), creates a missing section, and changes nothing live.
    #[tokio::test]
    async fn config_flag_set_writes_only_the_key() {
        let file = "# mine\nagent:\n  model: x  # keep\na2a:\n  listen: 127.0.0.1:1  # port\n  enabled: false # off for now\n";
        let (v, dir) = config_flag_call(
            "",
            Some(file),
            "config_flag_set",
            r#"["a2a.enabled", true]"#,
        )
        .await;
        assert_eq!(
            v["result"],
            serde_json::json!({ "key": "a2a.enabled", "enabled": true, "running": false, "restart_required": true }),
            "{v}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            file.replace("enabled: false #", "enabled: true #")
        );

        let (v, dir) = config_flag_call(
            "",
            Some(file),
            "config_flag_set",
            r#"["computer.enabled", false]"#,
        )
        .await;
        assert_eq!(v["result"]["enabled"], false, "{v}");
        assert_eq!(v["result"]["running"], true, "{v}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            format!("{file}\ncomputer:\n  enabled: false\n")
        );

        let file = "double:\n  retention_days: 30  # short\n  enabled: false\n";
        let (v, dir) = config_flag_call(
            "",
            Some(file),
            "config_flag_set",
            r#"["double.enabled", true]"#,
        )
        .await;
        assert_eq!(
            v["result"],
            serde_json::json!({ "key": "double.enabled", "enabled": true, "running": false, "restart_required": true }),
            "{v}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            file.replace("enabled: false", "enabled: true")
        );

        // 16 §5 (2026-11-11): a non-`enabled` key lands in its own section, beside the rest.
        let file = "web:\n  max_redirects: 3  # mine\n";
        let (v, dir) = config_flag_call(
            "",
            Some(file),
            "config_flag_set",
            r#"["web.allow_private_network", false]"#,
        )
        .await;
        assert_eq!(
            v["result"],
            serde_json::json!({ "key": "web.allow_private_network", "enabled": false, "running": true, "restart_required": true }),
            "{v}"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            "web:\n  allow_private_network: false\n  max_redirects: 3  # mine\n"
        );
    }

    /// 212 §15: the three loop switches read their section's default (on), report the file
    /// value as running (no restart, whatever the boot config says), and `set` writes the
    /// key and still asks for no restart.
    #[tokio::test]
    async fn config_flag_loop_switches_apply_without_restart() {
        let flag = |key: &str, on: bool| serde_json::json!({ "key": key, "enabled": on, "running": on, "restart_required": false });
        let keys = [
            "agent.status_tool",
            "agent.auto_title",
            "agent.suggestions",
            "knowledge.peek",
        ];
        for key in keys {
            let (v, _dir) =
                config_flag_call("", None, "config_flag_get", &format!(r#"["{key}"]"#)).await;
            assert_eq!(v["result"], flag(key, true), "{key} default: {v}");
        }
        let boot = "agent:\n  status_tool: true\n  auto_title: true\n  suggestions: true\nknowledge:\n  peek: true\n";
        let file = "agent:\n  model: x  # keep\n  status_tool: false\n  auto_title: false\n  suggestions: false\nknowledge:\n  peek: false\n";
        for key in keys {
            let (v, _dir) =
                config_flag_call(boot, Some(file), "config_flag_get", &format!(r#"["{key}"]"#))
                    .await;
            assert_eq!(v["result"], flag(key, false), "{key}: {v}");
        }
        let (v, dir) = config_flag_call(
            boot,
            Some("agent:\n  model: x  # keep\n"),
            "config_flag_set",
            r#"["agent.auto_title", false]"#,
        )
        .await;
        assert_eq!(v["result"], flag("agent.auto_title", false), "{v}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            "agent:\n  auto_title: false\n  model: x  # keep\n"
        );
        let (v, dir) =
            config_flag_call(boot, None, "config_flag_set", r#"["knowledge.peek", false]"#).await;
        assert_eq!(v["result"], flag("knowledge.peek", false), "{v}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            "knowledge:\n  peek: false\n"
        );
    }

    /// 215: `onboarding.completed` round-trips under that wire name, reads `false` when
    /// absent (never a restart, whatever the boot config says), and `set` writes
    /// `onboarding.completed` beside the file's other lines and comments.
    #[tokio::test]
    async fn config_flag_onboarding_completed() {
        let key = ConfigFlagKey::OnboardingCompleted;
        assert_eq!(serde_json::to_value(key).unwrap(), "onboarding.completed");
        assert_eq!(
            serde_json::from_value::<ConfigFlagKey>("onboarding.completed".into()).unwrap(),
            key
        );
        let flag = |on: bool| serde_json::json!({ "key": "onboarding.completed", "enabled": on, "running": on, "restart_required": false });
        let (v, _dir) =
            config_flag_call("", None, "config_flag_get", r#"["onboarding.completed"]"#).await;
        assert_eq!(v["result"], flag(false), "default: {v}");
        let (v, _dir) = config_flag_call(
            "onboarding:\n  completed: true\n",
            Some("onboarding:\n  completed: false\n"),
            "config_flag_get",
            r#"["onboarding.completed"]"#,
        )
        .await;
        assert_eq!(v["result"], flag(false), "file wins: {v}");
        let (v, dir) = config_flag_call(
            "",
            Some("agent:\n  model: x  # keep\n"),
            "config_flag_set",
            r#"["onboarding.completed", true]"#,
        )
        .await;
        assert_eq!(v["result"], flag(true), "{v}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            "agent:\n  model: x  # keep\n\nonboarding:\n  completed: true\n"
        );
    }

    /// 212 §15: `set_peek_threshold` writes `knowledge.peek_threshold` inside 0.05–0.95 and
    /// refuses anything else with the Settings message, writing nothing.
    #[tokio::test]
    async fn set_peek_threshold_writes_within_the_range() {
        let (v, dir) = config_flag_call("", None, "set_peek_threshold", "[0.65]").await;
        assert_eq!(v["result"], serde_json::Value::Null, "{v}");
        assert_eq!(
            std::fs::read_to_string(dir.path().join("config.yaml")).unwrap(),
            "knowledge:\n  peek_threshold: 0.65\n"
        );
        for bad in ["[0.01]", "[1.0]"] {
            let (v, dir) = config_flag_call("", None, "set_peek_threshold", bad).await;
            assert_eq!(v["error"]["code"], -32602, "{v}");
            assert_eq!(
                v["error"]["message"],
                "peek threshold must be between 0.05 and 0.95"
            );
            assert!(!dir.path().join("config.yaml").exists());
        }
    }

    /// 188: only the known keys parse; anything else is `INVALID_PARAMS` naming them,
    /// and nothing is written.
    #[tokio::test]
    async fn config_flag_rejects_an_unknown_key() {
        for (method, params) in [
            ("config_flag_get", r#"["dreaming.enabled"]"#),
            ("config_flag_set", r#"["agent.model", true]"#),
        ] {
            let (v, dir) = config_flag_call("", None, method, params).await;
            assert_eq!(v["error"]["code"], -32602, "{v}");
            let text = v["error"].to_string();
            for key in [
                "decision.enabled",
                "a2a.enabled",
                "computer.enabled",
                "double.enabled",
                "web.allow_private_network",
                "browser.allow_private_network",
            ] {
                assert!(text.contains(key), "{key} missing from {text}");
            }
            assert!(!dir.path().join("config.yaml").exists());
        }
    }

    /// 139 §9.2 (D18): the owner side of a routed `resume_turn` / `answer_ask` /
    /// `resolve_approval` — dispatched exactly as `forward::serve` does (internal
    /// peer + `Forwarded` marker) with the positional params the forwarder sends.
    /// Past the marker the call is local and reaches the actor lookup (no actors
    /// here → `NOT_RUNNING` / `ASK_FAILED`), never a second hop and never
    /// `INVALID_PARAMS`.
    #[tokio::test]
    async fn routed_methods_dispatch_on_the_owner_with_the_forwarders_params() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
        methods.extensions_mut().insert(rpc_core::RpcPeer::internal());
        methods
            .extensions_mut()
            .insert(forward::Forwarded(replica::PeerId("engine-b".into())));
        let session = SessionId::new();
        let ask_id = AskId(Uuid::now_v7());
        let approval = ApprovalId(Uuid::now_v7());
        let answers: Option<Vec<Answer>> = None;
        for (method, params, code) in [
            (
                "resume_turn",
                serde_json::to_string(&(session,)).unwrap(),
                NOT_RUNNING,
            ),
            (
                "answer_ask",
                serde_json::to_string(&(ask_id, &answers)).unwrap(),
                ASK_FAILED,
            ),
            (
                "resolve_approval",
                serde_json::to_string(&(approval, Decision::Allow)).unwrap(),
                NOT_RUNNING,
            ),
        ] {
            let req =
                format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params},"id":1}}"#);
            let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
            let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(i64::from(code)),
                "{req} → {v}"
            );
        }
    }

    /// 191 (D3): `a2a_forward` is mesh-internal (no `Forwarded` marker: `UNAUTHORIZED`, even
    /// on an internal connection); a forwarder `mesh.messaging_off` lists is refused in words
    /// that name no machine; any other forwarder reaches the owner side (no a2a plugin here:
    /// `OWNER_UNREACHABLE`, which the receiving machine reads as offline).
    #[tokio::test]
    async fn a2a_forward_needs_the_forwarded_marker_and_messaging_on() {
        let call = |forwarder: Option<&str>| {
            let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
            let mut methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
            methods.extensions_mut().insert(rpc_core::RpcPeer::internal());
            if let Some(f) = forwarder {
                methods
                    .extensions_mut()
                    .insert(forward::Forwarded(replica::PeerId(f.into())));
            }
            async move {
                let req = r#"{"jsonrpc":"2.0","method":"a2a_forward","params":["caller-eid","/agents/x/rpc","{}"],"id":1}"#;
                let (resp, _) = methods.raw_json_request(req, 1).await.unwrap();
                let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
                (
                    v["error"]["code"].as_i64(),
                    v["error"]["message"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                )
            }
        };
        assert_eq!(call(None).await.0, Some(i64::from(UNAUTHORIZED)));
        mesh::set_messaging_off(vec!["fwd-off-engine".into()]);
        let off = call(Some("fwd-off-engine")).await;
        let on = call(Some("fwd-on-engine")).await;
        mesh::set_messaging_off(Vec::new());
        assert_eq!(
            off,
            (
                Some(i64::from(ASK_FAILED)),
                A2A_FORWARD_MESSAGING_OFF.to_owned()
            )
        );
        assert_eq!(on.0, Some(i64::from(forward::OWNER_UNREACHABLE)), "{on:?}");
    }

    /// 185 R1 A5: the hub side of a forwarded Dream now — the positional
    /// `[RunRequest]` the forwarder sends dispatches into dreaming (no actor
    /// here → `NOT_RUNNING`), never `INVALID_PARAMS`, never a second hop.
    #[tokio::test]
    async fn a_forwarded_dream_now_runs_on_the_hub_with_the_forwarders_params() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
        methods.extensions_mut().insert(rpc_core::RpcPeer::internal());
        methods
            .extensions_mut()
            .insert(forward::Forwarded(replica::PeerId("node".into())));
        let request = dreaming::RunRequest {
            budget_usd: 2.0,
            mode: dreaming::RunMode::Pass,
            run_timeout_secs: None,
        };
        let params = serde_json::to_string(&[&request]).unwrap();
        let req =
            format!(r#"{{"jsonrpc":"2.0","method":"dreaming.run_now","params":{params},"id":1}}"#);
        let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
        let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
        assert_eq!(
            v["error"]["code"].as_i64(),
            Some(i64::from(NOT_RUNNING)),
            "{v}"
        );
    }

    /// 193 §7: `session_call_tool` refuses a name outside `SESSION_TOOLS` as invalid params
    /// before authorize (even for an unknown session); the six pass the allowlist (an
    /// unauthenticated raw call then fails at authorize, with another error).
    #[tokio::test]
    async fn session_call_tool_allowlist_precedes_authorize() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
        let call = |name: &str| {
            let methods = methods.clone();
            let req = format!(
                r#"{{"jsonrpc":"2.0","method":"session_call_tool","params":["{}","{name}",{{}}],"id":1}}"#,
                uuid::Uuid::now_v7()
            );
            async move {
                let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
                serde_json::from_str::<serde_json::Value>(resp.get()).unwrap()
            }
        };
        let invalid = i64::from(jsonrpsee::types::error::INVALID_PARAMS_CODE);
        let v = call("shell").await;
        assert_eq!(v["error"]["code"].as_i64(), Some(invalid), "{v}");
        for name in SESSION_TOOLS {
            let v = call(name).await;
            assert_ne!(v["error"]["code"].as_i64(), Some(invalid), "{name}: {v}");
            assert!(
                !v["error"]["message"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("not offered"),
                "{name}: {v}"
            );
        }
    }


    /// 139 §9.2 (D18), R1 §8.11: an answer to a question whose replicated row
    /// names a session owned elsewhere goes to that owner — forward, never take
    /// — and is `-32010` when the owner is not connected; a question of our own
    /// session, or one with no row, is answered locally.
    #[tokio::test]
    async fn answers_follow_the_questions_row_to_its_owner_or_fail_unreachable() {
        use tools::journal::{QuestionKind, QuestionRow, QuestionState};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sessions.sqlite");
        let (theirs_s, mine_s) = (SessionId::new(), SessionId::new());
        let (theirs_q, mine_q) = (Uuid::now_v7(), Uuid::now_v7());
        let question = |id: Uuid, session: SessionId| QuestionRow {
            id: id.to_string(),
            session,
            tool_call_id: None,
            kind: QuestionKind::Ask,
            payload: serde_json::json!({}),
            state: QuestionState::Pending,
            asked_at: chrono::Utc::now(),
            deadline: chrono::Utc::now(),
            answered_at: None,
            answer: None,
            answered_by: None,
        };
        let theirs = agent::store::Store::open(&path, Some(mesh::EngineId("them".into())))
            .await
            .unwrap();
        theirs
            .create_session(theirs_s, "m", None, None, None)
            .await
            .unwrap();
        theirs
            .question_put(&question(theirs_q, theirs_s))
            .await
            .unwrap();
        theirs.close().await;
        let store = agent::store::Store::open(&path, Some(mesh::EngineId("me".into())))
            .await
            .unwrap();
        store
            .create_session(mine_s, "m", None, None, None)
            .await
            .unwrap();
        store.question_put(&question(mine_q, mine_s)).await.unwrap();

        assert_eq!(
            question_owner(&store, &theirs_q).await.unwrap(),
            Some(replica::PeerId("them".into()))
        );
        assert_eq!(question_owner(&store, &mine_q).await.unwrap(), None);
        assert_eq!(question_owner(&store, &Uuid::now_v7()).await.unwrap(), None);

        let ext = Extensions::new();
        let local: Option<bool> = route_question_via(
            &ext,
            &store,
            &mine_q,
            "answer_ask",
            (AskId(mine_q), None::<Vec<Answer>>),
        )
        .await
        .unwrap();
        assert_eq!(local, None, "our own question: answered here");
        // No replica transport in this process: the owner is not connected.
        let e = route_question_via::<bool>(
            &ext,
            &store,
            &theirs_q,
            "answer_ask",
            (AskId(theirs_q), None::<Vec<Answer>>),
        )
        .await
        .unwrap_err();
        assert_eq!(e.code(), forward::OWNER_UNREACHABLE);
        let data: serde_json::Value = serde_json::from_str(e.data().unwrap().get()).unwrap();
        assert_eq!(
            data,
            serde_json::json!({ "OwnerUnreachable": { "owner": "them", "last_seen": null } })
        );
        store.close().await;
    }
    #[test]
    fn communication_changed_reaches_scoped_and_tree_subscribers() {
        let global =
            serde_json::json!({ "type": "CommunicationChanged", "session": null, "revision": 3 });
        let root =
            serde_json::json!({ "type": "CommunicationChanged", "session": "r", "revision": 4 });
        let other = serde_json::json!({ "type": "TitleChanged", "session": "r", "title": "x" });
        let mut tree: HashSet<String> = ["c".to_owned()].into();
        assert!(wanted(&global, Some("c"), None));
        assert!(wanted(&root, Some("c"), None));
        assert!(wanted(&global, Some("c"), Some(&mut tree)));
        assert!(wanted(&root, Some("c"), Some(&mut tree)));
        assert!(!wanted(&other, Some("c"), None));
        assert!(!wanted(&other, Some("c"), Some(&mut tree)));
    }

    /// 130 E1 / 207 §5.5: the inbox-changed event is a tagged object every `subscribe_events(null)`
    /// client sees; it carries the changed items, the order, its seq / epoch and the alerts flag;
    /// a pre-207 payload (no new fields) still decodes.
    #[test]
    fn attention_changed_wire_shape() {
        let item = roster::attention::AttentionItem::new(
            "finished:s1:t9".into(),
            roster::attention::AttentionKind::Finished,
            "s1 finished".into(),
            None,
            5,
        );
        let ev = roster::attention::AttentionChanged {
            count: 2,
            ids_added: vec!["finished:s1:t9".into()],
            ids_removed: vec!["ask:a1".into()],
            at_ms: 1_700_000_000_000,
            items: vec![item.clone()],
            order: vec!["finished:s1:t9".into(), "show:c1".into()],
            seq: 3,
            epoch_ms: 1_699_999_000_000,
            alerts_changed: true,
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "type": "AttentionChanged", "count": 2, "ids_added": ["finished:s1:t9"],
                "ids_removed": ["ask:a1"], "at_ms": 1_700_000_000_000u64,
                "items": [serde_json::to_value(&item).unwrap()],
                "order": ["finished:s1:t9", "show:c1"], "seq": 3,
                "epoch_ms": 1_699_999_000_000u64, "alerts_changed": true
            })
        );
        assert_eq!(v["items"][0]["score"], 0);
        assert_eq!(v["items"][0]["session_live"], false);
        assert!(v["items"][0]["stop"].is_null());
        let old: roster::attention::AttentionChanged = serde_json::from_value(serde_json::json!({
            "type": "AttentionChanged", "count": 0, "ids_added": [], "ids_removed": [], "at_ms": 1
        }))
        .unwrap();
        assert_eq!((old.seq, old.epoch_ms, old.alerts_changed), (0, 0, false));
        assert!(old.items.is_empty() && old.order.is_empty());
        assert!(
            wanted(&v, None, None),
            "session-less: every global subscriber"
        );
        assert!(wanted(&v, Some("other"), None));
        assert_eq!(
            serde_json::to_value(roster::attention::AttentionKind::Finished).unwrap(),
            "finished"
        );
    }

    /// LIVE-GLYPHS A2: the fired reminder reaches the desktop as a tagged
    /// object; a global subscriber sees it like `JobStarted`.
    #[test]
    fn reminder_fired_wire_shape() {
        let s = SessionId::new();
        let v = serde_json::to_value(tools::ReminderEvent::Fired {
            session: s,
            id: "r1".into(),
        })
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "type": "ReminderFired", "session": s, "id": "r1" })
        );
        assert!(wanted(&v, None, None));
        assert!(!is_delta(&v));
    }

    /// Private events reach their owner only (51 §4.2): a member's event is
    /// dropped for every other peer; `SubagentSpawned` under a private parent
    /// enrols the child so its own events are filtered too; events naming no
    /// private member pass for everyone.
    #[test]
    fn private_events_reach_only_their_owner() {
        let (p, c, d) = (SessionId::new(), SessionId::new(), SessionId::new());
        let private: PrivateMap = Default::default();
        private.write().insert(p, (p, 7));
        let title = serde_json::json!({ "type": "TitleChanged", "session": p, "title": "x" });
        assert!(admitted(&title, 7, &private));
        assert!(!admitted(&title, 8, &private));

        let spawned = serde_json::json!({ "type": "SubagentSpawned", "parent": p, "child": c });
        assert!(!admitted(&spawned, 8, &private));
        assert!(admitted(&spawned, 7, &private));
        assert_eq!(private.read().get(&c), Some(&(p, 7)));
        let child = serde_json::json!({ "type": "TextDelta", "session": c, "text": "hi" });
        assert!(admitted(&child, 7, &private));
        assert!(!admitted(&child, 8, &private));

        let durable = serde_json::json!({ "type": "TitleChanged", "session": d, "title": "y" });
        let none = serde_json::json!({ "type": "Lagged", "count": 1 });
        for peer in [7, 8] {
            assert!(admitted(&durable, peer, &private));
            assert!(admitted(&none, peer, &private));
        }
    }

    /// 141 §8.2: `ObservedChildChanged` is the watch-only twin of
    /// `SubagentSpawned` for both filters — it enrols `child` in a private
    /// tree and in a `descendants` subscription, so the child's transcript
    /// events follow; a repeat (every state change re-announces) is harmless.
    #[test]
    fn observed_child_changed_enrols_like_subagent_spawned() {
        let (p, c, other) = (SessionId::new(), SessionId::new(), SessionId::new());
        let announce = serde_json::json!({
            "type": "ObservedChildChanged", "parent": p, "child": c, "name": "ReadA",
            "model": null, "observed": { "source": "omp_task", "external": "ReadA", "state": "running" }
        });
        let private: PrivateMap = Default::default();
        private.write().insert(p, (p, 7));
        assert!(!admitted(&announce, 8, &private));
        assert!(admitted(&announce, 7, &private));
        assert_eq!(private.read().get(&c), Some(&(p, 7)));
        let child = serde_json::json!({ "type": "TextDelta", "session": c, "text": "hi" });
        assert!(!admitted(&child, 8, &private));

        let mut tree: HashSet<String> = HashSet::from([p.to_string()]);
        assert!(!wanted(&child, Some(&p.to_string()), Some(&mut tree)));
        assert!(wanted(&announce, Some(&p.to_string()), Some(&mut tree)));
        assert!(
            wanted(&announce, Some(&p.to_string()), Some(&mut tree)),
            "repeat"
        );
        assert!(wanted(&child, Some(&p.to_string()), Some(&mut tree)));
        let stranger = serde_json::json!({ "type": "ObservedChildChanged", "parent": other, "child": SessionId::new() });
        assert!(!wanted(&stranger, Some(&p.to_string()), Some(&mut tree)));
        assert_eq!(tree.len(), 2);
    }

    /// 129 §3: focus filter, delta merging, bounded backlog, `Lagged`.
    mod event_focus {
        use super::*;
        use jsonrpsee::core::server::Subscription;
        #[cfg(test)]
use std::time::Duration;

        fn delta(kind: &str, s: SessionId, turn: &str, text: &str) -> serde_json::Value {
            serde_json::json!({ "type": kind, "session": s, "turn": turn, "text": text })
        }

        fn progress(s: SessionId, call: &str, args: &str) -> serde_json::Value {
            serde_json::json!({ "type": "ToolCallProgress", "session": s, "turn": "t", "call_id": call, "args": args })
        }

        fn title(s: SessionId, title: impl Display) -> serde_json::Value {
            serde_json::json!({ "type": "TitleChanged", "session": s, "title": title.to_string() })
        }


        struct Harness {
            methods: jsonrpsee::Methods,
            tx: broadcast::Sender<Arc<WireFrame>>,
            focus: FocusMap,
            peer: rpc_core::RpcPeer,
            guard: Option<rpc_core::RpcPeerGuard>,
        }

        impl Harness {
            /// A server whose feed is a ring of `ring` slots, every call authenticated as one peer.
            fn new(ring: usize) -> Self {
                let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
                let mut server = crate::new_server(rt);
                let tx = broadcast::channel(ring).0;
                server.events = tx.clone();
                let focus = server.focus.clone();
                let mut methods: jsonrpsee::Methods = test_rpc_module(server).into();
                let (peer, guard) = rpc_core::RpcPeer::open();
                methods.extensions_mut().insert(peer.clone());
                Self {
                    methods,
                    tx,
                    focus,
                    peer,
                    guard: Some(guard),
                }
            }

            /// `subscribe_events(null, null)` with a sink buffer of `buf`; the snapshot is consumed.
            async fn stream(&self, buf: usize) -> Subscription {
                let mut sub = self
                    .methods
                    .subscribe(
                        "subscribe_events",
                        [serde_json::Value::Null, serde_json::Value::Null],
                        buf,
                    )
                    .await
                    .unwrap();
                assert_eq!(next(&mut sub).await["type"], "PendingPrompts");
                sub
            }

            async fn focus(&self, sessions: Vec<SessionId>) {
                self.methods
                    .call::<_, ()>("set_event_focus", (sessions,))
                    .await
                    .unwrap();
            }

            fn publish(&self, ev: serde_json::Value) {
                self.tx.send(Arc::new(WireFrame {
                    policy: policy(&ev), descriptor: None, event: ev,
                })).unwrap();
            }
        }

        async fn next(sub: &mut Subscription) -> serde_json::Value {
            tokio::time::timeout(Duration::from_secs(5), sub.next::<serde_json::Value>())
                .await
                .expect("an event in time")
                .expect("stream open")
                .expect("json")
                .0
        }

        async fn quiet(sub: &mut Subscription) {
            let r =
                tokio::time::timeout(Duration::from_secs(1), sub.next::<serde_json::Value>()).await;
            assert!(r.is_err(), "unexpected event: {r:?}");
        }

        /// (a) With a focus set, deltas of other sessions are dropped and their
        /// non-delta events pass; `[]` drops every delta; a later call replaces
        /// the set on a stream opened before it; no call passes everything.
        #[tokio::test(start_paused = true)]
        async fn focus_filters_deltas_only() {
            let h = Harness::new(1024);
            let mut sub = h.stream(1024).await;
            let (a, b) = (SessionId::new(), SessionId::new());
            h.focus(vec![a]).await;
            h.publish(delta("TextDelta", a, "t", "x"));
            h.publish(delta("TextDelta", b, "t", "y"));
            h.publish(progress(b, "c", "{}"));
            h.publish(title(b, "T"));
            assert_eq!(next(&mut sub).await, delta("TextDelta", a, "t", "x"));
            assert_eq!(next(&mut sub).await, title(b, "T"));
            quiet(&mut sub).await;

            h.focus(vec![b]).await;
            h.publish(delta("TextDelta", a, "t", "x"));
            h.publish(delta("ReasoningDelta", b, "t", "r"));
            assert_eq!(next(&mut sub).await, delta("ReasoningDelta", b, "t", "r"));
            quiet(&mut sub).await;

            h.focus(vec![]).await;
            h.publish(delta("TextDelta", b, "t", "x"));
            h.publish(progress(b, "c", "{}"));
            h.publish(title(a, "U"));
            assert_eq!(next(&mut sub).await, title(a, "U"));
            quiet(&mut sub).await;

            let open = Harness::new(1024);
            let mut sub = open.stream(1024).await;
            open.publish(delta("TextDelta", a, "t", "x"));
            open.publish(progress(b, "c", "{}"));
            assert_eq!(next(&mut sub).await, delta("TextDelta", a, "t", "x"));
            assert_eq!(next(&mut sub).await, progress(b, "c", "{}"));
        }

        /// (c) A client that stops reading while 10 000 deltas arrive through a
        /// 1024-slot ring sees no `Lagged`: the ring is drained regardless, and
        /// the text reaches it whole in a few frames.
        #[tokio::test]
        async fn stalled_sink_merges_deltas_without_lag() {
            let h = Harness::new(1024);
            let mut sub = h.stream(1).await;
            let s = SessionId::new();
            let mut want = String::new();
            for i in 0..10_000 {
                let piece = format!("{i} ");
                want.push_str(&piece);
                h.publish(delta("TextDelta", s, "t", &piece));
                // The publisher's ring is read by the stream's own task.
                tokio::task::yield_now().await;
            }
            h.publish(title(s, "end"));
            let mut got = String::new();
            let mut frames = 0;
            loop {
                let ev = next(&mut sub).await;
                assert_ne!(ev["type"], "Lagged", "{ev}");
                if ev["type"] == "TitleChanged" {
                    break;
                }
                assert_eq!(ev["type"], "TextDelta");
                got.push_str(ev["text"].as_str().unwrap());
                frames += 1;
            }
            assert_eq!(got, want);
            assert!(frames <= 3, "{frames} frames");
        }

        /// (d) end to end: a stalled sink past `BACKLOG` non-delta events gets one
        /// `Lagged` whose count is exactly what is missing, then the snapshot,
        /// then the rest in order.
        #[tokio::test]
        async fn backlog_overflow_lags_once_then_snapshots() {
            let h = Harness::new(1 << 14);
            let mut sub = h.stream(1).await;
            let s = SessionId::new();
            let n = BACKLOG + 3;
            for i in 0..n {
                h.publish(title(s, i));
                tokio::task::yield_now().await;
            }
            let mut lagged = None;
            let mut titles = Vec::new();
            while titles.len() + lagged.unwrap_or(0) < n {
                let ev = next(&mut sub).await;
                match ev["type"].as_str().unwrap() {
                    "Lagged" => {
                        assert!(lagged.is_none(), "second Lagged");
                        lagged = Some(ev["count"].as_u64().unwrap() as usize);
                        assert_eq!(next(&mut sub).await["type"], "PendingPrompts");
                    }
                    "TitleChanged" => {
                        titles.push(ev["title"].as_str().unwrap().parse::<usize>().unwrap())
                    }
                    other => panic!("{other}"),
                }
            }
            let lost = lagged.expect("one Lagged");
            assert!(lost >= 1, "{lost}");
            assert!(titles.windows(2).all(|w| w[0] < w[1]));
            assert_eq!(*titles.last().unwrap(), n - 1);
        }

        /// (e) The focus entry dies with its connection.
        #[tokio::test]
        async fn focus_dies_with_the_connection() {
            let mut h = Harness::new(1024);
            let id = h.peer.id();
            h.focus(vec![SessionId::new()]).await;
            h.focus(vec![]).await;
            assert!(h.focus.read().get(&id).is_some_and(|set| set.is_empty()));
            drop(h.guard.take());
            for _ in 0..100 {
                if !h.focus.read().contains_key(&id) {
                    return;
                }
                tokio::task::yield_now().await;
            }
            panic!("focus entry outlived its connection");
        }
    }

    /// Reads and mutations on a session need an authenticated connection
    /// before anything else is consulted; a subscription is no exception.
    #[tokio::test]
    async fn session_calls_need_an_authenticated_peer() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
        let id = SessionId::new();
        for req in [
            format!(r#"{{"jsonrpc":"2.0","method":"get_todos","params":["{id}"],"id":1}}"#),
            format!(r#"{{"jsonrpc":"2.0","method":"session_cost","params":["{id}"],"id":1}}"#),
            format!(
                r#"{{"jsonrpc":"2.0","method":"put_session_attachment","params":["{id}","AA==","png"],"id":1}}"#
            ),
            r#"{"jsonrpc":"2.0","method":"create_session","params":[{}],"id":1}"#.to_owned(),
            r#"{"jsonrpc":"2.0","method":"subscribe_events","params":[null,null],"id":1}"#
                .to_owned(),
        ] {
            let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
            let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(i64::from(UNAUTHORIZED)),
                "{req}"
            );
        }
    }

    /// The JSON-RPC error code of a raw `app_close` on `methods`.
    async fn app_close_code(methods: &jsonrpsee::Methods) -> i64 {
        let (resp, _) = methods
            .raw_json_request(
                r#"{"jsonrpc":"2.0","method":"app_close","params":["v1"],"id":1}"#,
                1,
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
        v["error"]["code"].as_i64().expect("error reply")
    }

    /// App calls are scoped to the connection that made them: a dispatch with
    /// no `RpcPeer` in its extensions is unauthorized, never treated as local
    /// (46 §6.3). With a peer the call reaches the actor lookup.
    #[tokio::test]
    async fn app_calls_need_an_authenticated_peer() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let base: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
        assert_eq!(app_close_code(&base).await, i64::from(UNAUTHORIZED));

        let (peer, _guard) = rpc_core::RpcPeer::open();
        let mut with_peer = base.clone();
        with_peer.extensions_mut().insert(peer);
        // No app actor in this process: past the peer gate, the lookup fails.
        assert_eq!(app_close_code(&with_peer).await, i64::from(NOT_RUNNING));
    }

    /// Dispatch a raw dictation request as a remote (relay) peer on a fresh server.
    async fn remote_dictation_call(method: &str, params: &str) -> serde_json::Value {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let mut methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
        let (peer, _guard) = rpc_core::RpcPeer::open_remote();
        methods.extensions_mut().insert(peer);
        let req = format!(r#"{{"jsonrpc":"2.0","method":"{method}","params":{params},"id":1}}"#);
        let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
        serde_json::from_str(resp.get()).unwrap()
    }

    /// The daemon's microphone is out of a relay peer's reach: `dictation_start`
    /// without a `sample_rate` is refused with the reason, before any actor
    /// lookup (54 §8).
    #[tokio::test]
    async fn dictation_mic_refuses_relay_peers() {
        let v = remote_dictation_call("dictation_start", "[null]").await;
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(ASK_FAILED)));
        assert_eq!(
            v["error"]["message"].as_str(),
            Some("microphone is on the harness host; pass sample_rate to stream audio")
        );
    }

    /// Everything but the daemon mic is open to a relay peer: `dictation_status`
    /// reaches the actor lookup (no voice actor here, so `NOT_RUNNING`).
    #[tokio::test]
    async fn dictation_status_reaches_relay_peers() {
        let v = remote_dictation_call("dictation_status", "[]").await;
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(NOT_RUNNING)));
    }

    /// 128 Rev 3: `spend_report` is registered, parses `[days]` and reaches
    /// the agent lookup (no actor here, so `NOT_RUNNING`); its reply type
    /// round-trips with `kind` on the wire as the snake-case string.
    #[tokio::test]
    async fn spend_report_is_registered_and_round_trips() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
        assert!(methods.method_names().any(|m| m == "spend_report"));
        let (resp, _) = methods
            .raw_json_request(
                r#"{"jsonrpc":"2.0","method":"spend_report","params":[7],"id":1}"#,
                1,
            )
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(NOT_RUNNING)));

        let report = SpendReport {
            days: vec![agent::SpendDay {
                day: "2026-10-07".into(),
                usd: 1302.38,
                calls: 3,
                unresolved: 1,
            }],
            month: agent::SpendDay {
                day: "2026-10".into(),
                usd: 4102.0,
                calls: 36_861,
                unresolved: 0,
            },
            owners: vec![agent::SpendOwner {
                kind: agent::SpendOwnerKind::Agent,
                name: "builder".into(),
                usd: 12.5,
                calls: 40,
            }],
        };
        let json = serde_json::to_value(&report).unwrap();
        assert_eq!(json["owners"][0]["kind"], "agent");
        assert_eq!(json["days"][0]["day"], "2026-10-07");
        let back: SpendReport = serde_json::from_value(json).unwrap();
        assert_eq!(back, report);
    }

    /// 160 §3.2/§4/§5: the four methods are registered, `cost_calls` takes a
    /// partial query (absent filters = none), `usage_limits` answers from the
    /// hub, and the wire shapes the UI codes against hold.
    #[tokio::test]
    async fn cost_methods_are_registered_with_their_wire_shapes() {
        let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
        let methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt)).into();
        let call = |body: String| {
            let methods = methods.clone();
            async move {
                let (resp, _) = methods.raw_json_request(&body, 1).await.unwrap();
                serde_json::from_str::<serde_json::Value>(resp.get()).unwrap()
            }
        };
        for (m, params) in [
            ("cost_cube", "[null]"),
            ("cost_calls", r#"[{"bases":["reported"],"limit":5}]"#),
            ("usage_limits", "[]"),
            ("usage_limits_refresh", "[null]"),
        ] {
            let v = call(format!(
                r#"{{"jsonrpc":"2.0","method":"{m}","params":{params},"id":1}}"#
            ))
            .await;
            let code = v["error"]["code"].as_i64();
            assert!(
                code.is_none() || code == Some(i64::from(NOT_RUNNING)),
                "{m}: {v}"
            );
        }
        let v =
            call(r#"{"jsonrpc":"2.0","method":"usage_limits","params":[],"id":1}"#.into()).await;
        assert!(v["result"]["providers"].is_array(), "{v}");
        let ev = serde_json::to_value(agent::CostCubeChanged {
            epoch: "e".into(),
            rev: 3,
        })
        .unwrap();
        assert_eq!(
            ev,
            serde_json::json!({"type": "CostCubeChanged", "epoch": "e", "rev": 3})
        );
        let q: agent::CostCallsQuery =
            serde_json::from_str(r#"{"bases":["unpriced"],"before":{"at_ms":5,"id":"x"}}"#)
                .unwrap();
        assert_eq!(q.bases, Some(vec![agent::CostBasis::Unpriced]));
        assert_eq!(q.limit, None);
    }

    /// The daemon's devices are out of a relay peer's reach: `talk_start` with
    /// `sink: device` is refused before any actor lookup (76 §5.1).
    #[tokio::test]
    async fn talk_device_sink_refuses_relay_peers() {
        let v = remote_dictation_call(
            "talk_start",
            &format!(r#"["{}","device",null]"#, SessionId::new()),
        )
        .await;
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(ASK_FAILED)));
        assert!(
            v["error"]["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("speaker and microphone are on the harness host"))
        );
        // A client sink without a rate is refused for anyone.
        let v = remote_dictation_call(
            "talk_start",
            &format!(r#"["{}","client",null]"#, SessionId::new()),
        )
        .await;
        assert_eq!(
            v["error"]["message"].as_str(),
            Some("sink: client needs sample_rate")
        );
        // Well-formed: reaches the actor lookup (no talk actor here).
        let v = remote_dictation_call(
            "talk_start",
            &format!(r#"["{}","client",48000]"#, SessionId::new()),
        )
        .await;
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(NOT_RUNNING)));
        let v = remote_dictation_call("talk_status", "[]").await;
        assert_eq!(v["error"]["code"].as_i64(), Some(i64::from(NOT_RUNNING)));
    }

    /// 120 §13.1: `page_preview` driven against the real `answer_page_preview`
    /// dispatch. Time is paused, so the 2 s / 15 s deadlines elapse at once.
    mod page_preview {
        use super::*;
        #[cfg(test)]
use std::time::Duration;

        use artifact::preview::{
            Client, ConsoleLevel, ConsoleLine, PagePreviewAnswer, PagePreviewRequested,
            PagePreviewResolved, PagePreviewTool, PreviewArgs, PreviewId,
        };
        use kameo::actor::Spawn;
        use kameo::message::Context;
        use tokio::sync::mpsc;
        use tools::{Tool, ToolCx, ToolOutput};

        /// Bus subscriber: the request ids and the resolved ids, in order.
        #[derive(kameo::Actor)]
        struct Capture(mpsc::UnboundedSender<(bool, PreviewId)>);

        impl Message<PagePreviewRequested> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: PagePreviewRequested, _: &mut Context<Self, ()>) {
                assert_eq!(m.html, "<p>x</p>");
                let _ = self.0.send((false, m.id));
            }
        }

        impl Message<PagePreviewResolved> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: PagePreviewResolved, _: &mut Context<Self, ()>) {
                let _ = self.0.send((true, m.id));
            }
        }

        struct Harness {
            rt: Runtime,
            stores: Arc<artifact::Stores>,
            methods: jsonrpsee::Methods,
            events: mpsc::UnboundedReceiver<(bool, PreviewId)>,
            _dir: tempfile::TempDir,
        }

        impl Harness {
            async fn new() -> Self {
                let dir = tempfile::tempdir().unwrap();
                let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
                // Paused time auto-advances while sqlx waits on its worker thread and
                // trips the pool's acquire timeout: open the store on real time.
                tokio::time::resume();
                let state = artifact::State::open(dir.path(), rt.clone(), Vec::new())
                    .await
                    .unwrap();
                tokio::time::pause();
                let stores = Arc::new(artifact::Stores::new(Arc::new(state)));
                let methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt.clone())).into();
                let (tx, events) = mpsc::unbounded_channel();
                let cap = Capture::spawn(Capture(tx));
                rt.subscribe::<Capture, PagePreviewRequested>(&cap).await;
                rt.subscribe::<Capture, PagePreviewResolved>(&cap).await;
                Self {
                    rt,
                    stores,
                    methods,
                    events,
                    _dir: dir,
                }
            }

            /// Run the tool for `session` on its own task; the width defaults.
            fn call(&self, session: SessionId) -> tokio::task::JoinHandle<ToolOutput> {
                let tool = PagePreviewTool {
                    rt: self.rt.clone(),
                    stores: self.stores.clone(),
                };
                let cx = ToolCx {
                    session,
                    call_id: "c".into(),
                    quiet: false,
                    resources: tools::SessionResources::durable_arc(&std::env::temp_dir()),
                    cancel: tools::CancellationToken::new(),
                    promote_after: None,
                };
                tokio::spawn(async move {
                    Tool::call(
                        &tool,
                        PreviewArgs {
                            html: Some("<p>x</p>".into()),
                            ..Default::default()
                        },
                        cx,
                    )
                    .await
                    .unwrap()
                })
            }

            async fn requested(&mut self) -> PreviewId {
                let (resolved, id) = self.events.recv().await.unwrap();
                assert!(!resolved, "expected a request");
                id
            }

            async fn resolved(&mut self) -> PreviewId {
                let (resolved, id) = self.events.recv().await.unwrap();
                assert!(resolved, "expected a resolution");
                id
            }

            /// The raw `answer_page_preview` result.
            async fn answer(&self, id: PreviewId, answer: &PagePreviewAnswer) -> bool {
                let req = format!(
                    r#"{{"jsonrpc":"2.0","method":"answer_page_preview","params":[{},{}],"id":1}}"#,
                    serde_json::to_string(&id).unwrap(),
                    serde_json::to_string(answer).unwrap()
                );
                let (resp, _) = self.methods.raw_json_request(&req, 1).await.unwrap();
                let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
                v["result"].as_bool().unwrap_or_else(|| panic!("{v}"))
            }
        }

        fn started() -> PagePreviewAnswer {
            PagePreviewAnswer::Started {
                client: Client::Desktop,
            }
        }

        fn done(height: u32) -> PagePreviewAnswer {
            PagePreviewAnswer::Done {
                content_height: height,
                console: vec![ConsoleLine {
                    level: ConsoleLevel::Error,
                    text: "boom".into(),
                }],
                loaded: true,
                client: Client::Desktop,
            }
        }

        #[tokio::test(start_paused = true)]
        async fn answer_page_preview_first_done_wins() {
            let mut h = Harness::new().await;
            let call = h.call(SessionId::new());
            let id = h.requested().await;
            assert!(h.answer(id, &started()).await);
            assert!(h.answer(id, &done(612)).await);
            assert!(!h.answer(id, &done(700)).await, "second Done is late");
            assert!(
                !h.answer(id, &started()).await,
                "Started after Done is late"
            );
            let out = call.await.unwrap();
            assert!(!out.is_error, "{}", out.content);
            assert!(out.content.starts_with(
                "Previewed at 728 px: contentHeight 612, 1 error. Pass height: 612 to show"
            ));
            assert_eq!(out.structured.unwrap()["contentHeight"], 612);
            assert_eq!(h.resolved().await, id);
        }

        #[tokio::test(start_paused = true)]
        async fn started_extends_the_deadline() {
            let mut h = Harness::new().await;
            let call = h.call(SessionId::new());
            let id = h.requested().await;
            assert!(h.answer(id, &started()).await);
            // Past the 2 s "no renderer" mark, inside the 15 s total.
            tokio::time::sleep(Duration::from_secs(5)).await;
            assert!(h.answer(id, &done(300)).await);
            let out = call.await.unwrap();
            assert!(!out.is_error, "{}", out.content);

            // A `Started` with no `Done` runs out at 15 s.
            let call = h.call(SessionId::new());
            h.resolved().await;
            let id = h.requested().await;
            assert!(h.answer(id, &started()).await);
            let out = call.await.unwrap();
            assert!(out.is_error);
            assert!(
                out.content
                    .starts_with("page_preview: preview timed out after 15 s"),
                "{}",
                out.content
            );
            assert!(!h.answer(id, &done(1)).await, "dead after the timeout");
        }

        #[tokio::test(start_paused = true)]
        async fn no_started_in_two_seconds_is_no_renderer() {
            let mut h = Harness::new().await;
            let call = h.call(SessionId::new());
            let id = h.requested().await;
            let out = call.await.unwrap();
            assert!(out.is_error);
            assert!(
                out.content
                    .starts_with("page_preview: no renderer connected — open the theuth desktop"),
                "{}",
                out.content
            );
            assert_eq!(h.resolved().await, id);
            assert!(!h.answer(id, &started()).await);
        }

        #[tokio::test(start_paused = true)]
        async fn two_previews_in_one_session_run_in_parallel() {
            let mut h = Harness::new().await;
            let session = SessionId::new();
            let first = h.call(session);
            let id = h.requested().await;
            let second = h.call(session);
            let other_id = h.requested().await;
            assert_ne!(other_id, id);
            assert!(h.answer(id, &done(100)).await);
            assert!(h.answer(other_id, &done(200)).await);
            assert!(!first.await.unwrap().is_error);
            assert!(!second.await.unwrap().is_error);
        }
    }

    /// 209 D8, P3: the `browser_*` tools on a tile, driven against the real
    /// `answer_web_view_call` dispatch by a fake client. Time is paused, so the 2 s / 30 s
    /// deadlines elapse at once.
    mod web_view_call {
        use super::*;
        use std::time::Duration;

        use artifact::webview::{
            WebViewCallAnswer, WebViewCallId, WebViewCallRequested, WebViewCallResolved, WebViewOp,
            WebViewState,
        };
        use kameo::actor::Spawn;
        use kameo::message::Context;
        use serde_json::{Value, json};
        use tokio::sync::mpsc;
        use tools::{DynTool, ToolCx, ToolOutput};

        const VIEW: &str = "0123456789abcdef";

        /// Bus subscriber: requests and resolutions, in order.
        #[derive(kameo::Actor)]
        struct Capture(mpsc::UnboundedSender<Result<WebViewCallRequested, WebViewCallId>>);

        impl Message<WebViewCallRequested> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: WebViewCallRequested, _: &mut Context<Self, ()>) {
                let _ = self.0.send(Ok(m));
            }
        }

        impl Message<WebViewCallResolved> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: WebViewCallResolved, _: &mut Context<Self, ()>) {
                let _ = self.0.send(Err(m.id));
            }
        }

        struct Harness {
            tools: Vec<DynTool>,
            methods: jsonrpsee::Methods,
            events: mpsc::UnboundedReceiver<Result<WebViewCallRequested, WebViewCallId>>,
            session: SessionId,
            dir: tempfile::TempDir,
            _peer: rpc_core::RpcPeerGuard,
        }

        impl Harness {
            async fn new() -> Self {
                let dir = tempfile::tempdir().unwrap();
                let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
                let mut methods: jsonrpsee::Methods =
                    test_rpc_module(crate::new_server(rt.clone())).into();
                let (peer, guard) = rpc_core::RpcPeer::open();
                methods.extensions_mut().insert(peer);
                let (tx, events) = mpsc::unbounded_channel();
                let cap = Capture::spawn(Capture(tx));
                rt.subscribe::<Capture, WebViewCallRequested>(&cap).await;
                rt.subscribe::<Capture, WebViewCallResolved>(&cap).await;
                // No isolate is ever made: every call below runs on the tile.
                let cfg = browser::BrowserConfig::default();
                let actor = browser::Browsers::spawn(browser::Browsers::new(cfg.clone()));
                let session = SessionId::new();
                // The session opened the tile (209 §9: attach only within the opener's tree).
                artifact::webview::record_state(WebViewState {
                    session,
                    view: VIEW.into(),
                    url: "https://a.test/".into(),
                    title: "A".into(),
                    loading: false,
                    closed: false,
                });
                Self {
                    tools: browser::tools::all(actor, cfg, rt),
                    methods,
                    events,
                    session,
                    dir,
                    _peer: guard,
                }
            }

            fn tool(&self, name: &str) -> DynTool {
                self.tools
                    .iter()
                    .find(|t| t.spec().name == name)
                    .unwrap_or_else(|| panic!("no tool {name}"))
                    .clone()
            }

            /// Run `name` for the session on its own task, with the owner's local-network grant.
            fn call(&self, name: &str, args: Value) -> tokio::task::JoinHandle<ToolOutput> {
                let tool = self.tool(name);
                let cx = ToolCx {
                    session: self.session,
                    call_id: "c".into(),
                    quiet: false,
                    resources: tools::SessionResources::durable_arc(self.dir.path()),
                    cancel: tools::CancellationToken::new(),
                    promote_after: None,
                };
                tokio::spawn(tools::egress::scope(true, async move {
                    tool.call(args, cx).await.unwrap()
                }))
            }

            async fn requested(&mut self) -> WebViewCallRequested {
                self.events
                    .recv()
                    .await
                    .unwrap()
                    .expect("expected a request, got a resolution")
            }

            async fn resolved(&mut self) -> WebViewCallId {
                match self.events.recv().await.unwrap() {
                    Err(id) => id,
                    Ok(r) => panic!("expected a resolution, got {r:?}"),
                }
            }

            /// The raw `answer_web_view_call` reply.
            async fn answer_raw(&self, id: WebViewCallId, answer: &WebViewCallAnswer) -> Value {
                let req = format!(
                    r#"{{"jsonrpc":"2.0","method":"answer_web_view_call","params":[{},{}],"id":1}}"#,
                    serde_json::to_string(&id).unwrap(),
                    serde_json::to_string(answer).unwrap()
                );
                let (resp, _) = self.methods.raw_json_request(&req, 1).await.unwrap();
                serde_json::from_str(resp.get()).unwrap()
            }

            async fn answer(&self, id: WebViewCallId, answer: &WebViewCallAnswer) -> bool {
                let v = self.answer_raw(id, answer).await;
                v["result"].as_bool().unwrap_or_else(|| panic!("{v}"))
            }

            /// Claim the next request as `client` and finish it with `result`.
            async fn serve(&mut self, result: Value) -> WebViewCallRequested {
                let r = self.requested().await;
                assert!(self.answer(r.id, &claimed("ipad")).await);
                assert!(self.answer(r.id, &done(result)).await);
                assert_eq!(self.resolved().await, r.id);
                r
            }

            /// `browser_attach` to [`VIEW`], answered by a client on `https://a.test/`.
            async fn attach(&mut self) -> ToolOutput {
                let call = self.call("browser_attach", json!({ "view": format!("web:{VIEW}") }));
                let r = self
                    .serve(json!({ "url": "https://a.test/inbox", "title": "Inbox" }))
                    .await;
                assert_eq!(r.view, VIEW);
                assert_eq!(r.session, self.session);
                assert!(!r.delegated);
                assert_eq!(r.op, WebViewOp::Script);
                assert_eq!(r.args["world"], "isolated");
                assert_eq!(r.args["arguments"]["cmd"], "location");
                call.await.unwrap()
            }
        }

        fn claimed(client: &str) -> WebViewCallAnswer {
            WebViewCallAnswer::Claimed {
                client: client.into(),
            }
        }

        fn done(result: Value) -> WebViewCallAnswer {
            WebViewCallAnswer::Done { result, png: None }
        }

        const PREFIX: &str = "[tile web:0123456789abcdef \u{b7} a.test]";

        /// Attach, read through the agent script, a second claimant refused, a late answer
        /// refused, every reply prefixed, close detaches without a call.
        #[tokio::test(start_paused = true)]
        async fn attach_read_and_detach_round_trip() {
            let mut h = Harness::new().await;
            let out = h.attach().await;
            assert!(!out.is_error, "{}", out.content);
            assert!(out.content.starts_with(PREFIX), "{}", out.content);
            assert!(out.content.contains("\"Inbox\" (https://a.test/inbox)"), "{}", out.content);

            let call = h.call("browser_read", json!({}));
            let r = h.requested().await;
            assert_eq!(r.args["arguments"]["cmd"], "read");
            assert_eq!(r.args["source"], *browser::tile::AGENT_JS);
            assert!(h.answer(r.id, &claimed("ipad")).await);
            assert!(!h.answer(r.id, &claimed("mac")).await, "the first claim wins");
            assert!(
                h.answer(
                    r.id,
                    &done(json!({ "url": "https://a.test/inbox", "title": "Inbox", "text": "ref=e1 button \"Archive\" role=button\nThree new mails" }))
                )
                .await
            );
            assert!(!h.answer(r.id, &done(json!({}))).await, "a second Done is late");
            assert_eq!(h.resolved().await, r.id);
            let out = call.await.unwrap();
            assert!(!out.is_error, "{}", out.content);
            assert_eq!(
                out.content,
                format!("{PREFIX}\nURL: https://a.test/inbox\nTitle: Inbox\n\nref=e1 button \"Archive\" role=button\nThree new mails")
            );

            // browser_tabs and browser_show stay off the tile, without a call.
            let out = h.call("browser_tabs", json!({ "action": "list" })).await.unwrap();
            assert!(out.is_error && out.content.starts_with(PREFIX), "{}", out.content);
            assert!(out.content.contains("web_views"), "{}", out.content);
            let out = h.call("browser_show", json!({})).await.unwrap();
            assert!(out.is_error && out.content.contains("already looking"), "{}", out.content);

            let out = h.call("browser_close", json!({})).await.unwrap();
            assert!(!out.is_error, "{}", out.content);
            assert!(out.content.starts_with(PREFIX), "{}", out.content);
            assert!(out.content.contains("Detached from tile web:0123456789abcdef"), "{}", out.content);
            let out = h.call("browser_attach", json!({})).await.unwrap();
            assert!(out.content.starts_with("Already on your own browser"), "{}", out.content);
            assert!(h.events.try_recv().is_err(), "detaching asks no client");
        }

        /// A client's `Failed` and the agent script's `{ error }` both reach the model as
        /// errors; acting calls on a tile are credentialed, reads are not.
        #[tokio::test(start_paused = true)]
        async fn failures_reach_the_model_and_acting_calls_are_credentialed() {
            let mut h = Harness::new().await;
            let click = h.tool("browser_click");
            assert!(!click.credentialed(h.session, &json!({ "ref": "e1" })), "not on a tile yet");
            h.attach().await;
            assert!(click.credentialed(h.session, &json!({ "ref": "e1" })));
            assert!(h.tool("browser_open").credentialed(h.session, &json!({ "url": "https://b.test/" })));
            assert!(h.tool("browser_eval").credentialed(h.session, &json!({ "expression": "1" })));
            assert!(!h.tool("browser_read").credentialed(h.session, &json!({})));

            let call = h.call("browser_click", json!({ "ref": "e9" }));
            let r = h.requested().await;
            assert_eq!(r.args["arguments"], json!({ "cmd": "click", "params": { "ref": "e9" } }));
            assert!(h.answer(r.id, &claimed("ipad")).await);
            assert!(
                h.answer(r.id, &done(json!({ "error": "stale ref e9: that element is gone from the page; re-run browser_elements" })))
                    .await
            );
            let out = call.await.unwrap();
            assert!(out.is_error);
            assert!(out.content.starts_with(PREFIX), "{}", out.content);
            assert!(out.content.contains("stale ref e9"), "{}", out.content);
            h.resolved().await;

            let call = h.call("browser_eval", json!({ "expression": "document.title" }));
            let r = h.requested().await;
            assert_eq!(r.args["world"], "page");
            assert!(h.answer(r.id, &claimed("ipad")).await);
            assert!(
                h.answer(r.id, &WebViewCallAnswer::Failed { reason: "ReferenceError: x".into() })
                    .await
            );
            let out = call.await.unwrap();
            assert!(out.is_error && out.content.ends_with("ReferenceError: x"), "{}", out.content);
        }

        /// No claim within 2 s: `no client holds tile web:<id>`, the call resolved, a late
        /// claim refused; a claim with no answer runs out after 30 s.
        #[tokio::test(start_paused = true)]
        async fn no_claim_and_no_answer_time_out() {
            let mut h = Harness::new().await;
            let call = h.call("browser_attach", json!({ "view": VIEW }));
            let r = h.requested().await;
            let out = call.await.unwrap();
            assert!(out.is_error);
            assert_eq!(
                out.content,
                "no client holds tile web:0123456789abcdef (the user may have closed it); browser_attach {} returns to your own browser"
            );
            assert_eq!(h.resolved().await, r.id);
            assert!(!h.answer(r.id, &claimed("ipad")).await, "late claim");

            h.attach().await;
            let call = h.call("browser_elements", json!({}));
            let r = h.requested().await;
            assert!(h.answer(r.id, &claimed("ipad")).await);
            // Past the 2 s claim mark: a claimed call waits for its answer.
            tokio::time::sleep(Duration::from_secs(5)).await;
            assert!(!call.is_finished());
            let out = call.await.unwrap();
            assert!(out.is_error);
            assert!(out.content.contains("did not finish `script` within 30 s"), "{}", out.content);
            assert!(!h.answer(r.id, &done(json!({}))).await, "dead after the timeout");
        }

        /// `browser_screenshot` on a tile: op `snapshot`, the PNG saved and attached like
        /// obscura's.
        #[tokio::test(start_paused = true)]
        async fn screenshot_is_a_snapshot_saved_as_a_file() {
            let mut h = Harness::new().await;
            h.attach().await;
            let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
            png.extend_from_slice(&13u32.to_be_bytes());
            png.extend_from_slice(b"IHDR");
            png.extend_from_slice(&390u32.to_be_bytes());
            png.extend_from_slice(&844u32.to_be_bytes());
            let call = h.call("browser_screenshot", json!({}));
            let r = h.requested().await;
            assert_eq!((r.op, &r.args), (WebViewOp::Snapshot, &json!({})));
            assert!(h.answer(r.id, &claimed("ipad")).await);
            let data = base64::engine::general_purpose::STANDARD.encode(&png);
            assert!(
                h.answer(
                    r.id,
                    &WebViewCallAnswer::Done { result: json!({ "width": 390, "height": 844 }), png: Some(data) }
                )
                .await
            );
            let out = call.await.unwrap();
            assert!(!out.is_error, "{}", out.content);
            assert_eq!(out.files.len(), 1);
            assert_eq!(std::fs::read(&out.files[0]).unwrap(), png);
            assert!(out.content.starts_with(&format!("{PREFIX}\nscreenshot 390\u{d7}844 saved to")), "{}", out.content);
        }

        /// Owner connections only.
        #[tokio::test(start_paused = true)]
        async fn a_guest_cannot_answer() {
            let mut h = Harness::new().await;
            let (guest, _g) = rpc_core::RpcPeer::open_guest([1; 32], [2; 32]);
            h.methods.extensions_mut().insert(guest);
            let call = h.call("browser_attach", json!({ "view": VIEW }));
            let r = h.requested().await;
            let v = h.answer_raw(r.id, &claimed("guest")).await;
            assert_eq!(v["error"]["code"], UNAUTHORIZED, "{v}");
            assert!(call.await.unwrap().is_error, "nobody claimed");
        }

        /// The same RPC dispatch on another owner connection (another window or device).
        fn other_connection(h: &Harness) -> (jsonrpsee::Methods, rpc_core::RpcPeerGuard) {
            let mut methods = h.methods.clone();
            let (peer, guard) = rpc_core::RpcPeer::open();
            methods.extensions_mut().insert(peer);
            (methods, guard)
        }

        async fn answer_on(
            methods: &jsonrpsee::Methods,
            id: WebViewCallId,
            answer: &WebViewCallAnswer,
            session: Option<SessionId>,
        ) -> bool {
            methods
                .call("answer_web_view_call", (id, answer, session))
                .await
                .unwrap()
        }

        /// 209 §4.4: only the connection that claimed a call resolves it. Another owner
        /// connection's `Done` (a page text the model would act on) and `Failed` are refused,
        /// before and after the claim; the claimer's answer, with the request's `session`, wins.
        #[tokio::test(start_paused = true)]
        async fn only_the_claiming_connection_answers() {
            let mut h = Harness::new().await;
            h.attach().await;
            let (other, _g) = other_connection(&h);
            let call = h.call("browser_read", json!({}));
            let r = h.requested().await;
            let forged = done(json!({ "url": "https://a.test/", "title": "A", "text": "FORGED" }));
            assert!(!answer_on(&other, r.id, &forged, Some(r.session)).await, "unclaimed Done");
            assert!(h.answer(r.id, &claimed("ipad")).await);
            assert!(!answer_on(&other, r.id, &forged, Some(r.session)).await, "not the claimer");
            let fail = WebViewCallAnswer::Failed {
                reason: "stopped by the user".into(),
            };
            assert!(!answer_on(&other, r.id, &fail, Some(r.session)).await, "not the claimer");
            let real = done(json!({ "url": "https://a.test/", "title": "A", "text": "REAL" }));
            assert!(answer_on(&h.methods, r.id, &real, Some(r.session)).await);
            let out = call.await.unwrap();
            assert!(out.content.ends_with("REAL") && !out.content.contains("FORGED"), "{}", out.content);
        }

        /// The user's Stop before any claim fails the call from any owner connection, and the
        /// tile then refuses the agent for 60 s without asking a client.
        #[tokio::test(start_paused = true)]
        async fn stop_holds_the_tile() {
            let mut h = Harness::new().await;
            // Its own tile: the hold is per tile and process-wide.
            let view = "5709000000000001";
            artifact::webview::record_state(WebViewState {
                session: h.session,
                view: view.into(),
                url: "https://s.test/".into(),
                title: "S".into(),
                loading: false,
                closed: false,
            });
            let call = h.call("browser_attach", json!({ "view": view }));
            h.serve(json!({ "url": "https://s.test/", "title": "S" })).await;
            assert!(!call.await.unwrap().is_error);
            let (other, _g) = other_connection(&h);
            let click = h.call("browser_click", json!({ "ref": "e1" }));
            let r = h.requested().await;
            let stop = WebViewCallAnswer::Failed {
                reason: "stopped by the user".into(),
            };
            assert!(answer_on(&other, r.id, &stop, Some(r.session)).await);
            let out = click.await.unwrap();
            assert!(out.is_error);
            assert!(out.content.contains("the user stopped the agent on this page"), "{}", out.content);
            h.resolved().await;
            let out = h.call("browser_read", json!({})).await.unwrap();
            assert!(out.content.contains("the user stopped the agent"), "{}", out.content);
            assert!(h.events.try_recv().is_err(), "a stopped tile asks no client");
            tokio::time::sleep(Duration::from_secs(61)).await;
            let read = h.call("browser_read", json!({}));
            let r = h.requested().await;
            assert_eq!(r.view, view, "asked again after the hold");
            assert!(h.answer(r.id, &claimed("ipad")).await);
            assert!(h.answer(r.id, &done(json!({ "url": "https://s.test/", "title": "S", "text": "back" }))).await);
            assert!(!read.await.unwrap().is_error);
        }

        /// 209 §9: a session outside the opener's tree cannot attach; nothing is asked.
        #[tokio::test(start_paused = true)]
        async fn attach_is_refused_outside_the_openers_tree() {
            let mut h = Harness::new().await;
            h.session = SessionId::new();
            let out = h.call("browser_attach", json!({ "view": format!("web:{VIEW}") })).await.unwrap();
            assert!(out.is_error);
            assert_eq!(out.content, "tile web:0123456789abcdef was not opened by this session or its tree");
            assert!(h.events.try_recv().is_err());
        }

        /// 209 §4.4: a tile the user closed is forgotten; attaching to it is refused.
        #[tokio::test(start_paused = true)]
        async fn a_closed_tile_cannot_be_attached() {
            let h = Harness::new().await;
            let view = "c105ed0000000001";
            let state = |closed| WebViewState {
                session: h.session,
                view: view.into(),
                url: "https://c.test/".into(),
                title: "C".into(),
                loading: false,
                closed,
            };
            artifact::webview::record_state(state(false));
            artifact::webview::record_state(state(true));
            let out = h.call("browser_attach", json!({ "view": view })).await.unwrap();
            assert!(out.content.contains("was not opened by this session"), "{}", out.content);
        }
    }

    /// 164 D3: the fence snapshot round trip against the real `answer_fence_snapshot`
    /// dispatch, and a full-size answer over a real authenticated WebSocket.
    mod fence_snapshot {
        use super::*;

        use artifact::snapshot::{
            Appearance, Client, FenceLang, FenceSnapshotAnswer, FenceSnapshotId,
            FenceSnapshotRequested, FenceSnapshotResolved, SnapshotOutcome,
        };
        use kameo::actor::Spawn;
        use kameo::message::Context;
        use tokio::sync::mpsc;

        /// Bus subscriber: `(resolved, id)` of every request and resolution, in order.
        #[derive(kameo::Actor)]
        struct Capture(mpsc::UnboundedSender<(bool, FenceSnapshotId)>);

        impl Message<FenceSnapshotRequested> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: FenceSnapshotRequested, _: &mut Context<Self, ()>) {
                let _ = self.0.send((false, m.id));
            }
        }

        impl Message<FenceSnapshotResolved> for Capture {
            type Reply = ();
            async fn handle(&mut self, m: FenceSnapshotResolved, _: &mut Context<Self, ()>) {
                let _ = self.0.send((true, m.id));
            }
        }

        /// A runtime with a capture on its bus and a live event feed (no subscriber at all
        /// short-circuits to "no renderer").
        async fn harness() -> (Runtime, mpsc::UnboundedReceiver<(bool, FenceSnapshotId)>) {
            artifact::preview::watch_feed(Arc::new(|| 1));
            let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
            let (tx, events) = mpsc::unbounded_channel();
            let cap = Capture::spawn(Capture(tx));
            rt.subscribe::<Capture, FenceSnapshotRequested>(&cap).await;
            rt.subscribe::<Capture, FenceSnapshotResolved>(&cap).await;
            (rt, events)
        }

        fn render(rt: &Runtime, source: &str) -> tokio::task::JoinHandle<SnapshotOutcome> {
            let (rt, source) = (rt.clone(), source.to_owned());
            tokio::spawn(async move {
                artifact::snapshot::render(
                    &rt,
                    FenceLang::JsonRender,
                    &source,
                    560,
                    Appearance::Light,
                    2,
                )
                .await
            })
        }

        /// A PNG header `w`×`h` padded with zeros to `len` bytes.
        fn png(w: u32, h: u32, len: usize) -> Vec<u8> {
            let mut b = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
            b.extend(b"IHDR");
            b.extend(w.to_be_bytes());
            b.extend(h.to_be_bytes());
            b.resize(len.max(24), 0);
            b
        }

        fn done(bytes: &[u8]) -> FenceSnapshotAnswer {
            FenceSnapshotAnswer::Done {
                png: base64::engine::general_purpose::STANDARD.encode(bytes),
                width: 1,
                height: 1,
                client: Client::Desktop,
            }
        }

        /// The raw `answer_fence_snapshot` result.
        async fn answer(
            methods: &jsonrpsee::Methods,
            id: FenceSnapshotId,
            answer: &FenceSnapshotAnswer,
        ) -> bool {
            let req = format!(
                r#"{{"jsonrpc":"2.0","method":"answer_fence_snapshot","params":[{},{}],"id":1}}"#,
                serde_json::to_string(&id).unwrap(),
                serde_json::to_string(answer).unwrap()
            );
            let (resp, _) = methods.raw_json_request(&req, 1).await.unwrap();
            let v: serde_json::Value = serde_json::from_str(resp.get()).unwrap();
            v["result"].as_bool().unwrap_or_else(|| panic!("{v}"))
        }

        #[tokio::test(start_paused = true)]
        async fn first_done_wins_and_late_answers_are_refused() {
            let (rt, mut events) = harness().await;
            let methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt.clone())).into();
            let call = render(&rt, "{\"a\":1}");
            let (resolved, id) = events.recv().await.unwrap();
            assert!(!resolved);
            let started = FenceSnapshotAnswer::Started {
                client: Client::Web,
            };
            assert!(answer(&methods, id, &started).await);
            let pic = png(1120, 600, 64);
            assert!(answer(&methods, id, &done(&pic)).await);
            assert!(
                !answer(&methods, id, &done(&pic)).await,
                "second Done is late"
            );
            assert!(
                !answer(&methods, id, &started).await,
                "Started after Done is late"
            );
            assert_eq!(
                call.await.unwrap(),
                SnapshotOutcome::Done {
                    png: pic,
                    width: 1120,
                    height: 600,
                    client: Client::Desktop
                }
            );
            assert_eq!(events.recv().await.unwrap(), (true, id));
        }

        /// The largest PNG a client sends (4 MiB → ~5.6 MB of base64, ~6 MB with the
        /// envelope) fits the server's request limit (jsonrpsee's default 10 MiB) over a
        /// real authenticated WebSocket.
        #[tokio::test]
        async fn a_four_mib_png_passes_the_websocket() {
            use jsonrpsee::core::client::ClientT;
            use jsonrpsee::rpc_params;
            use jsonrpsee::ws_client::{HeaderMap, HeaderValue, WsClientBuilder};

            let (rt, mut events) = harness().await;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let methods: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt.clone())).into();
            tokio::spawn(rpc::accept(
                listener,
                methods,
                "t".into(),
                std::future::pending(),
            ));
            let mut headers = HeaderMap::new();
            headers.insert("authorization", HeaderValue::from_static("Bearer t"));
            let client = WsClientBuilder::default()
                .set_headers(headers)
                .build(format!("ws://{addr}"))
                .await
                .unwrap();

            let call = render(&rt, "{\"big\":true}");
            let (_, id) = events.recv().await.unwrap();
            let pic = png(4000, 8000, artifact::snapshot::MAX_PNG);
            let answer = done(&pic);
            let body = serde_json::to_string(&answer).unwrap().len();
            assert!(body > 5_500_000, "{body}");
            let ok: bool = client
                .request("answer_fence_snapshot", rpc_params![id, answer])
                .await
                .unwrap();
            assert!(ok);
            match call.await.unwrap() {
                SnapshotOutcome::Done { png, height, .. } => {
                    assert_eq!((png.len(), height), (artifact::snapshot::MAX_PNG, 8000));
                }
                other => panic!("{other:?}"),
            }
        }

        /// A 1×1 RGBA PNG, standard base64.
        const DOT: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNkYPhfDwAChwGA60e6kgAAAABJRU5ErkJggg==";

        /// Bus subscriber handing every `FenceSnapshotRequested` over whole.
        #[derive(kameo::Actor)]
        struct Requests(mpsc::UnboundedSender<FenceSnapshotRequested>);

        impl Message<FenceSnapshotRequested> for Requests {
            type Reply = ();
            async fn handle(&mut self, m: FenceSnapshotRequested, _: &mut Context<Self, ()>) {
                let _ = self.0.send(m);
            }
        }

        /// `rt`'s methods behind `peer`.
        fn methods(rt: &Runtime, peer: rpc_core::RpcPeer) -> jsonrpsee::Methods {
            let mut m: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt.clone())).into();
            m.extensions_mut().insert(peer);
            m
        }

        /// A raw `fence_snapshot` call with positional `params`, in its own task: the whole
        /// JSON-RPC response.
        fn snapshot(
            m: &jsonrpsee::Methods,
            params: &str,
        ) -> tokio::task::JoinHandle<serde_json::Value> {
            let m = m.clone();
            let req = format!(
                r#"{{"jsonrpc":"2.0","method":"fence_snapshot","params":{params},"id":1}}"#
            );
            tokio::spawn(async move {
                let (resp, _) = m.raw_json_request(&req, 1).await.unwrap();
                serde_json::from_str(resp.get()).unwrap()
            })
        }

        fn failed(reason: &str) -> FenceSnapshotAnswer {
            FenceSnapshotAnswer::Failed {
                reason: reason.into(),
                client: Client::Desktop,
            }
        }

        /// 192: a client draws it → `done` with the PNG as standard base64 and its header size.
        #[tokio::test(start_paused = true)]
        async fn the_rpc_returns_the_drawn_png() {
            let (rt, mut events) = harness().await;
            let (peer, _guard) = rpc_core::RpcPeer::open();
            let m = methods(&rt, peer);
            let call = snapshot(&m, r#"["mermaid","graph TD; A-->B",560,"dark",2]"#);
            let (_, id) = events.recv().await.unwrap();
            let started = FenceSnapshotAnswer::Started {
                client: Client::Desktop,
            };
            assert!(answer(&m, id, &started).await);
            let drawn = FenceSnapshotAnswer::Done {
                png: DOT.into(),
                width: 1,
                height: 1,
                client: Client::Desktop,
            };
            assert!(answer(&m, id, &drawn).await);
            let v = call.await.unwrap();
            assert_eq!(
                v["result"],
                serde_json::json!({"outcome":"done","png":DOT,"width":1,"height":1,"error":null}),
                "{v}"
            );
        }

        /// 192: nobody on the bus to draw it → `no_renderer`, every other field null.
        #[tokio::test(start_paused = true)]
        async fn no_subscriber_is_no_renderer() {
            let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
            let (peer, _guard) = rpc_core::RpcPeer::open();
            let v = snapshot(&methods(&rt, peer), r#"["openui","x",560,"light",2]"#)
                .await
                .unwrap();
            assert_eq!(
                v["result"],
                serde_json::json!({"outcome":"no_renderer","png":null,"width":null,"height":null,"error":null}),
                "{v}"
            );
        }

        /// 192: a client that cannot draw it → `failed` with its reason.
        #[tokio::test(start_paused = true)]
        async fn a_failed_client_is_failed_with_its_reason() {
            let (rt, mut events) = harness().await;
            let (peer, _guard) = rpc_core::RpcPeer::open();
            let m = methods(&rt, peer);
            let call = snapshot(&m, r#"["json-render","{",560,"light",2]"#);
            let (_, id) = events.recv().await.unwrap();
            assert!(answer(&m, id, &failed("bad json")).await);
            let v = call.await.unwrap();
            assert_eq!(
                v["result"],
                serde_json::json!({"outcome":"failed","png":null,"width":null,"height":null,"error":"bad json"}),
                "{v}"
            );
        }

        /// 192: `width` is clamped to 240..=1200 and `scale` to 1..=3 before the request goes out.
        #[tokio::test(start_paused = true)]
        async fn width_and_scale_are_clamped() {
            let (rt, _events) = harness().await;
            let (tx, mut seen) = mpsc::unbounded_channel();
            let req = Requests::spawn(Requests(tx));
            rt.subscribe::<Requests, FenceSnapshotRequested>(&req).await;
            let (peer, _guard) = rpc_core::RpcPeer::open();
            let m = methods(&rt, peer);
            for (params, want) in [
                (r#"["mermaid","a",10,"dark",9]"#, (240, 3)),
                (r#"["mermaid","b",5000,"dark",0]"#, (1200, 1)),
            ] {
                let call = snapshot(&m, params);
                let r = seen.recv().await.unwrap();
                assert_eq!((r.width, r.scale), want, "{params}");
                assert!(answer(&m, r.id, &failed("x")).await);
                assert_eq!(call.await.unwrap()["result"]["outcome"], "failed");
            }
        }

        /// 192: a mode-2 document guest (and an unauthenticated call) is refused before any
        /// request goes out.
        #[tokio::test(start_paused = true)]
        async fn guests_are_refused() {
            let (rt, mut events) = harness().await;
            let (guest, _guard) = rpc_core::RpcPeer::open_guest([7; 32], [9; 32]);
            let params = r#"["mermaid","a",560,"dark",2]"#;
            let v = snapshot(&methods(&rt, guest), params).await.unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(i64::from(UNAUTHORIZED)),
                "{v}"
            );
            let bare: jsonrpsee::Methods = test_rpc_module(crate::new_server(rt.clone())).into();
            let v = snapshot(&bare, params).await.unwrap();
            assert_eq!(
                v["error"]["code"].as_i64(),
                Some(i64::from(UNAUTHORIZED)),
                "{v}"
            );
            assert!(events.try_recv().is_err(), "no request published");
        }
    }

    fn link_status(enabled: bool, harness: Option<&str>, mesh: Option<&str>) -> LinkStatus {
        LinkStatus {
            enabled,
            relay: "wss://relay.example/".into(),
            name: "mac".into(),
            harness_id: harness.map(Into::into),
            connected: enabled,
            devices: vec![],
            pairing: None,
            mesh: mesh.map(|m| link::MeshLink {
                device_id: m.into(),
                connected: false,
            }),
        }
    }

    /// 184 §7.1, §7.2, W-j: v2 only from the home in a mesh; everywhere else today's v1 —
    /// including a lone engine with a paired phone (`1.<mesh device id>`); Link off in a mesh is
    /// the v1 mesh link, whose id Link never holds there.
    #[test]
    fn share_links_are_v2_on_the_home_only_and_v1_as_today_elsewhere() {
        use document::LinkVia::{Home, Mesh};
        let ids = |s: &LinkStatus, m: Option<&str>, here| link_ids(s, m, here).unwrap();
        let phone = link_status(true, Some("H"), Some("M"));
        assert_eq!(
            ids(&phone, None, false),
            ("1.M".into(), Mesh),
            "lone engine + phone"
        );
        let alone = link_status(true, Some("H"), None);
        assert_eq!(
            ids(&alone, None, false),
            ("1.H".into(), Home),
            "before any device"
        );
        assert_eq!(
            ids(&phone, Some("M"), true),
            ("2.H.M".into(), Home),
            "the home"
        );
        let follower = link_status(true, Some("H"), None);
        assert_eq!(
            ids(&follower, Some("M"), true),
            ("2.H.M".into(), Home),
            "MeshLinkId read"
        );
        assert_eq!(
            ids(&phone, Some("M"), false),
            ("1.M".into(), Mesh),
            "not the home"
        );
        let off = link_status(false, Some("H"), None);
        assert_eq!(
            ids(&off, Some("M"), true),
            ("1.M".into(), Mesh),
            "Link off: a mesh link"
        );
        assert_eq!(
            link_ids(&off, None, true).unwrap_err().message(),
            link::MESH_LINK_UNAVAILABLE
        );
        assert_eq!(
            link_ids(&link_status(true, None, None), None, false)
                .unwrap_err()
                .message(),
            "link has no identity yet"
        );
        assert_eq!(link_origin(&phone.relay), "https://relay.example");
        assert_eq!(link_origin("ws://127.0.0.1:9/"), "http://127.0.0.1:9");
    }

    /// 184 §10.4: a `busy` join (a hand-back in flight) is retried by the page and never counts
    /// toward the guest's three refusals; any other refusal does.
    #[test]
    fn a_busy_join_is_not_a_refusal() {
        let (peer, _guard) = rpc_core::RpcPeer::open_guest([7; 32], [9; 32]);
        let guest = peer.guest().unwrap();
        // Z184-R2: a move in flight or a transient failure never counts toward the close.
        for e in [
            document::share::BUSY,
            "hub_moved",
            "server_moved",
            "NotHub:abc",
        ] {
            join_outcome::<()>(guest, &Err(failed(e)));
        }
        assert_eq!(guest.refusals(), 0);
        join_outcome::<()>(guest, &Err(failed("bad_proof")));
        join_outcome::<()>(guest, &Err(failed("not_member")));
        assert_eq!(guest.refusals(), 2);
        // Z184-R3: the join's own refusals count too.
        join_outcome::<()>(guest, &Err(failed("link_revoked")));
        join_outcome::<()>(guest, &Err(failed("member_revoked")));
        assert_eq!(guest.refusals(), 4);
    }

    /// 184 §12.4: Move to's refusals name the machine, or an unknown id's first 8 characters.
    #[test]
    fn move_refusals_name_the_machine() {
        let id = "0123456789abcdef0123";
        assert_eq!(
            named(&format!("{id} is offline"), id, Some("xeus")),
            "xeus is offline"
        );
        assert_eq!(
            named(&format!("already lives on {id}"), id, None),
            "already lives on 01234567"
        );
        assert_eq!(
            named(
                &format!("moved to {id}; it has not taken the note yet"),
                id,
                Some("C")
            ),
            "moved to C; it has not taken the note yet"
        );
    }
