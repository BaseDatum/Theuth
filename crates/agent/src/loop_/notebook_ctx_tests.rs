//! 157 D2–D5 through the real [`Turn`] at the fake provider of [`super::vision_fallback_tests`], the
//! notebook served by fake registry tools: the block once (turn 1), nothing when nothing changed (turn 2),
//! one `<notebook_changes>` with the id and none of the content after a source edit (turn 3) and after a
//! dial change, the manifest only after a compaction, no spurious notice after a restart (every direct
//! turn here is rebuilt from the store), and no re-injection when an endless session folds.

use std::sync::Arc;

use futures::future::BoxFuture;
use kameo::actor::Spawn as _;
use parking_lot::Mutex;
use serde_json::{Value, json};
use tools::{ErasedTool, Risk, ToolCx, ToolOutput, ToolSpec};

use super::vision_fallback_tests::{Fake, Harness};
use super::*;
use crate::endless::{ViewBlock, ViewRender};

const NB: &str = "NB157";
const SECRET: &str = "RUNBOOK-STEP-FOUR-SECRET";

/// The notebook the fake tools serve; the tests edit it between turns.
#[derive(Clone)]
struct Nb {
    description: String,
    /// `(kind, id, title, body)`.
    items: Vec<(&'static str, &'static str, String, String)>,
}

impl Nb {
    fn initial() -> Self {
        Self {
            description: "Verify the DR plan.".into(),
            items: vec![
                (
                    "source",
                    "S1",
                    "Runbook v7".into(),
                    format!("<!-- r1 -->\n## Steps\n\n<!-- r2 -->\nStep one. {SECRET}"),
                ),
                (
                    "note",
                    "N1",
                    "Drill timeline".into(),
                    "<!-- a -->\n# Drill timeline\n\n<!-- b -->\nT+0 failover".into(),
                ),
            ],
        }
    }
}

type Shared = Arc<Mutex<Nb>>;

/// `notebook_get` / `source_get` / `doc_read` over the shared notebook, by name.
struct NbTool {
    name: &'static str,
    nb: Shared,
}

impl ErasedTool for NbTool {
    fn spec(&self) -> ToolSpec {
        ToolSpec {
            name: self.name.into(),
            description: "fake".into(),
            schema: json!({"type": "object"}),
            risk: Risk::ReadOnly,
            direct: true,
            returns: "str".into(),
        }
    }

    fn call(&self, args: Value, _: ToolCx) -> BoxFuture<'_, anyhow::Result<ToolOutput>> {
        let nb = self.nb.lock().clone();
        let name = self.name;
        Box::pin(async move {
            Ok(match name {
                "notebook_get" => ToolOutput {
                    structured: Some(json!({
                        "notebook": {"doc": {"title": "DR verification"}},
                        "items": nb.items.iter().enumerate().map(|(n, (kind, id, _, _))| json!({"kind": kind, "item": id, "added_ms": 1000 + n as i64})).collect::<Vec<_>>(),
                    })),
                    ..ToolOutput::ok("")
                },
                "source_get" => {
                    let id = args["id"].as_str().unwrap_or_default();
                    let title = nb
                        .items
                        .iter()
                        .find(|(_, i, _, _)| *i == id)
                        .map(|(_, _, t, _)| t.clone())
                        .unwrap_or_default();
                    ToolOutput {
                        structured: Some(json!({"doc": {"title": title}, "words": 7, "origin": {"kind": "file"}, "status": "completed"})),
                        ..ToolOutput::ok("")
                    }
                }
                _ => {
                    let doc = args["doc"].as_str().unwrap_or_default();
                    let body = if doc == format!("notebook:{NB}") {
                        Some(nb.description.clone())
                    } else {
                        let id = doc.strip_prefix("source:").unwrap_or(doc);
                        nb.items
                            .iter()
                            .find(|(_, i, _, _)| *i == id)
                            .map(|(_, _, _, b)| b.clone())
                    };
                    match body {
                        Some(b) => ToolOutput::ok(b),
                        None => ToolOutput::err(format!("no such document {doc}")),
                    }
                }
            })
        })
    }
}

/// The process-global registry with the three fakes over `nb`.
async fn registry(nb: &Shared) -> crate::TestDir {
    let scratch = crate::TestDir::new("nb-ctx-reg");
    let reg_rt = Runtime::new(figment::Figment::new(), scratch.0.clone());
    let registry = tools::registry::start(&reg_rt).await.unwrap();
    for name in ["notebook_get", "source_get", "doc_read"] {
        registry
            .ask(tools::registry::RegisterTool(Arc::new(NbTool {
                name,
                nb: nb.clone(),
            })))
            .await
            .unwrap();
    }
    scratch
}

/// One direct turn over the stored frame (a fresh `Turn` each time: a restart per turn).
async fn turn(h: &mut Harness, text: &str) {
    let (turn, tid) = h.turn("openai/textonly", CancellationToken::new()).await;
    turn.run(tid, vec![h.input(text, false)]).await;
    h.drain().await;
}

