//! Design 223: a project is a first-class record — the replicated `projects` row (name, description,
//! status, icon, colour, created time, metadata) — while sessions, tasks, docs, notebooks and
//! meetings keep carrying the plain label. A label no row names is an *implicit* project, listed so
//! pre-223 data still shows. Deleting a project detaches: every store clears the label, then the
//! row goes; nothing else is deleted. Schedule files keep their `project:` key (v1), so a task a
//! schedule files later brings the label back as an implicit project.
//!
//! The agent crate cannot name the tasks or meeting crates (they depend on it), so each store that
//! carries the label is a [`LabelSource`]: sessions, docs and notebooks are built in, tasks and
//! meetings register theirs at start ([`register_label_source`]).

use std::collections::BTreeMap;
use std::sync::Arc;

use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use parking_lot::Mutex;
use rpc_core::{Server, stub_err};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

use crate::rpc_forward as forward;
use crate::store::Store;
use crate::store::projects::ProjectPatch;
use crate::{AGENT_ACTOR, Agents, DurableStore, ListSessions, SetSessionProject, project};

/// The values `Project::status` takes.
pub const STATUSES: [&str; 2] = ["active", "archived"];
/// Longest description in UTF-8 bytes (after trimming).
pub const DESCRIPTION_MAX: usize = 16 * 1024;
/// Most metadata entries.
pub const METADATA_MAX_ENTRIES: usize = 64;
/// Longest metadata key in UTF-8 bytes.
pub const METADATA_KEY_MAX: usize = 128;
/// Longest metadata value in UTF-8 bytes.
pub const METADATA_VALUE_MAX: usize = 4 * 1024;

/// One project as `projects_list` shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Project {
    /// The label sessions, tasks, docs, notebooks and meetings carry; the key.
    pub name: String,
    /// What the project is about.
    pub description: Option<String>,
    /// `active` | `archived`.
    pub status: String,
    /// Lucide icon name (allow-listed, `project::ICONS`), `None` for the client's default.
    pub icon: Option<String>,
    /// Palette key (allow-listed, `project::COLORS`), `None` for the client's default.
    pub color: Option<String>,
    /// When the row was created, unix ms; `None` for a pre-223 style row or an implicit project.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub created_ms: Option<i64>,
    /// Free string → string metadata (repo URL, default cwd, links …).
    pub metadata: BTreeMap<String, String>,
    /// No row: only the label exists, on sessions, tasks, docs, notebooks or meetings.
    pub implicit: bool,
}

impl Project {
    /// The project a bare label stands for.
    pub fn implicit(name: String) -> Self {
        Self {
            name,
            description: None,
            status: STATUSES[0].to_owned(),
            icon: None,
            color: None,
            created_ms: None,
            metadata: BTreeMap::new(),
            implicit: true,
        }
    }
}

/// What `project_delete` detached, per store. A store that failed counts what it achieved.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ProjectDeleted {
    /// Root sessions whose label was cleared.
    pub sessions: u32,
    /// Tasks whose label was cleared.
    pub tasks: u32,
    /// Notes and boards whose label was cleared.
    pub docs: u32,
    /// Notebooks whose label was cleared.
    pub notebooks: u32,
    /// Meetings whose label was cleared.
    pub meetings: u32,
}

impl ProjectDeleted {
    fn slot(&mut self, store: LabelStore) -> &mut u32 {
        match store {
            LabelStore::Sessions => &mut self.sessions,
            LabelStore::Tasks => &mut self.tasks,
            LabelStore::Docs => &mut self.docs,
            LabelStore::Notebooks => &mut self.notebooks,
            LabelStore::Meetings => &mut self.meetings,
        }
    }
}

/// Event: a project row changed (create, update, style, delete), here or on another machine of the
/// mesh. No body: reload with `projects_list` / `list_project_styles`.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct ProjectsChanged {}

impl wire::WireEvent for ProjectsChanged {
    const TOPIC: &'static str = "agent.projects";

    fn policy(json: &serde_json::Value) -> wire::EventPolicy {
        wire::EventPolicy {
            always_wanted: true,
            ..wire::EventPolicy::legacy_fields(json)
        }
    }
}

