use super::*;

#[rpc(server, client)]
pub trait ConceptsApi {
    /// Search the knowledge bundle (design doc 10 §8); `limit` defaults to 10, max 50.
    #[method(name = "knowledge_search")]
    async fn knowledge_search(&self, query: String, limit: Option<u32>) -> RpcResult<Vec<Hit>>;
    /// One concept verbatim with its links and citers.
    #[method(name = "knowledge_get")]
    async fn knowledge_get(&self, id: String) -> RpcResult<Concept>;
    /// Concept metas sorted by id; `limit` defaults to 50.
    #[method(name = "knowledge_list")]
    async fn knowledge_list(
        &self,
        prefix: Option<String>,
        #[argument(rename = "type")] type_: Option<String>,
        tag: Option<String>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<Meta>>;
    /// Create or update a concept as `source: user`.
    #[method(name = "knowledge_put")]
    async fn knowledge_put(&self, put: Put) -> RpcResult<Meta>;
    /// Delete a concept (restorable through `knowledge_restore`).
    #[method(name = "knowledge_delete")]
    async fn knowledge_delete(&self, id: String) -> RpcResult<()>;
    /// Create or update a concept from its raw text (frontmatter and body) as
    /// `source: user`; unknown frontmatter keys survive, unlike `knowledge_put`.
    #[method(name = "knowledge_write")]
    async fn knowledge_write(&self, id: String, text: String) -> RpcResult<Meta>;
    /// Reconcile the index with the files on disk now.
    #[method(name = "knowledge_sync")]
    async fn knowledge_sync(&self) -> RpcResult<knowledge::Sync>;
    /// Commits touching `id` (the whole bundle when absent), newest first; `limit` defaults to 20.
    #[method(name = "knowledge_history")]
    async fn knowledge_history(&self, id: Option<String>, limit: Option<u32>)
    -> RpcResult<History>;
    /// `commit` against its first parent (the empty tree for a root commit) as
    /// per-concept unified diffs, including deletions; `id` restricts to one concept.
    #[method(name = "knowledge_revision")]
    async fn knowledge_revision(&self, commit: String, id: Option<String>) -> RpcResult<Revision>;
    /// The full text of concept `id` as committed at `commit` (a full object id
    /// from a `knowledge_history` search hit's `snapshot_commit`); unknown
    /// revision is an error, never the current text (49 §7).
    #[method(name = "knowledge_read_revision")]
    async fn knowledge_read_revision(
        &self,
        id: String,
        commit: String,
    ) -> RpcResult<HistoricalConcept>;
    /// Rewrite `id` from `commit` (default: the commit before its latest change).
    #[method(name = "knowledge_restore")]
    async fn knowledge_restore(&self, id: String, commit: Option<String>) -> RpcResult<Change>;
    /// Harvest `session`'s finished frame now (design doc 10 §5.2). Refused while a turn is running.
    #[method(name = "knowledge_harvest", with_extensions)]
    async fn knowledge_harvest(&self, session: SessionId) -> RpcResult<HarvestResult>;
}

#[async_trait]
impl ConceptsApiServer for Server {

    async fn knowledge_search(&self, query: String, limit: Option<u32>) -> RpcResult<Vec<Hit>> {
        send(&knowledge()?, Search { query, limit }).await
    }

    async fn knowledge_get(&self, id: String) -> RpcResult<Concept> {
        send(&knowledge()?, Get { id }).await
    }

    async fn knowledge_list(
        &self,
        prefix: Option<String>,
        type_: Option<String>,
        tag: Option<String>,
        limit: Option<u32>,
    ) -> RpcResult<Vec<Meta>> {
        send(
            &knowledge()?,
            List {
                prefix,
                type_,
                tag,
                limit,
            },
        )
        .await
    }

    async fn knowledge_put(&self, put: Put) -> RpcResult<Meta> {
        send(
            &knowledge()?,
            PutConcept {
                put,
                source: Source::User,
            },
        )
        .await
    }

    async fn knowledge_delete(&self, id: String) -> RpcResult<()> {
        send(&knowledge()?, Delete { id }).await
    }

    async fn knowledge_write(&self, id: String, text: String) -> RpcResult<Meta> {
        send(&knowledge()?, Write { id, text }).await
    }

    async fn knowledge_sync(&self) -> RpcResult<knowledge::Sync> {
        send(&knowledge()?, SyncNow).await
    }

    async fn knowledge_history(
        &self,
        id: Option<String>,
        limit: Option<u32>,
    ) -> RpcResult<History> {
        send(&knowledge()?, HistoryOf { id, limit }).await
    }

    async fn knowledge_revision(&self, commit: String, id: Option<String>) -> RpcResult<Revision> {
        send(&knowledge()?, RevisionOf { commit, id }).await
    }

    async fn knowledge_read_revision(
        &self,
        id: String,
        commit: String,
    ) -> RpcResult<HistoricalConcept> {
        send(
            &knowledge()?,
            knowledge::search::ReadRevision { id, commit },
        )
        .await
    }

    async fn knowledge_restore(&self, id: String, commit: Option<String>) -> RpcResult<Change> {
        send(&knowledge()?, Restore { id, commit }).await
    }

    async fn knowledge_harvest(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<HarvestResult> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "knowledge_harvest", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, Harvest { session }).await
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    ConceptsApiServer::into_rpc(server)
}
