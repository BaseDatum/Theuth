//! End-to-end over a real store with a scripted compactor: attach → pump →
//! settle → render → tools → `/endless off|on`.

use std::sync::Arc;
use std::time::Duration;

use futures::future::BoxFuture;
use genai::chat::{ChatMessage, ContentPart, MessageContent, ToolCall, ToolResponse};
use parking_lot::Mutex;
use runtime::{Runtime, SessionId};

use super::compactor::{Compactor, Reply};
use super::{Attached, Endless, EndlessConfig, MemoryMode};
use crate::store::Store;

/// Echoes the step input (the text after the last `bytes:\n`) cut to 400
/// bytes — grounded, no ruler, no status words of its own.
struct Echo {
    calls: Mutex<u32>,
    model: models::Resolved,
}

impl Compactor for Echo {
    fn compress<'a>(&'a self, _ctx: &'a str, convo: &'a [ChatMessage]) -> BoxFuture<'a, Result<Reply, String>> {
        *self.calls.lock() += 1;
        let step = convo
            .first()
            .and_then(|m| m.content.iter().next())
            .and_then(|p| match p {
                ContentPart::Text(t) => Some(t.as_str()),
                _ => None,
            })
            .unwrap_or("");
        let input = step.rsplit_once("bytes:\n").map_or(step, |(_, s)| s);
        let line = super::guards::cut_utf8(input, 400).replace('\n', " ");
        Box::pin(async move {
            Ok(Reply {
                text: line,
                usage: serde_json::from_str(r#"{"prompt_tokens":1000,"completion_tokens":50,"prompt_tokens_details":{"cached_tokens":900}}"#).ok(),
                latency: Duration::from_millis(1),
            })
        })
    }

    fn model(&self) -> &models::Resolved {
        &self.model
    }
}

async fn harness() -> (Endless, Store, Arc<Echo>, SessionId, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("theuth-endless-e2e-{}", uuid::Uuid::now_v7()));
    std::fs::create_dir_all(&dir).unwrap();
    let store = Store::open(&dir.join("sessions.sqlite"), None).await.unwrap();
    let echo = Arc::new(Echo {
        calls: Mutex::new(0),
        model: super::pump::resolved_for_tests("fake/compactor"),
    });
    let mut endless = Endless::new(
        store.clone(),
        Runtime::new(figment::Figment::new(), dir.clone()),
        genai::Client::builder().build().unwrap(),
        crate::supervisor::Auth::stub(&dir),
        Arc::new(models::Catalog::load(&dir)),
        EndlessConfig {
            view_bytes: 16_000,
            settle_secs: 5,
            ..EndlessConfig::default()
        },
    );
    Arc::get_mut(&mut endless.inner).unwrap().compactor_for_tests = Some(echo.clone());
    let id = SessionId::new();
    store.create_session(id, "fake/session", None, None, None).await.unwrap();
    (endless, store, echo, id, dir)
}

fn attached() -> Attached {
    Attached {
        model: super::pump::resolved_for_tests("fake/session"),
        effort: None,
        agent: "theuth".into(),
        store: None,
    }
}

async fn seed(store: &Store, id: SessionId, long: &str) {
    store.append(id, &ChatMessage::user("hello, please look at the nginx config")).await.unwrap();
    store
        .append(
            id,
            &ChatMessage::assistant(MessageContent::from_parts(vec![
                ContentPart::Text("on it".into()),
                ContentPart::ToolCall(ToolCall {
                    call_id: "c1".into(),
                    fn_name: "read".into(),
                    fn_arguments: serde_json::json!({"path": "/etc/nginx/nginx.conf"}),
                    thought_signatures: None,
                }),
            ])),
        )
        .await
        .unwrap();
    store
        .append(id, &ChatMessage::from(vec![ToolResponse::new("c1", long.to_owned())]))
        .await
        .unwrap();
    store.append(id, &ChatMessage::assistant(long.to_owned())).await.unwrap();
    store.append(id, &ChatMessage::user("ok, keep the worker_processes line as it is")).await.unwrap();
}

