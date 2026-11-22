//! The two deno_core users in `t` — obscura (a custom startup snapshot) and js-runtime (V8's built-in
//! snapshot) — must be able to hold isolates at the same time in one process, in either order. V8 keeps
//! ONE read-only heap per isolate group, deserialised from the snapshot of whichever isolate is created
//! while the group is empty, and checks a later snapshot against it only in debug builds. Before
//! `browser::warmup` pinned an obscura isolate for the process lifetime, a js-runtime isolate alive while
//! a browser page started made obscura's snapshot deserialise against the built-in snapshot's (smaller)
//! read-only heap: `Deserializer::ReadReadOnlyHeapRef` indexed past its pages and libc++ aborted the
//! process (`vector[] index out of bounds`, crash of 2026-11-22 12:32:51).
//!
//! Sharing is sound in the order the fix pins: obscura's snapshot is built from V8's built-in one with
//! `extensible_ro_snapshot` (on in rusty_v8's V8), which extends the built-in read-only space in place
//! (src/heap/read-only-heap.cc `DeserializeIntoIsolate`, src/snapshot/snapshot.cc read-only promotion) and
//! the deserializer re-creates every page at its serialised index (src/snapshot/read-only-deserializer.cc
//! `AllocatePage`), so the built-in heap is a page-for-page prefix of obscura's. The js-runtime isolates
//! built on obscura's heap here replay all three bundles' golden transcripts byte for byte.
//!
//! One test, not several: the order of isolate creation is the subject, and the read-only heap and the
//! warmup are process-global (this file is its own test binary, so nothing else shares them).

use std::time::Duration;

use js_runtime::{APPS, DOCUMENTS, JsRuntime, Limits, MERMAID};
use obscura_js::runtime::ObscuraJsRuntime;
use serde_json::{Value, json};

#[path = "../../js-runtime/tests/support/documents_script.rs"]
mod documents_script;

#[derive(serde::Deserialize)]
struct Golden {
    version: String,
    cases: Vec<Case>,
}

#[derive(serde::Deserialize)]
struct Case {
    name: String,
    request: String,
    response: String,
}

/// A js-runtime isolate that does real work: the bundle's version and the golden cases that do not
/// depend on the JS engine's own error text.
fn exercise_js(js: &mut JsRuntime, golden: &Golden) {
    assert_eq!(js.version().expect("version"), golden.version);
    for case in golden
        .cases
        .iter()
        .filter(|c| !c.response.contains("invalid JSON: "))
    {
        let lines = js
            .call(&case.request)
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert_eq!(lines.last(), Some(&case.response), "{}", case.name);
    }
}

/// Every bundle's golden transcript, each in a fresh js-runtime isolate: [`APPS`] via [`exercise_js`],
/// [`DOCUMENTS`] via js-runtime's own driver, [`MERMAID`] (148 diagrams) exact but for the one case whose
/// message is the JS engine's own wording.
fn all_goldens(apps: &Golden) {
    exercise_js(
        &mut JsRuntime::new(&APPS, Limits::default()).expect("apps"),
        apps,
    );

    let mut docs = JsRuntime::new(&DOCUMENTS, DOCUMENTS.limits).expect("documents");
    let (failures, cases) = documents_script::documents_golden_failures(&mut docs);
    assert!(
        failures.is_empty(),
        "{} of {cases} documents cases differ:\n{}",
        failures.len(),
        failures.join("\n")
    );

    let mermaid: serde_json::Map<String, Value> =
        serde_json::from_str(include_str!("../../js-runtime/tests/mermaid_golden.json"))
            .expect("mermaid golden");
    assert_eq!(mermaid.len(), 148);
    let limits = Limits {
        deadline: Duration::from_secs(5),
        first_deadline: Duration::from_secs(10),
        heap_limit: 256 * 1024 * 1024,
        max_line: 4 * 1024 * 1024,
    };
    let mut js = JsRuntime::new(&MERMAID, limits).expect("mermaid");
    let mut exact = 0;
    for (name, case) in &mermaid {
        let line =
            json!({ "id": name, "op": "check_mermaid", "source": case["source"] }).to_string();
        let lines = js.call(&line).unwrap_or_else(|e| panic!("{name}: {e}"));
        let mut got: Value =
            serde_json::from_str(lines.last().expect("reply")).expect("reply JSON");
        got.as_object_mut().expect("object").remove("id");
        if got == case["reply"] {
            exact += 1;
        } else {
            assert_eq!(
                name, "edge/timeline-typeerror",
                "got {got} want {}",
                case["reply"]
            );
        }
    }
    assert_eq!(exact, 147);
}

fn exercise_page(page: &mut ObscuraJsRuntime) {
    let v = page
        .evaluate("[1, 2, 3].map(n => n * 2).join(',') + typeof document")
        .expect("evaluate");
    assert_eq!(v, serde_json::json!("2,4,6object"));
}

#[test]
fn js_runtime_and_obscura_isolates_coexist_in_either_order() {
    let golden: Golden =
        serde_json::from_str(include_str!("../../js-runtime/tests/apps_golden.json"))
            .expect("golden parses");
    // `t`'s startup order (crates/theuth/src/main.rs): the browser warmup, then js-runtime's platform init.
    browser::warmup(&browser::BrowserConfig::default());
    js_runtime::init_platform();

    // The crash's order: a js-runtime isolate (documents-js, app-js, …) is alive when a browser session
    // builds its page isolate.
    let mut js = JsRuntime::new(&APPS, Limits::default()).expect("js runtime");
    exercise_js(&mut js, &golden);
    let mut page = ObscuraJsRuntime::new();
    exercise_page(&mut page);
    exercise_js(&mut js, &golden);
    drop(js);

    // The other order: a page is alive when js-runtime isolates are (re)built — on obscura's read-only
    // heap, every bundle still replays its golden transcript.
    all_goldens(&golden);
    let mut js = JsRuntime::new(&APPS, Limits::default()).expect("js runtime");
    exercise_js(&mut js, &golden);
    exercise_page(&mut page);
    drop(page);

    // Every isolate of the test gone but the warmup's: a fresh js-runtime isolate first, then a page.
    exercise_js(&mut js, &golden);
    drop(js);
    let mut js = JsRuntime::new(&APPS, Limits::default()).expect("js runtime");
    let mut page = ObscuraJsRuntime::new();
    exercise_page(&mut page);
    exercise_js(&mut js, &golden);
}
