//! 125 §6.1: what the store keeps for the advisor — `sessions.kind`, the
//! 1:1 `session_advisors` link and the typed `messages.advisory` column
//! beside an ordinary user row (the vision pattern, 52 §6). Only that
//! column makes a row an Advisory card; such rows never open a turn and
//! are transparent to turn readers (`Row::is_note`).

use std::collections::BTreeMap;

use anyhow::Context as _;
use genai::chat::ChatMessage;
use runtime::SessionId;
use sqlx::FromRow;

use super::{Store, insert_in, rows_in, turn_starts};
use crate::{AdvisorLink, AdvisorState, Advisory, HistoryEntry, SessionKind};

/// `sessions.kind` text of a [`SessionKind`].
pub(super) fn kind_str(kind: SessionKind) -> &'static str {
    match kind {
        SessionKind::Chat => "chat",
        SessionKind::Advisor => "advisor",
    }
}

/// [`SessionKind`] of a `sessions.kind` text; anything unknown is a chat.
pub(super) fn kind_of(raw: &str) -> SessionKind {
    match raw {
        "advisor" => SessionKind::Advisor,
        _ => SessionKind::Chat,
    }
}

/// `session_advisors.state` text of an [`AdvisorState`].
fn state_str(state: AdvisorState) -> &'static str {
    match state {
        AdvisorState::Off => "off",
        AdvisorState::Idle => "idle",
        AdvisorState::Reviewing => "reviewing",
        AdvisorState::Paused => "paused",
        AdvisorState::Halted => "halted",
    }
}

/// [`AdvisorState`] of a `session_advisors.state` text; unknown text is
/// `Idle` (the row exists, so the link does).
fn state_of(raw: &str) -> AdvisorState {
    match raw {
        "off" => AdvisorState::Off,
        "reviewing" => AdvisorState::Reviewing,
        "paused" => AdvisorState::Paused,
        "halted" => AdvisorState::Halted,
        _ => AdvisorState::Idle,
    }
}

/// Parse stored advisory metadata; malformed JSON is a visible store error
/// (fail closed, as `note_of` does for vision notes).
pub(super) fn advisory_of(id: SessionId, seq: i64, raw: &str) -> anyhow::Result<Advisory> {
    serde_json::from_str(raw)
        .with_context(|| format!("bad advisory metadata on message {seq} of {id}"))
}

/// One `session_advisors` row as selected.
#[derive(FromRow)]
struct LinkRow {
    session: String,
    advisor: String,
    model: String,
    effort: Option<String>,
    cursor: i64,
    state: String,
}

impl LinkRow {
    fn link(self) -> anyhow::Result<AdvisorLink> {
        Ok(AdvisorLink {
            session: self.session.parse().context("bad session id")?,
            advisor: self.advisor.parse().context("bad advisor id")?,
            model: self.model,
            effort: self.effort,
            cursor: self.cursor,
            state: state_of(&self.state),
        })
    }
}

impl Store {
    /// The advisor link of `session`, if it has one.
    pub async fn advisor_link(&self, session: SessionId) -> anyhow::Result<Option<AdvisorLink>> {
        let row: Option<LinkRow> = sqlx::query_as(
            "SELECT session, advisor, model, effort, cursor, state FROM session_advisors \
             WHERE session = ?",
        )
        .bind(session.to_string())
        .fetch_optional(&self.read)
        .await?;
        row.map(LinkRow::link).transpose()
    }

    /// Every link, for the Advisors actor's boot load (125 §4.14).
    pub async fn advisor_links(&self) -> anyhow::Result<Vec<AdvisorLink>> {
        let rows: Vec<LinkRow> = sqlx::query_as(
            "SELECT session, advisor, model, effort, cursor, state FROM session_advisors \
             ORDER BY session",
        )
        .fetch_all(&self.read)
        .await?;
        rows.into_iter().map(LinkRow::link).collect()
    }

