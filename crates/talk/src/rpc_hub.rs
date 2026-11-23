use crate as talk;
use agent::{Agents, GetPolishModel};
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{ask, send, Server};
use talk::rpc_support::dictation;

/// Hub RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "talk", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait TalkHubApi {
    /// 153 F1: the live user-facing `voice.*` settings — what `set_shape`, `set_end_phrases`,
    /// `set_input_devices` and `set_polish_model` left behind, plus the config-only keys.
    #[method(name = "voice_settings")]
    async fn voice_settings(&self) -> RpcResult<voice::VoiceSettings>;
    /// `voice.stt_engine` in effect and the MLX engine (202 §4, §5): `available` (with the reason
    /// in `state` when not: not Apple silicon, no `mlx.metallib`) and absent | downloading
    /// (bytes) | ready | failed. Poll it for download progress.
    #[method(name = "voice_mlx_status")]
    async fn voice_mlx_status(&self) -> RpcResult<voice::mlx::MlxStatus>;
    /// The explicit MLX download (202 §4): the ~2.5 GB weights from the hub.
    /// Returns at once with the state; progress via `voice_mlx_status`.
    #[method(name = "voice_mlx_download")]
    async fn voice_mlx_download(&self) -> RpcResult<voice::mlx::MlxState>;
}

#[async_trait]
impl TalkHubApiServer for Server {

    async fn voice_settings(&self) -> RpcResult<voice::VoiceSettings> {
        let mut settings = send(&dictation()?, voice::CurrentSettings).await?;
        // The live slot (`set_polish_model`), not the one read at startup.
        settings.polish_model = ask::<Agents, _>(agent::AGENT_ACTOR, GetPolishModel)
            .await?
            .map(|m| m.id);
        settings.polish_enabled =
            ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetPolishEnabled).await?;
        Ok(settings)
    }

    async fn voice_mlx_status(&self) -> RpcResult<voice::mlx::MlxStatus> {
        let data_dir = self.rt.data_dir.clone();
        tokio::task::spawn_blocking(move || voice::mlx::status(&data_dir))
            .await
            .map_err(|e| rpc_core::failed(e.to_string()))
    }

    async fn voice_mlx_download(&self) -> RpcResult<voice::mlx::MlxState> {
        let data_dir = self.rt.data_dir.clone();
        let state = voice::mlx::state(&data_dir);
        if matches!(
            state,
            voice::mlx::MlxState::Unavailable { .. }
                | voice::mlx::MlxState::Ready
                | voice::mlx::MlxState::Downloading { .. }
        ) {
            return Ok(state);
        }
        tokio::spawn(async move {
            let _ = voice::mlx::install(&data_dir).await;
        });
        Ok(voice::mlx::MlxState::Downloading {
            done_bytes: 0,
            total_bytes: 0,
        })
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(TalkHubApiServer::into_rpc(server), rpc_descriptors())
        .expect("talk RPC descriptors")
}
