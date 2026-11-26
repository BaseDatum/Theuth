//! Design 223: the `projects` rows (0018 style + 0079 entity columns). The table rides the record
//! replica (per-column LWW, [`Store::replicate_records`]), so every write here reaches the mesh and
//! a delete removes the row everywhere. A row with `created_ms` NULL, no description, no metadata
//! and status `active` is a pre-223 style row: clearing its style still drops it.

use std::collections::BTreeMap;

use super::Store;
use crate::projects::{Project, STATUSES};

/// A write to one row: `None` leaves a field (on create: the column default), `Some(None)` clears
/// it. `metadata` replaces the whole map; an empty map stores NULL. The caller validated it
/// ([`crate::projects::ProjectCreate::validate`], [`crate::projects::ProjectUpdate::validate`]).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProjectPatch {
    /// Description.
    pub description: Option<Option<String>>,
    /// `active` | `archived`.
    pub status: Option<String>,
    /// Icon key.
    pub icon: Option<Option<String>>,
    /// Colour key.
    pub color: Option<Option<String>>,
    /// Metadata.
    pub metadata: Option<BTreeMap<String, String>>,
}

type Row = (
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<String>,
);

const COLS: &str = "name, description, status, icon, color, created_ms, metadata";

fn project((name, description, status, icon, color, created_ms, metadata): Row) -> Project {
    Project {
        name,
        description,
        // A row that arrived a column at a time may lack it; the column default.
        status: status.unwrap_or_else(|| STATUSES[0].to_owned()),
        icon,
        color,
        created_ms,
        // A peer's unreadable map shows as none rather than failing the list.
        metadata: metadata
            .and_then(|m| serde_json::from_str(&m).ok())
            .unwrap_or_default(),
        implicit: false,
    }
}

fn apply(p: &mut Project, patch: &ProjectPatch) {
    if let Some(d) = &patch.description {
        p.description.clone_from(d);
    }
    if let Some(s) = &patch.status {
        p.status.clone_from(s);
    }
    if let Some(i) = &patch.icon {
        p.icon.clone_from(i);
    }
    if let Some(c) = &patch.color {
        p.color.clone_from(c);
    }
    if let Some(m) = &patch.metadata {
        p.metadata.clone_from(m);
    }
}

impl Store {
    /// Every project row, by name.
    pub async fn project_rows(&self) -> anyhow::Result<Vec<Project>> {
        Ok(sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM projects ORDER BY name"
        )))
        .fetch_all(&self.read)
        .await?
        .into_iter()
        .map(project)
        .collect())
    }

    /// The row of project `name`, if any.
    pub async fn project_row(&self, name: &str) -> anyhow::Result<Option<Project>> {
        Ok(sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM projects WHERE name = ?"
        )))
        .bind(name)
        .fetch_optional(&self.read)
        .await?
        .map(project))
    }

    /// Insert the row of project `name` (normalized by the caller), created now; `Err` when it
    /// exists.
    pub async fn create_project(
        &self,
        name: &str,
        patch: &ProjectPatch,
    ) -> anyhow::Result<Project> {
        let mut p = Project::implicit(name.to_owned());
        p.implicit = false;
        p.created_ms = Some(platform::time::now_ms_i64());
        apply(&mut p, patch);
        let mut tx = self.begin_write().await?;
        let exists: Option<i64> = sqlx::query_scalar("SELECT 1 FROM projects WHERE name = ?")
            .bind(name)
            .fetch_optional(&mut *tx)
            .await?;
        anyhow::ensure!(exists.is_none(), "project {name:?} exists");
        write(&mut tx, &p).await?;
        tx.commit().await?;
        Ok(p)
    }

    /// Patch the row of project `name`, inserting it (created now) when there is none; a pre-223
    /// style row gets its `created_ms` on the first update.
    pub async fn update_project(
        &self,
        name: &str,
        patch: &ProjectPatch,
    ) -> anyhow::Result<Project> {
        let mut tx = self.begin_write().await?;
        let row = sqlx::query_as::<_, Row>(sqlx::AssertSqlSafe(format!(
            "SELECT {COLS} FROM projects WHERE name = ?"
        )))
        .bind(name)
        .fetch_optional(&mut *tx)
        .await?;
        let mut p = row.map_or_else(
            || Project {
                implicit: false,
                ..Project::implicit(name.to_owned())
            },
            project,
        );
        p.created_ms = p.created_ms.or_else(|| Some(platform::time::now_ms_i64()));
        apply(&mut p, patch);
        write(&mut tx, &p).await?;
        tx.commit().await?;
        Ok(p)
    }

    /// Delete the row of project `name`; whether there was one.
    pub async fn delete_project_row(&self, name: &str) -> anyhow::Result<bool> {
        let mut tx = self.begin_write().await?;
        let n = sqlx::query("DELETE FROM projects WHERE name = ?")
            .bind(name)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        tx.commit().await?;
        Ok(n > 0)
    }
}

