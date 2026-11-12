use std::path::{Path, PathBuf};
use agent::context::save_blob;
use app::{AppDoc, AppToolResult, CallArgs, CallStatusOf, CallTool, CloseApp, PublishFromUi};
use artifact::search::ArtifactMatchContext;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use runtime::SessionId;
use agent::rpc_forward as forward;
use rpc_core::{send, failed, peer, owner, ASK_FAILED, Server};
use super::{apps, artifact, decode_attachment};
use agent::rpc_support::RpcServerExt;
use base64::Engine as _;

#[rpc(server, client)]
pub trait ArtifactsApi {
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
    /// A client's answer to a `PagePreviewRequested` (120 §11.2): `Started`
    /// extends the wait, the first `Done`/`Failed` resolves it. `true` if the
    /// preview was still pending.
    #[method(name = "answer_page_preview")]
    async fn answer_page_preview(
        &self,
        id: artifact::preview::PreviewId,
        answer: artifact::preview::PagePreviewAnswer,
    ) -> RpcResult<bool>;
    /// A client's answer to a `FenceSnapshotRequested` (164 D3): `Started`
    /// extends the wait, the first `Done`/`Failed` resolves it. `true` while
    /// the request was pending.
    #[method(name = "answer_fence_snapshot")]
    async fn answer_fence_snapshot(
        &self,
        id: artifact::snapshot::FenceSnapshotId,
        answer: artifact::snapshot::FenceSnapshotAnswer,
    ) -> RpcResult<bool>;
    /// 192: draw one fence through the 164 renderer (a connected desktop/web client) and
    /// return the PNG — the native companions' pictures. `width` is clamped to 240..=1200
    /// CSS px, `scale` to 1..=3. Read-only; guest peers are refused.
    #[method(name = "fence_snapshot", with_extensions)]
    async fn fence_snapshot(
        &self,
        lang: artifact::snapshot::FenceLang,
        source: String,
        width: u32,
        appearance: artifact::snapshot::Appearance,
        scale: u32,
    ) -> RpcResult<artifact::snapshot::FenceSnapshotReply>;
    /// Store base64 `bytes` as `<data_dir>/attachments/<sha256[..16]>.<ext>`
    /// and return the path, for `send_input { files }` or `transcribe`. `ext`
    /// ∈ png, jpg, jpeg, gif, webp, pdf, m4a, mp4, webm, ogg, opus, mp3, wav;
    /// images ≤ 5 MB, PDFs ≤ 32 MB, audio ≤ 16 MB decoded.
    #[method(name = "put_attachment")]
    async fn put_attachment(&self, bytes: String, ext: String) -> RpcResult<PathBuf>;
    /// Read a file the harness produced or stored, base64-encoded. Only
    /// paths under `<root>/{attachments,browser,computer,artifacts}` (after
    /// canonicalization) are served, where `<root>` is `data_dir`, or with
    /// `session` the tree's own root — a private tree's files are served to
    /// its owner only, and never without naming the session (51 §4.2).
    #[method(name = "get_attachment", with_extensions)]
    async fn get_attachment(&self, path: PathBuf, session: Option<SessionId>) -> RpcResult<String>;
    /// Every artifact shown in `session` (`null` = every session), newest
    /// first; `descendants: true` (default `false`) widens to the session's
    /// subtree (35 §3.5).
    #[method(name = "list_artifacts", with_extensions)]
    async fn list_artifacts(
        &self,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<artifact::Artifact>>;
    /// Copy artifact `id`'s blob to `dest` (parent created; refuses to
    /// overwrite); returns the written path (35 §3.5). A private artifact
    /// needs its owning `session` (authorized, 51 §4.2).
    #[method(name = "export_artifact", with_extensions)]
    async fn export_artifact(
        &self,
        id: String,
        dest: PathBuf,
        session: Option<SessionId>,
    ) -> RpcResult<PathBuf>;
    /// Publish a reply fence's OpenUI source as an app artifact of `session` without a
    /// model turn (30 §9): validated and committed exactly like the `app` tool with
    /// `edit: false`, under a synthetic `ui-<uuid v7>` call id, no transcript entry. The
    /// error message is the tool's located diagnostics text. `session` is
    /// authorized: a private tree takes apps from its owner only.
    #[method(name = "publish_app", with_extensions)]
    async fn publish_app(
        &self,
        session: SessionId,
        name: String,
        source: String,
    ) -> RpcResult<AppDoc>;
    /// Run one pinned Query/Mutation binding of an open app view on behalf of
    /// the calling connection (46 §6). A `press` trigger is the approval; the
    /// tool name comes from the binding, never the client. `-32002` when no
    /// authenticated connection carries the call.
    #[method(name = "app_call_tool", with_extensions)]
    async fn app_call_tool(&self, args: CallArgs) -> RpcResult<AppToolResult>;
    /// Status (and stored result) of a pressed invocation by `request_id`,
    /// for reconnect and duplicate-press resolution (46 §6.4).
    #[method(name = "app_call_status", with_extensions)]
    async fn app_call_status(&self, request_id: String) -> RpcResult<AppToolResult>;
    /// Close one open view: stop its reads, drop queued work (46 §7). Idempotent.
    #[method(name = "app_close", with_extensions)]
    async fn app_close(&self, instance_id: String) -> RpcResult<()>;
    /// The artifact record of one `show` call, for opening a search hit
    /// without listing everything; vanished record is an error (49 §4.2).
    /// A private record is served only to its tree's owner naming the
    /// `session` (authorized, 51 §4.2), as `get_attachment`.
    #[method(name = "get_artifact", with_extensions)]
    async fn get_artifact(
        &self,
        call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<artifact::Artifact>;
    /// The member listing of a `Kind::Archive` record (35 §3.2): paths,
    /// sizes and dates from the archive's headers, at most 5,000 entries
    /// (`truncated` past that). Rejects when the record vanished or the
    /// archive was unreadable at show time (no listing sidecar). A private
    /// record needs its owning `session`, as `get_artifact`.
    #[method(name = "get_artifact_listing", with_extensions)]
    async fn get_artifact_listing(
        &self,
        call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<artifact::archive::ArchiveListing>;
    /// Up to 2,000 characters of the artifact's indexed text around the
    /// occurrence `query` matches, with highlight ranges and content coverage;
    /// unavailable extraction is an error, never the live `origin` (49 §7).
    /// Durable records only: a private tree is never indexed (51 §4.5).
    #[method(name = "artifact_search_match")]
    async fn artifact_search_match(
        &self,
        call_id: String,
        query: String,
    ) -> RpcResult<ArtifactMatchContext>;
}

#[async_trait]
impl ArtifactsApiServer for Server {

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

    async fn answer_page_preview(
        &self,
        id: artifact::preview::PreviewId,
        answer: artifact::preview::PagePreviewAnswer,
    ) -> RpcResult<bool> {
        Ok(artifact::preview::answer(id, answer))
    }

    async fn answer_fence_snapshot(
        &self,
        id: artifact::snapshot::FenceSnapshotId,
        answer: artifact::snapshot::FenceSnapshotAnswer,
    ) -> RpcResult<bool> {
        Ok(artifact::snapshot::answer(id, answer))
    }

    async fn fence_snapshot(
        &self,
        ext: &Extensions,
        lang: artifact::snapshot::FenceLang,
        source: String,
        width: u32,
        appearance: artifact::snapshot::Appearance,
        scale: u32,
    ) -> RpcResult<artifact::snapshot::FenceSnapshotReply> {
        owner(ext)?;
        let (width, scale) = (width.clamp(240, 1200), scale.clamp(1, 3));
        Ok(
            artifact::snapshot::render(&self.rt, lang, &source, width, appearance, scale)
                .await
                .into(),
        )
    }

    async fn list_artifacts(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        descendants: Option<bool>,
    ) -> RpcResult<Vec<artifact::Artifact>> {
        self.authorize_opt(ext, session).await?;
        send(
            &artifact()?,
            artifact::List {
                session,
                descendants: descendants.unwrap_or(false),
            },
        )
        .await
    }

    async fn export_artifact(
        &self,
        ext: &Extensions,
        id: String,
        dest: PathBuf,
        session: Option<SessionId>,
    ) -> RpcResult<PathBuf> {
        self.authorize_opt(ext, session).await?;
        send(&artifact()?, artifact::Export { id, dest, session }).await
    }

    async fn publish_app(
        &self,
        ext: &Extensions,
        session: SessionId,
        name: String,
        source: String,
    ) -> RpcResult<AppDoc> {
        self.authorize(ext, session).await?;
        send(
            &apps()?,
            PublishFromUi {
                session,
                name,
                source,
            },
        )
        .await
    }

    async fn app_call_tool(&self, ext: &Extensions, args: CallArgs) -> RpcResult<AppToolResult> {
        let peer = peer(ext)?;
        send(&apps()?, CallTool { peer, args }).await
    }

    async fn app_call_status(
        &self,
        ext: &Extensions,
        request_id: String,
    ) -> RpcResult<AppToolResult> {
        let peer = peer(ext)?;
        send(&apps()?, CallStatusOf { peer, request_id }).await
    }

    async fn app_close(&self, ext: &Extensions, instance_id: String) -> RpcResult<()> {
        let peer = peer(ext)?;
        send(&apps()?, CloseApp { peer, instance_id }).await
    }

    async fn get_artifact(
        &self,
        ext: &Extensions,
        call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<artifact::Artifact> {
        self.authorize_opt(ext, session).await?;
        match send(
            &artifact()?,
            artifact::search::Get {
                call_id: call_id.clone(),
                session,
            },
        )
        .await
        {
            Ok(a) => Ok(a),
            // 68 §7: the index is engine-local; a record shown elsewhere comes from a peer
            // (peers serve durable records only).
            Err(e) => artifact::mesh::fetch_record(&self.rt.data_dir, &call_id, None)
                .await
                .map_err(|_| e),
        }
    }

    async fn get_artifact_listing(
        &self,
        ext: &Extensions,
        call_id: String,
        session: Option<SessionId>,
    ) -> RpcResult<artifact::archive::ArchiveListing> {
        self.authorize_opt(ext, session).await?;
        let a = send(&artifact()?, artifact::search::Get { call_id, session }).await?;
        if a.kind != artifact::Kind::Archive {
            return Err(ErrorObjectOwned::owned(
                ASK_FAILED,
                format!("{}: not an archive", a.name),
                None::<()>,
            ));
        }
        artifact::archive::read_listing(&a.path)
            .await
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e, None::<()>))
    }

    async fn artifact_search_match(
        &self,
        call_id: String,
        query: String,
    ) -> RpcResult<ArtifactMatchContext> {
        send(
            &artifact()?,
            artifact::search::ReadSearchMatch { call_id, query },
        )
        .await
    }

    async fn put_attachment(&self, bytes: String, ext: String) -> RpcResult<PathBuf> {
        let bytes = decode_attachment(bytes, &ext)?;
        save_blob(&bytes, &ext, &self.rt.data_dir).map_err(|e| failed(e.to_string()))
    }

    async fn get_attachment(
        &self,
        ext: &Extensions,
        path: PathBuf,
        session: Option<SessionId>,
    ) -> RpcResult<String> {
        // A private tree's files live under its scratch root, never under
        // `data_dir`: without the owning session they are not served at all.
        let root = match session {
            Some(s) => self.authorize(ext, s).await?.resources.root.clone(),
            None => self.rt.data_dir.clone(),
        };
        let real = match std::fs::canonicalize(&path) {
            Ok(p) => p,
            // 68 §7: a blob (`…/artifacts/<hash>.<ext>`), spill file
            // (`…/spill/<session>/<call_id>.txt`) or attachment
            // (`…/attachments/<sha16>.<ext>`) this engine never wrote is fetched
            // from a peer — the session's owner first — and cached under ours.
            Err(e) => {
                let prefer = match session {
                    Some(s) => forward::owner_peer(s).await,
                    None => None,
                };
                let prefer = prefer.as_ref();
                let dir = |n: usize| {
                    path.ancestors()
                        .nth(n)
                        .and_then(Path::file_name)
                        .and_then(|d| d.to_str())
                };
                let fetched = match (path.file_name().and_then(|n| n.to_str()), dir(1), dir(2)) {
                    (Some(name), Some("artifacts"), _) => {
                        artifact::mesh::fetch_blob(&root.join("artifacts"), name, prefer).await
                    }
                    (Some(name), Some("attachments"), _) => {
                        artifact::mesh::fetch_attachment(&root.join("attachments"), name, prefer)
                            .await
                    }
                    (Some(_), Some(session), Some("spill")) => {
                        match (session.parse(), path.file_stem().and_then(|s| s.to_str())) {
                            (Ok(session), Some(call_id)) => {
                                artifact::mesh::fetch_spill(&root, session, call_id, prefer).await
                            }
                            _ => return Err(failed(format!("{}: {e}", path.display()))),
                        }
                    }
                    _ => return Err(failed(format!("{}: {e}", path.display()))),
                }
                .map_err(|e| failed(format!("{}: {e:#}", path.display())))?;
                std::fs::canonicalize(&fetched)
                    .map_err(|e| failed(format!("{}: {e}", fetched.display())))?
            }
        };
        let allowed = ["attachments", "browser", "computer", "artifacts", "spill"]
            .iter()
            .any(|d| std::fs::canonicalize(root.join(d)).is_ok_and(|dir| real.starts_with(dir)));
        if !allowed {
            return Err(failed(format!("{}: not an attachment", path.display())));
        }
        let bytes = tokio::fs::read(&real)
            .await
            .map_err(|e| failed(format!("{}: {e}", path.display())))?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    ArtifactsApiServer::into_rpc(server)
}
