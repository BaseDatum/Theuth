//! 144 §8 W-k: `sessions.notebook` — bind/unbind on the root, children inherit, `notebook_chats`
//! newest first and roots only.

use agent::store::Store;
use runtime::SessionId;

async fn fresh() -> (Store, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("theuth-nb-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Store::open(&dir.join("sessions.sqlite"), None)
        .await
        .unwrap();
    (store, dir)
}

#[tokio::test]
async fn binding_is_root_metadata_and_chats_list_newest_first() {
    let (store, dir) = fresh().await;
    let a = SessionId::new();
    let b = SessionId::new();
    let plain = SessionId::new();
    for id in [a, b, plain] {
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
    }
    let child = SessionId::new();
    store
        .create_session(child, "m", Some(a), Some("sub"), None)
        .await
        .unwrap();

    // Unbound: `None` everywhere, no chats.
    assert_eq!(store.effective_notebook(a).await.unwrap(), None);
    assert!(store.notebook_chats("nb1").await.unwrap().is_empty());

    // Binding through the child lands on the root; the child inherits.
    assert_eq!(store.set_notebook(child, Some("nb1")).await.unwrap(), a);
    assert_eq!(store.set_notebook(b, Some("nb1")).await.unwrap(), b);
    assert_eq!(
        store.effective_notebook(child).await.unwrap().as_deref(),
        Some("nb1")
    );
    assert_eq!(
        store.session(a).await.unwrap().unwrap().notebook.as_deref(),
        Some("nb1")
    );
    assert_eq!(store.session(plain).await.unwrap().unwrap().notebook, None);

    // Roots only, newest first (b was created after a), one row per root.
    let chats = store.notebook_chats("nb1").await.unwrap();
    let ids: Vec<SessionId> = chats.iter().map(|s| s.id).collect();
    assert_eq!(ids, vec![b, a]);
    assert_eq!(
        store.notebook_chat_counts().await.unwrap(),
        vec![("nb1".to_owned(), 2)]
    );

    // Unbind; unknown session is an error.
    store.set_notebook(a, None).await.unwrap();
    assert_eq!(store.effective_notebook(child).await.unwrap(), None);
    assert_eq!(
        store.notebook_chats("nb1").await.unwrap().len(),
        1,
        "only b stays bound"
    );
    assert_eq!(
        store.notebook_chat_counts().await.unwrap(),
        vec![("nb1".to_owned(), 1)],
        "`Notebook.chats` counts the same roots the list shows"
    );
    assert!(
        store
            .set_notebook(SessionId::new(), Some("nb1"))
            .await
            .is_err()
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// 144 §5.1 (E1): migration 0045 applies; `session_context` rows set / overwrite / clear per session,
/// the pins map clamps sessions and artifacts to `mention` and drops unreadable levels.
#[tokio::test]
async fn session_context_rows_round_trip_per_session() {
    let (store, dir) = fresh().await;
    let a = SessionId::new();
    let b = SessionId::new();
    assert!(store.context_get(a).await.unwrap().is_empty());
    store
        .context_set(a, "S1", "source", "summary")
        .await
        .unwrap();
    store.context_set(a, "N1", "note", "off").await.unwrap();
    store.context_set(b, "S1", "source", "full").await.unwrap();
    assert_eq!(
        store.context_get(a).await.unwrap(),
        vec![
            ("S1".to_owned(), "source".to_owned(), "summary".to_owned()),
            ("N1".to_owned(), "note".to_owned(), "off".to_owned()),
        ]
    );
    // Overwrite keeps one row per (session, item); the other session is untouched.
    store.context_set(a, "S1", "source", "full").await.unwrap();
    let rows = store.context_get(a).await.unwrap();
    assert_eq!(rows.len(), 2);
    assert!(rows.contains(&("S1".to_owned(), "source".to_owned(), "full".to_owned())));
    assert_eq!(
        store.context_get(b).await.unwrap(),
        vec![("S1".to_owned(), "source".to_owned(), "full".to_owned())]
    );
    // Clear → back to the default rule for that item only.
    store.context_clear(a, "S1").await.unwrap();
    assert_eq!(
        store.context_get(a).await.unwrap(),
        vec![("N1".to_owned(), "note".to_owned(), "off".to_owned())]
    );
    let pins = agent::notebook_context::pins_of(vec![
        ("N1".into(), "note".into(), "off".into()),
        ("C1".into(), "session".into(), "full".into()),
        ("X".into(), "artifact".into(), "nope".into()),
    ]);
    assert_eq!(pins.get("N1"), Some(&agent::notebook_context::Level::Off));
    assert_eq!(
        pins.get("C1"),
        Some(&agent::notebook_context::Level::Mention)
    );
    assert!(!pins.contains_key("X"));
    std::fs::remove_dir_all(&dir).unwrap();
}
