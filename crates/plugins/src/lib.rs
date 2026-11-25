//! External plugins (design doc 216): a folder with `plugin.yaml` and a `run:` command in any
//! language, spawned as a child process speaking JSON-RPC 2.0 over stdin/stdout.
mod actor;      // B1: PluginsActor and the Message impls (§5.2)
mod attention;  // B1: roster::attention::Source — install / widen / code-changed cards, notify items
mod broker;     // B1: plugin → host requests: call (headless session), input, notify, catalog, ping
mod catalog;    // B1: dynamic catalog — exposed wire topics split per `type` + plugin events
mod grants;     // B1: grants.yaml read/write, HMAC-SHA256 under the vault row plugins/grants-key
mod host;       // B1: boot (orphan reap, scan, start), per-plugin supervisors, lifecycle state machine
mod logs;       // B1: per-plugin redacted log files, tail
mod proc;       // B1: spawn under ProcessTree, two-lane writer, stdout reader, backoff, .pgids reaper
mod ptools;     // B1: manifest tools → tools::DynTool (ToolSource::Plugin)
mod route;      // B1: wire feed → external_view → grants → session filters → redaction → event lane
mod ui;         // B1: placement surfaces, ui validation (app::helper, surface::parse), rate limits
mod watch;      // B1: notify watcher on plugins/, 300 ms debounce, widening detection
mod atools;     // B2: agent-facing plugin_* tools (§7.1)
mod rpc;        // B2: PluginsApi (§6)
pub mod config; // B0, frozen
pub mod error;  // B0, frozen
pub mod events; // B0, frozen
pub mod manifest; // B1 — pub only for check_offline / check_name (§3.4)
pub mod messages; // B0, frozen
pub mod press;  // B2 (B0 writes it complete)
pub mod scaffold; // B2 — pub for new_plugin (§3.4)
pub mod types;  // B0, frozen

pub use actor::PluginsActor;
pub use config::PluginsConfig;
pub use error::PluginsError;
pub use events::{PluginEvent, PluginUi};
pub use messages::*;
pub use press::HumanPress;
pub use types::*;
pub use rpc::rpc_module;

/// Registered actor name.
pub const PLUGINS_ACTOR: &str = "plugins";
/// The wire protocol major this host speaks (design §6).
pub const PROTOCOL: u32 = 1;
/// Topic of [`PluginEvent`].
pub const EVENTS_TOPIC: &str = "plugins.events";
/// Topic of [`PluginUi`].
pub const UI_TOPIC: &str = "plugins.ui";

/// Declare both wire topics (always, even when disabled — the ABI must not depend on config).
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<PluginEvent>().await?;
    rt.register_wire_event::<PluginUi>().await
}

/// The native host of external plugins (216).
pub struct PluginsPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for PluginsPlugin {
    fn name(&self) -> &'static str {
        "plugins"
    }

    async fn declare_wire(&mut self, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await
    }

    async fn start(self: Box<Self>, rt: &runtime::Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg: PluginsConfig = rt.section("plugins").unwrap_or_else(|e| {
            tracing::debug!(error = %e, "plugins: bad section, using defaults");
            PluginsConfig::default()
        });
        if !cfg.enabled {
            tracing::info!("plugins: disabled (plugins.enabled: false)");
            return Ok(());
        }
        if cfg!(windows) {
            tracing::warn!("plugins: not supported on Windows in v1 (216 §9.5)");
            return Ok(());
        }
        let actor = actor::spawn(rt, cfg.clone())?; // registers PLUGINS_ACTOR
        tokio::spawn(atools::register(rt.clone())); // B2: agent tools (§7.1)
        tokio::spawn(host::boot(rt.clone(), actor, cfg)); // B1: reap, scan, start, watcher, attention Source
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wire::RuntimeWire as _;

    #[test]
    fn config_defaults_match_the_contract() {
        let want = PluginsConfig {
            enabled: true,
            dir: "plugins".into(),
            watch: true,
            dev: false,
            remote_sessions: false,
            restart_first_ms: 1000,
            restart_cap_ms: 300_000,
            crash_loop: 5,
            crash_window_s: 60,
            shutdown_grace_ms: 3000,
            max_line_bytes: 1_048_576,
            queue_frames: 2048,
            emit_per_s: 50,
            ui_per_s: 4,
            query_per_s: 10,
            input_per_min: 6,
            request_timeout_ms: 10_000,
            string_cap: 65_536,
            log_max_bytes: 10_485_760,
            log_days: 5,
        };
        assert_eq!(PluginsConfig::default(), want);
        let empty: PluginsConfig = serde_yaml::from_str("{}").unwrap();
        assert_eq!(empty, want);
    }

    #[tokio::test]
    async fn both_topics_declare() {
        let rt = runtime::Runtime::new(Default::default(), std::env::temp_dir());
        declare_wire(&rt).await.unwrap();
        // Repeats are idempotent: `start` declares again after `declare_wire`.
        declare_wire(&rt).await.unwrap();
        let topics: Vec<&str> = rt.wire_schema().iter().map(|d| d.topic).collect();
        assert!(topics.contains(&EVENTS_TOPIC), "{topics:?}");
        assert!(topics.contains(&UI_TOPIC), "{topics:?}");
    }

    #[test]
    fn wire_json_shapes() {
        let ev = PluginUi::PluginsChanged;
        assert_eq!(serde_json::to_value(&ev).unwrap(), serde_json::json!({"type": "PluginsChanged"}));
        let st = PluginUi::PluginStatus {
            plugin: "turns".into(),
            placement: "turns".into(),
            dot: Dot::Warn,
            badge: Some(Badge::Count(51)),
            label: Some("51/50".into()),
            tooltip: Some("Turns today: 51".into()),
        };
        assert_eq!(
            serde_json::to_value(&st).unwrap(),
            serde_json::json!({"type":"PluginStatus","plugin":"turns","placement":"turns","dot":"warn","badge":51,"label":"51/50","tooltip":"Turns today: 51"})
        );
        let pe = PluginEvent {
            plugin: "turns".into(),
            event: "limit_reached".into(),
            version: 1,
            session: None,
            data: serde_json::json!({"day": "2026-11-24", "turns": 50}),
        };
        assert_eq!(
            serde_json::to_value(&pe).unwrap(),
            serde_json::json!({"type":"PluginEvent","plugin":"turns","event":"limit_reached","version":1,"session":null,"data":{"day":"2026-11-24","turns":50}})
        );
        assert_eq!(
            serde_json::to_value(SurfaceFormat::JsonRender).unwrap(),
            serde_json::json!("json-render")
        );
    }
}