/// The user rows of the frame that open with `<notebook` (block, manifest or notice), in order.
async fn notebook_rows(h: &Harness) -> Vec<String> {
    let (_, _, history) = h.store.frame(h.id).await.unwrap();
    history
        .iter()
        .filter(|(_, m)| m.role == ChatRole::User)
        .map(|(_, m)| text_of(&m.content))
        .filter(|t| t.starts_with("<notebook"))
        .collect()
}

#[test]
fn block_once_then_notices_then_manifest_after_compaction() {
    crate::with_test_registry(async {
        let nb: Shared = Arc::new(Mutex::new(Nb::initial()));
        let _reg = registry(&nb).await;
        let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
        h.store.set_notebook(h.id, Some(NB)).await.unwrap();

        // Turn 1: the block at the dial levels, content included, fingerprints on every line.
        turn(&mut h, "hi").await;
        let rows = notebook_rows(&h).await;
        assert_eq!(rows.len(), 1, "{rows:#?}");
        let block = &rows[0];
        assert!(block.starts_with(&format!("<notebook id=\"{NB}\" title=\"DR verification\">")));
        assert!(block.contains(SECRET), "turn 1 carries the content");
        assert!(block.contains("id=\"S1\" title=\"Runbook v7\" origin=\"file\" level=\"full\" status=\"available\" s=\""));
        assert!(block.contains("id=\"N1\" title=\"Drill timeline\" level=\"full\" status=\"available\" s=\""));
        assert!(!block.contains("manifest=\"true\""));
        println!("--- 157 example: first block ---\n{block}\n");

        // Turn 2, nothing changed: no `<notebook` row at all (a restart: the turn is rebuilt from the store).
        turn(&mut h, "and then?").await;
        assert_eq!(notebook_rows(&h).await.len(), 1, "unchanged notebook adds nothing");

        // A source refresh (renamed too) and a note edit before turn 3: exactly one notice, the ids and read
        // calls on it, none of the content.
        {
            let mut n = nb.lock();
            n.items[0].2 = "Runbook v8".into();
            n.items[0].3 = format!("{} Step two. NEW-STEP-TEXT", n.items[0].3);
            n.items[1].3.push_str("\n\n<!-- c -->\nT+5 DNS flipped EDITED-NOTE");
        }
        turn(&mut h, "what changed?").await;
        let rows = notebook_rows(&h).await;
        assert_eq!(rows.len(), 2, "{rows:#?}");
        let notice = &rows[1];
        assert!(notice.starts_with(&format!("<notebook_changes notebook=\"{NB}\">\n")));
        assert_eq!(notice.matches("<change ").count(), 2, "{notice}");
        assert!(notice.contains("<change kind=\"source\" id=\"S1\" title=\"Runbook v8\" level=\"full\" s=\""));
        assert!(notice.contains("renamed from \"Runbook v7\"; content changed (text or insights); read it with doc_read(\"source:S1\") when it matters (cite theuth://sources/S1)"), "{notice}");
        assert!(notice.contains("<change kind=\"note\" id=\"N1\" title=\"Drill timeline\" level=\"full\" s=\""));
        assert!(notice.contains("edited; read it with doc_read(\"N1\")"), "{notice}");
        for leak in [SECRET, "NEW-STEP-TEXT", "EDITED-NOTE", "Step one", "T+0", "T+5"] {
            assert!(!notice.contains(leak), "the notice leaks {leak:?}: {notice}");
        }
        println!("--- 157 example: change notice ---\n{notice}\n");

        // Turn 4, nothing changed since the notice: nothing (the notice updated the baseline).
        turn(&mut h, "ok").await;
        assert_eq!(notebook_rows(&h).await.len(), 2);

        // A dial change (D7): one line, no re-injection; `off` has its own wording.
        h.store
            .context_set(h.id, "S1", "source", "off")
            .await
            .unwrap();
        turn(&mut h, "drop the runbook").await;
        let rows = notebook_rows(&h).await;
        assert_eq!(rows.len(), 3, "{rows:#?}");
        assert_eq!(rows[2].matches("<change ").count(), 1, "{}", rows[2]);
        assert!(rows[2].contains("<change kind=\"source\" id=\"S1\" title=\"Runbook v8\" level=\"off\">now off: leave it out unless the user brings it back</change>"), "{}", rows[2]);
        h.store
            .context_set(h.id, "S1", "source", "summary")
            .await
            .unwrap();
        turn(&mut h, "bring it back as a summary").await;
        let rows = notebook_rows(&h).await;
        assert_eq!(rows.len(), 4, "{rows:#?}");
        assert!(rows[3].contains("<change kind=\"source\" id=\"S1\" title=\"Runbook v8\" level=\"summary\" s=\""), "{}", rows[3]);
        assert!(rows[3].contains(">in context now at summary; read it with doc_read(\"source:S1\")"), "{}", rows[3]);
        assert!(!rows[3].contains(SECRET));

        // A compaction (`/clear`-style fold so the whole epoch, block included, is folded): the next
        // turn gets the manifest only — every non-off item as one line, the sentence, none of the content.
        let (mut t, _) = h.turn("openai/textonly", CancellationToken::new()).await;
        let epoch = t
            .compact_now(Some(CompactStyle::Clear), Some("SUMMARY-EPOCH-HEAD".into()))
            .await
            .unwrap();
        assert_eq!(epoch, 1);
        h.drain_until(|e| matches!(e, AgentEvent::Compacted { .. }))
            .await;
        turn(&mut h, "after the fold").await;
        let rows = notebook_rows(&h).await;
        assert_eq!(rows.len(), 1, "{rows:#?}");
        let manifest = &rows[0];
        assert!(manifest.starts_with(&format!("<notebook id=\"{NB}\" title=\"DR verification\" manifest=\"true\">")));
        assert!(manifest.contains("<item kind=\"source\" id=\"S1\" title=\"Runbook v8\" level=\"summary\" s=\""));
        assert!(manifest.contains("<item kind=\"note\" id=\"N1\" title=\"Drill timeline\" level=\"full\" s=\""));
        assert!(manifest.contains("The contents of these items were in context earlier in this chat; read what you need with doc_read / nb_search."));
        for leak in [SECRET, "NEW-STEP-TEXT", "EDITED-NOTE", "Step one", "T+0"] {
            assert!(!manifest.contains(leak), "the manifest leaks {leak:?}: {manifest}");
        }
        println!("--- 157 example: post-compaction manifest ---\n{manifest}\n");

        // The epoch after: the manifest is the baseline — nothing changed, nothing sent; an edit is a notice.
        turn(&mut h, "still there?").await;
        assert_eq!(notebook_rows(&h).await.len(), 1);
        nb.lock().description = "Verify the DR plan, then the failback.".into();
        turn(&mut h, "description?").await;
        let rows = notebook_rows(&h).await;
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert!(rows[1].contains(&format!("<change kind=\"notebook\" id=\"{NB}\" s=\"")), "{}", rows[1]);
        assert!(rows[1].contains(&format!("description changed; read it with doc_read(\"notebook:{NB}\")")));
        assert!(!rows[1].contains("failback"));
        // The frame as the model sees it holds exactly: head, then one manifest, one notice.
        let (epoch, head, _) = h.store.frame(h.id).await.unwrap();
        assert_eq!(epoch, 1);
        assert!(head.is_some());
    });
}

