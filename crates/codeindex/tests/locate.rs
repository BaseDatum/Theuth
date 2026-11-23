//! 71 §5 row 7a: `Locate` on a scratch git repository — identical, changed,
//! moved (lines inserted above the symbol), missing (file and symbol gone),
//! bare links, and the root/path rejections. Nothing is written inside this
//! repository.

use std::path::Path;
use std::sync::Arc;

use codeindex::locate::{locate, text_sha};
use codeindex::{Codeindex, CodeindexConfig, Locate, LocateAnchor, LocateStatus};

mod common;
use common::tempdir;

fn git(dir: &Path, args: &[&str]) {
    let st = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

const HELPER: &str = "pub fn helper() -> u32 {\n    1\n}";
const UTIL: &str = "pub fn helper() -> u32 {\n    1\n}\n\npub fn unrelated() -> u32 {\n    2\n}\n";

fn anchor(
    path: &str,
    start: u32,
    end: u32,
    symbol: Option<&str>,
    text: Option<&str>,
) -> LocateAnchor {
    LocateAnchor {
        path: path.into(),
        start: Some(start),
        end: Some(end),
        symbol: symbol.map(str::to_owned),
        text_sha: text.map(text_sha),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn locate_states_and_rejections() {
    let repo = tempdir();
    let root = repo.path().canonicalize().unwrap();
    git(&root, &["init", "-q"]);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"fixture-locate\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/util.rs"), UTIL).unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub mod util;\n").unwrap();
    let data = tempdir();
    let ix = Arc::new(Codeindex::new(CodeindexConfig::default(), data.path()).unwrap());
    let run = |anchors: Vec<LocateAnchor>| {
        locate(
            &ix,
            Locate {
                root: root.clone(),
                anchors,
            },
        )
    };

    // Identical: hash matches; bare link (no sha) resolves; whole file has no range.
    let out = run(vec![
        anchor("src/util.rs", 1, 3, Some("helper"), Some(HELPER)),
        anchor("src/util.rs", 5, 7, None, None),
        LocateAnchor {
            path: "src/util.rs".into(),
            ..Default::default()
        },
    ])
    .await
    .unwrap();
    assert_eq!(out.len(), 3);
    assert_eq!(out[0].status, LocateStatus::Same);
    assert_eq!((out[0].start, out[0].end), (1, 3));
    assert_eq!(out[0].text.as_deref(), Some(HELPER));
    assert_eq!(out[1].status, LocateStatus::Same);
    assert_eq!(
        out[1].text.as_deref(),
        Some("pub fn unrelated() -> u32 {\n    2\n}")
    );
    assert_eq!(
        (out[2].status, out[2].start, out[2].end),
        (LocateStatus::Same, 1, 7)
    );

    // Changed: the body differs, the symbol still starts on line 1 — with and without `symbol`.
    std::fs::write(
        root.join("src/util.rs"),
        UTIL.replace("    1\n", "    1 + 0\n"),
    )
    .unwrap();
    let out = run(vec![
        anchor("src/util.rs", 1, 3, Some("helper"), Some(HELPER)),
        anchor("src/util.rs", 1, 3, None, Some(HELPER)),
    ])
    .await
    .unwrap();
    for l in &out {
        assert_eq!(l.status, LocateStatus::Changed, "{l:?}");
        assert_eq!((l.start, l.end), (1, 3));
        assert_eq!(
            l.text.as_deref(),
            Some("pub fn helper() -> u32 {\n    1 + 0\n}")
        );
    }

    // Moved: two lines inserted above; the symbol now starts at 3 and the range keeps its length.
    std::fs::write(root.join("src/util.rs"), format!("// a\n// b\n{UTIL}")).unwrap();
    let out = run(vec![
        anchor("src/util.rs", 1, 3, Some("helper"), Some(HELPER)),
        // Without a symbol the same edit can only be reported as changed.
        anchor("src/util.rs", 1, 3, None, Some(HELPER)),
    ])
    .await
    .unwrap();
    assert_eq!(out[0].status, LocateStatus::Moved, "{out:?}");
    assert_eq!((out[0].start, out[0].end), (3, 5));
    assert_eq!(out[0].text.as_deref(), Some(HELPER));
    assert_eq!(out[1].status, LocateStatus::Changed);

    // Missing: symbol renamed away (file present), range past EOF, file deleted.
    std::fs::write(
        root.join("src/util.rs"),
        UTIL.replace("helper", "assistant"),
    )
    .unwrap();
    std::fs::remove_file(root.join("src/lib.rs")).unwrap();
    let out = run(vec![
        anchor("src/util.rs", 1, 3, Some("helper"), Some(HELPER)),
        anchor("src/util.rs", 40, 42, None, Some(HELPER)),
        anchor("src/lib.rs", 1, 1, None, None),
        anchor("src/nope.rs", 1, 1, Some("helper"), Some(HELPER)),
    ])
    .await
    .unwrap();
    for l in &out {
        assert_eq!(l.status, LocateStatus::Missing, "{l:?}");
        assert!(l.text.is_none());
    }
    assert_eq!(
        (out[1].start, out[1].end),
        (40, 42),
        "missing echoes the requested range"
    );

    // Out of root: `..`, absolute, and a symlink that escapes — each fails the whole batch,
    // even beside a good anchor, and even when the file does not exist.
    let outside = tempdir();
    std::fs::write(outside.path().join("secret.rs"), "fn s() {}\n").unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("src/link")).unwrap();
    let good = || anchor("src/util.rs", 1, 3, None, None);
    for bad in [
        "../secret.rs",
        "/etc/hosts",
        "../../nope.rs",
        "src/link/secret.rs",
    ] {
        let err = run(vec![good(), anchor(bad, 1, 1, None, None)])
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("outside") || err.contains("root-relative"),
            "{bad}: {err}"
        );
    }

    // `root` must exist and be a git top-level.
    let sub = root.join("src");
    let err = locate(
        &ix,
        Locate {
            root: sub,
            anchors: vec![anchor("util.rs", 1, 1, None, None)],
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("git top-level"), "{err}");
    let plain = tempdir();
    let err = locate(
        &ix,
        Locate {
            root: plain.path().to_path_buf(),
            anchors: vec![],
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("git top-level"), "{err}");
    let err = locate(
        &ix,
        Locate {
            root: root.join("gone"),
            anchors: vec![],
        },
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("does not exist"), "{err}");
}
