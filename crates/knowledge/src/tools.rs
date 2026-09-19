//! The `knowledge_*` tools (design doc 10 §6), registered by
//! `KnowledgePlugin::start` once the tool registry is up (12 §9).

use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::registry::ToolRegistry;
use tools::{RegisterPrompt, RegisterTool, Risk, Tool, ToolCx, ToolOutput};

use crate::store::Knowledge;
use crate::types::{Meta, Put, Source};

/// Name of [`GetTool`]; the loop counts its calls for harvest cadence (10 §5.1).
pub const GET: &str = "knowledge_get";

/// The plugin's system-prompt block (07 §4 part 7, 10 §4.3): when to reach for
/// the knowledge base. Static, so it never busts the system cache.
pub const PROMPT: &str = "Durable facts about the user's machines, projects, accounts and decisions belong in the knowledge base: \
check <knowledge> hints and knowledge_search before assuming, record with knowledge_put.";

/// Register every `knowledge_*` tool and the prompt block on `reg`.
pub async fn register(reg: &ActorRef<ToolRegistry>, knowledge: Arc<Knowledge>) {
    let _ = reg
        .ask(RegisterPrompt {
            plugin: "knowledge",
            text: PROMPT.to_owned(),
        })
        .await;
    let k = &knowledge;
    let _ = reg.ask(RegisterTool(Arc::new(SearchTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(GetTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ListTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(PutTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(DeleteTool(k.clone())))).await;
    let _ = reg
        .ask(RegisterTool(Arc::new(HistoryTool(k.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(RestoreTool(k.clone()))))
        .await;
}

/// Text for the transcript, the same data as a Python value for the runtime.
fn structured<T: Serialize>(text: String, data: T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

fn cwd() -> PathBuf {
    std::env::current_dir().unwrap_or_default()
}

/// 51 §5: a private session leaves no trace in the durable bundle.
fn writable(cx: &ToolCx) -> anyhow::Result<()> {
    anyhow::ensure!(
        !cx.resources.is_private(),
        "knowledge mutations are disabled in a private session"
    );
    Ok(())
}

fn meta_line(s: &mut String, m: &Meta) {
    s.push_str(&format!("`{}` — {} [{}]", m.id, m.title, m.type_));
    if !m.description.is_empty() {
        s.push_str(": ");
        s.push_str(&m.description);
    }
}

/// Arguments for [`SearchTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SearchArgs {
    /// Free-text query; words are stemmed and OR-ed.
    pub query: String,
    /// Max hits (default 10, max 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `knowledge_search`: full-text search over the bundle.
pub struct SearchTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for SearchTool {
    const NAME: &'static str = "knowledge_search";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    type Args = SearchArgs;

    fn description(&self) -> String {
        "Search the knowledge base (durable facts about the user's environment, decisions, procedures, \
         entities) by keywords. Returns ids to pass to knowledge_get."
            .into()
    }

    async fn call(&self, a: SearchArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let limit = a.limit.unwrap_or(10).clamp(1, 50);
        let hits = self.0.search(&a.query, &cwd(), limit).await?;
        let mut s = String::new();
        let mut rows = Vec::with_capacity(hits.len());
        for h in &hits {
            s.push_str(&format!(
                "`{}` — {} [{}] ({:.1}): {}\n",
                h.meta.id, h.meta.title, h.meta.type_, h.score, h.snippet
            ));
            // Frontmatter fields plus score and snippet, flat.
            let mut row = serde_json::to_value(&h.meta)?;
            row["score"] = h.score.into();
            row["snippet"] = h.snippet.as_str().into();
            rows.push(row);
        }
        if s.is_empty() {
            s.push_str("no matches");
        }
        Ok(structured(s, rows))
    }
}

/// Arguments naming one concept.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct IdArgs {
    /// Concept id (`dir/slug`).
    pub id: String,
}

/// `knowledge_get`: one concept verbatim with its links and backlinks.
pub struct GetTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for GetTool {
    const NAME: &'static str = GET;
    const RISK: Risk = Risk::ReadOnly;
    type Args = IdArgs;

    fn description(&self) -> String {
        "Read a knowledge concept by id: the file verbatim (frontmatter incl. source/timestamp, body), \
         its links (→ missing when the target does not exist) and the concepts citing it."
            .into()
    }

    async fn call(&self, a: IdArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let c = self.0.get(&a.id).await?;
        let mut s = c.text;
        if !c.links.is_empty() {
            s.push_str("\nLinks: ");
            let links: Vec<String> = c
                .links
                .iter()
                .map(|(t, ok)| {
                    if *ok {
                        t.clone()
                    } else {
                        format!("{t} → missing")
                    }
                })
                .collect();
            s.push_str(&links.join(", "));
            s.push('\n');
        }
        if !c.cited_by.is_empty() {
            s.push_str("Cited by: ");
            s.push_str(&c.cited_by.join(", "));
            s.push('\n');
        }
        Ok(ToolOutput::ok(s))
    }
}

/// Arguments for [`ListTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ListArgs {
    /// Id prefix (`facts/`).
    #[serde(default)]
    pub prefix: Option<String>,
    /// Frontmatter `type` (fact | entity | decision | procedure | project).
    #[serde(default, rename = "type")]
    pub type_: Option<String>,
    /// A tag every result must carry.
    #[serde(default)]
    pub tag: Option<String>,
    /// Max lines (default 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `knowledge_list`: concepts by id prefix, type and tag.
pub struct ListTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for ListTool {
    const NAME: &'static str = "knowledge_list";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ListArgs;

    fn description(&self) -> String {
        "List knowledge concepts sorted by id, optionally filtered by id prefix, type and tag."
            .into()
    }

    async fn call(&self, a: ListArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let metas = self
            .0
            .list(
                a.prefix.as_deref(),
                a.type_.as_deref(),
                a.tag.as_deref(),
                a.limit.unwrap_or(50),
            )
            .await?;
        let mut s = String::new();
        for m in &metas {
            meta_line(&mut s, m);
            s.push('\n');
        }
        if s.is_empty() {
            s.push_str("no concepts");
        }
        Ok(structured(s, &metas))
    }
}

/// `knowledge_put`: create or update a concept.
pub struct PutTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for PutTool {
    const NAME: &'static str = "knowledge_put";
    // ReadOnly on purpose (10 §6): a validated file in the harness's own bundle,
    // committed and restorable. Asking on every fact would stop the model recording.
    const RISK: Risk = Risk::ReadOnly;
    type Args = Put;

    fn description(&self) -> String {
        "Record durable knowledge across sessions: facts about the user's environment (hosts, paths, \
         toolchains, accounts, conventions), decisions and their reasons, procedures that worked, and \
         entities (projects, services, people) belong here. Put on an existing id updates it — check \
         knowledge_search first and reuse the id rather than adding a near-duplicate. Ids are lowercase \
         `dir/slug`; include the evidence (command, path, error, the user's words) in the body and link \
         related concepts as [title](/other/id.md)."
            .into()
    }

    async fn call(&self, p: Put, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        writable(&cx)?;
        let (meta, created) = self.0.put(p, Source::Model).await?;
        let how = if created { "new" } else { "updated" };
        Ok(ToolOutput::ok(format!("put {} ({how})", meta.id)))
    }
}

/// `knowledge_delete`: remove a concept (recoverable via `knowledge_restore`).
pub struct DeleteTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for DeleteTool {
    const NAME: &'static str = "knowledge_delete";
    const RISK: Risk = Risk::Destructive;
    type Args = IdArgs;

    fn description(&self) -> String {
        "Delete a knowledge concept by id. History keeps it; knowledge_restore brings it back."
            .into()
    }

    async fn call(&self, a: IdArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        writable(&cx)?;
        self.0.delete(&a.id).await?;
        Ok(ToolOutput::ok(format!("deleted {}", a.id)))
    }
}

/// Arguments for [`HistoryTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct HistoryArgs {
    /// Concept id; omit for the whole bundle's history.
    #[serde(default)]
    pub id: Option<String>,
    /// Max commits (default 20).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `knowledge_history`: commits touching a concept (or the bundle), with the latest diff.
pub struct HistoryTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for HistoryTool {
    const NAME: &'static str = "knowledge_history";
    const RETURNS: &'static str = "dict";
    const RISK: Risk = Risk::ReadOnly;
    type Args = HistoryArgs;

    fn description(&self) -> String {
        "Commits that changed a knowledge concept (or the whole bundle when id is omitted), newest first: \
         `commit  time  message`. With an id, the unified diff of its latest change follows."
            .into()
    }

    async fn call(&self, a: HistoryArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let h = self
            .0
            .history(a.id.as_deref(), a.limit.unwrap_or(20))
            .await?;
        let mut s = String::new();
        for c in &h.changes {
            s.push_str(&format!("{}  {}  {}\n", c.commit, c.time, c.message));
        }
        if s.is_empty() {
            s.push_str("no history");
        }
        if let Some(diff) = &h.diff {
            s.push('\n');
            s.push_str(diff);
        }
        Ok(structured(s, &h))
    }
}

/// Arguments for [`RestoreTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RestoreArgs {
    /// Concept id.
    pub id: String,
    /// Abbreviated commit to restore from (default: the commit before the latest change).
    #[serde(default)]
    pub commit: Option<String>,
}

/// `knowledge_restore`: rewrite a concept from an earlier commit.
pub struct RestoreTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for RestoreTool {
    const NAME: &'static str = "knowledge_restore";
    const RISK: Risk = Risk::Mutating;
    type Args = RestoreArgs;

    fn description(&self) -> String {
        "Restore a knowledge concept to its content at an earlier commit (see knowledge_history); \
         also undoes a delete. Default: the commit before the latest change."
            .into()
    }

    async fn call(&self, a: RestoreArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        writable(&cx)?;
        let c = self.0.restore(&a.id, a.commit.as_deref()).await?;
        Ok(ToolOutput::ok(format!(
            "restored {} from {}",
            a.id, c.commit
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_names_match_doc() {
        assert_eq!(
            [
                SearchTool::NAME,
                GetTool::NAME,
                ListTool::NAME,
                PutTool::NAME,
                DeleteTool::NAME,
                HistoryTool::NAME,
                RestoreTool::NAME,
            ],
            [
                "knowledge_search",
                "knowledge_get",
                "knowledge_list",
                "knowledge_put",
                "knowledge_delete",
                "knowledge_history",
                "knowledge_restore",
            ]
        );
        assert_eq!(PutTool::RISK, Risk::ReadOnly);
        assert_eq!(DeleteTool::RISK, Risk::Destructive);
        assert_eq!(RestoreTool::RISK, Risk::Mutating);
    }

    fn cx(private: bool) -> ToolCx {
        let dir = std::env::temp_dir();
        ToolCx {
            session: runtime::SessionId::new(),
            call_id: String::new(),
            quiet: false,
            resources: Arc::new(tools::SessionResources {
                root: dir.clone(),
                retention: if private {
                    tools::Retention::Private
                } else {
                    tools::Retention::Durable
                },
                generation: 1,
            }),
        }
    }

    fn put() -> Put {
        Put {
            id: "facts/x".into(),
            type_: "fact".into(),
            title: "X".into(),
            description: None,
            tags: Vec::new(),
            scope: None,
            body: "body".into(),
        }
    }

    #[tokio::test]
    async fn private_session_refuses_mutations_durable_writes() {
        let dir = std::env::temp_dir().join(format!("wth-ktools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let k = Knowledge::open(&dir, rt).await.unwrap();
        let msg = "knowledge mutations are disabled in a private session";
        let e = PutTool(k.clone()).call(put(), cx(true)).await.unwrap_err();
        assert_eq!(e.to_string(), msg);
        let id = IdArgs { id: "facts/x".into() };
        let e = DeleteTool(k.clone()).call(id, cx(true)).await.unwrap_err();
        assert_eq!(e.to_string(), msg);
        let restore = RestoreArgs {
            id: "facts/x".into(),
            commit: None,
        };
        let e = RestoreTool(k.clone())
            .call(restore, cx(true))
            .await
            .unwrap_err();
        assert_eq!(e.to_string(), msg);
        assert!(k.get("facts/x").await.is_err());
        PutTool(k.clone()).call(put(), cx(false)).await.unwrap();
        assert!(k.get("facts/x").await.is_ok());
    }
}
