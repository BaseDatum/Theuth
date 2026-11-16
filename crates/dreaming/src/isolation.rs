//! Read-only provenance check for synchronous dreaming file boundaries.
use std::path::Path;

/// Unknown rows are not isolated; unreadable identity stores fail closed.
pub fn excluded(data_dir: &Path, session: runtime::SessionId) -> bool {
    let path = data_dir.join("sessions.sqlite");
    if !path.exists() {
        return false;
    }
    let read = || -> rusqlite::Result<bool> {
        let db = rusqlite::Connection::open_with_flags(
            path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )?;
        db.query_row("WITH RECURSIVE lineage(id, parent, knowledge_identity) AS (
            SELECT id, parent, knowledge_identity FROM sessions WHERE id = ?1
            UNION SELECT s.id, s.parent, s.knowledge_identity FROM sessions s JOIN lineage l ON s.id = l.parent
        ) SELECT EXISTS(SELECT 1 FROM lineage WHERE json_extract(knowledge_identity, '$[1]') = 1)",
            [session.to_string()], |r| r.get(0))
    };
    read().unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn isolated_journals_sessions_counters_rewinds_and_cases_are_excluded() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        let store = agent::store::Store::open(&d.join("sessions.sqlite"), None)
            .await
            .unwrap();
        let (root, child, plain) = (
            runtime::SessionId::new(),
            runtime::SessionId::new(),
            runtime::SessionId::new(),
        );
        store
            .create_session(root, "fake", None, None, None)
            .await
            .unwrap();
        store.set_memory_identity(root, "ops", true).await.unwrap();
        store
            .create_session(child, "fake", Some(root), None, None)
            .await
            .unwrap();
        store
            .create_session(plain, "fake", None, None, None)
            .await
            .unwrap();
        for session in [root, child] {
            assert!(excluded(d, session));
            store
                .append(
                    session,
                    &genai::chat::ChatMessage::system(
                        "<knowledge>\n- `facts/secret` — isolated\n</knowledge>",
                    ),
                )
                .await
                .unwrap();
            store
                .inner_call_started(
                    session,
                    "python.1",
                    "knowledge_get",
                    &serde_json::json!({"id": "facts/secret"}),
                )
                .await
                .unwrap();
        }
        assert!(!excluded(d, plain));
        assert!(crate::dry_run::uses_from_sessions(d).await.is_empty());
        assert_eq!(
            crate::dry_run::read_sessions(d, "2000-01-01T00:00:00Z")
                .await
                .0,
            1
        );
        std::fs::create_dir_all(d.join("agents")).unwrap();
        std::fs::write(
            d.join("agents/ops.md"),
            "---\nisolated_memory: true\n---\nOps.",
        )
        .unwrap();
        let journals = d.join(roster::desk::JOURNAL_DIR);
        std::fs::create_dir_all(&journals).unwrap();
        std::fs::write(journals.join("ops.md"), "isolated journal").unwrap();
        std::fs::write(journals.join("ordinary.md"), "ordinary journal").unwrap();
        assert_eq!(
            crate::ledger::copy_desk_journals(d, "night"),
            ["ordinary.md"]
        );
        let rewind = agent::LedgerRewind {
            session: child,
            at: "2026-11-16T00:00:00Z".into(),
            dropped_assistant_text: "secret".into(),
        };
        std::fs::create_dir_all(crate::ledger::pending_dir(d)).unwrap();
        std::fs::write(
            crate::ledger::pending_dir(d).join("isolated.json"),
            serde_json::to_vec(&rewind).unwrap(),
        )
        .unwrap();
        assert!(crate::ledger::pending(d).is_empty());
        let mut case = crate::cases::sample("case-isolated", "read");
        case.source_session = child;
        assert!(crate::cases::append(d, &case, "night").is_err());
        // Even an older imported case cannot re-enter the candidate or replay pool.
        std::fs::create_dir_all(d.join("dreaming")).unwrap();
        std::fs::write(
            crate::cases::path(d),
            format!("{}\n", serde_json::to_string(&case).unwrap()),
        )
        .unwrap();
        assert!(crate::cases::all(d).unwrap().is_empty());
        store.close().await;
    }
}
