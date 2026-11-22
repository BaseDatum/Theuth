//! Regression coverage without the old binary: the synthetic parity corpus replayed in-process against
//! transcripts recorded from the shipped jsdom helper (driver: `support/documents_script.rs`).

use js_runtime::{DOCUMENTS, JsRuntime};

#[path = "support/documents_script.rs"]
mod documents_script;

#[test]
fn golden_transcripts_match_the_jsdom_helper() {
    let mut js = JsRuntime::new(&DOCUMENTS, DOCUMENTS.limits).expect("runtime");
    let (failures, cases) = documents_script::documents_golden_failures(&mut js);
    assert!(
        failures.is_empty(),
        "{} of {} cases differ:\n{}",
        failures.len(),
        cases,
        failures.join("\n")
    );
}
