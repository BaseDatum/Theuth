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
