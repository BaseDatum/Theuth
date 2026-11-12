use agent::{Agents, GetPolishModel};
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{ask, send, Server};
use talk::rpc_support::dictation;

#[rpc(server, client)]
pub trait TalkHubApi {
    /// 153 F1: the live user-facing `voice.*` settings — what `set_shape`, `set_end_phrases`,
    /// `set_input_devices` and `set_polish_model` left behind, plus the config-only keys.
    #[method(name = "voice_settings")]
    async fn voice_settings(&self) -> RpcResult<voice::VoiceSettings>;
}

#[async_trait]
impl TalkHubApiServer for Server {

    async fn voice_settings(&self) -> RpcResult<voice::VoiceSettings> {
        let mut settings = send(&dictation()?, voice::CurrentSettings).await?;
        // The live slot (`set_polish_model`), not the one read at startup.
        settings.polish_model = ask::<Agents, _>(agent::AGENT_ACTOR, GetPolishModel)
            .await?
            .map(|m| m.id);
        Ok(settings)
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    TalkHubApiServer::into_rpc(server)
}
