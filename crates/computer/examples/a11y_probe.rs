//! Design doc 161 §9: a read-only, headless accessibility snapshot of one app
//! (default Finder) through the same `a11y` module the tools use. Prints
//! `ref_count` and the first lines of the tree, then retires the worker so
//! its agent-desktop namespace is reclaimed: `end_session` on it, then
//! agent-desktop's store-wide `gc` of ended sessions, which also removes any
//! other already-ended agent-desktop session. It never acts and never moves
//! the pointer. Run with `cargo run -p computer --example a11y_probe [App]`
//! from a terminal that has Accessibility granted.

use computer::a11y::args::SnapshotArgs;
use computer::a11y::{A11y, Request};
use runtime::SessionId;

/// Lines of the pretty-printed tree to show.
const LINES: usize = 25;

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let app = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Finder".to_owned());
    // Its own engine identity, so it never matches a real engine's sweep.
    let a11y = A11y::new(&std::env::temp_dir().join("a11y_probe"));
    let session = SessionId::new();
    let request = Request::Snapshot(SnapshotArgs {
        app: Some(app.clone()),
        ..SnapshotArgs::default()
    });
    match a11y.run(session, request).await {
        Ok(data) => match serde_json::from_str::<serde_json::Value>(&data) {
            Ok(v) => {
                println!(
                    "{app}: ref_count {} · snapshot {} · complete {} · {} bytes",
                    v["ref_count"],
                    v["snapshot_id"],
                    v["complete"],
                    data.len()
                );
                let tree = serde_json::to_string_pretty(&v["tree"]).unwrap_or_default();
                for line in tree.lines().take(LINES) {
                    println!("{line}");
                }
            }
            Err(e) => println!("{app}: unparsable data ({e}): {data}"),
        },
        Err(envelope) => println!("{app}: error {envelope}"),
    }
    for handle in a11y.forget(&[session]) {
        if handle.join().is_err() {
            println!("the a11y worker panicked while reclaiming its namespace");
        }
    }
}
