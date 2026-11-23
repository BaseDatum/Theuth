// `web_view_state` takes six positional params (209 §4.4) plus the connection's extensions; the
// RPC macro's generated server method trips the argument-count lint.
#![allow(clippy::too_many_arguments)]

use crate as artifact;
use agent::rpc_forward as forward;
use std::path::{Path, PathBuf};
use artifact::search::ArtifactMatchContext;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::server::{PendingSubscriptionSink, SubscriptionMessage};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use runtime::SessionId;
use rpc_core::{send, failed, owner, ASK_FAILED, Server};
use artifact::rpc_support::artifact;
use agent::rpc_support::RpcServerExt;
use base64::Engine as _;

/// Artifacts RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "artifact", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait ArtifactArtifactsApi {
    /// A client's answer to a `PagePreviewRequested` (120 §11.2): `Started`
    /// extends the wait, the first `Done`/`Failed` resolves it. `true` if the
    /// preview was still pending. 209 §4.4: with `session` (the request's), an
    /// answer for a preview another engine issued is forwarded to that session's owner.
    #[method(name = "answer_page_preview", with_extensions)]
    async fn answer_page_preview(
        &self,
        id: artifact::preview::PreviewId,
        answer: artifact::preview::PagePreviewAnswer,
        session: Option<SessionId>,
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
    /// 209 D9: a client's report of one web view tile (address, title, loading), recorded for
    /// the `web_views` tool; `closed` forgets the tile. Forwarded to the session's owner when
    /// another engine owns it.
    #[method(name = "web_view_state", with_extensions)]
    async fn web_view_state(
        &self,
        session: SessionId,
        view: String,
        url: String,
        title: String,
        loading: bool,
        closed: Option<bool>,
    ) -> RpcResult<artifact::webview::WebViewStateReply>;
    /// 212 §2, §6: the text of a file a `show_file` tile shows (at most 2 MiB, `truncated`
    /// past that); `unchanged` with empty text when `if_mtime_ms` is still its mtime. Served for
    /// what the session showed in this engine lifetime or anything inside its working directory
    /// or repository, never a cell record; `not a shown file` otherwise. Forwarded to the
    /// session's owner.
    #[method(name = "file_view_read", with_extensions)]
    async fn file_view_read(
        &self,
        session: SessionId,
        path: String,
        if_mtime_ms: Option<u64>,
    ) -> RpcResult<artifact::codeview::FileViewText>;
    /// 212 §2: the unified patch a `show_diff` tile shows, recomputed now, with per-file
    /// numbers (text at most 1 MiB, then a truncation line). Authorized as `file_view_read`
    /// (`not a shown diff`); forwarded to the session's owner.
    #[method(name = "diff_view_read", with_extensions)]
    async fn diff_view_read(
        &self,
        session: SessionId,
        spec: artifact::codeview::DiffSpec,
    ) -> RpcResult<artifact::codeview::DiffViewText>;
    /// 212 §6: one item each time a shown file settles after a change (written, replaced,
    /// deleted, back). Authorized as `file_view_read`. The watch runs where the file is: a
    /// session owned by another engine is refused with `RUNS_ELSEWHERE` (the tile then polls
    /// `file_view_read` with `if_mtime_ms`).
    #[subscription(name = "subscribe_file_view", unsubscribe = "unsubscribe_file_view", item = artifact::codeview::FileViewChange, with_extensions)]
    async fn subscribe_file_view(&self, session: SessionId, path: String) -> SubscriptionResult;
    /// 209 D8, §4.4: a client's answer to a `WebViewCallRequested`: the first `Claimed` wins
    /// (later ones answer `false`) and binds the call to this connection; only that connection's
    /// `Done`/`Failed` resolves it (before any claim, only a `Failed` "stopped by the user" or
    /// "window closed"). `true` while the call was pending and the answer was taken. Owner
    /// connections only. `session` is the request's: an id another engine issued (the event came
    /// through the relay) is forwarded to that session's owner.
    #[method(name = "answer_web_view_call", with_extensions)]
    async fn answer_web_view_call(
        &self,
        id: artifact::webview::WebViewCallId,
        answer: artifact::webview::WebViewCallAnswer,
        session: Option<SessionId>,
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
    /// Read a file the harness produced or stored, base64-encoded. Only
    /// paths under `<root>/{attachments,browser,computer,artifacts}` (after
    /// canonicalization) are served, where `<root>` is `data_dir`, or with
    /// `session` the tree's own root — a private tree's files are served to
    /// its owner only, and never without naming the session (51 §4.2).
    #[method(name = "get_attachment", with_extensions)]
    async fn get_attachment(&self, path: PathBuf, session: Option<SessionId>) -> RpcResult<String>;
}

#[async_trait]
impl ArtifactArtifactsApiServer for Server {

    async fn answer_page_preview(
        &self,
        ext: &Extensions,
        id: artifact::preview::PreviewId,
        answer: artifact::preview::PagePreviewAnswer,
        session: Option<SessionId>,
    ) -> RpcResult<bool> {
        if !artifact::preview::pending_here(id)
            && let Some(session) = session
            && let Some(r) =
                forward::route(ext, session, "answer_page_preview", (id, &answer, Some(session)))
                    .await?
        {
            return Ok(r);
        }
        Ok(artifact::preview::answer(id, answer))
    }

    async fn answer_fence_snapshot(
        &self,
        id: artifact::snapshot::FenceSnapshotId,
        answer: artifact::snapshot::FenceSnapshotAnswer,
    ) -> RpcResult<bool> {
        Ok(artifact::snapshot::answer(id, answer))
    }

    async fn web_view_state(
        &self,
        ext: &Extensions,
        session: SessionId,
        view: String,
        url: String,
        title: String,
        loading: bool,
        closed: Option<bool>,
    ) -> RpcResult<artifact::webview::WebViewStateReply> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(
            ext,
            session,
            "web_view_state",
            (session, &view, &url, &title, loading, closed),
        )
        .await?
        {
            return Ok(r);
        }
        Ok(artifact::webview::record_state(artifact::webview::WebViewState {
            session,
            view,
            url,
            title,
            loading,
            closed: closed.unwrap_or(false),
        }))
    }

    async fn file_view_read(
        &self,
        ext: &Extensions,
        session: SessionId,
        path: String,
        if_mtime_ms: Option<u64>,
    ) -> RpcResult<artifact::codeview::FileViewText> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) =
            forward::route(ext, session, "file_view_read", (session, &path, if_mtime_ms)).await?
        {
            return Ok(r);
        }
        let root = self.authorize(ext, session).await?.resources.root.clone();
        tokio::task::spawn_blocking(move || {
            artifact::codeview::file_view_read(session, &root, &path, if_mtime_ms)
        })
        .await
        .map_err(|e| failed(e.to_string()))?
        .map_err(failed)
    }

    async fn diff_view_read(
        &self,
        ext: &Extensions,
        session: SessionId,
        spec: artifact::codeview::DiffSpec,
    ) -> RpcResult<artifact::codeview::DiffViewText> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "diff_view_read", (session, &spec)).await? {
            return Ok(r);
        }
        let root = self.authorize(ext, session).await?.resources.root.clone();
        tokio::task::spawn_blocking(move || {
            artifact::codeview::diff_view_read(session, &root, &spec)
        })
        .await
        .map_err(|e| failed(e.to_string()))?
        .map_err(failed)
    }

    async fn subscribe_file_view(
        &self,
        pending: PendingSubscriptionSink,
        ext: &Extensions,
        session: SessionId,
        path: String,
    ) -> SubscriptionResult {
        let admit = async {
            let root = self.authorize(ext, session).await?.resources.root.clone();
            if let Some(owner) = forward::owner_peer(session).await {
                return Err(forward::runs_elsewhere(owner.0));
            }
            artifact::codeview::authorize_file(session, &root, &path).map_err(failed)
        };
        let path = match admit.await {
            Ok(p) => p,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        // One watcher per file, whichever spelling of its path the client sent.
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        let mut watch = artifact::codewatch::watch(&path);
        tokio::spawn(async move {
            loop {
                let change = tokio::select! {
                    _ = sink.closed() => return,
                    c = watch.next() => c,
                };
                let Some(change) = change else { return };
                let item = SubscriptionMessage::from(
                    serde_json::value::to_raw_value(&change).expect("FileViewChange serializes"),
                );
                if sink.send(item).await.is_err() {
                    return;
                }
            }
        });
        Ok(())
    }

    async fn answer_web_view_call(
        &self,
        ext: &Extensions,
        id: artifact::webview::WebViewCallId,
        answer: artifact::webview::WebViewCallAnswer,
        session: Option<SessionId>,
    ) -> RpcResult<bool> {
        use artifact::webview;
        let peer = owner(ext)?;
        // 209 §4.4: the answerer is this connection, or the engine a forwarded answer came from
        // (that engine binds it to its own connection, `webview::relay_allows`).
        let by = match ext.get::<forward::Forwarded>() {
            Some(f) => format!("engine:{}", f.0.0),
            None => format!("conn:{}", peer.id()),
        };
        if !webview::pending_here(&self.rt, id)
            && let Some(session) = session.or_else(|| webview::session_of(id))
        {
            if !webview::relay_allows(id, &answer, &by) {
                return Ok(false);
            }
            if let Some(taken) =
                forward::route(ext, session, "answer_web_view_call", (id, &answer, Some(session)))
                    .await?
            {
                webview::relayed(id, &answer, &by, taken);
                return Ok(taken);
            }
        }
        Ok(webview::answer(&self.rt, id, answer, &by))
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

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(ArtifactArtifactsApiServer::into_rpc(server), rpc_descriptors())
        .expect("artifact RPC descriptors")
}
