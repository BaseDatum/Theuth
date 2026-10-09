//! The wave-1 notebook tools (design 144 §13.2): `notebook_create/list/get/add/remove`,
//! `source_add/list/get/refresh`, `nb_search`. Registered by `DocumentPlugin::start` beside the
//! `doc_*` tools; each body delegates to the owning module — the `notebook` actor messages (144-C),
//! `ingest` (W1), `index` (W2). Reads are `ReadOnly`, writes `Mutating`; writes are hub-only and answer
//! `NotHub` off the hub like every document write.

use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterTool, Risk, Tool, ToolCx, ToolOutput};

use crate::actor::DocumentActor;
use crate::index::{self, NbSearchArgs};
use crate::ingest::{self, SourceAddArgs};
use crate::meta::{DocId, Principal};
use crate::notebook::{
    InsightInstruction, ItemKind, NotebookCreate, NotebookGet, NotebookItemAdd, NotebookItemRemove,
    NotebookList,
};

/// What the tools hold.
#[derive(Clone)]
pub struct Notebooks {
    actor: ActorRef<DocumentActor>,
}

/// Register the ten wave-1 tools and the two wave-2 insight tools on `reg`.
pub async fn register(reg: &ActorRef<ToolRegistry>, actor: ActorRef<DocumentActor>) {
    let n = Notebooks { actor };
    let _ = reg
        .ask(RegisterTool(Arc::new(NotebookCreateTool(n.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(NotebookListTool(n.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(NotebookGetTool(n.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(NotebookAddTool(n.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(NotebookRemoveTool(n.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SourceAddTool(n.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SourceListTool(n.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SourceGetTool(n.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SourceRefreshTool(n.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(NbSearchTool(n.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(InsightRunTool(n.clone()))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(InsightPromoteTool(n)))).await;
}

fn agent(cx: &ToolCx) -> Principal {
    Principal::Agent {
        session: cx.session.0,
    }
}

/// A `DocId`, with or without a `notebook:` / `source:` prefix.
fn doc_id(s: &str) -> anyhow::Result<DocId> {
    let bare = s
        .strip_prefix("notebook:")
        .or_else(|| s.strip_prefix("source:"))
        .unwrap_or(s);
    DocId::parse(bare).ok_or_else(|| anyhow::anyhow!("`{s}` is not a document id"))
}

/// Text for the transcript, the same data as a Python value for the runtime.
fn structured<T: Serialize>(text: String, data: &T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

/// A handler's `Err(String)` as the model's error (a `NotHub:` refusal included, verbatim).
fn handler<M, T>(r: Result<T, kameo::error::SendError<M, String>>) -> anyhow::Result<T>
where
    kameo::error::SendError<M, String>: std::fmt::Display,
{
    match r {
        Ok(v) => Ok(v),
        Err(kameo::error::SendError::HandlerError(e)) => Err(anyhow::anyhow!(e)),
        Err(e) => Err(anyhow::anyhow!(e.to_string())),
    }
}

/// Arguments for [`NotebookCreateTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct NotebookCreateArgs {
    /// The notebook's name.
    pub title: String,
    /// The description the model reads as a prompt block in every chat of the notebook, markdown.
    #[serde(default)]
    pub description: Option<String>,
    /// Optional `project` label (the one tasks and sessions carry).
    #[serde(default)]
    pub project: Option<String>,
}

/// `notebook_create` (Mutating): → `{ notebook, url: theuth://notebooks/<id> }`.
pub struct NotebookCreateTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for NotebookCreateTool {
    const NAME: &'static str = "notebook_create";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = NotebookCreateArgs;

    fn description(&self) -> String {
        "Create a notebook: a container over notes, sources (files, URLs, meetings, pasted text, past chats, \
         Confluence/Jira pages, YouTube, audio, video) and chats. `description` is what the model reads about it. \
         Returns {notebook, url}; pass `notebook` to notebook_add / source_add / doc_create(notebook=…)."
            .into()
    }

    async fn call(&self, a: NotebookCreateArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        anyhow::ensure!(!a.title.trim().is_empty(), "title must not be empty");
        let nb = handler(
            self.0
                .actor
                .ask(NotebookCreate {
                    title: a.title.trim().to_owned(),
                    description: a.description.unwrap_or_default(),
                    project: a.project.filter(|p| !p.trim().is_empty()),
                    by: agent(&cx),
                })
                .await,
        )?;
        let url = format!("theuth://notebooks/{}", nb.doc.id);
        Ok(structured(
            format!("created notebook {} ({url})\n", nb.doc.id),
            &serde_json::json!({ "notebook": nb, "url": url }),
        ))
    }
}

/// Arguments for [`NotebookListTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct NotebookListArgs {
    /// Include archived notebooks (default false).
    #[serde(default)]
    pub archived: bool,
}

/// `notebook_list` (ReadOnly): notebooks with counts, newest first.
pub struct NotebookListTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for NotebookListTool {
    const NAME: &'static str = "notebook_list";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = NotebookListArgs;

    fn description(&self) -> String {
        "List notebooks (id, title, project, counts of sources, notes and chats), newest first; \
         `archived=True` includes archived ones."
            .into()
    }

    async fn call(&self, a: NotebookListArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let list = handler(
            self.0
                .actor
                .ask(NotebookList {
                    archived: a.archived,
                })
                .await,
        )?;
        let mut text = format!("{} notebooks\n", list.len());
        for n in &list {
            text.push_str(&format!(
                "- {} {:?} · {} sources · {} notes · {} chats{}{}\n",
                n.doc.id,
                n.doc.title,
                n.sources,
                n.notes,
                n.chats,
                n.project
                    .as_deref()
                    .map_or(String::new(), |p| format!(" · project {p}")),
                if n.archived_ms.is_some() {
                    " · archived"
                } else {
                    ""
                }
            ));
        }
        Ok(structured(text, &list))
    }
}

/// Arguments for [`NotebookGetTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct NotebookGetArgs {
    /// The notebook id.
    pub id: String,
}

/// `notebook_get` (ReadOnly): one notebook and its items.
pub struct NotebookGetTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for NotebookGetTool {
    const NAME: &'static str = "notebook_get";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = NotebookGetArgs;

    fn description(&self) -> String {
        "One notebook with its items (kind, item id, added when/by). Read the description with \
         doc_read(\"notebook:<id>\"); read a source's text with doc_read(\"source:<id>\")."
            .into()
    }

    async fn call(&self, a: NotebookGetArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let id = doc_id(&a.id)?;
        let d = handler(self.0.actor.ask(NotebookGet { id, by: agent(&cx) }).await)?;
        let mut text = format!(
            "notebook {} {:?} · {} sources · {} notes · {} chats\n",
            d.notebook.doc.id,
            d.notebook.doc.title,
            d.notebook.sources,
            d.notebook.notes,
            d.notebook.chats
        );
        for i in &d.items {
            text.push_str(&format!(
                "- {} {} (added by {})\n",
                i.kind, i.item, i.added_by
            ));
        }
        Ok(structured(text, &d))
    }
}

/// Arguments for [`NotebookAddTool`] and [`NotebookRemoveTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct NotebookItemArgs {
    /// The notebook id.
    pub notebook: String,
    /// `source` | `note` | `session` | `artifact`.
    pub kind: ItemKind,
    /// The item id: a document id for sources and notes, a session id, an artifact call id.
    pub item: String,
}

/// `notebook_add` (Mutating): one item row.
pub struct NotebookAddTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for NotebookAddTool {
    const NAME: &'static str = "notebook_add";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = NotebookItemArgs;

    fn description(&self) -> String {
        "Add an existing note, source, chat (session) or artifact to a notebook. A source is added to \
         another notebook this way (one document, several notebooks); new sources come from source_add."
            .into()
    }

    async fn call(&self, a: NotebookItemArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let notebook = doc_id(&a.notebook)?;
        let item = handler(
            self.0
                .actor
                .ask(NotebookItemAdd {
                    notebook,
                    kind: a.kind,
                    item: a.item,
                    by: agent(&cx),
                })
                .await,
        )?;
        Ok(structured(
            format!(
                "added {} {} to notebook {}\n",
                item.kind, item.item, item.notebook
            ),
            &item,
        ))
    }
}

/// `notebook_remove` (Mutating): soft removal; says "also in N" other notebooks.
pub struct NotebookRemoveTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for NotebookRemoveTool {
    const NAME: &'static str = "notebook_remove";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = NotebookItemArgs;

    fn description(&self) -> String {
        "Remove an item from a notebook. Nothing is deleted: a note stays a note, a source in no other \
         notebook becomes unfiled (Library › Sources)."
            .into()
    }

    async fn call(&self, a: NotebookItemArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let notebook = doc_id(&a.notebook)?;
        let r = handler(
            self.0
                .actor
                .ask(NotebookItemRemove {
                    notebook: notebook.clone(),
                    kind: a.kind,
                    item: a.item.clone(),
                    by: agent(&cx),
                })
                .await,
        )?;
        Ok(structured(
            format!(
                "removed {} {} from notebook {notebook}; also in {} other notebook{}\n",
                a.kind,
                a.item,
                r.also_in,
                if r.also_in == 1 { "" } else { "s" }
            ),
            &r,
        ))
    }
}

/// `source_add` (Mutating): pre-flight, then the job; answers at once with the card.
pub struct SourceAddTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for SourceAddTool {
    const NAME: &'static str = "source_add";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = SourceAddArgs;

    fn description(&self) -> String {
        "Add a source to a notebook from exactly one of: `path` (a file: PDF, document, audio, video), `url` \
         (a web page or YouTube), `text` (pasted), `meeting` (a meeting id), `session` (a past chat), `mcp` \
         ({server, tool, args}: a Confluence page or Jira issue). Returns the source card at once with \
         `status` (new → queued → running → completed | failed); read the text later with \
         doc_read(\"source:<id>\"); source_get polls the status."
            .into()
    }

    async fn call(&self, a: SourceAddArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let given = [
            a.path.is_some(),
            a.url.is_some(),
            a.text.is_some(),
            a.meeting.is_some(),
            a.session.is_some(),
            a.mcp.is_some(),
        ]
        .into_iter()
        .filter(|b| *b)
        .count();
        if given != 1 {
            return Ok(ToolOutput::err(
                "source_add takes exactly one of path, url, text, meeting, session or mcp",
            ));
        }
        let s = ingest::source_add(&self.0.actor, a, agent(&cx)).await?;
        Ok(structured(
            format!("source {} {:?} · {}\n", s.doc.id, s.doc.title, s.status),
            &s,
        ))
    }
}

/// Arguments for [`SourceListTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct SourceListArgs {
    /// Restrict to one notebook; default every source, unfiled ones included.
    #[serde(default)]
    pub notebook: Option<String>,
}

/// `source_list` (ReadOnly).
pub struct SourceListTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for SourceListTool {
    const NAME: &'static str = "source_list";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = SourceListArgs;

    fn description(&self) -> String {
        "Sources of a notebook (or all): id, title, origin, status, size. `doc_read(\"source:<id>\")` reads the text."
            .into()
    }

    async fn call(&self, a: SourceListArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let notebook = a.notebook.as_deref().map(doc_id).transpose()?;
        let list = ingest::source_list(&self.0.actor, notebook).await?;
        let mut text = format!("{} sources\n", list.len());
        for s in &list {
            text.push_str(&format!(
                "- {} {:?} · {} · {}\n",
                s.doc.id,
                s.doc.title,
                s.status,
                serde_json::to_value(&s.origin)
                    .ok()
                    .and_then(|v| v["kind"].as_str().map(str::to_owned))
                    .unwrap_or_default()
            ));
        }
        Ok(structured(text, &list))
    }
}

/// Arguments for [`SourceGetTool`] and [`SourceRefreshTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SourceIdArgs {
    /// The source id.
    pub id: String,
}

/// `source_get` (ReadOnly).
pub struct SourceGetTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for SourceGetTool {
    const NAME: &'static str = "source_get";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = SourceIdArgs;

    fn description(&self) -> String {
        "One source's card: origin, status (and error), provenance, size, insights count.".into()
    }

    async fn call(&self, a: SourceIdArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let s = ingest::source_get(&self.0.actor, doc_id(&a.id)?).await?;
        Ok(structured(
            format!("source {} {:?} · {}\n", s.doc.id, s.doc.title, s.status),
            &s,
        ))
    }
}

/// `source_refresh` (Mutating): re-fetch from the origin or the blob.
pub struct SourceRefreshTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for SourceRefreshTool {
    const NAME: &'static str = "source_refresh";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = SourceIdArgs;

    fn description(&self) -> String {
        "Re-fetch a source from its origin (URL, MCP call, YouTube) or re-extract its file; unchanged blocks \
         keep their ids. Refused while the job is queued or running."
            .into()
    }

    async fn call(&self, a: SourceIdArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let s = ingest::source_refresh(&self.0.actor, doc_id(&a.id)?, agent(&cx)).await?;
        Ok(structured(
            format!("source {} {:?} · {}\n", s.doc.id, s.doc.title, s.status),
            &s,
        ))
    }
}

/// `nb_search` (ReadOnly): hybrid block hits with ids and snippets.
pub struct NbSearchTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for NbSearchTool {
    const NAME: &'static str = "nb_search";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = NbSearchArgs;

    fn description(&self) -> String {
        "Search notes, sources, insights and notebook descriptions at block level (lexical + semantic). Each hit \
         carries the document, the block id, a snippet and the `theuth://` href to cite it with. Narrow with \
         `notebooks`, `docs` or `kinds`."
            .into()
    }

    async fn call(&self, a: NbSearchArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let hits = index::nb_search(&self.0.actor, a).await?;
        let mut text = format!("{} hits\n", hits.len());
        for h in &hits {
            text.push_str(&format!(
                "- {} {:?} #{}{}: {}\n",
                h.href,
                h.title,
                h.block,
                h.heading
                    .as_deref()
                    .map_or(String::new(), |x| format!(" ({x})")),
                h.r#match.snippet
            ));
        }
        Ok(structured(text, &hits))
    }
}

/// Arguments for [`InsightRunTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct InsightRunArgs {
    /// The source id.
    pub source: String,
    /// Exactly one of `builtin` (`key_insights` | `toc` | `open_questions`) or `skill` (a skill whose front
    /// matter has `notebook_insight: true`).
    pub instruction: InsightInstruction,
}

/// `insight_run` (Mutating): one more `###` section under the source's `## Insights`.
pub struct InsightRunTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for InsightRunTool {
    const NAME: &'static str = "insight_run";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = InsightRunArgs;

    fn description(&self) -> String {
        "Write one insight about a source (a model side call): a builtin — key_insights, toc, open_questions — \
         or a `notebook_insight` skill by name. Appended as `### <name> · <model> · <date>` under `## Insights`; \
         returns {block}: the heading's block id, citable as theuth://sources/<id>#<block>."
            .into()
    }

    async fn call(&self, a: InsightRunArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let r = ingest::insights::insight_run(
            &self.0.actor,
            doc_id(&a.source)?,
            a.instruction,
            Some(cx.session),
        )
        .await?;
        Ok(structured(
            format!("insight written: block {}\n", r.block),
            &r,
        ))
    }
}

/// Arguments for [`InsightPromoteTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct InsightPromoteArgs {
    /// The source id.
    pub source: String,
    /// The insight section's heading block (or any block inside it).
    pub block: String,
    /// The notebook to file the note in; omitted = every live notebook holding the source.
    #[serde(default)]
    pub notebook: Option<String>,
}

/// `insight_promote` (Mutating): an insight section becomes a note with a provenance line.
pub struct InsightPromoteTool(pub Notebooks);

#[async_trait::async_trait]
impl Tool for InsightPromoteTool {
    const NAME: &'static str = "insight_promote";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = InsightPromoteArgs;

    fn description(&self) -> String {
        "Promote an insight section of a source to a new note titled \"<insight> — <source>\", ending with \
         `*From [<source>](theuth://sources/<id>#<block>) · <model> · <date>*`, filed in `notebook` (default: every \
         notebook holding the source). Returns {doc, url}."
            .into()
    }

    async fn call(&self, a: InsightPromoteArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let notebook = a.notebook.as_deref().map(doc_id).transpose()?;
        let doc = ingest::insights::insight_promote(
            &self.0.actor,
            doc_id(&a.source)?,
            a.block,
            notebook,
            agent(&cx),
        )
        .await?;
        let url = format!("theuth://notes/{doc}");
        Ok(structured(
            format!("note {doc} created ({url})\n"),
            &serde_json::json!({ "doc": doc, "url": url }),
        ))
    }
}
