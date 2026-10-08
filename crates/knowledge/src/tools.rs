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
use crate::types::{Meta, Put, Source, Visibility};

/// Name of [`GetTool`]; the loop counts its calls for harvest cadence (10 §5.1).
pub const GET: &str = "knowledge_get";

/// The plugin's system-prompt block (07 §4 part 7, 10 §4.3): when to reach for
/// the knowledge base. Static, so it never busts the system cache.
pub const PROMPT: &str = "Durable facts about the user's machines, projects, accounts and decisions belong in the knowledge base: \
check <knowledge> hints and `knowledge_search()` before assuming, record with `knowledge_put()` (both python).";

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
    let _ = reg.ask(RegisterTool(Arc::new(ShareTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(DeleteTool(k.clone())))).await;
    let _ = reg
        .ask(RegisterTool(Arc::new(HistoryTool(k.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(RestoreTool(k.clone()))))
        .await;
    // 136 §2.1: usage counters and the five consolidation ops (bodies W1's).
    let _ = reg.ask(RegisterTool(Arc::new(StatsTool(k.clone())))).await;
    let _ = reg.ask(RegisterTool(Arc::new(MergeTool(k.clone())))).await;
    let _ = reg
        .ask(RegisterTool(Arc::new(SupersedeTool(k.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(ArchiveTool(k.clone()))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(VerifyTool(k.clone())))).await;
    let _ = reg
        .ask(RegisterTool(Arc::new(RetitleTool(k.clone()))))
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

/// 136 §2.1: the five consolidation tools run only from the dreaming
/// routine's session or the owner's own root session (plus [`writable`]).
async fn consolidating(k: &Knowledge, name: &str, cx: &ToolCx) -> anyhow::Result<()> {
    writable(cx)?;
    anyhow::ensure!(
        k.may_consolidate(cx.session).await,
        "{name} runs only in the dreaming routine's session or the owner's own root session (136 §2.1)"
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

    async fn call(&self, a: SearchArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let limit = a.limit.unwrap_or(10).clamp(1, 50);
        let hits = self
            .0
            .search_for(&a.query, &cwd(), limit, Some(cx.session))
            .await?;
        let mut s = String::new();
        let mut rows = Vec::with_capacity(hits.len());
        for h in &hits {
            // 136 §3.1: archived (and other non-live) items stay searchable, marked.
            let mark = if h.meta.status.is_live() {
                String::new()
            } else {
                format!(" [{}]", h.meta.status.as_str())
            };
            s.push_str(&format!(
                "`{}` — {} [{}]{mark} ({:.1}): {}\n",
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
    /// Concept id (`dir/slug`); `dir/slug@<commit>` reads a past revision (read-only).
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
         its links (→ missing when the target does not exist) and the concepts citing it. \
         `<id>@<commit>` (as `search` prints it) reads that past revision, read-only."
            .into()
    }

    async fn call(&self, a: IdArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if let Some((id, commit)) = a.id.split_once('@') {
            let c = self.0.read_revision(id, commit).await?;
            let date = c.time.get(..10).unwrap_or(&c.time);
            return Ok(ToolOutput::ok(format!(
                "revision {} of {date} (read-only; latest: knowledge_get(\"{}\"))\n{}",
                &c.commit[..7.min(c.commit.len())],
                c.id,
                c.text
            )));
        }
        let c = self.0.get(&a.id).await?;
        // 136 §5.3: every read is a vote for the hint shelf.
        if let Err(e) = self.0.record_opened(&c.meta.id, chrono::Utc::now()).await {
            tracing::debug!(id = %c.meta.id, "record_opened: {e:#}");
        }
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

    async fn call(&self, a: ListArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let metas = self
            .0
            .list_for(
                a.prefix.as_deref(),
                a.type_.as_deref(),
                a.tag.as_deref(),
                a.limit.unwrap_or(50),
                Some(cx.session),
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
        let (meta, created) = self.0.put_for(p, Source::Model, Some(cx.session)).await?;
        let how = if created { "new" } else { "updated" };
        Ok(ToolOutput::ok(format!("put {} ({how})", meta.id)))
    }
}

/// Arguments for [`ShareTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ShareArgs {
    /// Concept id.
    pub id: String,
    /// `private` (owner only) | `team` (owner's reporting line) | `shared` (everyone).
    pub visibility: Visibility,
}

/// `knowledge_share`: change who may read a concept (67 §9).
pub struct ShareTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for ShareTool {
    const NAME: &'static str = "knowledge_share";
    // ReadOnly like `knowledge_put`: the closing turn of a `read_only` desk must be able to share.
    const RISK: Risk = Risk::ReadOnly;
    type Args = ShareArgs;

    fn description(&self) -> String {
        "Change who may read a knowledge concept: `private` (you alone), `team` (you, your managers and the \
         agents reporting to you) or `shared` (everyone). Concepts you record are private by default; share \
         what your team or everyone should know. Only the owner or someone in its reporting line may change it."
            .into()
    }

    async fn call(&self, a: ShareArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        writable(&cx)?;
        let meta = self.0.share(&a.id, a.visibility, Some(cx.session)).await?;
        Ok(ToolOutput::ok(format!(
            "{} is now {}",
            meta.id,
            meta.visibility.as_str()
        )))
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

/// Arguments for [`StatsTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct StatsArgs {
    /// Concept ids.
    pub ids: Vec<String>,
}

/// `knowledge_stats` (136 §2.1): impressions and opens per id.
pub struct StatsTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for StatsTool {
    const NAME: &'static str = "knowledge_stats";
    const RETURNS: &'static str = "list[dict]";
    const RISK: Risk = Risk::ReadOnly;
    type Args = StatsArgs;

    fn description(&self) -> String {
        "Usage counters of knowledge concepts: how often each id was shown in a <knowledge> hint and opened \
         with knowledge_get, with the last times. Unknown ids report zeros."
            .into()
    }

    async fn call(&self, a: StatsArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let uses = self.0.stats(&a.ids).await?;
        let mut s = String::new();
        for u in &uses {
            s.push_str(&format!(
                "`{}` shown {} (last {}) opened {} (last {})\n",
                u.id,
                u.shown,
                u.last_shown.as_deref().unwrap_or("never"),
                u.opened,
                u.last_opened.as_deref().unwrap_or("never")
            ));
        }
        if s.is_empty() {
            s.push_str("no ids");
        }
        Ok(structured(s, uses))
    }
}

/// Arguments for [`MergeTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MergeArgs {
    /// Ids folded into `into`; each becomes a redirect stub.
    pub ids: Vec<String>,
    /// The canonical id (one of `ids` or a new one).
    pub into: String,
    /// The canonical's body.
    pub body: String,
    /// Session ids (or `user`) to add to the canonical's `evidence`.
    #[serde(default)]
    pub evidence: Vec<String>,
}

/// `knowledge_merge` (136 §2.1): fold near-duplicates into one living entry.
pub struct MergeTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for MergeTool {
    const NAME: &'static str = "knowledge_merge";
    const RISK: Risk = Risk::Mutating;
    type Args = MergeArgs;

    fn description(&self) -> String {
        "Dreaming only: merge several knowledge concepts into one canonical entry (`into`, with `body`); \
         the folded ids become `status: merged` redirect stubs, evidence and tags are unioned, links retargeted."
            .into()
    }

    async fn call(&self, a: MergeArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        consolidating(&self.0, Self::NAME, &cx).await?;
        // The session's agent scope (67 §9); the user's when it has none.
        let scope =
            self.0
                .scope_for(Some(cx.session))
                .unwrap_or_else(|| crate::types::AgentScope {
                    owner: crate::types::USER_OWNER.to_owned(),
                    line: Default::default(),
                });
        let r = self
            .0
            .merge(&a.ids, &a.into, &a.body, &a.evidence, &scope)
            .await?;
        Ok(structured(
            format!("merged {} into {}", r.ids[1..].join(", "), r.ids[0]),
            r,
        ))
    }
}

/// Arguments for [`SupersedeTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SupersedeArgs {
    /// The outdated concept.
    pub loser: String,
    /// The concept that replaces it.
    pub winner: String,
    /// RFC 3339: when the loser stopped being true (from the evidence, not now).
    #[serde(default)]
    pub valid_until: Option<String>,
    /// One line for the loser's body.
    #[serde(default)]
    pub note: Option<String>,
}

/// `knowledge_supersede` (136 §2.1): a newer claim replaces an older one.
pub struct SupersedeTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for SupersedeTool {
    const NAME: &'static str = "knowledge_supersede";
    const RISK: Risk = Risk::Mutating;
    type Args = SupersedeArgs;

    fn description(&self) -> String {
        "Dreaming only: mark `loser` as superseded by `winner` (`superseded_by`, `valid_until`, `hint: false`); \
         the loser stays searchable and restorable."
            .into()
    }

    async fn call(&self, a: SupersedeArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        consolidating(&self.0, Self::NAME, &cx).await?;
        let r = self
            .0
            .supersede(
                &a.loser,
                &a.winner,
                a.valid_until.as_deref(),
                a.note.as_deref(),
            )
            .await?;
        Ok(structured(
            format!("{} superseded by {}", a.loser, a.winner),
            r,
        ))
    }
}

/// Arguments for [`ArchiveTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveArgs {
    /// Concept id.
    pub id: String,
    /// Why it leaves the hint shelf; appended under `## Archived`.
    pub reason: String,
}

/// `knowledge_archive` (136 §2.1): off the hint shelf, still searchable.
pub struct ArchiveTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for ArchiveTool {
    const NAME: &'static str = "knowledge_archive";
    const RISK: Risk = Risk::Mutating;
    type Args = ArchiveArgs;

    fn description(&self) -> String {
        "Dreaming only: archive a knowledge concept (`status: archived, hint: false`) with a reason; \
         never deleted — still searchable and knowledge_restore-able."
            .into()
    }

    async fn call(&self, a: ArchiveArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        consolidating(&self.0, Self::NAME, &cx).await?;
        let r = self.0.archive(&a.id, &a.reason).await?;
        Ok(structured(format!("archived {}", a.id), r))
    }
}

/// Arguments for [`VerifyTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct VerifyArgs {
    /// Concept id.
    pub id: String,
    /// `ok` (the claim still holds) or `stale` (a read-tool check failed).
    pub outcome: crate::types::VerifyOutcome,
    /// What was checked, or what changed.
    #[serde(default)]
    pub note: Option<String>,
}

/// `knowledge_verify` (136 §2.1): record a read-tool check's verdict.
pub struct VerifyTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for VerifyTool {
    const NAME: &'static str = "knowledge_verify";
    const RISK: Risk = Risk::Mutating;
    type Args = VerifyArgs;

    fn description(&self) -> String {
        "Dreaming only: record the outcome of checking a knowledge concept against the world with read tools: \
         `ok` sets `verified: <today>`, `stale` sets `status: stale` and keeps the note. Never a model's opinion."
            .into()
    }

    async fn call(&self, a: VerifyArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        consolidating(&self.0, Self::NAME, &cx).await?;
        let r = self.0.verify(&a.id, a.outcome, a.note.as_deref()).await?;
        Ok(structured(format!("verified {}: {:?}", a.id, a.outcome), r))
    }
}

/// Arguments for [`RetitleTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RetitleArgs {
    /// Concept id.
    pub id: String,
    /// New title, ≤ 120 chars — the first clause of the old one.
    pub title: String,
    /// Paragraph prepended to the body so nothing of the old title is lost.
    pub summary: String,
}

/// `knowledge_retitle` (136 §2.1): a readable hint line.
pub struct RetitleTool(pub Arc<Knowledge>);

#[async_trait::async_trait]
impl Tool for RetitleTool {
    const NAME: &'static str = "knowledge_retitle";
    const RISK: Risk = Risk::Mutating;
    type Args = RetitleArgs;

    fn description(&self) -> String {
        "Dreaming only: shorten a knowledge concept's title (≤ 120 chars) and prepend the rest of the old \
         title to the body as a Summary paragraph."
            .into()
    }

    async fn call(&self, a: RetitleArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        consolidating(&self.0, Self::NAME, &cx).await?;
        let r = self.0.retitle(&a.id, &a.title, &a.summary).await?;
        Ok(structured(format!("retitled {}", a.id), r))
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
        // 136 §2.1
        assert_eq!(
            [
                StatsTool::NAME,
                MergeTool::NAME,
                SupersedeTool::NAME,
                ArchiveTool::NAME,
                VerifyTool::NAME,
                RetitleTool::NAME,
            ],
            [
                "knowledge_stats",
                "knowledge_merge",
                "knowledge_supersede",
                "knowledge_archive",
                "knowledge_verify",
                "knowledge_retitle",
            ]
        );
        assert_eq!(StatsTool::RISK, Risk::ReadOnly);
        for r in [
            MergeTool::RISK,
            SupersedeTool::RISK,
            ArchiveTool::RISK,
            VerifyTool::RISK,
            RetitleTool::RISK,
        ] {
            assert_eq!(r, Risk::Mutating);
        }
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
            visibility: None,
            ..Put::default()
        }
    }

    #[tokio::test]
    async fn private_session_refuses_mutations_durable_writes() {
        let dir = std::env::temp_dir().join(format!("theuth-ktools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let k = Knowledge::open(&dir, rt).await.unwrap();
        let msg = "knowledge mutations are disabled in a private session";
        let e = PutTool(k.clone()).call(put(), cx(true)).await.unwrap_err();
        assert_eq!(e.to_string(), msg);
        let id = IdArgs {
            id: "facts/x".into(),
        };
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

    /// 136 §3.1: `knowledge_search` keeps archived items and marks them;
    /// `knowledge_get` records an open.
    #[tokio::test]
    async fn search_marks_archived_and_get_records_an_open() {
        let dir = std::env::temp_dir().join(format!("theuth-ktools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let k = Knowledge::open(&dir, rt).await.unwrap();
        k.put(put(), Source::Model).await.unwrap();
        k.put(
            Put {
                id: "facts/y".into(),
                status: Some(crate::types::Status::Archived),
                ..put()
            },
            Source::Model,
        )
        .await
        .unwrap();
        let out = SearchTool(k.clone())
            .call(
                SearchArgs {
                    query: "body".into(),
                    limit: None,
                },
                cx(false),
            )
            .await
            .unwrap();
        let lines: Vec<&str> = out.content.lines().collect();
        assert_eq!(lines.len(), 2);
        let y = lines.iter().find(|l| l.starts_with("`facts/y`")).unwrap();
        assert!(y.contains("[fact] [archived]"), "{y}");
        let x = lines.iter().find(|l| l.starts_with("`facts/x`")).unwrap();
        assert!(x.contains("[fact] (") && !x.contains("[archived]"), "{x}");

        GetTool(k.clone())
            .call(
                IdArgs {
                    id: "facts/x".into(),
                },
                cx(false),
            )
            .await
            .unwrap();
        let s = k.stats(&["facts/x".into()]).await.unwrap();
        assert_eq!((s[0].shown, s[0].opened), (0, 1));
    }

    /// Says yes to exactly one session.
    struct OneSession(runtime::SessionId);

    #[async_trait::async_trait]
    impl crate::Consolidator for OneSession {
        async fn may_consolidate(&self, s: runtime::SessionId) -> bool {
            s == self.0
        }
    }

    /// 136 §2.1: the five consolidation tools refuse without a gate, refuse a
    /// session the gate rejects, and reach the store (which wants a file) for one it
    /// allows; `knowledge_stats` answers anywhere.
    #[tokio::test]
    async fn consolidation_tools_need_owner_root_or_unattended() {
        let dir = std::env::temp_dir().join(format!("theuth-ktools-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let rt = runtime::Runtime::new(figment::Figment::new(), dir.clone());
        let k = Knowledge::open(&dir, rt).await.unwrap();
        let archive = || ArchiveArgs {
            id: "facts/x".into(),
            reason: "old".into(),
        };
        // No gate installed: nobody may.
        let e = ArchiveTool(k.clone())
            .call(archive(), cx(false))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("knowledge_archive runs only"), "{e}");
        // Private sessions are refused first.
        let e = ArchiveTool(k.clone())
            .call(archive(), cx(true))
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("private session"), "{e}");
        // A gate: the allowed session reaches the store; another is refused.
        let allowed = cx(false);
        k.set_consolidator(Arc::new(OneSession(allowed.session)));
        let e = ArchiveTool(k.clone())
            .call(archive(), allowed.clone())
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("no such concept: facts/x"), "{e}");
        let e = VerifyTool(k.clone())
            .call(
                VerifyArgs {
                    id: "facts/x".into(),
                    outcome: crate::types::VerifyOutcome::Ok,
                    note: None,
                },
                cx(false),
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(e.contains("knowledge_verify runs only"), "{e}");
        // Stats is read-only and real.
        let out = StatsTool(k.clone())
            .call(
                StatsArgs {
                    ids: vec!["facts/x".into()],
                },
                cx(true),
            )
            .await
            .unwrap();
        assert!(out.content.contains("shown 0"), "{}", out.content);
        assert_eq!(out.structured.unwrap()[0]["opened"], 0);
    }
}
