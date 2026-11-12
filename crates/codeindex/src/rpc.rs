use std::path::PathBuf;
use crate as codeindex;
use codeindex::{
    ChangeLens, FitLens, HereLens, IndexStats, LensMeta, LocateAnchor, Located,
    MapElement, MapExport, MapHit, MapIndex, MapJob,
};
use codeindex::rpc_support::codeindex;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{Server, send};
use runtime::SessionId;

#[rpc_core_macros::rpc_descriptors(owner = "codeindex", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait CodemapApi {
    /// Row counts and freshness of the code index for `root` (default: the
    /// git top-level of the process cwd; design doc 60 §10). Opens the
    /// index without reconciling it.
    #[method(name = "codeindex_stats")]
    async fn codeindex_stats(&self, root: Option<String>) -> RpcResult<IndexStats>;
    /// Verify board anchors against the working tree under `root` (71 §2.5,
    /// M11): one `Located` per anchor, in order — `same` / `changed` /
    /// `moved` (new range) / `missing`. Read-only; `root` must be a git
    /// top-level and every `path` must stay under it (the whole batch errors
    /// otherwise).
    #[method(name = "code_locate")]
    async fn code_locate(
        &self,
        root: String,
        anchors: Vec<LocateAnchor>,
    ) -> RpcResult<Vec<Located>>;
    /// CodeMap Here lens (73 §5.2): the stored ranked map of an already
    /// indexed `root` cut to `limit` rows (default 2 000) with badges;
    /// `focus`/`edited` personalise the rank. Store reads only, never a
    /// walk; `ASK_FAILED` when the root was never indexed.
    #[method(name = "codemap_here")]
    async fn codemap_here(
        &self,
        root: String,
        focus: Option<Vec<String>>,
        edited: Option<Vec<String>>,
        limit: Option<u32>,
    ) -> RpcResult<HereLens>;
    /// CodeMap Fit lens (73 §5.2): the butterfly around `symbol` (a moniker,
    /// a name, `path:line` or a symbol id) — callers, focus, callees — cut to
    /// `limit` nodes (default 20) with counted remainders.
    #[method(name = "codemap_fit")]
    async fn codemap_fit(
        &self,
        root: String,
        symbol: String,
        limit: Option<u32>,
    ) -> RpcResult<FitLens>;
    /// CodeMap Change lens (73 §5.2): `code_impact` over the open index in
    /// four bands (changed, direct, transitive, tests); neither `git_range`
    /// nor `files` means `WORKTREE`.
    #[method(name = "codemap_change")]
    async fn codemap_change(
        &self,
        root: String,
        git_range: Option<String>,
        files: Option<Vec<String>>,
        include_tests: Option<bool>,
    ) -> RpcResult<ChangeLens>;
    /// CodeMap touch (73 §5.2): stat and re-parse only `paths` (root-relative;
    /// a path outside the root fails the call), or force a full reconcile when
    /// `paths` is null. Publishes `CodeReconciled`.
    #[method(name = "codemap_touch")]
    async fn codemap_touch(&self, root: String, paths: Option<Vec<String>>) -> RpcResult<LensMeta>;
    /// CodeMap overview (147 §10.2): every perspective of an indexed `root`
    /// with its element tree and `meta.state`; the first skeleton pass runs
    /// inline when the map file is missing. Store reads only, never a walk;
    /// `ASK_FAILED` when the root was never indexed.
    #[method(name = "codemap_overview")]
    async fn codemap_overview(&self, root: String) -> RpcResult<MapIndex>;
    /// CodeMap element (147 §10.2): one element — diagram, prose, edges with
    /// evidence, children with thumbnails, listing for leaves.
    #[method(name = "codemap_element")]
    async fn codemap_element(&self, root: String, element: String) -> RpcResult<MapElement>;
    /// CodeMap generate (147 §10.2, S1/S2): start the root's one generation
    /// job charged to `session` (a live native session), or attach to the
    /// running one; `element` set regenerates that subtree only.
    #[method(name = "codemap_generate")]
    async fn codemap_generate(
        &self,
        root: String,
        session: SessionId,
        element: Option<String>,
    ) -> RpcResult<MapJob>;
    /// CodeMap cancel (147 §6.7): abort the root's running job; stored
    /// elements stay.
    #[method(name = "codemap_cancel")]
    async fn codemap_cancel(&self, root: String) -> RpcResult<MapJob>;
    /// CodeMap search (147 §10.2): elements by name, or by a root-relative
    /// path → the element chain whose anchors contain it; `limit` default 20.
    #[method(name = "codemap_search")]
    async fn codemap_search(
        &self,
        root: String,
        query: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<MapHit>>;
    /// CodeMap export (147 §13.1, S3): write `<root>/.omm/` in
    /// oh-my-mermaid's layout; refuses when a foreign `.omm/` exists.
    #[method(name = "codemap_export")]
    async fn codemap_export(&self, root: String) -> RpcResult<MapExport>;
}

