//! A session's own knowledge harvester (`session_knowledge_harvesters`): the
//! per-chat pick over `agent.knowledge_harvester_model`. No row = the default.

use runtime::SessionId;

use super::Store;
use crate::SessionKnowledgeHarvester;

impl Store {
    /// `session`'s own knowledge harvester; both `None` without a row.
    pub async fn session_knowledge_harvester(
        &self,
        session: SessionId,
    ) -> anyhow::Result<SessionKnowledgeHarvester> {
        let row: Option<(String, Option<String>)> = sqlx::query_as(
            "SELECT model, effort FROM session_knowledge_harvesters WHERE session = ?",
        )
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        Ok(
            row.map_or_else(SessionKnowledgeHarvester::default, |(model, effort)| {
                SessionKnowledgeHarvester {
                    model: Some(model),
                    effort,
                }
            }),
        )
    }

    /// Store `pick` for `session`; `model: None` deletes the row (the default again).
    pub async fn set_session_knowledge_harvester(
        &self,
        session: SessionId,
        pick: &SessionKnowledgeHarvester,
    ) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        match &pick.model {
            Some(model) => {
                sqlx::query(
                    "INSERT INTO session_knowledge_harvesters (session, model, effort) VALUES (?, ?, ?) \
                     ON CONFLICT(session) DO UPDATE SET model = excluded.model, effort = excluded.effort",
                )
                .bind(session.to_string())
                .bind(model)
                .bind(&pick.effort)
                .execute(&mut *tx)
                .await?;
            }
            None => {
                sqlx::query("DELETE FROM session_knowledge_harvesters WHERE session = ?")
                    .bind(session.to_string())
                    .execute(&mut *tx)
                    .await?;
            }
        }
        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A pick survives a reopen of the database (an engine restart); a reset deletes it.
    #[tokio::test]
    async fn session_knowledge_harvester_persists_and_resets() {
        let scratch = crate::test_dir("knowledge-harvester");
        let path = scratch.path().join("sessions.sqlite");
        let store = Store::open(&path, None).await.unwrap();
        let (a, b) = (SessionId::new(), SessionId::new());
        for id in [a, b] {
            store
                .create_session(id, "m", None, None, None)
                .await
                .unwrap();
        }
        assert_eq!(
            store.session_knowledge_harvester(a).await.unwrap(),
            SessionKnowledgeHarvester::default()
        );
        let pick = SessionKnowledgeHarvester {
            model: Some("p/cheap".into()),
            effort: Some("low".into()),
        };
        store
            .set_session_knowledge_harvester(a, &pick)
            .await
            .unwrap();
        drop(store);

        let store = Store::open(&path, None).await.unwrap();
        assert_eq!(store.session_knowledge_harvester(a).await.unwrap(), pick);
        assert_eq!(
            store.session_knowledge_harvester(b).await.unwrap(),
            SessionKnowledgeHarvester::default(),
            "another session keeps the default"
        );
        store
            .set_session_knowledge_harvester(a, &SessionKnowledgeHarvester::default())
            .await
            .unwrap();
        assert_eq!(
            store.session_knowledge_harvester(a).await.unwrap(),
            SessionKnowledgeHarvester::default()
        );
    }
}
