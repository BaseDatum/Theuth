//! 183 §5.3: the typed `messages.moved` column beside an ordinary user row
//! (the advisory pattern, `store/advisor.rs`). Such a row never opens a turn
//! (`Row::is_note`); its text is the model-facing `<session_moved/>`.

use std::collections::BTreeMap;

use anyhow::Context as _;
use runtime::SessionId;

use super::{Mask, Store, insert_in};
use crate::SessionMoved;

impl Store {
    /// Moved metadata per seq of `session`; malformed JSON is a store error
    /// (fail closed, like [`Store::advisories`]).
    pub async fn moved(&self, session: SessionId) -> anyhow::Result<BTreeMap<i64, SessionMoved>> {
        let rows: Vec<(i64, String)> = sqlx::query_as(
            "SELECT seq, moved FROM messages WHERE session_id = ? AND moved IS NOT NULL ORDER BY seq",
        )
        .bind(session.to_string())
        .fetch_all(&self.read)
        .await?;
        rows.into_iter()
            .map(|(seq, raw)| {
                let m = serde_json::from_str(&raw)
                    .with_context(|| format!("bad moved metadata on message {seq} of {session}"))?;
                Ok((seq, m))
            })
            .collect()
    }

    /// Whether `session` ever ran Python (a `python` tool call): its heap is a
    /// local file, so a move resets the runtime (§5.3 `runtime`).
    pub async fn ran_python(&self, session: SessionId) -> anyhow::Result<bool> {
        Ok(sqlx::query_scalar(
            "SELECT EXISTS(SELECT 1 FROM tool_calls WHERE session_id = ? AND name = 'python')",
        )
        .bind(session.to_string())
        .fetch_one(&self.read)
        .await?)
    }

    /// Append a moved user row with its metadata in one transaction; returns
    /// the seq. 183 R1 (S5): the row is in context only when `session` has no
    /// open turn *inside this transaction* (the write connection serializes
    /// it against every step and input), so no context-visible row lands
    /// mid-turn. With a turn open it also writes `excluded = {"whole": true}`:
    /// a card for the UIs, never model context (an open turn reads the
    /// extended `<engine_restart/>` instead). Opens no turn.
    pub async fn append_moved(
        &self,
        session: SessionId,
        text: &str,
        moved: &SessionMoved,
    ) -> anyhow::Result<i64> {
        let json = serde_json::to_string(moved)?;
        let sid = session.to_string();
        let mut tx = self.begin_write().await?;
        // The newest step of some turn is not `turn_finished` (`open_turn`'s predicate).
        let open: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM (SELECT kind, ROW_NUMBER() OVER (PARTITION BY turn_id \
                 ORDER BY (kind = 'assistant_partial'), n DESC) AS rn \
               FROM turn_steps WHERE session_id = ?) \
             WHERE rn = 1 AND kind != 'turn_finished' LIMIT 1",
        )
        .bind(&sid)
        .fetch_optional(&mut *tx)
        .await?;
        let excluded = if open.is_some() {
            Mask {
                whole: true,
                ..Default::default()
            }
            .encode()?
        } else {
            None
        };
        let seq = insert_in(
            &mut tx,
            &sid,
            &genai::chat::ChatMessage::user(text),
            None,
            None,
        )
        .await?;
        sqlx::query("UPDATE messages SET moved = ?, excluded = ? WHERE session_id = ? AND seq = ?")
            .bind(&json)
            .bind(&excluded)
            .bind(&sid)
            .bind(seq)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(seq)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HistoryEntry;
    use crate::store::turn_openers;
    use genai::chat::ChatMessage;

    pub(crate) fn sample() -> SessionMoved {
        SessionMoved {
            to: "box".into(),
            from: "mac".into(),
            to_id: "b".into(),
            from_id: "a".into(),
            os: "linux".into(),
            os_name: "Arch Linux".into(),
            arch: "x86_64".into(),
            host: "box.lan".into(),
            cwd: "/w".into(),
            cwd_missing: false,
            cwd_was: None,
            at: chrono::Utc::now(),
            reason: "user".into(),
            jobs_left: 0,
            runtime: "none".into(),
            approvals: "ask".into(),
            messaging: true,
            pending: false,
        }
    }

    #[tokio::test]
    async fn moved_row_opens_no_turn_and_masks_when_out_of_context() {
        let scratch = crate::TestDir::new("moved");
        let store = Store::open(&scratch.0.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let id = SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        store.append(id, &ChatMessage::user("hi")).await.unwrap();
        store
            .append(id, &ChatMessage::assistant("yo"))
            .await
            .unwrap();
        let m = sample();
        // 183 R1 (S5): in context while idle, out of it once a turn is open.
        let a = store
            .append_moved(id, "<session_moved/>", &m)
            .await
            .unwrap();
        store
            .step(
                id,
                runtime::TurnId::new(),
                crate::durability::StepKind::InputAccepted,
                None,
                &serde_json::json!({}),
            )
            .await
            .unwrap();
        let b = store
            .append_moved(id, "<session_moved/>", &m)
            .await
            .unwrap();
        store.append(id, &ChatMessage::user("again")).await.unwrap();

        let h = store.context_history(id).await.unwrap();
        let moved = store.moved(id).await.unwrap();
        assert_eq!(moved.keys().copied().collect::<Vec<_>>(), [a, b]);
        let openers = turn_openers(&h.messages, |s| moved.contains_key(&s));
        assert_eq!(openers, [0, b + 1]);
        // Out of context: the masked row is gone from the model's history.
        assert!(!h.entries[&a].removed);
        assert!(h.entries[&b].removed);
        assert!(store.effective_message(id, b).await.unwrap().is_none());

        let rows = crate::supervisor::render_with(
            &h.messages,
            &h.entries,
            &h.thinking,
            &BTreeMap::new(),
            &moved,
        );
        assert!(
            rows.iter()
                .any(|(s, e)| *s == a
                    && matches!(e, HistoryEntry::Moved { moved, .. } if *moved == m))
        );
    }
}
