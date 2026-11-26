//! Design 223: the four project tools (`project_list`, `project_create`, `project_update`,
//! `project_delete`) over [`agent::projects`], and the tasks store's [`LabelSource`] so a project
//! lists the labels tasks carry and its delete detaches them. They live here because the tasks
//! plugin registers its tools once the registry is up; the agent crate owns the store.

use std::sync::Arc;

use agent::projects::{self, LabelSource, LabelStore, ProjectCreate, ProjectUpdate};
use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::types::{TaskFilter, TaskPatch};
use crate::{ListTasks, Tasks, UpdateTask};

/// Register the four tools and the tasks label source.
pub async fn register(reg: &ActorRef<ToolRegistry>) {
    projects::register_label_source(LabelStore::Tasks, Arc::new(TaskLabels(None)));
    let _ = reg.ask(RegisterTool(Arc::new(ProjectListTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ProjectCreateTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ProjectUpdateTool))).await;
    let _ = reg.ask(RegisterTool(Arc::new(ProjectDeleteTool))).await;
}

/// Tasks' project labels through the [`Tasks`] actor (the running one when `None`). Detaching is
/// a `user` `task_update` with an empty project, so the field log and `TaskChanged` record it.
pub struct TaskLabels(pub Option<ActorRef<Tasks>>);

impl TaskLabels {
    fn actor(&self) -> anyhow::Result<ActorRef<Tasks>> {
        self.0
            .clone()
            .or_else(crate::actor)
            .ok_or_else(|| anyhow::anyhow!("tasks is not running"))
    }
}

#[async_trait::async_trait]
impl LabelSource for TaskLabels {
    async fn labels(&self) -> anyhow::Result<Vec<String>> {
        Ok(self
            .actor()?
            .ask(ListTasks(TaskFilter::default()))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?
            .into_iter()
            .filter_map(|t| t.project)
            .collect())
    }

    async fn detach(&self, name: &str) -> anyhow::Result<u32> {
        let actor = self.actor()?;
        let tasks = actor
            .ask(ListTasks(TaskFilter {
                project: Some(name.to_owned()),
                ..TaskFilter::default()
            }))
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let mut n = 0;
        for t in tasks {
            let r = actor
                .ask(UpdateTask {
                    id: t.id.clone(),
                    patch: TaskPatch {
                        project: Some(String::new()),
                        ..TaskPatch::default()
                    },
                    actor: "user".into(),
                    session: None,
                })
                .await;
            match r {
                Ok(_) => n += 1,
                Err(e) => {
                    tracing::warn!(task = %t.id, project = name, "project_delete: task label not cleared: {e}");
                }
            }
        }
        Ok(n)
    }
}

fn out<T: Serialize>(r: anyhow::Result<T>) -> anyhow::Result<ToolOutput> {
    Ok(match r {
        Ok(v) => ToolOutput::ok(serde_json::to_string_pretty(&v)?),
        Err(e) => ToolOutput::err(format!("{e:#}")),
    })
}

/// Arguments for [`ProjectListTool`]: none.
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProjectListArgs {}

/// `project_list`: every project with its description, status and metadata.
pub struct ProjectListTool;

#[async_trait::async_trait]
impl Tool for ProjectListTool {
    const NAME: &'static str = "project_list";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ProjectListArgs;

    fn description(&self) -> String {
        "List the projects: each one's name (the label sessions, tasks, notes, notebooks and meetings \
         carry — pass it as `project` to task_create, task_inbox and the note and notebook tools), \
         description, status (`active` | `archived`), icon, colour and metadata. `implicit: true` marks \
         a label that items carry but nobody created as a project yet; `project_update` turns it into \
         one."
            .into()
    }

    async fn call(&self, _: ProjectListArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        out(projects::list().await)
    }
}

/// `project_create`: a new project record.
pub struct ProjectCreateTool;

#[async_trait::async_trait]
impl Tool for ProjectCreateTool {
    const NAME: &'static str = "project_create";
    const RISK: Risk = Risk::Mutating;
    type Args = ProjectCreate;

    fn description(&self) -> String {
        "Create a project: a name (1–128 bytes, case kept; the label its sessions, tasks and notes will \
         carry), an optional description, icon (lucide name: Folder, Briefcase, Code, Terminal, Bug, \
         Database, Server, Cloud, Globe, Book, FileText, MessageSquare, Mic, Music, Camera, Heart, Star, \
         Flag, Zap, Rocket, Wrench, FlaskConical, Shield, House), colour (carnelian, ochre, sand, \
         malachite, faience, lapis, slate, plum, umber, …) and string → string metadata (repo URL, \
         default cwd, links). Fails when the project exists: call project_list first and use \
         project_update for an existing or implicit one. Returns the project."
            .into()
    }

    async fn call(&self, a: ProjectCreate, _: ToolCx) -> anyhow::Result<ToolOutput> {
        out(projects::create(a).await)
    }
}

/// `project_update`: change a project's fields.
pub struct ProjectUpdateTool;

#[async_trait::async_trait]
impl Tool for ProjectUpdateTool {
    const NAME: &'static str = "project_update";
    const RISK: Risk = Risk::Mutating;
    type Args = ProjectUpdate;

    fn description(&self) -> String {
        "Update a project by name: description, status (`active` | `archived`), icon, colour or \
         metadata. Omitted fields stay; an empty string clears the description, icon or colour; \
         `metadata` replaces the whole map (`{}` clears it), so pass every key to keep. An implicit \
         project (a label without a record) becomes a project. It cannot rename. Returns the project."
            .into()
    }

