use crate as agent;
use std::path::PathBuf;
use agent::context::save_blob;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use runtime::SessionId;
use rpc_core::{failed, Server};
use agent::rpc_attachments::decode_attachment;
use agent::rpc_support::RpcServerExt;

/// Artifacts RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait AgentArtifactsApi {
    /// Store base64 `bytes` under `session`'s tree (51 §8.1): the durable
    /// attachments dir, or the private scratch root — ownership is resolved
    /// before any byte is written. Same extension / size rules as
    /// `put_attachment` (`file_ext` ∈ png, jpg, …).
    #[method(name = "put_session_attachment", with_extensions)]
    async fn put_session_attachment(
        &self,
        session: SessionId,
        bytes: String,
        file_ext: String,
    ) -> RpcResult<PathBuf>;
    /// Store base64 `bytes` as `<data_dir>/attachments/<sha256[..16]>.<ext>`
    /// and return the path, for `send_input { files }` or `transcribe`. `ext`
    /// ∈ png, jpg, jpeg, gif, webp, pdf, m4a, mp4, webm, ogg, opus, mp3, wav;
    /// images ≤ 5 MB, PDFs ≤ 32 MB, audio ≤ 16 MB decoded.
    #[method(name = "put_attachment")]
    async fn put_attachment(&self, bytes: String, ext: String) -> RpcResult<PathBuf>;
}

#[async_trait]
impl AgentArtifactsApiServer for Server {

    async fn put_session_attachment(
        &self,
        ext: &Extensions,
        session: SessionId,
        bytes: String,
        file_ext: String,
    ) -> RpcResult<PathBuf> {
        // Ownership first: nothing is decoded or written for a session the
        // caller may not touch (51 §8.1).
        let authorized = self.authorize(ext, session).await?;
        let bytes = decode_attachment(bytes, &file_ext)?;
        save_blob(&bytes, &file_ext, &authorized.resources.root).map_err(|e| failed(e.to_string()))
    }

    async fn put_attachment(&self, bytes: String, ext: String) -> RpcResult<PathBuf> {
        let bytes = decode_attachment(bytes, &ext)?;
        save_blob(&bytes, &ext, &self.rt.data_dir).map_err(|e| failed(e.to_string()))
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(AgentArtifactsApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}