/// Publish [`ProjectsChanged`] for every peer batch the replica applies to `projects` (local writes
/// publish from their call). Ends with the feed.
pub(crate) fn watch(rt: Runtime, handle: &replica::ReplicaHandle) {
    let mut feed = handle.subscribe();
    let origin = handle.origin().clone();
    tokio::spawn(async move {
        use tokio::sync::broadcast::error::RecvError;
        loop {
            match feed.recv().await {
                Ok(batch) => {
                    if batch.origin != origin && batch.changes.iter().any(|c| c.table == "projects")
                    {
                        rt.publish(ProjectsChanged {}).await;
                    }
                }
                Err(RecvError::Lagged(_)) => rt.publish(ProjectsChanged {}).await,
                Err(RecvError::Closed) => return,
            }
        }
    });
}

/// `project_create` input.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectCreate {
    /// Project name: the label sessions, tasks and notes carry (1–128 bytes, case kept).
    pub name: String,
    /// What the project is about (markdown).
    #[serde(default)]
    pub description: Option<String>,
    /// Lucide icon name from the allow-list (e.g. `Folder`, `Code`, `Rocket`).
    #[serde(default)]
    pub icon: Option<String>,
    /// Palette key from the allow-list (e.g. `lapis`, `malachite`, `ochre`).
    #[serde(default)]
    pub color: Option<String>,
    /// String → string metadata (repo URL, default cwd, links …).
    #[serde(default)]
    pub metadata: Option<BTreeMap<String, String>>,
}

/// `project_update` input: an omitted field stays; `""` clears a description, icon or colour; a
/// metadata object replaces the whole map (`{}` clears it).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectUpdate {
    /// Project name (an implicit project gets its row).
    pub name: String,
    /// New description; `""` clears it.
    #[serde(default)]
    pub description: Option<String>,
    /// `active` | `archived`.
    #[serde(default)]
    pub status: Option<String>,
    /// New lucide icon name; `""` clears it.
    #[serde(default)]
    pub icon: Option<String>,
    /// New palette key; `""` clears it.
    #[serde(default)]
    pub color: Option<String>,
    /// The whole new metadata map; `{}` clears it.
    #[serde(default)]
    pub metadata: Option<BTreeMap<String, String>>,
}

/// The project name rule (`project::normalize_project`), required.
pub fn check_name(name: &str) -> Result<String, String> {
    project::normalize_project(Some(name))?.ok_or_else(|| "project must not be empty".to_owned())
}

/// Trimmed description, `None` when empty; too long is an error.
fn check_description(d: &str) -> Result<Option<String>, String> {
    let d = d.trim();
    if d.len() > DESCRIPTION_MAX {
        return Err(format!(
            "description must be at most {DESCRIPTION_MAX} bytes"
        ));
    }
    Ok((!d.is_empty()).then(|| d.to_owned()))
}

/// Trimmed keys, at most [`METADATA_MAX_ENTRIES`]; keys 1..=[`METADATA_KEY_MAX`] bytes without
/// control characters, values up to [`METADATA_VALUE_MAX`] bytes.
fn check_metadata(m: BTreeMap<String, String>) -> Result<BTreeMap<String, String>, String> {
    if m.len() > METADATA_MAX_ENTRIES {
        return Err(format!(
            "metadata takes at most {METADATA_MAX_ENTRIES} entries"
        ));
    }
    let mut out = BTreeMap::new();
    for (k, v) in m {
        let key = k.trim();
        if key.is_empty() || key.len() > METADATA_KEY_MAX || key.chars().any(char::is_control) {
            return Err(format!(
                "metadata key {k:?}: 1–{METADATA_KEY_MAX} bytes, no control characters"
            ));
        }
        if v.len() > METADATA_VALUE_MAX {
            return Err(format!(
                "metadata value of {key:?} must be at most {METADATA_VALUE_MAX} bytes"
            ));
        }
        if out.insert(key.to_owned(), v).is_some() {
            return Err(format!("metadata key {key:?} given twice"));
        }
    }
    Ok(out)
}

/// `""` (after trimming) → clear, else the trimmed value.
fn clearable(v: Option<String>) -> Option<Option<String>> {
    v.map(|s| Some(s.trim().to_owned()).filter(|s| !s.is_empty()))
}