/// A fresh endless session: units derive from the rows, free and model-built
/// nodes land, the view settles, tools answer, the ledger fills, `/endless
/// off` writes the head and `on` needs no backfill.
#[tokio::test]
async fn pump_builds_tree_and_tools_answer() {
    let (endless, store, echo, id, _dir) = harness().await;
    store.set_session_memory(id, MemoryMode::Endless).await.unwrap();
    let long = (0..60)
        .map(|k| format!("worker_processes auto; line {k} of the nginx config with error_log /var/log/nginx/error.log"))
        .collect::<Vec<_>>()
        .join("\n");
    seed(&store, id, &long).await;

    endless.attach(id, attached()).await.unwrap();
    assert_eq!(endless.mode(id).await.unwrap(), MemoryMode::Endless);
    assert!(endless.active(id), "no history before the mode: no backfill");
    assert!(endless.settle(id, Duration::from_secs(10)).await, "settled");

    let render = endless.render_for_turn(id).await.unwrap();
    assert!(render.settled && render.unbuilt == 0);
    let text = render.text();
    assert!(text.starts_with("<chat>\n") && text.ends_with("</chat>"));
    assert!(text.contains("0+1|user: hello, please look at the nginx config"), "{text}");
    assert_eq!(text.lines().count(), 6 + 2, "six units, one line each under budget");
    assert!(*echo.calls.lock() >= 2, "the two long units needed the model");

    let mem = endless.memory(id).await.unwrap();
    assert_eq!(mem.mode, MemoryMode::Endless);
    assert_eq!(mem.units, 6);
    assert!(mem.view.iter().all(|l| l.built));
    assert_eq!(mem.model, "fake/compactor");
    assert!(mem.stats.calls >= 2 && mem.stats.input_tokens >= 2000 && mem.stats.cache_read_tokens >= 1800);
    assert!(mem.backfill.is_none());

    let nodes = store.endless_nodes(id).await.unwrap();
    assert!(nodes.iter().any(|n| n.l == 2), "merges above level 1 built: {}", nodes.len());
    assert!(nodes.iter().any(|n| n.l == 0 && n.free) && nodes.iter().any(|n| n.l == 0 && !n.free));
    let kinds: Vec<String> = sqlx::query_scalar("SELECT kind FROM model_calls WHERE owner_id = ?")
        .bind(id.to_string())
        .fetch_all(store.pool())
        .await
        .unwrap();
    assert!(!kinds.is_empty() && kinds.iter().all(|k| k == "endless"), "{kinds:?}");

    // Tools.
    let whole = endless.zoom(id, 0, 1).await.unwrap();
    assert_eq!(whole[0].text, "user: hello, please look at the nginx config");
    let children = endless.zoom(id, 0, 2).await.unwrap();
    assert_eq!((children[0].id, children[0].n, children[1].id), (0, 1, 1));
    assert_eq!(endless.zoom(id, 1, 2).await.unwrap_err(), "No line 1+2.");
    assert_eq!(endless.zoom(id, 8, 1).await.unwrap_err(), "No line 8+1.");
    let hits = endless.recall(id, "worker_processes", 8).await.unwrap();
    assert!(!hits.is_empty() && hits.iter().all(|h| h.n == 1), "{hits:?}");
    let raw = endless.recall(id, "zzz-nowhere", 8).await.unwrap();
    assert!(raw.is_empty());
    assert!(endless.date(id, 0).await.unwrap().is_some());
    assert_eq!(endless.date(id, 99).await.unwrap_err(), "No message 99.");

    // /endless off → head, then on → no backfill (the tree exists).
    let off = endless.set_mode(id, MemoryMode::Epochs, 3).await.unwrap();
    let head = off.head.unwrap();
    assert!(head.starts_with("<context_summary epoch=\"3\" style=\"endless\">\n<chat>\n"));
    assert!(head.ends_with("</chat>\n</context_summary>"));
    assert_eq!(off.memory.mode, MemoryMode::Epochs);
    assert!(off.memory.view.is_empty());
    assert!(!endless.active(id));
    let on = endless.set_mode(id, MemoryMode::Endless, 3).await.unwrap();
    assert!(on.head.is_none() && on.memory.backfill.is_none());
    assert!(endless.active(id));
    endless.detach(id);
}

/// D14: history before the switch is a backfill — `active` stays false (the
/// loop keeps epochs) until level 0 covers it; a second attach (restart)
/// reloads the tree from rows without rebuilding.
#[tokio::test]
async fn history_backfills_then_activates_and_resumes_from_rows() {
    let (endless, store, echo, id, _dir) = harness().await;
    seed(&store, id, "short tool output").await;
    endless.attach(id, attached()).await.unwrap();
    assert!(!endless.active(id), "epochs session");
    let on = endless.set_mode(id, MemoryMode::Endless, 1).await.unwrap();
    let mem = on.memory;
    assert!(mem.backfill.is_some_and(|b| b.total == 6) || endless.active(id), "{mem:?}");
    assert!(endless.settle(id, Duration::from_secs(10)).await);
    assert!(endless.active(id), "covered → active");
    // Settle waits for the view only; let the merges above finish (6 + 3 + 1 nodes).
    for _ in 0..200 {
        if store.endless_nodes(id).await.unwrap().len() == 10 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let built_before = store.endless_nodes(id).await.unwrap().len();
    assert_eq!(built_before, 10);
    let calls_before = *echo.calls.lock();
    endless.detach(id);

    // Restart: rows are the hand-off (8.7).
    let endless2 = {
        let (e, ..) = harness().await;
        e
    };
    assert_eq!(endless2.resume_all().await.unwrap(), 0, "another store: nothing to resume");
    endless.attach(id, attached()).await.unwrap();
    assert!(endless.active(id));
    assert!(endless.settle(id, Duration::from_secs(5)).await);
    assert_eq!(store.endless_nodes(id).await.unwrap().len(), built_before);
    assert_eq!(*echo.calls.lock(), calls_before, "nothing rebuilt");
    assert_eq!(endless.resume_all().await.unwrap(), 1);
}
