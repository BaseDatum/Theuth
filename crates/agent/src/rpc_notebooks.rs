use crate as agent;
use agent::{Agents, SessionSummary};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use runtime::SessionId;
use rpc_core::{lookup, owner, ASK_FAILED, Server};
use agent::rpc_document::{check_session_cites, doc_forward, level_str, notebook_chat_counts};
use document::rpc_support::{doc_send, documents, member_outcome, nb_result, principal};

/// Notebooks RPC methods owned by this plugin.
#[rpc(server, client)]
pub trait AgentNotebooksApi {
    /// Notebooks (144 §13.3): live ones newest first; `archived` includes archived. Any engine, from the replica.
    #[method(name = "notebook_list")]
    async fn notebook_list(&self, archived: Option<bool>) -> RpcResult<Vec<document::Notebook>>;
    /// One notebook with counts and its live items. Any engine; mode-2 guests see the notebooks they are members of.
    #[method(name = "notebook_get", with_extensions)]
    async fn notebook_get(&self, id: document::DocId) -> RpcResult<document::NotebookDetail>;
    /// A new notebook (144 §3.2): a `DocKind::Notebook` document named `title` whose body is `description`,
    /// plus its `notebooks` row. Hub only (`-32011` → forwarded).
    #[method(name = "notebook_create", with_extensions)]
    async fn notebook_create(
        &self,
        title: String,
        description: Option<String>,
        project: Option<String>,
    ) -> RpcResult<document::Notebook>;
    /// Rename, re-project (`project: ""` clears) or (un)archive a notebook. Hub only.
    #[method(name = "notebook_update", with_extensions)]
    async fn notebook_update(
        &self,
        id: document::DocId,
        title: Option<String>,
        project: Option<String>,
        archived: Option<bool>,
    ) -> RpcResult<()>;
    /// Trash the notebook document, soft-remove its items and, when `delete_exclusive_sources`, the sources no
    /// other live notebook holds. Notes are never deleted; members get `Closed{deleted}`. Hub only.
    #[method(name = "notebook_delete", with_extensions)]
    async fn notebook_delete(
        &self,
        id: document::DocId,
        delete_exclusive_sources: bool,
    ) -> RpcResult<()>;
    /// Add (or revive) one item row. Hub only.
    #[method(name = "notebook_item_add", with_extensions)]
    async fn notebook_item_add(
        &self,
        notebook: document::DocId,
        kind: document::ItemKind,
        item: String,
    ) -> RpcResult<document::NotebookItem>;
    /// Soft-remove one item row; answers how many other live notebooks still hold the item. Hub only.
    #[method(name = "notebook_item_remove", with_extensions)]
    async fn notebook_item_remove(
        &self,
        notebook: document::DocId,
        kind: document::ItemKind,
        item: String,
    ) -> RpcResult<document::ItemRemoved>;
    /// Sessions bound to `notebook`, newest first (144 §8). Owner only — never on the guest allow-list (D13).
    #[method(name = "notebook_chats", with_extensions)]
    async fn notebook_chats(&self, notebook: document::DocId) -> RpcResult<Vec<SessionSummary>>;
    /// Bind `session` to a notebook (`sessions.notebook`), or unbind with `None`. Owner only.
    #[method(name = "session_set_notebook", with_extensions)]
    async fn session_set_notebook(
        &self,
        session: SessionId,
        notebook: Option<document::DocId>,
    ) -> RpcResult<()>;
    /// Resolve `theuth://` hrefs (144 §6.4) to citations with their drift status, positionally. Any engine;
    /// `available: false` for trashed targets or ones the caller may not read.
    #[method(name = "cite_resolve", with_extensions)]
    async fn cite_resolve(&self, hrefs: Vec<String>) -> RpcResult<Vec<document::Citation>>;
    /// Save the assistant message of the turn opened at seq `turn` (`HistoryEntry.turn`, like `regenerate`) as a new
    /// note in `notebook`, chips intact (144 §8). Owner only.
    #[method(name = "save_answer_as_note", with_extensions)]
    async fn save_answer_as_note(
        &self,
        session: SessionId,
        turn: u32,
        notebook: document::DocId,
    ) -> RpcResult<agent::notebooks::SavedAnswer>;
    /// Append the assistant message of the turn opened at seq `turn` to `note` under `## From chat · <date>` (144 §8). Owner only.
    #[method(name = "promote_answer", with_extensions)]
    async fn promote_answer(
        &self,
        session: SessionId,
        turn: u32,
        note: document::DocId,
    ) -> RpcResult<()>;
    /// 144 §5.1 (wave 2): pin a notebook item's dial level for `session` (`session_context`). Owner only.
    #[method(name = "notebook_context_set", with_extensions)]
    async fn notebook_context_set(
        &self,
        session: SessionId,
        item: String,
        kind: document::ItemKind,
        level: document::ContextLevel,
    ) -> RpcResult<()>;
    /// Ask a question across `scope` (144 §9): a new `kind: ask` session plans, searches each notebook and writes
    /// one cited answer as its first turn; the run streams as `NotebookAsk` on `subscribe_events`. `save_into` files
    /// the answer as a note there when done. Owner only.
    #[method(name = "notebook_ask", with_extensions)]
    async fn notebook_ask(
        &self,
        scope: Vec<document::DocId>,
        question: String,
        save_into: Option<document::DocId>,
    ) -> RpcResult<agent::ask::AskStarted>;
    /// Back to the default rule for `item` in `session`. Owner only.
    #[method(name = "notebook_context_clear", with_extensions)]
    async fn notebook_context_clear(&self, session: SessionId, item: String) -> RpcResult<()>;
    /// Pin every live item of `notebook` (of `kind` when given) at `level` for `session`. Owner only.
    #[method(name = "notebook_context_set_all", with_extensions)]
    async fn notebook_context_set_all(
        &self,
        session: SessionId,
        notebook: document::DocId,
        kind: Option<document::ItemKind>,
        level: document::ContextLevel,
    ) -> RpcResult<()>;
    /// 144 §5.2 / 157 D9: what the chat's `<notebook>` block carries — per item the effective level, pinned
    /// or not, tokens and status; the total; whether the block is already in the chat and how many change
    /// lines the next turn would send. Sized by the thread's own context, never a notebook budget. Read-only,
    /// no model call; `session: None` = a new thread (the default rule, the engine's default model). Owner only.
    #[method(name = "notebook_context_estimate", with_extensions)]
    async fn notebook_context_estimate(
        &self,
        notebook: document::DocId,
        session: Option<SessionId>,
    ) -> RpcResult<document::ContextEstimate>;
    /// 144 §4.3: one more insight section on a source — a builtin (`key_insights` | `toc` | `open_questions`)
    /// or a `notebook_insight: true` skill; answers the `###` heading's block id. Hub only; owner only.
    #[method(name = "insight_run", with_extensions)]
    async fn insight_run(
        &self,
        source: document::DocId,
        instruction: document::InsightInstruction,
    ) -> RpcResult<document::InsightRun>;
    /// Promote an insight section to a note titled `<insight> — <source>` with the provenance trailer, filed in
    /// `notebook` (or in every live notebook holding the source). Hub only; owner only.
    #[method(name = "insight_promote", with_extensions)]
    async fn insight_promote(
        &self,
        source: document::DocId,
        block: String,
        notebook: Option<document::DocId>,
    ) -> RpcResult<document::DocId>;
    /// The ask sessions, newest first. Owner only.
    #[method(name = "notebook_ask_list", with_extensions)]
    async fn notebook_ask_list(&self, limit: Option<u32>)
    -> RpcResult<Vec<agent::ask::AskSummary>>;
    /// Generate (or regenerate → next version) one overview of `notebook` (144 §10) in its overview session; the
    /// artifact is filed in the notebook when the `show` lands. Owner only.
    #[method(name = "notebook_overview_run", with_extensions)]
    async fn notebook_overview_run(
        &self,
        notebook: document::DocId,
        kind: agent::overviews::OverviewKind,
    ) -> RpcResult<agent::overviews::OverviewRun>;
    /// The overviews of `notebook`, newest version first per kind (a running or failed run on top). Owner only.
    #[method(name = "notebook_overviews", with_extensions)]
    async fn notebook_overviews(
        &self,
        notebook: document::DocId,
    ) -> RpcResult<Vec<agent::overviews::Overview>>;
}