impl ProjectCreate {
    /// The normalized name and the row to write.
    pub fn validate(self) -> Result<(String, ProjectPatch), String> {
        let name = check_name(&self.name)?;
        let icon = clearable(self.icon).flatten();
        let color = clearable(self.color).flatten();
        project::validate_style(icon.as_deref(), color.as_deref())?;
        Ok((
            name,
            ProjectPatch {
                description: Some(
                    self.description
                        .as_deref()
                        .map(check_description)
                        .transpose()?
                        .flatten(),
                ),
                status: None,
                icon: Some(icon),
                color: Some(color),
                metadata: self.metadata.map(check_metadata).transpose()?,
            },
        ))
    }
}

impl ProjectUpdate {
    /// The normalized name and the patch.
    pub fn validate(self) -> Result<(String, ProjectPatch), String> {
        let name = check_name(&self.name)?;
        if let Some(s) = &self.status
            && !STATUSES.contains(&s.as_str())
        {
            return Err(format!("status must be one of {}", STATUSES.join(", ")));
        }
        let icon = clearable(self.icon);
        let color = clearable(self.color);
        project::validate_style(
            icon.clone().flatten().as_deref(),
            color.clone().flatten().as_deref(),
        )?;
        Ok((
            name,
            ProjectPatch {
                description: self
                    .description
                    .as_deref()
                    .map(check_description)
                    .transpose()?,
                status: self.status,
                icon,
                color,
                metadata: self.metadata.map(check_metadata).transpose()?,
            },
        ))
    }
}

/// Which store a [`LabelSource`] reads; the [`ProjectDeleted`] slot its count lands in.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum LabelStore {
    /// Root sessions (`sessions.project`).
    Sessions,
    /// Tasks (`tasks.project`).
    Tasks,
    /// Notes and boards (`docs.project`).
    Docs,
    /// Notebooks (`notebooks.project`).
    Notebooks,
    /// Meetings (`meetings.project`).
    Meetings,
}

/// A store that carries project labels.
#[async_trait]
pub trait LabelSource: Send + Sync {
    /// Every label this store carries (duplicates allowed).
    async fn labels(&self) -> anyhow::Result<Vec<String>>;
    /// Clear label `name` on every record that carries it; how many were cleared. A record that
    /// fails is logged at WARN and skipped; `Err` only when the store cannot be read at all.
    async fn detach(&self, name: &str) -> anyhow::Result<u32>;
}

static SOURCES: Mutex<Vec<(LabelStore, Arc<dyn LabelSource>)>> = Mutex::new(Vec::new());

/// Register the label source of `store` (tasks and meetings, at their start); a second
/// registration replaces the first.
pub fn register_label_source(store: LabelStore, source: Arc<dyn LabelSource>) {
    let mut s = SOURCES.lock();
    s.retain(|(k, _)| *k != store);
    s.push((store, source));
}

/// The built-in sources (sessions, docs, notebooks; `ext` routes a write to the engine that owns
/// the record) and the registered ones.
fn sources(ext: &Extensions) -> Vec<(LabelStore, Arc<dyn LabelSource>)> {
    let mut v: Vec<(LabelStore, Arc<dyn LabelSource>)> = vec![
        (
            LabelStore::Sessions,
            Arc::new(SessionLabels { ext: ext.clone() }),
        ),
        (LabelStore::Docs, Arc::new(DocLabels)),
        (
            LabelStore::Notebooks,
            Arc::new(NotebookLabels { ext: ext.clone() }),
        ),
    ];
    v.extend(SOURCES.lock().iter().cloned());
    v
}

/// Every row plus every label no row names (`implicit`), by name. A source that fails is logged
/// and left out.
pub async fn list_projects(
    store: &Store,
    sources: &[(LabelStore, Arc<dyn LabelSource>)],
) -> anyhow::Result<Vec<Project>> {
    let mut all: BTreeMap<String, Project> = store
        .project_rows()
        .await?
        .into_iter()
        .map(|p| (p.name.clone(), p))
        .collect();
    for (kind, source) in sources {
        match source.labels().await {
            Ok(labels) => {
                for l in labels {
                    if !all.contains_key(&l) {
                        all.insert(l.clone(), Project::implicit(l));
                    }
                }
            }
            Err(e) => tracing::warn!(store = ?kind, "projects_list: labels unreadable: {e:#}"),
        }
    }
    Ok(all.into_values().collect())
}