/// 157 D2 under endless memory (137): the frame is rebuilt from the view every turn and `state.history`
/// is cleared, so the baseline is read from the store's rows — the second turn sends nothing, a change
/// is one notice, and the block is never re-injected.
#[test]
fn endless_session_does_not_reinject_when_it_folds() {
    crate::with_test_registry(async {
        let nb: Shared = Arc::new(Mutex::new(Nb::initial()));
        let _reg = registry(&nb).await;
        let mut h = Harness::new(Fake::compact_held(Duration::ZERO)).await;
        h.store.set_notebook(h.id, Some(NB)).await.unwrap();
        let (epoch, head, history) = h.store.frame(h.id).await.unwrap();
        let session = Session::new(
            h.id,
            0,
            (h.resolve("openai/textonly"), None),
            None,
            SessionState::resume(epoch, head, history),
            Arc::new(AgentConfig::default()),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            Arc::default(),
            h.store.clone(),
            h.client.clone(),
            Auth::stub(&h.dir),
            h.rt.clone(),
            h.catalog.clone(),
            None,
            Arc::new(Semaphore::new(1)),
            SessionResources::durable_arc(&h.dir),
        );
        *session.shared.test_view.lock() = Some(ViewRender {
            blocks: vec![ViewBlock {
                text: "<chat>\n0+2|user: hello; talk: hi\n</chat>".into(),
                mark: true,
            }],
            settled: true,
            unbuilt: 0,
        });
        let actor = Session::spawn(session);
        async fn send(h: &mut Harness, actor: &ActorRef<Session>, text: &str) {
            actor
                .tell(UserInput {
                    id: None,
                    text: text.into(),
                    files: Vec::new(),
                    ticket: None,
                    scope: None,
                    defer: false,
                })
                .await
                .unwrap();
            h.drain().await;
        }
        send(&mut h, &actor, "hi").await;
        let rows = notebook_rows(&h).await;
        assert_eq!(rows.len(), 1, "{rows:#?}");
        assert!(rows[0].starts_with("<notebook id=") && rows[0].contains(SECRET));
        // The fold: `endless_start` cleared the in-memory history before this turn; the store still
        // holds the block, so nothing is injected again.
        send(&mut h, &actor, "again").await;
        send(&mut h, &actor, "and again").await;
        assert_eq!(notebook_rows(&h).await.len(), 1, "no re-injection across endless turns");
        nb.lock().items[1].3.push_str("\n\n<!-- c -->\nmore");
        send(&mut h, &actor, "note?").await;
        let rows = notebook_rows(&h).await;
        assert_eq!(rows.len(), 2, "{rows:#?}");
        assert!(rows[1].starts_with("<notebook_changes") && rows[1].contains("id=\"N1\""));
        assert!(!rows[1].contains("more"));
        send(&mut h, &actor, "done").await;
        assert_eq!(notebook_rows(&h).await.len(), 2);
    });
}