#[async_trait]
impl CodemapApiServer for Server {

    async fn codeindex_stats(&self, root: Option<String>) -> RpcResult<IndexStats> {
        send(
            &codeindex()?,
            codeindex::Stats {
                root: root.map(PathBuf::from),
            },
        )
        .await
    }

    async fn code_locate(
        &self,
        root: String,
        anchors: Vec<LocateAnchor>,
    ) -> RpcResult<Vec<Located>> {
        send(
            &codeindex()?,
            codeindex::Locate {
                root: PathBuf::from(root),
                anchors,
            },
        )
        .await
    }

    async fn codemap_here(
        &self,
        root: String,
        focus: Option<Vec<String>>,
        edited: Option<Vec<String>>,
        limit: Option<u32>,
    ) -> RpcResult<HereLens> {
        send(
            &codeindex()?,
            codeindex::Here {
                root: PathBuf::from(root),
                focus,
                edited,
                limit,
            },
        )
        .await
    }

    async fn codemap_fit(
        &self,
        root: String,
        symbol: String,
        limit: Option<u32>,
    ) -> RpcResult<FitLens> {
        send(
            &codeindex()?,
            codeindex::Fit {
                root: PathBuf::from(root),
                symbol,
                limit,
            },
        )
        .await
    }

    async fn codemap_change(
        &self,
        root: String,
        git_range: Option<String>,
        files: Option<Vec<String>>,
        include_tests: Option<bool>,
    ) -> RpcResult<ChangeLens> {
        send(
            &codeindex()?,
            codeindex::Change {
                root: PathBuf::from(root),
                git_range,
                files,
                include_tests,
            },
        )
        .await
    }

    async fn codemap_touch(&self, root: String, paths: Option<Vec<String>>) -> RpcResult<LensMeta> {
        send(
            &codeindex()?,
            codeindex::Touch {
                root: PathBuf::from(root),
                paths,
            },
        )
        .await
    }

    async fn codemap_overview(&self, root: String) -> RpcResult<MapIndex> {
        send(
            &codeindex()?,
            codeindex::map::Overview {
                root: PathBuf::from(root),
            },
        )
        .await
    }

    async fn codemap_element(&self, root: String, element: String) -> RpcResult<MapElement> {
        send(
            &codeindex()?,
            codeindex::map::Element {
                root: PathBuf::from(root),
                element,
            },
        )
        .await
    }

    async fn codemap_generate(
        &self,
        root: String,
        session: SessionId,
        element: Option<String>,
    ) -> RpcResult<MapJob> {
        send(
            &codeindex()?,
            codeindex::map::Generate {
                root: PathBuf::from(root),
                session,
                element,
            },
        )
        .await
    }

    async fn codemap_cancel(&self, root: String) -> RpcResult<MapJob> {
        send(
            &codeindex()?,
            codeindex::map::Cancel {
                root: PathBuf::from(root),
            },
        )
        .await
    }

    async fn codemap_search(
        &self,
        root: String,
        query: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<MapHit>> {
        send(
            &codeindex()?,
            codeindex::map::Search {
                root: PathBuf::from(root),
                query,
                limit,
            },
        )
        .await
    }

    async fn codemap_export(&self, root: String) -> RpcResult<MapExport> {
        send(
            &codeindex()?,
            codeindex::map::Export {
                root: PathBuf::from(root),
            },
        )
        .await
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(CodemapApiServer::into_rpc(server), rpc_descriptors())
        .expect("codeindex RPC descriptors")
}
