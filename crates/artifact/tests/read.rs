//! Design 66 §6.2 / §11 row 2: `artifact_read` by `call_id` — text ranges
//! with the version header, PDF → Markdown, image → picture, archive →
//! listing, app → source, a missing id, and the bound-document mention
//! (that last one only with the documents helper built).

use std::io::Write as _;
use std::sync::Arc;

use artifact::tools::{ReadArgs, ReadTool};
use artifact::{Commit, Kind, ShowArgs, State, Stores};
use runtime::SessionId;
use tools::{Tool, ToolCx};

fn cx() -> ToolCx {
    ToolCx {
        session: SessionId::new(),
        call_id: String::new(),
        quiet: false,
        resources: Arc::new(tools::SessionResources {
            root: std::env::temp_dir(),
            retention: tools::Retention::Durable,
            generation: 1,
        }),
        cancel: tools::CancellationToken::new(),
        promote_after: None,
    }
}

async fn open() -> (Arc<State>, ReadTool, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let rt = runtime::Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
    let state = Arc::new(State::open(dir.path(), rt, Vec::new()).await.unwrap());
    let stores = Arc::new(Stores::new(state.clone()));
    (state, ReadTool { stores }, dir)
}

fn args(call_id: &str) -> ReadArgs {
    ReadArgs {
        artifact: call_id.into(),
        ..ReadArgs::default()
    }
}

async fn show(
    state: &State,
    session: SessionId,
    call: &str,
    name: &str,
    text: &str,
) -> artifact::Artifact {
    state
        .show(
            session,
            call.into(),
            ShowArgs {
                content: Some(text.into()),
                name: Some(name.into()),
                ..ShowArgs::default()
            },
        )
        .await
        .unwrap()
        .artifact
}

#[tokio::test]
async fn text_pages_with_ranges_and_names_the_newer_version() {
    let (state, tool, _d) = open().await;
    let s = SessionId::new();
    let body: String = (1..=30).map(|i| format!("line {i}\n")).collect();
    let v1 = show(&state, s, "c1", "notes.txt", &body).await;
    let v2 = show(&state, s, "c2", "notes.txt", &format!("{body}line 31\n")).await;
    assert_eq!((v1.version, v2.version), (1, 2));

    let out = tool
        .call(
            ReadArgs {
                ranges: Some(vec![[2, 3], [29, 30]]),
                ..args("c1")
            },
            cx(),
        )
        .await
        .unwrap();
    assert!(!out.is_error, "{}", out.content);
    let first = out.content.lines().next().unwrap();
    assert!(
        first.starts_with("[artifact_read \"notes.txt\" (notes.txt v1 of 2, text, "),
        "{first}"
    );
    assert!(
        first.contains(&format!("; latest is {}", v2.call_id)),
        "{first}"
    );
    assert!(first.contains("; showing 2 ranges, 4 lines]"), "{first}");
    assert!(
        out.content
            .contains("@@ lines 2–3 @@\n 2\tline 2\n 3\tline 3"),
        "{}",
        out.content
    );
    assert!(out.content.contains("@@ lines 29–30 @@"), "{}", out.content);

    let out = tool
        .call(
            ReadArgs {
                tail: Some(1),
                ..args("c2")
            },
            cx(),
        )
        .await
        .unwrap();
    let first = out.content.lines().next().unwrap();
    assert!(first.contains("v2 of 2, text,"), "{first}");
    assert!(!first.contains("latest is"), "{first}");
    assert!(out.content.ends_with("31\tline 31"), "{}", out.content);
}

#[tokio::test]
async fn pdf_is_rendered_as_markdown() {
    let (state, tool, _d) = open().await;
    let s = SessionId::new();
    let a = show(
        &state,
        s,
        "c1",
        "report.pdf",
        "# Quarterly\n\nrevenue grew twelve percent\n",
    )
    .await;
    assert_eq!(a.kind, Kind::Pdf);
    let out = tool.call(args("c1"), cx()).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    let first = out.content.lines().next().unwrap();
    assert!(first.contains(", pdf, "), "{first}");
    assert!(first.contains(" pages, shown "), "{first}");
    assert!(
        out.content.contains("rendered as Markdown"),
        "{}",
        out.content
    );
    assert!(
        out.content.contains("revenue grew twelve percent"),
        "{}",
        out.content
    );
    assert!(!out.content.contains("%PDF"), "{}", out.content);
}