/// Upsert every column of `p`.
async fn write(tx: &mut sqlx::SqliteConnection, p: &Project) -> anyhow::Result<()> {
    let metadata = (!p.metadata.is_empty())
        .then(|| serde_json::to_string(&p.metadata))
        .transpose()?;
    sqlx::query(
        "INSERT INTO projects (name, description, status, icon, color, created_ms, metadata, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) ON CONFLICT(name) DO UPDATE SET \
         description = excluded.description, status = excluded.status, icon = excluded.icon, \
         color = excluded.color, created_ms = excluded.created_ms, metadata = excluded.metadata, \
         updated_at = excluded.updated_at",
    )
    .bind(&p.name)
    .bind(&p.description)
    .bind(&p.status)
    .bind(&p.icon)
    .bind(&p.color)
    .bind(p.created_ms)
    .bind(metadata)
    .bind(chrono::Utc::now().to_rfc3339())
    .execute(tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn open(name: &str) -> (Store, crate::TestDir) {
        let dir = crate::test_dir(name);
        let store = Store::open(&dir.path().join("sessions.sqlite"), None)
            .await
            .unwrap();
        (store, dir)
    }

    fn meta(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect()
    }

    #[tokio::test]
    async fn create_update_list_and_delete_a_row() {
        let (store, _dir) = open("projects-rows").await;
        let created = store
            .create_project(
                "Alpha",
                &ProjectPatch {
                    description: Some(Some("the A team".into())),
                    icon: Some(Some("Rocket".into())),
                    metadata: Some(meta(&[("repo", "git@x:a")])),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(created.status, "active");
        assert!(created.created_ms.is_some() && !created.implicit);
        assert_eq!(
            store.project_row("Alpha").await.unwrap(),
            Some(created.clone())
        );
        // A second create is refused and leaves the row.
        assert!(
            store
                .create_project("Alpha", &ProjectPatch::default())
                .await
                .is_err()
        );

        // Omitted fields stay, `Some(None)` clears, metadata replaces whole.
        let updated = store
            .update_project(
                "Alpha",
                &ProjectPatch {
                    status: Some("archived".into()),
                    icon: Some(None),
                    metadata: Some(meta(&[("cwd", "/src/a")])),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.description.as_deref(), Some("the A team"));
        assert_eq!(updated.icon, None);
        assert_eq!(updated.status, "archived");
        assert_eq!(updated.metadata, meta(&[("cwd", "/src/a")]));
        assert_eq!(updated.created_ms, created.created_ms);

        // Update of an implicit label creates its row; an empty map stores NULL.
        let beta = store
            .update_project(
                "Beta",
                &ProjectPatch {
                    metadata: Some(BTreeMap::new()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(beta.created_ms.is_some());
        let raw: Option<String> =
            sqlx::query_scalar("SELECT metadata FROM projects WHERE name = 'Beta'")
                .fetch_one(&store.read)
                .await
                .unwrap();
        assert_eq!(raw, None);
        let names: Vec<_> = store
            .project_rows()
            .await
            .unwrap()
            .into_iter()
            .map(|p| p.name)
            .collect();
        assert_eq!(names, ["Alpha", "Beta"]);

        assert!(store.delete_project_row("Alpha").await.unwrap());
        assert!(!store.delete_project_row("Alpha").await.unwrap());
        assert_eq!(store.project_row("Alpha").await.unwrap(), None);
    }

    /// The style API keeps working on the same row: clearing a pre-223 style row drops it, clearing
    /// a project's style keeps the project.
    #[tokio::test]
    async fn style_writes_share_the_row() {
        let (store, _dir) = open("projects-style").await;
        store
            .set_project_style("Old", Some("Bug"), None)
            .await
            .unwrap();
        store
            .create_project("New", &ProjectPatch::default())
            .await
            .unwrap();
        store
            .set_project_style("New", Some("Code"), Some("teal"))
            .await
            .unwrap();
        let new = store.project_row("New").await.unwrap().unwrap();
        assert_eq!(
            (new.icon.as_deref(), new.color.as_deref()),
            (Some("Code"), Some("teal"))
        );
        assert!(
            new.created_ms.is_some(),
            "a style write keeps the entity columns"
        );
        let styled: Vec<_> = store
            .list_project_styles()
            .await
            .unwrap()
            .into_iter()
            .map(|s| s.name)
            .collect();
        assert_eq!(styled, ["New", "Old"]);

        store.set_project_style("Old", None, None).await.unwrap();
        store.set_project_style("New", None, None).await.unwrap();
        assert_eq!(store.project_row("Old").await.unwrap(), None);
        let new = store.project_row("New").await.unwrap().unwrap();
        assert_eq!((new.icon, new.color), (None, None));
        assert!(store.list_project_styles().await.unwrap().is_empty());
    }

    /// `projects` is a record-replica table, not a session-log one: 0079 leaves the session wire
    /// schema where 219's 0078 put it.
    #[tokio::test]
    async fn record_table_no_wire_bump() {
        assert_eq!(crate::store::SESSIONS_WIRE_SCHEMA, 78);
        assert!(
            !super::super::capture::ATTACHED
                .iter()
                .any(|(t, _)| *t == "projects")
        );
    }
}