#[async_trait]
impl AgentNotebooksApiServer for Server {

    async fn notebook_list(&self, archived: Option<bool>) -> RpcResult<Vec<document::Notebook>> {
        let mut list = doc_send(
            &documents()?,
            document::NotebookList {
                archived: archived.unwrap_or(false),
            },
        )
        .await?;
        let counts = notebook_chat_counts().await;
        for nb in &mut list {
            nb.chats = counts.get(&nb.doc.id.to_string()).copied().unwrap_or(0);
        }
        Ok(list)
    }

    async fn notebook_get(
        &self,
        ext: &Extensions,
        id: document::DocId,
    ) -> RpcResult<document::NotebookDetail> {
        let (peer, by) = principal(ext)?;
        let mut r = doc_send(&documents()?, document::NotebookGet { id, by }).await;
        member_outcome(&peer, &r);
        if let Ok(d) = &mut r {
            d.notebook.chats = notebook_chat_counts()
                .await
                .get(&d.notebook.doc.id.to_string())
                .copied()
                .unwrap_or(0);
        }
        r
    }

    async fn notebook_create(
        &self,
        ext: &Extensions,
        title: String,
        description: Option<String>,
        project: Option<String>,
    ) -> RpcResult<document::Notebook> {
        owner(ext)?;
        if title.trim().is_empty() {
            return Err(ErrorObjectOwned::owned(
                jsonrpsee::types::error::INVALID_PARAMS_CODE,
                "title must not be empty",
                None::<()>,
            ));
        }
        let r = doc_send(
            &documents()?,
            document::NotebookCreate {
                title: title.trim().to_owned(),
                description: description.clone().unwrap_or_default(),
                project: project.clone().filter(|p| !p.trim().is_empty()),
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(ext, "notebook_create", (&title, &description, &project), r).await
    }

    async fn notebook_update(
        &self,
        ext: &Extensions,
        id: document::DocId,
        title: Option<String>,
        project: Option<String>,
        archived: Option<bool>,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::NotebookUpdate {
                id: id.clone(),
                title: title.clone().filter(|t| !t.trim().is_empty()),
                project: project
                    .clone()
                    .map(|p| Some(p).filter(|p| !p.trim().is_empty())),
                archived,
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(
            ext,
            "notebook_update",
            (&id, &title, &project, &archived),
            r,
        )
        .await
    }

    async fn notebook_delete(
        &self,
        ext: &Extensions,
        id: document::DocId,
        delete_exclusive_sources: bool,
    ) -> RpcResult<()> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::NotebookDelete {
                id: id.clone(),
                delete_exclusive_sources,
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(ext, "notebook_delete", (&id, &delete_exclusive_sources), r).await
    }

    async fn notebook_item_add(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
        kind: document::ItemKind,
        item: String,
    ) -> RpcResult<document::NotebookItem> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::NotebookItemAdd {
                notebook: notebook.clone(),
                kind,
                item: item.clone(),
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(ext, "notebook_item_add", (&notebook, &kind, &item), r).await
    }

    async fn notebook_item_remove(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
        kind: document::ItemKind,
        item: String,
    ) -> RpcResult<document::ItemRemoved> {
        owner(ext)?;
        let r = doc_send(
            &documents()?,
            document::NotebookItemRemove {
                notebook: notebook.clone(),
                kind,
                item: item.clone(),
                by: document::Principal::Owner { device: None },
            },
        )
        .await;
        doc_forward(ext, "notebook_item_remove", (&notebook, &kind, &item), r).await
    }

    async fn notebook_chats(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
    ) -> RpcResult<Vec<SessionSummary>> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(agent::notebooks::notebook_chats(&agents, notebook.to_string()).await)
    }

    async fn session_set_notebook(
        &self,
        ext: &Extensions,
        session: SessionId,
        notebook: Option<document::DocId>,
    ) -> RpcResult<()> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::notebooks::session_set_notebook(
                &agents,
                session,
                notebook.map(|n| n.to_string()),
            )
            .await,
        )
    }

