use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{failed, Server};
use super::s7_composition_support::{ConfigFlag, ConfigFlagKey};

#[rpc(server, client)]
pub trait RpcComposeSettingsApi {
    /// 188: one [`ConfigFlagKey`] (`decision.enabled`, `a2a.enabled`, `computer.enabled`,
    /// `double.enabled`, `web.allow_private_network`, `browser.allow_private_network`) as
    /// config.yaml holds it and as this engine started with it. Node-level, never
    /// forwarded; any other key is `INVALID_PARAMS` naming them.
    #[method(name = "config_flag_get")]
    fn config_flag_get(&self, key: ConfigFlagKey) -> RpcResult<ConfigFlag>;
    /// 188: write only that one key (`<section>.enabled` or
    /// `<section>.allow_private_network`) into config.yaml (every other line,
    /// comments included, untouched). Nothing changes live: the plugin keeps its boot
    /// state until theuth restarts, so the reply's `restart_required` says whether one is due.
    #[method(name = "config_flag_set")]
    fn config_flag_set(&self, key: ConfigFlagKey, on: bool) -> RpcResult<ConfigFlag>;
}

#[async_trait]
impl RpcComposeSettingsApiServer for Server {

    fn config_flag_get(&self, key: ConfigFlagKey) -> RpcResult<ConfigFlag> {
        Ok(self.config_flag(key))
    }

    fn config_flag_set(&self, key: ConfigFlagKey, on: bool) -> RpcResult<ConfigFlag> {
        let path = self.rt.data_dir.join("config.yaml");
        let (section, field) = key.path();
        runtime::write_config_value(&path, section, field, if on { "true" } else { "false" })
            .map_err(|e| failed(format!("config.yaml: {e}")))?;
        Ok(self.config_flag(key))
    }
}

pub(super) trait SettingsConfigExt {
    fn config_flag(&self, key: ConfigFlagKey) -> ConfigFlag;
}

impl SettingsConfigExt for Server {
    /// 188: `key` in config.yaml now (read fresh) and in the boot config (never re-read).
    fn config_flag(&self, key: ConfigFlagKey) -> ConfigFlag {
        use figment::providers::{Format, Yaml};
        let file = figment::Figment::from(Yaml::file_exact(self.rt.data_dir.join("config.yaml")));
        let (enabled, running) = (key.read(&file), key.read(&self.rt.config));
        ConfigFlag {
            key,
            enabled,
            running,
            restart_required: enabled != running,
        }
    }

}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    RpcComposeSettingsApiServer::into_rpc(server)
}
