//! The replicated `secret_requests` rows (79 §6.6, contract §2.7): one per
//! `secret_request` / `secret_request_form` call, answered from any engine.
//! Row bytes only; the waiting logic is the actor's ([`crate::SecretsActor`]).

use sqlx::sqlite::SqlitePool;

use crate::{RequestId, SecretRequest, SecretRequestField};

type RowTuple = (
    String,
    String,
    String,
    Option<String>,
    String,
    i64,
    Option<i64>,
    Option<String>,
    Option<String>,
);

fn from_row(r: RowTuple) -> anyhow::Result<SecretRequest> {
    let fields: Vec<SecretRequestField> = serde_json::from_str(&r.4)?;
    Ok(SecretRequest {
        id: RequestId(r.0.parse()?),
        engine: r.1,
        session: r.2,
        title: r.3,
        fields,
        created_ms: r.5 as u64,
        answered_ms: r.6.map(|v| v as u64),
        outcome: r.7,
        hlc: r.8,
    })
}

/// Write a new row (`hlc` is stamped by the replica's trigger).
pub async fn insert(pool: &SqlitePool, r: &SecretRequest) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO secret_requests (id, engine, session, title, fields, created_ms) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(r.id.0.to_string())
    .bind(&r.engine)
    .bind(&r.session)
    .bind(&r.title)
    .bind(serde_json::to_string(&r.fields)?)
    .bind(r.created_ms as i64)
    .execute(pool)
    .await?;
    Ok(())
}

/// One row by id.
pub async fn get(pool: &SqlitePool, id: RequestId) -> anyhow::Result<Option<SecretRequest>> {
    let r: Option<RowTuple> = sqlx::query_as(
        "SELECT id, engine, session, title, fields, created_ms, answered_ms, outcome, hlc \
         FROM secret_requests WHERE id = ?",
    )
    .bind(id.0.to_string())
    .fetch_optional(pool)
    .await?;
    r.map(from_row).transpose()
}

/// Every row (or only the unanswered ones), newest first. A row whose `fields`
/// JSON does not parse (a newer peer's shape) is skipped with a warning.
pub async fn list(pool: &SqlitePool, unanswered_only: bool) -> anyhow::Result<Vec<SecretRequest>> {
    let sql = if unanswered_only {
        "SELECT id, engine, session, title, fields, created_ms, answered_ms, outcome, hlc \
         FROM secret_requests WHERE answered_ms IS NULL ORDER BY created_ms DESC"
    } else {
        "SELECT id, engine, session, title, fields, created_ms, answered_ms, outcome, hlc \
         FROM secret_requests ORDER BY created_ms DESC"
    };
    let rows: Vec<RowTuple> = sqlx::query_as(sql).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .filter_map(|r| match from_row(r) {
            Ok(r) => Some(r),
            Err(e) => {
                tracing::warn!("secrets: secret_requests row skipped: {e:#}");
                None
            }
        })
        .collect())
}

/// Set `answered_ms` / `outcome` (`stored` | `declined` | `timeout`) on an
/// unanswered row. `Ok(false)` when there is no such row or it was already
/// answered (the first answer sticks; LWW would otherwise let a later
/// `timeout` from the origin overwrite a peer's `stored`).
pub async fn answer(
    pool: &SqlitePool,
    id: RequestId,
    outcome: &str,
    now_ms: u64,
) -> anyhow::Result<bool> {
    let r = sqlx::query(
        "UPDATE secret_requests SET answered_ms = ?, outcome = ? WHERE id = ? AND answered_ms IS NULL",
    )
    .bind(now_ms as i64)
    .bind(outcome)
    .bind(id.0.to_string())
    .execute(pool)
    .await?;
    Ok(r.rows_affected() == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: u128, title: Option<&str>, names: &[&str], created_ms: u64) -> SecretRequest {
        SecretRequest {
            id: RequestId(uuid::Uuid::from_u128(id)),
            engine: "e1".into(),
            session: "s1".into(),
            title: title.map(Into::into),
            fields: names
                .iter()
                .map(|n| SecretRequestField {
                    name: (*n).to_owned(),
                    label: format!("{n} label"),
                    masked: true,
                    current: (*n == "TOKEN").then_some(2),
                })
                .collect(),
            created_ms,
            answered_ms: None,
            outcome: None,
            hlc: None,
        }
    }

    #[tokio::test]
    async fn insert_list_answer() {
        let dir = tempfile::tempdir().unwrap();
        let pool = crate::store::open(&dir.path().join("s.sqlite"))
            .await
            .unwrap();
        let single = req(1, None, &["TOKEN"], 10);
        let form = req(2, Some("GitHub"), &["gh/user", "gh/pass", "gh/totp"], 20);
        insert(&pool, &single).await.unwrap();
        insert(&pool, &form).await.unwrap();

        let open = list(&pool, true).await.unwrap();
        assert_eq!(
            open,
            [form.clone(), single.clone()],
            "newest first, fields round-trip"
        );
        assert_eq!(get(&pool, single.id).await.unwrap().unwrap(), single);

        assert!(answer(&pool, single.id, "stored", 30).await.unwrap());
        assert!(
            !answer(&pool, single.id, "declined", 31).await.unwrap(),
            "first answer sticks"
        );
        assert!(
            !answer(&pool, RequestId(uuid::Uuid::from_u128(9)), "stored", 32)
                .await
                .unwrap()
        );
        let s = get(&pool, single.id).await.unwrap().unwrap();
        assert_eq!(
            (s.answered_ms, s.outcome.as_deref()),
            (Some(30), Some("stored"))
        );
        assert_eq!(list(&pool, true).await.unwrap(), [form]);
        assert_eq!(list(&pool, false).await.unwrap().len(), 2);
    }
}