    async fn cite_resolve(
        &self,
        ext: &Extensions,
        hrefs: Vec<String>,
    ) -> RpcResult<Vec<document::Citation>> {
        let (peer, by) = principal(ext)?;
        let mut r = doc_send(&documents()?, document::CiteResolve { hrefs, by }).await;
        member_outcome(&peer, &r);
        if let Ok(cites) = &mut r {
            check_session_cites(cites).await;
        }
        r
    }

    async fn save_answer_as_note(
        &self,
        ext: &Extensions,
        session: SessionId,
        turn: u32,
        notebook: document::DocId,
    ) -> RpcResult<agent::notebooks::SavedAnswer> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::notebooks::save_answer_as_note(&agents, session, turn, notebook.to_string())
                .await,
        )
    }

    async fn promote_answer(
        &self,
        ext: &Extensions,
        session: SessionId,
        turn: u32,
        note: document::DocId,
    ) -> RpcResult<()> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(agent::notebooks::promote_answer(&agents, session, turn, note.to_string()).await)
    }

    async fn notebook_context_set(
        &self,
        ext: &Extensions,
        session: SessionId,
        item: String,
        kind: document::ItemKind,
        level: document::ContextLevel,
    ) -> RpcResult<()> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::notebook_context::context_set(
                &agents,
                session,
                item,
                kind.as_str().to_owned(),
                level_str(level),
            )
            .await,
        )
    }

    async fn notebook_ask(
        &self,
        ext: &Extensions,
        scope: Vec<document::DocId>,
        question: String,
        save_into: Option<document::DocId>,
    ) -> RpcResult<agent::ask::AskStarted> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::ask::notebook_ask(
                &agents,
                scope.iter().map(ToString::to_string).collect(),
                question,
                save_into.map(|n| n.to_string()),
            )
            .await,
        )
    }

    async fn notebook_context_clear(
        &self,
        ext: &Extensions,
        session: SessionId,
        item: String,
    ) -> RpcResult<()> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(agent::notebook_context::context_clear(&agents, session, item).await)
    }

    async fn notebook_context_set_all(
        &self,
        ext: &Extensions,
        session: SessionId,
        notebook: document::DocId,
        kind: Option<document::ItemKind>,
        level: document::ContextLevel,
    ) -> RpcResult<()> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::notebook_context::context_set_all(
                &agents,
                session,
                notebook.to_string(),
                kind.map(|k| k.as_str().to_owned()),
                level_str(level),
            )
            .await,
        )
    }

    async fn notebook_context_estimate(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
        session: Option<SessionId>,
    ) -> RpcResult<document::ContextEstimate> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        let est = nb_result(
            agent::notebook_context::estimate(&agents, session, &notebook.to_string(), &self.rt)
                .await,
        )?;
        // The agent's estimate serialises to the contract's shape (string kinds, lowercase levels).
        serde_json::to_value(&est)
            .and_then(serde_json::from_value)
            .map_err(|e| ErrorObjectOwned::owned(ASK_FAILED, e.to_string(), None::<()>))
    }

    async fn insight_run(
        &self,
        ext: &Extensions,
        source: document::DocId,
        instruction: document::InsightInstruction,
    ) -> RpcResult<document::InsightRun> {
        owner(ext)?;
        let r = nb_result(
            document::ingest::insights::insight_run(
                &documents()?,
                source.clone(),
                instruction.clone(),
                None,
            )
            .await,
        );
        doc_forward(ext, "insight_run", (&source, &instruction), r).await
    }

    async fn insight_promote(
        &self,
        ext: &Extensions,
        source: document::DocId,
        block: String,
        notebook: Option<document::DocId>,
    ) -> RpcResult<document::DocId> {
        owner(ext)?;
        let r = nb_result(
            document::ingest::insights::insight_promote(
                &documents()?,
                source.clone(),
                block.clone(),
                notebook.clone(),
                document::Principal::Owner { device: None },
            )
            .await,
        );
        doc_forward(ext, "insight_promote", (&source, &block, &notebook), r).await
    }

    async fn notebook_ask_list(
        &self,
        ext: &Extensions,
        limit: Option<u32>,
    ) -> RpcResult<Vec<agent::ask::AskSummary>> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(agent::ask::notebook_ask_list(&agents, limit).await)
    }

    async fn notebook_overview_run(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
        kind: agent::overviews::OverviewKind,
    ) -> RpcResult<agent::overviews::OverviewRun> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(
            agent::overviews::notebook_overview_run(&agents, notebook.to_string(), kind).await,
        )
    }

    async fn notebook_overviews(
        &self,
        ext: &Extensions,
        notebook: document::DocId,
    ) -> RpcResult<Vec<agent::overviews::Overview>> {
        owner(ext)?;
        let agents = lookup::<Agents>(agent::AGENT_ACTOR)?;
        nb_result(agent::overviews::notebook_overviews(&agents, notebook.to_string()).await)
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    AgentNotebooksApiServer::into_rpc(server)
}