    async fn call(&self, a: ProjectUpdate, _: ToolCx) -> anyhow::Result<ToolOutput> {
        out(projects::update(a).await)
    }
}

/// Arguments for [`ProjectDeleteTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ProjectDeleteArgs {
    /// Project name.
    pub name: String,
}

/// `project_delete`: detach every item, drop the record.
pub struct ProjectDeleteTool;

#[async_trait::async_trait]
impl Tool for ProjectDeleteTool {
    const NAME: &'static str = "project_delete";
    const RISK: Risk = Risk::Destructive;
    type Args = ProjectDeleteArgs;

    fn description(&self) -> String {
        "Delete a project: its label is cleared on every session, task, note, board, notebook and \
         meeting that carries it (they are kept, only ungrouped), then its record (description, \
         metadata, icon, colour) is gone. Schedule files keep their `project:` key. Only on the \
         user's explicit request. Returns how many items of each kind were detached."
            .into()
    }

    async fn call(&self, a: ProjectDeleteArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        out(projects::delete(&jsonrpsee::Extensions::new(), &a.name).await)
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use kameo::actor::Spawn as _;

    use super::*;
    use crate::ledger;
    use crate::types::TaskCreate;

    fn cx() -> ToolCx {
        let dir = std::env::temp_dir();
        ToolCx {
            session: runtime::SessionId::new(),
            call_id: "c".into(),
            quiet: false,
            resources: tools::SessionResources::durable_arc(&dir),
            cancel: tools::CancellationToken::new(),
            promote_after: None,
        }
    }

    /// Bad arguments are the model's error, refused before any store is touched (no agents run).
    #[tokio::test]
    async fn tools_refuse_invalid_arguments() {
        let err = |o: ToolOutput| {
            assert!(o.is_error, "{}", o.content);
            o.content
        };
        let c = err(ProjectCreateTool
            .call(
                ProjectCreate {
                    name: " \t".into(),
                    ..Default::default()
                },
                cx(),
            )
            .await
            .unwrap());
        assert!(c.contains("project must"), "{c}");
        let c = err(ProjectCreateTool
            .call(
                ProjectCreate {
                    name: "Alpha".into(),
                    icon: Some("Skull".into()),
                    ..Default::default()
                },
                cx(),
            )
            .await
            .unwrap());
        assert!(c.contains("icon"), "{c}");
        let c = err(ProjectUpdateTool
            .call(
                ProjectUpdate {
                    name: "Alpha".into(),
                    status: Some("done".into()),
                    ..Default::default()
                },
                cx(),
            )
            .await
            .unwrap());
        assert!(c.contains("status"), "{c}");
        let c = err(ProjectUpdateTool
            .call(
                ProjectUpdate {
                    name: "Alpha".into(),
                    metadata: Some(BTreeMap::from([(String::new(), "v".into())])),
                    ..Default::default()
                },
                cx(),
            )
            .await
            .unwrap());
        assert!(c.contains("metadata key"), "{c}");
        let c = err(ProjectDeleteTool
            .call(ProjectDeleteArgs { name: "\n".into() }, cx())
            .await
            .unwrap());
        assert!(c.contains("control"), "{c}");
        // The schema the model sees: `name` required, the rest optional.
        let schema = serde_json::to_value(schemars::schema_for!(ProjectUpdate)).unwrap();
        assert_eq!(schema["required"], serde_json::json!(["name"]));
    }

    /// A real ledger behind a real actor: detaching clears the label on exactly the tasks that
    /// carried it, keeps the tasks, and logs the change.
    #[tokio::test]
    async fn task_labels_detach_on_a_real_ledger() {
        let r = ledger::tests::rig().await;
        let mut ids = Vec::new();
        for project in [Some("Alpha"), Some("Alpha"), Some("Beta"), None] {
            let t = r
                .ledger
                .create(
                    TaskCreate {
                        state: crate::types::TaskState::Backlog,
                        title: "t".into(),
                        brief: String::new(),
                        owner: crate::types::Owner::User,
                        reviewer: None,
                        parent: None,
                        project: project.map(str::to_owned),
                        where_: None,
                        harness: None,
                        external: vec![],
                        done_when: None,
                        attachments: vec![],
                        priority: None,
                        blockers: vec![],
                    },
                    "user",
                    None,
                    crate::WakeReason::Assigned,
                )
                .await
                .unwrap();
            ids.push(t.id);
        }
        let actor = Tasks::spawn(Tasks {
            ledger: r.ledger,
            escalations: Default::default(),
        });
        let source = TaskLabels(Some(actor.clone()));
        let mut labels = source.labels().await.unwrap();
        labels.sort();
        assert_eq!(labels, ["Alpha", "Alpha", "Beta"]);

        assert_eq!(source.detach("Alpha").await.unwrap(), 2);
        assert_eq!(source.labels().await.unwrap(), ["Beta"]);
        let all = actor.ask(ListTasks(TaskFilter::default())).await.unwrap();
        assert_eq!(all.len(), 4, "detach keeps the tasks");
        let events = actor.ask(crate::TaskEvents(ids[0].clone())).await.unwrap();
        assert!(events.iter().any(|e| e.field == "project"
            && e.before.as_deref() == Some("Alpha")
            && e.after.is_none()));
        assert_eq!(source.detach("Alpha").await.unwrap(), 0);
    }
}
