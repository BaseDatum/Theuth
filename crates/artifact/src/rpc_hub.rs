use std::path::PathBuf;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{send, failed, owner, Server};
use artifact::rpc_support::artifact;
use document::rpc_support::{doc_send, documents, member_outcome, principal};

#[rpc(server, client)]
pub trait ArtifactHubApi {
    /// Export `doc` (72): `markdown` = `doc_markdown`'s text, `pdf` = the print lowering through Typst
    /// with `assets` (client-rendered diagram SVGs by block id) as figures. `dest` (owner only, under
    /// the home directory) is written owner-only and atomically; `None` stores an artifact instead.
    #[method(name = "doc_export", with_extensions)]
    async fn doc_export(
        &self,
        doc: document::DocId,
        format: artifact::ExportFormat,
        dest: Option<PathBuf>,
        assets: Vec<artifact::ExportAsset>,
    ) -> RpcResult<artifact::ExportResult>;
}

#[async_trait]
impl ArtifactHubApiServer for Server {

    async fn doc_export(
        &self,
        ext: &Extensions,
        doc: document::DocId,
        format: artifact::ExportFormat,
        dest: Option<PathBuf>,
        assets: Vec<artifact::ExportAsset>,
    ) -> RpcResult<artifact::ExportResult> {
        let (peer, by) = principal(ext)?;
        if let Some(dest) = &dest {
            owner(ext)?;
            let home = dirs::home_dir().ok_or_else(|| failed("no home directory"))?;
            let inside = dest.is_absolute()
                && dest.starts_with(&home)
                && !dest
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir));
            if !inside {
                return Err(failed(format!(
                    "{}: not under {}",
                    dest.display(),
                    home.display()
                )));
            }
        }
        let docs = documents()?;
        let meta = doc_send(
            &docs,
            document::MetaOf {
                doc: doc.clone(),
                by,
            },
        )
        .await;
        member_outcome(&peer, &meta);
        let meta = meta?;
        let annotate = format == artifact::ExportFormat::Pdf;
        // 171 §4.4: the user's export carries each cell's last output and run line.
        let markdown = document::reader_markdown(&docs, doc, None, annotate)
            .await
            .map_err(|e| failed(e.to_string()))?;
        send(
            &artifact()?,
            artifact::ExportDoc {
                title: meta.title,
                format,
                markdown,
                dest,
                assets,
                record: None,
            },
        )
        .await
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    ArtifactHubApiServer::into_rpc(server)
}