    /// Insert or replace the link of `link.session`.
    pub async fn put_advisor_link(&self, link: &AdvisorLink) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query(
            "INSERT INTO session_advisors (session, advisor, model, effort, cursor, state, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(session) DO UPDATE SET advisor = excluded.advisor, model = excluded.model, \
             effort = excluded.effort, cursor = excluded.cursor, state = excluded.state, \
             updated_at = excluded.updated_at",
        )
        .bind(link.session.to_string())
        .bind(link.advisor.to_string())
        .bind(&link.model)
        .bind(&link.effort)
        .bind(link.cursor)
        .bind(state_str(link.state))
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Drop the link of `session`; the advisor session and its rows stay (D1).
    pub async fn delete_advisor_link(&self, session: SessionId) -> anyhow::Result<()> {
        let mut tx = self.begin_write().await?;
        sqlx::query("DELETE FROM session_advisors WHERE session = ?")
            .bind(session.to_string())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Typed metadata of every Advisory row of `session`, by seq (visible or
    /// not: the column is the identity, like `vision_notes`). Malformed
    /// metadata is an error.
    pub async fn advisories(&self, session: SessionId) -> anyhow::Result<BTreeMap<i64, Advisory>> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT seq, advisory FROM messages WHERE session_id = ? AND advisory IS NOT NULL \
             ORDER BY seq",
        )
        .bind(session.to_string())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(seq, raw)| Ok((seq, advisory_of(session, seq, &raw)?)))
            .collect()
    }

    /// Rendered entries (`supervisor::render_with`) of `session` with seq >=
    /// `from_seq`, Advisory rows included as [`HistoryEntry::Advisory`].
    pub async fn rows_since(
        &self,
        session: SessionId,
        from_seq: i64,
    ) -> anyhow::Result<Vec<(i64, HistoryEntry)>> {
        // ponytail: the whole visible transcript is read and rendered per
        // call so tool cards keep their call names; a seq-ranged read is the
        // upgrade if a measured session makes the advisor's step slow.
        let h = self.context_history(session).await?;
        let advisories = self.advisories(session).await?;
        let mut rows =
            crate::supervisor::render_with(&h.messages, &h.entries, &h.thinking, &advisories);
        rows.retain(|(seq, _)| *seq >= from_seq);
        Ok(rows)
    }

    /// Append an advisory user row with its metadata in one transaction;
    /// returns the seq. The row is model context (`updated_at` and the
    /// revision bump, like any append) but opens no turn and writes no
    /// journal step.
    pub async fn append_advisory(
        &self,
        session: SessionId,
        text: &str,
        meta: &Advisory,
    ) -> anyhow::Result<i64> {
        let advisory = serde_json::to_string(meta)?;
        let sid = session.to_string();
        let mut tx = self.begin_write().await?;
        let seq = insert_in(&mut tx, &sid, &ChatMessage::user(text), None, None).await?;
        sqlx::query("UPDATE messages SET advisory = ? WHERE session_id = ? AND seq = ?")
            .bind(&advisory)
            .bind(&sid)
            .bind(seq)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(seq)
    }

    /// Seq of the first turn opener of `session` (for the advisor's
    /// `RestartTurn` reset); `None` for an empty session.
    pub async fn first_turn(&self, session: SessionId) -> anyhow::Result<Option<i64>> {
        let mut conn = self.read.acquire().await?;
        let rows = rows_in(&mut conn, session, 0).await?;
        Ok(turn_starts(&rows).first().map(|&i| rows[i].seq))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::turn_openers;
    use crate::{AdvisorSeverity, AdvisoryNote};

    /// Fresh store in a temp dir with one chat session; caller removes `dir`.
    async fn fresh() -> (Store, SessionId, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("theuth-advisor-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let store = Store::open(&dir.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        (store, id, dir)
    }

    /// An advisor child of `advised`, created the way W1 will (`kind = advisor`).
    async fn advisor_of(store: &Store, advised: SessionId) -> SessionId {
        let advisor = SessionId::new();
        store
            .create_session_kind(
                advisor,
                "prov/adv",
                Some(advised),
                Some("advisor"),
                None,
                SessionKind::Advisor,
            )
            .await
            .unwrap();
        advisor
    }

    fn card(advisor: SessionId, severity: AdvisorSeverity, note: &str) -> Advisory {
        Advisory {
            advisor,
            step: 3,
            turn: Some(0),
            at_ms: 1_700_000_000_000,
            notes: vec![AdvisoryNote {
                severity,
                note: note.into(),
            }],
        }
    }

    #[tokio::test]
    async fn link_upserts_round_trips_and_deletes() {
        let (store, id, dir) = fresh().await;
        assert_eq!(store.advisor_link(id).await.unwrap(), None);
        assert!(store.advisor_links().await.unwrap().is_empty());

        let advisor = advisor_of(&store, id).await;
        let mut link = AdvisorLink {
            session: id,
            advisor,
            model: "prov/adv".into(),
            effort: Some("high".into()),
            cursor: 4,
            state: AdvisorState::Idle,
        };
        store.put_advisor_link(&link).await.unwrap();
        assert_eq!(store.advisor_link(id).await.unwrap().as_ref(), Some(&link));
        assert_eq!(store.advisor_links().await.unwrap(), vec![link.clone()]);

        // Upsert: same session, moved cursor, new state and model.
        link.cursor = 9;
        link.state = AdvisorState::Halted;
        link.model = "prov/other".into();
        link.effort = None;
        store.put_advisor_link(&link).await.unwrap();
        assert_eq!(store.advisor_link(id).await.unwrap().as_ref(), Some(&link));
        assert_eq!(store.advisor_links().await.unwrap().len(), 1);

        store.delete_advisor_link(id).await.unwrap();
        assert_eq!(store.advisor_link(id).await.unwrap(), None);
        // The advisor session itself stays (D1).
        assert!(store.session(advisor).await.unwrap().is_some());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 125 F1: the link is captured (68 §3), so a session moved to another
    /// engine by replaying its log rows arrives with its advisor link intact.
    #[tokio::test]
    async fn link_survives_capture_and_apply_into_a_fresh_store() {
        use std::ffi::{c_int, c_void};
        unsafe extern "C" fn abort_on_conflict(
            _ctx: *mut c_void,
            _kind: c_int,
            _it: *mut libsqlite3_sys::sqlite3_changeset_iter,
        ) -> c_int {
            libsqlite3_sys::SQLITE_CHANGESET_ABORT
        }
        let (store, id, dir) = fresh().await;
        let advisor = advisor_of(&store, id).await;
        let link = AdvisorLink {
            session: id,
            advisor,
            model: "prov/adv".into(),
            effort: Some("high".into()),
            cursor: 7,
            state: AdvisorState::Paused,
        };
        store.put_advisor_link(&link).await.unwrap();
        let rows = store.log_after(0).await.unwrap();
        assert!(
            rows.iter().any(|r| r.session_id == id.to_string()),
            "put_advisor_link left no log row for the advised session"
        );

        let other = Store::open(&dir.join("other.sqlite"), None).await.unwrap();
        for row in &rows {
            let mut tx = other.begin_apply().await.unwrap();
            let rc = tx
                .with_raw(|db| unsafe {
                    libsqlite3_sys::sqlite3changeset_apply(
                        db.as_ptr(),
                        c_int::try_from(row.changeset.len()).unwrap(),
                        row.changeset.as_ptr().cast::<c_void>().cast_mut(),
                        None,
                        Some(abort_on_conflict),
                        std::ptr::null_mut(),
                    )
                })
                .await
                .unwrap();
            assert_eq!(rc, libsqlite3_sys::SQLITE_OK, "apply of seq {}", row.seq);
            tx.commit().await.unwrap();
        }
        assert_eq!(other.advisor_link(id).await.unwrap(), Some(link));
        assert!(other.session(advisor).await.unwrap().is_some());
        drop((store, other));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn appended_advisory_opens_no_turn_and_renders_typed() {
        let (store, id, dir) = fresh().await;
        let advisor = advisor_of(&store, id).await;
        store.append(id, &ChatMessage::user("u1")).await.unwrap();
        store
            .append(id, &ChatMessage::assistant("a1"))
            .await
            .unwrap();
        let meta = card(advisor, AdvisorSeverity::Concern, "check the lock file");
        let seq = store
            .append_advisory(id, "<advisory>check the lock file</advisory>", &meta)
            .await
            .unwrap();
        assert_eq!(seq, 2);
        store.append(id, &ChatMessage::user("u2")).await.unwrap();

        // The advisory is a note: not an opener; the real user row after it is.
        let advisories = store.advisories(id).await.unwrap();
        assert_eq!(advisories, BTreeMap::from([(2, meta.clone())]));
        let messages = store.messages(id).await.unwrap();
        let openers = turn_openers(&messages, |s| advisories.contains_key(&s));
        assert_eq!(openers, [0, 3]);
        assert_eq!(store.first_turn(id).await.unwrap(), Some(0));
        assert_eq!(store.hidden_turns(id).await.unwrap(), 0);
        // Undo takes the turn the note sits in, not a phantom turn of its own.
        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u2"));
        assert_eq!(store.undo(id).await.unwrap().as_deref(), Some("u1"));
        assert_eq!(store.hidden_turns(id).await.unwrap(), 2);
        store.redo(id).await.unwrap();
        store.redo(id).await.unwrap();

        // Rendered rows carry the typed card; everything else is untouched.
        let rows = store.rows_since(id, 0).await.unwrap();
        let roles: Vec<(i64, &str)> = rows
            .iter()
            .map(|(seq, e)| {
                (
                    *seq,
                    match e {
                        HistoryEntry::User { .. } => "user",
                        HistoryEntry::Assistant { .. } => "assistant",
                        HistoryEntry::Advisory { .. } => "advisory",
                        _ => "other",
                    },
                )
            })
            .collect();
        assert_eq!(
            roles,
            [(0, "user"), (1, "assistant"), (2, "advisory"), (3, "user")]
        );
        match &rows[2].1 {
            HistoryEntry::Advisory { advisory, context } => {
                assert_eq!(advisory, &meta);
                assert_eq!(
                    context.as_ref().unwrap().target,
                    crate::ContextTarget::Message { seq: 2 }
                );
            }
            other => panic!("{other:?}"),
        }
        let since = store.rows_since(id, 2).await.unwrap();
        assert_eq!(since.iter().map(|(s, _)| *s).collect::<Vec<_>>(), [2, 3]);

        // Corrupt metadata fails closed for the typed readers.
        sqlx::query("UPDATE messages SET advisory = '{nope' WHERE session_id = ? AND seq = 2")
            .bind(id.to_string())
            .execute(&store.write)
            .await
            .unwrap();
        let err = store.advisories(id).await.unwrap_err();
        assert!(
            err.to_string()
                .contains("bad advisory metadata on message 2"),
            "{err}"
        );
        assert!(store.rows_since(id, 0).await.is_err());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn advisors_hidden_from_lists_trees_and_search_unless_asked() {
        let (store, root, dir) = fresh().await;
        let kid = SessionId::new();
        store
            .create_session(kid, "m", Some(root), Some("kid"), None)
            .await
            .unwrap();
        let advisor = advisor_of(&store, root).await;
        store.set_title(advisor, "advisor").await.unwrap();
        store
            .append(advisor, &ChatMessage::user("### Session update"))
            .await
            .unwrap();
        store
            .append(root, &ChatMessage::user("hello"))
            .await
            .unwrap();

        let ids =
            |rows: Vec<crate::SessionSummary>| rows.into_iter().map(|s| s.id).collect::<Vec<_>>();
        let listed = store.list_sessions().await.unwrap();
        assert!(listed.iter().all(|s| s.kind == SessionKind::Chat));
        assert!(!ids(listed).contains(&advisor));
        let all = store.list_sessions_incl(true).await.unwrap();
        assert_eq!(
            all.iter().find(|s| s.id == advisor).map(|s| s.kind),
            Some(SessionKind::Advisor)
        );
        assert_eq!(
            store.session(advisor).await.unwrap().unwrap().kind,
            SessionKind::Advisor
        );

        let tree = |rows: Vec<(u8, crate::SessionSummary)>| {
            rows.into_iter().map(|(_, s)| s.id).collect::<Vec<_>>()
        };
        assert_eq!(tree(store.descendants(root).await.unwrap()), [kid]);
        let full = tree(store.descendants_incl(root, true).await.unwrap());
        assert_eq!(full.len(), 2);
        assert!(full.contains(&advisor));

        // Search sees neither the advisor's title nor its bodies.
        let titles = store.search_titles().await.unwrap();
        assert!(titles.iter().all(|(id, ..)| *id != advisor));
        assert_eq!(titles.len(), 2);
        let bodies = store.search_bodies().await.unwrap();
        assert!(bodies.iter().all(|(id, ..)| *id != advisor));
        assert_eq!(bodies.len(), 1);

        // Deleting the advised session takes the hidden child with it.
        let removed = store.delete_session(root).await.unwrap();
        assert!(removed.contains(&advisor) && removed.contains(&kid));
        assert!(store.session(advisor).await.unwrap().is_none());
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 0034 applies on a database at 0033: existing rows read as `chat` with
    /// no advisory, and the new table is usable.
    #[tokio::test]
    async fn migration_applies_on_a_0033_database() {
        let dir = std::env::temp_dir().join(format!("theuth-advisor-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sessions.sqlite");
        let old = SessionId::new();
        {
            let pool = sqlx::SqlitePool::connect_with(
                sqlx::sqlite::SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
            sqlx::migrate!("./migrations")
                .run_to(33, &pool)
                .await
                .unwrap();
            sqlx::query(
                "INSERT INTO sessions (id, created_at, updated_at, title, model, owner, epoch) \
                 VALUES (?, '2026-01-01T00:00:00Z', '2026-01-01T00:00:00Z', 'old', 'm', '', 1)",
            )
            .bind(old.to_string())
            .execute(&pool)
            .await
            .unwrap();
            sqlx::query("INSERT INTO messages (session_id, seq, body) VALUES (?, 0, ?)")
                .bind(old.to_string())
                .bind(serde_json::to_string(&ChatMessage::user("hi")).unwrap())
                .execute(&pool)
                .await
                .unwrap();
            pool.close().await;
        }
        let store = Store::open(&path, None).await.unwrap();
        let row = store.session(old).await.unwrap().unwrap();
        assert_eq!(row.kind, SessionKind::Chat);
        assert_eq!(store.list_sessions().await.unwrap().len(), 1);
        assert!(store.advisories(old).await.unwrap().is_empty());
        assert_eq!(store.first_turn(old).await.unwrap(), Some(0));
        // A session created on the new schema is a chat too, and can carry an advisor.
        let fresh = SessionId::new();
        store
            .create_session(fresh, "m", None, None, None)
            .await
            .unwrap();
        assert_eq!(
            store.session(fresh).await.unwrap().unwrap().kind,
            SessionKind::Chat
        );
        let advisor = advisor_of(&store, fresh).await;
        assert_eq!(store.list_sessions().await.unwrap().len(), 2);
        store
            .put_advisor_link(&AdvisorLink {
                session: fresh,
                advisor,
                model: "prov/adv".into(),
                effort: None,
                cursor: 1,
                state: AdvisorState::Idle,
            })
            .await
            .unwrap();
        assert_eq!(store.advisor_links().await.unwrap().len(), 1);
        drop(store);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
