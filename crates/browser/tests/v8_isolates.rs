//! The two deno_core users in `t` — obscura (a custom startup snapshot) and js-runtime (V8's built-in
//! snapshot) — must be able to hold isolates at the same time in one process, in either order. V8 keeps
//! ONE read-only heap per isolate group, deserialised from the snapshot of whichever isolate is created
//! while the group is empty, and checks a later snapshot against it only in debug builds. Before
//! `browser::warmup` pinned an obscura isolate for the process lifetime, a js-runtime isolate alive while
//! a browser page started made obscura's snapshot deserialise against the built-in snapshot's (smaller)
//! read-only heap: `Deserializer::ReadReadOnlyHeapRef` indexed past its pages and libc++ aborted the
//! process (`vector[] index out of bounds`, crash of 2026-11-22 12:32:51).
//!
//! One test, not several: the order of isolate creation is the subject, and the read-only heap and the
//! warmup are process-global (this file is its own test binary, so nothing else shares them).

use js_runtime::{APPS, JsRuntime, Limits};
use obscura_js::runtime::ObscuraJsRuntime;

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
    for case in golden.cases.iter().filter(|c| !c.response.contains("invalid JSON: ")) {
        let lines = js.call(&case.request).unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert_eq!(lines.last(), Some(&case.response), "{}", case.name);
    }
}

fn exercise_page(page: &mut ObscuraJsRuntime) {
    let v = page
        .evaluate("[1, 2, 3].map(n => n * 2).join(',') + typeof document")
        .expect("evaluate");
    assert_eq!(v, serde_json::json!("2,4,6object"));
}

#[test]
fn js_runtime_and_obscura_isolates_coexist_in_either_order() {
    let golden: Golden = serde_json::from_str(include_str!("../../js-runtime/tests/apps_golden.json"))
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

    // The other order: a page is alive when a js-runtime isolate is (re)built.
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