#[tokio::test]
async fn image_comes_back_as_a_picture() {
    let (state, tool, d) = open().await;
    let s = SessionId::new();
    let png = d.path().join("chart.png");
    std::fs::copy(
        concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/pdf/chart.png"),
        &png,
    )
    .unwrap();
    let a = state
        .show(
            s,
            "c1".into(),
            ShowArgs {
                path: Some(png),
                ..ShowArgs::default()
            },
        )
        .await
        .unwrap()
        .artifact;
    assert_eq!(a.kind, Kind::Image);
    let out = tool.call(args("c1"), cx()).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert_eq!(out.files, vec![a.path.clone()]);
    assert!(
        out.content
            .starts_with("[artifact_read \"chart.png\" (chart.png v1 of 1, image, "),
        "{}",
        out.content
    );
    assert!(
        out.content
            .ends_with(&format!(": image, {}]", artifact::kind::human(a.bytes))),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn archive_lists_its_members() {
    let (state, tool, d) = open().await;
    let s = SessionId::new();
    let zip_path = d.path().join("src.zip");
    {
        let mut w = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
        let opts = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        w.add_directory("src/", opts).unwrap();
        w.start_file("src/main.rs", opts).unwrap();
        w.write_all(b"fn main() {}").unwrap();
        w.start_file("README.md", opts).unwrap();
        w.write_all(b"# hi").unwrap();
        w.finish().unwrap();
    }
    let a = state
        .show(
            s,
            "c1".into(),
            ShowArgs {
                path: Some(zip_path),
                ..ShowArgs::default()
            },
        )
        .await
        .unwrap()
        .artifact;
    assert_eq!(a.kind, Kind::Archive);
    let out = tool.call(args("c1"), cx()).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    let mut lines = out.content.lines();
    let first = lines.next().unwrap();
    assert!(first.contains(": archive, 3 entries, "), "{first}");
    let rest: Vec<&str> = lines.collect();
    assert_eq!(
        rest,
        ["src/", "src/main.rs\t12 B", "README.md\t4 B"],
        "{}",
        out.content
    );
}

#[tokio::test]
async fn app_returns_its_source() {
    let (state, tool, _d) = open().await;
    let s = SessionId::new();
    let src = "root = TextContent(\"hi\")\n";
    let a = state
        .commit(Commit {
            session: s,
            call_id: "c1".into(),
            name: "panel.openui".into(),
            title: "Panel".into(),
            kind: Kind::App,
            bytes: src.as_bytes().to_vec(),
            source: Some(("app.json".into(), br#"{"library_hash":"h1"}"#.to_vec())),
            expected_base: None,
        })
        .await
        .unwrap();
    assert_eq!(a.kind, Kind::App);
    let out = tool.call(args("c1"), cx()).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    assert!(
        out.content
            .starts_with("[artifact_read \"Panel\" (panel.openui v1 of 1, app, "),
        "{}",
        out.content
    );
    assert!(
        out.content.ends_with("1\troot = TextContent(\"hi\")"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn missing_call_id_and_missing_blob_are_named() {
    let (state, tool, _d) = open().await;
    let out = tool.call(args("nope"), cx()).await.unwrap();
    assert!(out.is_error);
    assert_eq!(
        out.content,
        "artifact nope not found (deleted or from another data dir)"
    );

    let a = show(&state, SessionId::new(), "c1", "a.txt", "x\n").await;
    std::fs::remove_file(&a.path).unwrap();
    let out = tool.call(args("c1"), cx()).await.unwrap();
    assert!(out.is_error);
    assert!(out.content.ends_with("]: blob missing"), "{}", out.content);
    assert!(
        out.content.contains("(a.txt v1 of 1, text,"),
        "{}",
        out.content
    );
}

#[tokio::test]
async fn markdown_names_its_bound_document() {
    let bin = document::helper::helper_path();
    use document::actor::DocumentActor;
    use kameo::actor::Spawn as _;
    let (state, tool, d) = open().await;
    let rt = runtime::Runtime::new(figment::Figment::new(), d.path().to_path_buf());
    let store = document::store::Store::open(d.path()).await.unwrap();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let h = document::helper::Helper::new(bin, tx, rt.shutdown.clone());
    let actor = DocumentActor::spawn(DocumentActor::new(
        store,
        h,
        document::DocumentsConfig::default(),
        rt,
    ));
    actor.register(document::DOCUMENT_ACTOR).unwrap();
    let a2 = actor.clone();
    tokio::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = a2.tell(ev).await;
        }
    });
    let s = SessionId::new();
    show(
        &state,
        s,
        "c1",
        "report.md",
        "# Report\n\nfirst paragraph\n",
    )
    .await;
    let doc = actor
        .ask(document::Lookup {
            binding: document::Binding::Artifact {
                session: s.to_string(),
                name: "report.md".into(),
            },
        })
        .await
        .unwrap()
        .expect("show binds a document");
    let out = tool.call(args("c1"), cx()).await.unwrap();
    assert!(!out.is_error, "{}", out.content);
    let first = out.content.lines().next().unwrap();
    assert!(
        first.contains(&format!("; editable as document {doc} (doc_read))")),
        "{first}"
    );
    let _ = actor.stop_gracefully().await;
}