/// Detach `name` from every store, then delete its row. A failing store is logged at WARN and
/// counts what it achieved. `Err` when there is neither a row nor a record that carried the label.
pub async fn delete_project(
    store: &Store,
    name: &str,
    sources: &[(LabelStore, Arc<dyn LabelSource>)],
) -> anyhow::Result<ProjectDeleted> {
    let mut out = ProjectDeleted::default();
    for (kind, source) in sources {
        match source.detach(name).await {
            Ok(n) => *out.slot(*kind) += n,
            Err(e) => {
                tracing::warn!(store = ?kind, project = name, "project_delete: detach failed: {e:#}")
            }
        }
    }
    let had_row = store.delete_project_row(name).await?;
    anyhow::ensure!(
        had_row || out != ProjectDeleted::default(),
        "no project {name:?}"
    );
    Ok(out)
}

fn agents() -> anyhow::Result<ActorRef<Agents>> {
    ActorRef::<Agents>::lookup(AGENT_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("agents are not running"))
}

/// `ask` → the agents' runtime, so a call outside an RPC (a tool) can publish.
struct AgentsRuntime;

impl Message<AgentsRuntime> for Agents {
    type Reply = Result<Runtime, String>;

    async fn handle(
        &mut self,
        _: AgentsRuntime,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        Ok(self.rt.clone())
    }
}

async fn handles() -> anyhow::Result<(Store, Runtime)> {
    let a = agents()?;
    let store = a.ask(DurableStore).await?;
    let rt = a
        .ask(AgentsRuntime)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok((store, rt))
}

/// `projects_list` for a caller outside an RPC (the `project_list` tool).
pub async fn list() -> anyhow::Result<Vec<Project>> {
    let (store, _) = handles().await?;
    list_projects(&store, &sources(&Extensions::new())).await
}

/// `project_create`: a new row; `Err` when one exists.
pub async fn create(input: ProjectCreate) -> anyhow::Result<Project> {
    let (name, patch) = input.validate().map_err(anyhow::Error::msg)?;
    let (store, rt) = handles().await?;
    let p = store.create_project(&name, &patch).await?;
    rt.publish(ProjectsChanged {}).await;
    Ok(p)
}

/// `project_update`: patch the row, creating it for an implicit project.
pub async fn update(input: ProjectUpdate) -> anyhow::Result<Project> {
    let (name, patch) = input.validate().map_err(anyhow::Error::msg)?;
    let (store, rt) = handles().await?;
    let p = store.update_project(&name, &patch).await?;
    rt.publish(ProjectsChanged {}).await;
    Ok(p)
}

/// `project_delete`: detach every store, drop the row. `ext` routes session writes to their owner.
pub async fn delete(ext: &Extensions, name: &str) -> anyhow::Result<ProjectDeleted> {
    let name = check_name(name).map_err(anyhow::Error::msg)?;
    let (store, rt) = handles().await?;
    let r = delete_project(&store, &name, &sources(ext)).await;
    rt.publish(ProjectsChanged {}).await;
    r
}

/// Root sessions through the agents actor; a write goes to the engine that owns the session
/// (`set_session_project`'s route), else here.
struct SessionLabels {
    ext: Extensions,
}

#[async_trait]
impl LabelSource for SessionLabels {
    async fn labels(&self) -> anyhow::Result<Vec<String>> {
        Ok(agents()?
            .ask(ListSessions)
            .await?
            .into_iter()
            .filter(|s| s.parent.is_none())
            .filter_map(|s| s.project)
            .collect())
    }

    async fn detach(&self, name: &str) -> anyhow::Result<u32> {
        let agents = agents()?;
        let roots: Vec<_> = agents
            .ask(ListSessions)
            .await?
            .into_iter()
            .filter(|s| s.parent.is_none() && s.project.as_deref() == Some(name))
            .map(|s| s.id)
            .collect();
        let mut n = 0;
        for session in roots {
            let routed = forward::route::<()>(
                &self.ext,
                session,
                "set_session_project",
                (session, None::<String>),
            )
            .await;
            let r = match routed {
                Ok(Some(())) => Ok(()),
                Ok(None) => agents
                    .ask(SetSessionProject {
                        session,
                        project: None,
                    })
                    .await
                    .map_err(|e| e.to_string()),
                Err(e) => Err(e.message().to_owned()),
            };
            match r {
                Ok(()) => n += 1,
                Err(e) => {
                    tracing::warn!(%session, project = name, "project_delete: session label not cleared: {e}")
                }
            }
        }
        Ok(n)
    }
}

