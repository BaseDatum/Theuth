//! Replays `golden.json` (recorded from the current TypeScript by
//! `packages/apps/scripts/record-inproc-golden.ts`: check.test.ts cases, the scripts/apps/fixtures.json
//! program and edits, protocol errors) through the committed bundle in deno_core: every response line must
//! equal the recorded one byte for byte.

use app_js::{APPS, BundleThread, Limits};
use tokio_util::sync::CancellationToken;

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

#[tokio::test]
async fn golden_transcript_replays_byte_for_byte() {
    let golden: Golden =
        serde_json::from_str(include_str!("golden.json")).expect("golden.json parses");
    assert!(golden.cases.len() >= 30, "{} cases", golden.cases.len());
    let t = BundleThread::new(&APPS, Limits::default(), CancellationToken::new());
    // Enqueue everything first: one runtime serves the transcript in order.
    let tickets: Vec<_> = golden
        .cases
        .iter()
        .map(|c| t.enqueue(c.request.clone()))
        .collect();
    for (case, ticket) in golden.cases.iter().zip(tickets) {
        let reply = ticket
            .await
            .unwrap_or_else(|e| panic!("{}: {e}", case.name));
        assert!(reply.events.is_empty(), "{}", case.name);
        assert_eq!(reply.reply, case.response, "case {:?}", case.name);
    }
    assert_eq!(t.generation(), 1);
    let version = std::thread::spawn(|| {
        app_js::BundleJs::new(&APPS, Limits::default())
            .and_then(|mut js| js.version())
            .expect("version")
    })
    .join()
    .expect("version thread");
    assert_eq!(version, golden.version);
}
