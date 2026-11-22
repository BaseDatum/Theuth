//! The documents golden replay, shared by `tests/documents_golden.rs` and crates/browser's
//! `tests/v8_isolates.rs` (which replays it in a js-runtime isolate sharing V8's read-only heap with an
//! obscura isolate). Transcripts were recorded from the shipped jsdom helper (`bun run
//! scripts/golden-inproc.ts` in packages/documents writes tests/documents_golden.json). Keep `run_script` in
//! step with that script's `script`.

use js_runtime::JsRuntime;
use serde_json::{Map, Value, json};

const EMPTY_STATE: &str = "AAA=";
const INSERTED: &str = "Inserted **para** with [link](theuth://notes/1)\n\n- x\n  - y";
const REPLACED: &str = "## Replaced `heading`";
const APPENDED: &str = "\n\nAppended paragraph with *emphasis*.\n";

/// JSON with object keys sorted, as `canon` in golden-inproc.ts writes it.
fn canon(v: &Value) -> String {
    match v {
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(canon).collect::<Vec<_>>().join(",")
        ),
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| format!("{}:{}", Value::String(k.clone()), canon(&map[k])))
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        other => other.to_string(),
    }
}

fn is_uuid(s: &[u8]) -> bool {
    s.len() == 36
        && s.iter().enumerate().all(|(i, &c)| match i {
            8 | 13 | 18 | 23 => c == b'-',
            _ => c.is_ascii_digit() || (b'a'..=b'f').contains(&c),
        })
}

/// Rename every UUID to `ID<n>` in order of first appearance (`ids` spans one input).
fn normalise(s: &str, ids: &mut Vec<String>) -> String {
    let b = s.as_bytes();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if i + 36 <= b.len() && is_uuid(&b[i..i + 36]) {
            let u = &s[i..i + 36];
            let n = ids.iter().position(|x| x == u).unwrap_or_else(|| {
                ids.push(u.to_owned());
                ids.len() - 1
            });
            out.push_str(&format!("ID{n}"));
            i += 36;
        } else {
            let ch = s[i..].chars().next().expect("char");
            out.push(ch);
            i += ch.len_utf8();
        }
    }
    out
}

struct Run<'a> {
    js: &'a mut JsRuntime,
    id: u64,
    ids: Vec<String>,
    transcript: Vec<String>,
}

impl Run<'_> {
    fn r(&mut self, op: &str, extra: Value) -> Value {
        self.id += 1;
        let mut body = json!({ "id": self.id, "op": op, "doc": "d" });
        for (k, v) in extra.as_object().expect("object") {
            body[k] = v.clone();
        }
        let lines = self
            .js
            .call(&body.to_string())
            .unwrap_or_else(|e| panic!("{op}: {e}"));
        for l in &lines {
            let mut v: Value = serde_json::from_str(l).expect("json line");
            if v.get("ev").is_some()
                && let Some(o) = v.as_object_mut()
            {
                o.remove("update");
            }
            let line = normalise(&canon(&v), &mut self.ids);
            self.transcript.push(line);
        }
        serde_json::from_str(lines.last().expect("reply")).expect("reply json")
    }
}

fn run_script(js: &mut JsRuntime, text: &str, frontmatter: bool) -> Vec<String> {
    let mut run = Run {
        js,
        id: 0,
        ids: Vec::new(),
        transcript: Vec::new(),
    };
    run.r("open", json!({ "state": EMPTY_STATE }));
    run.r(
        "import",
        json!({ "markdown": text, "mode": "replace", "frontmatter": frontmatter }),
    );
    run.r("markdown", json!({}));
    let outline = run.r("outline", json!({}));
    let body: Vec<String> = outline["result"]["blocks"]
        .as_array()
        .expect("blocks")
        .iter()
        .filter(|b| b["type"] != "frontmatter")
        .map(|b| b["id"].as_str().expect("id").to_owned())
        .collect();
    let mut authors = Map::new();
    if let Some(first) = body.first() {
        authors.insert(first.clone(), json!("Ada"));
    }
    run.r("markdown", json!({ "annotate": true, "authors": authors }));
    let mut insert = json!({ "op": "insert", "markdown": INSERTED });
    if let Some(first) = body.first() {
        insert["after"] = json!(first);
    }
    let mut edits = vec![insert];
    if body.len() >= 3 {
        edits.push(json!({ "op": "replace", "blocks": [body[1]], "markdown": REPLACED }));
        edits.push(json!({ "op": "move", "blocks": [body[body.len() - 1]], "at": "start" }));
    }
    run.r("edit", json!({ "edits": edits }));
    run.r("markdown", json!({}));
    run.r("import", json!({ "markdown": format!("{text}{APPENDED}"), "mode": "diff", "frontmatter": frontmatter }));
    run.r("markdown", json!({}));
    run.r("close", json!({}));
    run.transcript
}

/// Replay every case of `documents_golden.json` on `js` (a [`js_runtime::DOCUMENTS`] runtime); the
/// differing cases, described, and the number of cases.
pub fn documents_golden_failures(js: &mut JsRuntime) -> (Vec<String>, usize) {
    let golden: Value =
        serde_json::from_str(include_str!("../documents_golden.json")).expect("golden.json");
    let cases = golden["cases"].as_array().expect("cases");
    assert!(cases.len() >= 10, "{} cases", cases.len());
    let mut failures = Vec::new();
    for case in cases {
        let name = case["name"].as_str().expect("name");
        let text = case["text"].as_str().expect("text");
        let frontmatter = case["frontmatter"].as_bool().expect("frontmatter");
        let expected: Vec<&str> = case["transcript"]
            .as_array()
            .expect("transcript")
            .iter()
            .map(|l| l.as_str().expect("line"))
            .collect();
        let got = run_script(js, text, frontmatter);
        if got != expected {
            let at = got
                .iter()
                .zip(&expected)
                .position(|(g, e)| g != e)
                .unwrap_or(got.len().min(expected.len()));
            failures.push(format!(
                "{name}: line {at}\n  helper:  {}\n  inproc:  {}",
                expected.get(at).unwrap_or(&"<none>"),
                got.get(at).map_or("<none>", String::as_str)
            ));
        }
    }
    (failures, cases.len())
}