/// The document actor, or `None` when documents are off (nothing carries a label then).
fn documents() -> Option<ActorRef<document::DocumentActor>> {
    ActorRef::<document::DocumentActor>::lookup(document::DOCUMENT_ACTOR)
        .ok()
        .flatten()
}

/// Every free-standing note and board.
async fn free_docs() -> anyhow::Result<Vec<document::DocSummary>> {
    let Some(docs) = documents() else {
        return Ok(Vec::new());
    };
    Ok(docs
        .ask(document::ListDocs {
            kind: None,
            limit: Some(u32::MAX),
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .into_iter()
        .filter(|d| d.kind.free_standing())
        .collect())
}

/// Notes and boards (`docs.project`, merged per column by the replica, so written here).
struct DocLabels;

#[async_trait]
impl LabelSource for DocLabels {
    async fn labels(&self) -> anyhow::Result<Vec<String>> {
        Ok(free_docs()
            .await?
            .into_iter()
            .filter_map(|d| d.project)
            .collect())
    }

    async fn detach(&self, name: &str) -> anyhow::Result<u32> {
        let Some(docs) = documents() else {
            return Ok(0);
        };
        let mut n = 0;
        for d in free_docs()
            .await?
            .into_iter()
            .filter(|d| d.project.as_deref() == Some(name))
        {
            match docs
                .ask(document::SetProject {
                    doc: d.id.clone(),
                    project: None,
                })
                .await
            {
                Ok(_) => n += 1,
                Err(e) => {
                    tracing::warn!(doc = %d.id, project = name, "project_delete: doc label not cleared: {e}")
                }
            }
        }
        Ok(n)
    }
}

/// Notebooks (archived too); a hub-only write is forwarded to the hub like `notebook_update`.
struct NotebookLabels {
    ext: Extensions,
}

async fn notebooks() -> anyhow::Result<Vec<document::Notebook>> {
    let Some(docs) = documents() else {
        return Ok(Vec::new());
    };
    docs.ask(document::NotebookList { archived: true })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

#[async_trait]
impl LabelSource for NotebookLabels {
    async fn labels(&self) -> anyhow::Result<Vec<String>> {
        Ok(notebooks()
            .await?
            .into_iter()
            .filter_map(|n| n.project)
            .collect())
    }

    async fn detach(&self, name: &str) -> anyhow::Result<u32> {
        let Some(docs) = documents() else {
            return Ok(0);
        };
        let mut n = 0;
        for nb in notebooks()
            .await?
            .into_iter()
            .filter(|nb| nb.project.as_deref() == Some(name))
        {
            let id = nb.doc.id;
            let r = document::rpc_support::doc_send(
                &docs,
                document::NotebookUpdate {
                    id: id.clone(),
                    title: None,
                    project: Some(None),
                    archived: None,
                    by: document::Principal::Owner { device: None },
                },
            )
            .await;
            // `notebook_update`'s positional params: id, title, project (`""` clears), archived.
            let r = crate::rpc_document::doc_forward(
                &self.ext,
                "notebook_update",
                (&id, None::<String>, Some(""), None::<bool>),
                r,
            )
            .await;
            match r {
                Ok(()) => n += 1,
                Err(e) => {
                    tracing::warn!(notebook = %id, project = name, "project_delete: notebook label not cleared: {}", e.message())
                }
            }
        }
        Ok(n)
    }
}

/// Project RPC methods (223).
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait ProjectsApi {
    /// Every project: each row, plus every label sessions, tasks, docs, notebooks or meetings
    /// carry that no row names (`implicit: true`), by name. Re-read on `ProjectsChanged`.
    #[method(name = "projects_list", with_extensions)]
    async fn projects_list(&self) -> RpcResult<Vec<Project>>;
    /// Create project `name` (the session label rule); an error when its row exists. Icon and
    /// colour are allow-listed keys; `metadata` is a string → string map. `ProjectsChanged` follows.
    #[method(name = "project_create")]
    async fn project_create(
        &self,
        name: String,
        description: Option<String>,
        icon: Option<String>,
        color: Option<String>,
        metadata: Option<BTreeMap<String, String>>,
    ) -> RpcResult<Project>;
    /// Patch project `name`, creating the row for an implicit project. An omitted field stays;
    /// `""` clears description, icon or colour; `status` is `active` | `archived`; `metadata`
    /// replaces the whole map (`{}` clears it). `ProjectsChanged` follows.
    #[method(name = "project_update")]
    async fn project_update(
        &self,
        name: String,
        description: Option<String>,
        status: Option<String>,
        icon: Option<String>,
        color: Option<String>,
        metadata: Option<BTreeMap<String, String>>,
    ) -> RpcResult<Project>;
    /// Delete project `name`: clear its label on sessions, tasks, docs, notebooks and meetings
    /// (nothing else is deleted), then drop its row. A store that fails counts what it achieved.
    /// Schedule files keep their `project:` key. `ProjectsChanged` follows.
    #[method(name = "project_delete", with_extensions)]
    async fn project_delete(&self, name: String) -> RpcResult<ProjectDeleted>;
}

#[async_trait]
impl ProjectsApiServer for Server {
    async fn projects_list(&self, ext: &Extensions) -> RpcResult<Vec<Project>> {
        let store = rpc_core::ask::<Agents, _>(AGENT_ACTOR, DurableStore).await?;
        list_projects(&store, &sources(ext)).await.map_err(stub_err)
    }

    async fn project_create(
        &self,
        name: String,
        description: Option<String>,
        icon: Option<String>,
        color: Option<String>,
        metadata: Option<BTreeMap<String, String>>,
    ) -> RpcResult<Project> {
        create(ProjectCreate {
            name,
            description,
            icon,
            color,
            metadata,
        })
        .await
        .map_err(stub_err)
    }

    async fn project_update(
        &self,
        name: String,
        description: Option<String>,
        status: Option<String>,
        icon: Option<String>,
        color: Option<String>,
        metadata: Option<BTreeMap<String, String>>,
    ) -> RpcResult<Project> {
        update(ProjectUpdate {
            name,
            description,
            status,
            icon,
            color,
            metadata,
        })
        .await
        .map_err(stub_err)
    }

    async fn project_delete(&self, ext: &Extensions, name: String) -> RpcResult<ProjectDeleted> {
        delete(ext, &name).await.map_err(stub_err)
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(ProjectsApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An in-memory label store: `(record, label)` pairs; `broken` fails every call.
    #[derive(Default)]
    struct Fake {
        rows: Mutex<Vec<(u32, Option<String>)>>,
        broken: bool,
    }

    impl Fake {
        fn with(labels: &[Option<&str>]) -> Arc<Self> {
            Arc::new(Self {
                rows: Mutex::new(
                    labels
                        .iter()
                        .enumerate()
                        .map(|(i, l)| (i as u32, l.map(str::to_owned)))
                        .collect(),
                ),
                broken: false,
            })
        }
    }

    #[async_trait]
    impl LabelSource for Fake {
        async fn labels(&self) -> anyhow::Result<Vec<String>> {
            anyhow::ensure!(!self.broken, "store down");
            Ok(self
                .rows
                .lock()
                .iter()
                .filter_map(|(_, l)| l.clone())
                .collect())
        }

        async fn detach(&self, name: &str) -> anyhow::Result<u32> {
            anyhow::ensure!(!self.broken, "store down");
            let mut n = 0;
            for (_, l) in self.rows.lock().iter_mut() {
                if l.as_deref() == Some(name) {
                    *l = None;
                    n += 1;
                }
            }
            Ok(n)
        }
    }

    async fn open(name: &str) -> (Store, crate::TestDir) {
        let dir = crate::test_dir(name);
        let store = Store::open(&dir.path().join("sessions.sqlite"), None)
            .await
            .unwrap();
        (store, dir)
    }

    fn create(name: &str) -> ProjectPatch {
        ProjectCreate {
            name: name.into(),
            ..Default::default()
        }
        .validate()
        .unwrap()
        .1
    }

    #[tokio::test]
    async fn list_merges_rows_with_implicit_labels_and_skips_a_broken_store() {
        let (store, _dir) = open("projects-list").await;
        store
            .create_project("Alpha", &create("Alpha"))
            .await
            .unwrap();
        let tasks = Fake::with(&[Some("Alpha"), Some("Beta"), None]);
        let broken = Arc::new(Fake {
            broken: true,
            ..Default::default()
        });
        let sources: Vec<(LabelStore, Arc<dyn LabelSource>)> =
            vec![(LabelStore::Tasks, tasks), (LabelStore::Meetings, broken)];
        let list = list_projects(&store, &sources).await.unwrap();
        let got: Vec<_> = list.iter().map(|p| (p.name.as_str(), p.implicit)).collect();
        assert_eq!(got, [("Alpha", false), ("Beta", true)]);
        assert!(list[0].created_ms.is_some());
        assert_eq!(list[1], Project::implicit("Beta".into()));
    }

    #[tokio::test]
    async fn delete_detaches_every_store_counts_and_survives_a_failing_one() {
        let (store, _dir) = open("projects-delete").await;
        store
            .create_project("Alpha", &create("Alpha"))
            .await
            .unwrap();
        let tasks = Fake::with(&[Some("Alpha"), Some("Alpha"), Some("Beta")]);
        let docs = Fake::with(&[Some("Alpha")]);
        let broken = Arc::new(Fake {
            broken: true,
            ..Default::default()
        });
        let sources: Vec<(LabelStore, Arc<dyn LabelSource>)> = vec![
            (LabelStore::Tasks, tasks.clone()),
            (LabelStore::Meetings, broken),
            (LabelStore::Docs, docs.clone()),
        ];
        let d = delete_project(&store, "Alpha", &sources).await.unwrap();
        assert_eq!(
            d,
            ProjectDeleted {
                tasks: 2,
                docs: 1,
                ..Default::default()
            }
        );
        // Detached, not deleted: the records stay, the other label too.
        assert_eq!(tasks.labels().await.unwrap(), ["Beta"]);
        assert_eq!(tasks.rows.lock().len(), 3);
        assert!(store.project_row("Alpha").await.unwrap().is_none());
        // Nothing left to delete: neither a row nor a carrier.
        assert!(delete_project(&store, "Alpha", &sources).await.is_err());
        // An implicit project (no row) deletes by detaching.
        let d = delete_project(&store, "Beta", &sources).await.unwrap();
        assert_eq!(d.tasks, 1);
    }

    #[test]
    fn inputs_are_normalized_and_validated() {
        let (name, p) = ProjectCreate {
            name: "  Alpha ".into(),
            description: Some("  ".into()),
            icon: Some("Rocket".into()),
            color: Some("".into()),
            metadata: Some(BTreeMap::from([(" repo ".into(), "x".into())])),
        }
        .validate()
        .unwrap();
        assert_eq!(name, "Alpha");
        assert_eq!(p.description, Some(None));
        assert_eq!(p.icon, Some(Some("Rocket".into())));
        assert_eq!(p.color, Some(None));
        assert_eq!(
            p.metadata,
            Some(BTreeMap::from([("repo".into(), "x".into())]))
        );
        let bad = |u: ProjectUpdate| u.validate().unwrap_err();
        let named = || ProjectUpdate {
            name: "Alpha".into(),
            ..Default::default()
        };
        assert!(
            bad(ProjectUpdate {
                name: " ".into(),
                ..named()
            })
            .contains("empty")
        );
        assert!(
            bad(ProjectUpdate {
                status: Some("done".into()),
                ..named()
            })
            .contains("status")
        );
        assert!(
            bad(ProjectUpdate {
                icon: Some("Skull".into()),
                ..named()
            })
            .contains("icon")
        );
        assert!(
            bad(ProjectUpdate {
                metadata: Some(BTreeMap::from([("".into(), "v".into())])),
                ..named()
            })
            .contains("metadata key")
        );
        assert!(
            bad(ProjectUpdate {
                metadata: Some(BTreeMap::from([
                    ("a".into(), "v".into()),
                    (" a".into(), "w".into())
                ])),
                ..named()
            })
            .contains("twice")
        );
        // Omitted stays, `""` clears.
        let (_, p) = ProjectUpdate {
            description: Some(String::new()),
            ..named()
        }
        .validate()
        .unwrap();
        assert_eq!((p.description, p.icon, p.status), (Some(None), None, None));
    }
}
