//! JSON data islands (design doc 122 D9): prose recovered from `__NEXT_DATA__`,
//! `application/json` scripts, `ld+json` `articleBody` and SvelteKit `kit.start` payloads
//! without evaluating JavaScript.

use std::collections::HashSet;

use kuchikiki::{parse_html, traits::TendrilSink};
use serde_json::Value;

/// Strings shorter than this are labels, not prose.
pub const MIN_CHUNK_CHARS: usize = 40;
/// At most this many paragraphs are kept …
pub const MAX_CHUNKS: usize = 1_000;
/// … and at most this many chars in total.
pub const MAX_CHARS: usize = 500_000;

/// D7's fingerprint A: lowercase alphanumerics with whitespace collapsed.
fn fingerprint(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut pending_space = false;
    for c in s.chars() {
        if c.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.extend(c.to_lowercase());
        } else if c.is_whitespace() {
            pending_space = true;
        }
    }
    out
}

fn url_like(s: &str) -> bool {
    if s.starts_with('/') || s.starts_with("www.") {
        return true;
    }
    // `^[a-z]+://`
    let scheme_end = s.find("://");
    scheme_end.is_some_and(|i| i > 0 && s[..i].bytes().all(|b| b.is_ascii_lowercase()))
}

fn qualifies(s: &str) -> bool {
    s.chars().count() >= MIN_CHUNK_CHARS && !url_like(s) && s.contains(char::is_whitespace)
}

struct Collector {
    seen: HashSet<String>,
    chunks: Vec<String>,
    chars: usize,
}

impl Collector {
    fn full(&self) -> bool {
        self.chunks.len() >= MAX_CHUNKS || self.chars >= MAX_CHARS
    }

    fn push(&mut self, s: &str) {
        if self.full() || !qualifies(s) {
            return;
        }
        let s = s.trim();
        let n = s.chars().count();
        if self.chars + n > MAX_CHARS || !self.seen.insert(fingerprint(s)) {
            return;
        }
        self.chars += n;
        self.chunks.push(s.to_owned());
    }

    /// Every string value, depth first, in document order; `key` restricts to one object key.
    fn walk(&mut self, v: &Value, key: Option<&str>) {
        if self.full() {
            return;
        }
        match v {
            Value::String(s) if key.is_none() => self.push(s),
            Value::Array(a) => {
                for x in a {
                    self.walk(x, key);
                }
            }
            Value::Object(o) => {
                for (k, x) in o {
                    match key {
                        Some(want) if k == want => self.walk(x, None),
                        _ => self.walk(x, key),
                    }
                }
            }
            _ => {}
        }
    }
}

/// The JSON array literal after `data:` inside `kit.start(`, when it parses (trailing commas
/// stripped; no other JS repair).
fn sveltekit_data(script: &str) -> Option<Value> {
    let start = script.find("kit.start(")?;
    let rest = &script[start..];
    let data = rest.find("data:")?;
    let after = rest[data + 5..].trim_start();
    if !after.starts_with('[') {
        return None;
    }
    let end = array_end(after)?;
    let literal = &after[..=end];
    serde_json::from_str(literal)
        .ok()
        .or_else(|| serde_json::from_str(&strip_trailing_commas(literal)).ok())
}

/// Index of the `]` closing the array that opens at `s[0]`, honouring JSON strings.
fn array_end(s: &str) -> Option<usize> {
    let mut depth = 0i32;
    let mut in_str = false;
    let mut escaped = false;
    for (i, c) in s.char_indices() {
        if in_str {
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '[' | '{' => depth += 1,
            ']' | '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

fn strip_trailing_commas(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_str = false;
    let mut escaped = false;
    let chars: Vec<char> = s.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if in_str {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                in_str = false;
            }
            i += 1;
            continue;
        }
        if c == '"' {
            in_str = true;
        } else if c == ',' {
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            if j < chars.len() && matches!(chars[j], ']' | '}') {
                i += 1;
                continue;
            }
        }
        out.push(c);
        i += 1;
    }
    out
}

/// Prose recovered from JSON data islands, as paragraphs in document order; `None` when nothing
/// qualifies. No JavaScript is evaluated.
///
/// Sources: `<script id="__NEXT_DATA__">`, `<script type="application/json">`, `<script
/// type="application/ld+json">` (`articleBody` values only), SvelteKit `kit.start(…, {data: […]})`.
/// Strings ≥ 40 chars that are not URL-like and contain whitespace are kept, deduplicated by
/// fingerprint, joined with blank lines; at most [`MAX_CHUNKS`] chunks / [`MAX_CHARS`] chars.
#[must_use]
pub fn extract_islands(html: &str) -> Option<String> {
    let doc = parse_html().one(html);
    let scripts = doc.select("script").ok()?;
    let mut c = Collector {
        seen: HashSet::new(),
        chunks: Vec::new(),
        chars: 0,
    };
    for script in scripts {
        if c.full() {
            break;
        }
        let (is_next, kind) = {
            let attrs = script.attributes.borrow();
            let is_next = attrs
                .get("id")
                .is_some_and(|id| id.eq_ignore_ascii_case("__NEXT_DATA__"));
            let kind = attrs
                .get("type")
                .map(|t| t.trim().to_ascii_lowercase())
                .unwrap_or_default();
            (is_next, kind)
        };
        let text = script.text_contents();
        if is_next || kind == "application/json" {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                c.walk(&v, None);
            }
        } else if kind == "application/ld+json" {
            if let Ok(v) = serde_json::from_str::<Value>(&text) {
                c.walk(&v, Some("articleBody"));
            }
        } else if text.contains("kit.start(") {
            if let Some(v) = sveltekit_data(&text) {
                c.walk(&v, None);
            }
        }
    }
    (!c.chunks.is_empty()).then(|| c.chunks.join("\n\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const PROSE: &str =
        "The harbour council met on Tuesday and approved the night ferry by seven votes to two.";

    #[test]
    fn next_data_island() {
        let html = format!(
            r#"<html><body><div id="__next"></div><script id="__NEXT_DATA__" type="application/json">{{"props":{{"pageProps":{{"title":"Short","slug":"night-ferry-long-read-with-many-words-and-dashes-in-it","url":"https://example.test/a/very/long/path/that/is/not/prose/at/all","blocks":[{{"text":"{PROSE}"}},{{"text":"{PROSE}"}},{{"text":"A second paragraph about the breakwater lights and the pilot boat schedule."}}]}}}}}}</script></body></html>"#
        );
        let out = extract_islands(&html).expect("island");
        assert_eq!(
            out,
            format!(
                "{PROSE}\n\nA second paragraph about the breakwater lights and the pilot boat schedule."
            )
        );
    }

    #[test]
    fn sources_json_ld_and_sveltekit() {
        let html = format!(
            r#"<script type="application/ld+json">{{"@type":"NewsArticle","headline":"A headline long enough to pass the forty char gate","description":"Also long enough to pass the forty character gate easily","articleBody":"{PROSE}"}}</script>
<script type="application/json" id="whatever">{{"a":["{PROSE} Again."]}}</script>
<script>import("/_app/start.js").then(function(kit){{kit.start(app, element, {{node_ids: [0, 2], data: [{{"type":"data","data":{{"text":"{PROSE} Third time."}},}},], form: null,}});}});</script>"#
        );
        let out = extract_islands(&html).expect("islands");
        let parts: Vec<&str> = out.split("\n\n").collect();
        assert_eq!(
            parts,
            vec![
                PROSE,
                &format!("{PROSE} Again."),
                &format!("{PROSE} Third time.")
            ]
        );
    }

    #[test]
    fn filters_short_urls_keys_and_duplicates() {
        let html = r#"<script type="application/json">["short string here","https://example.test/this/is/a/long/url/that/has/no/spaces/x","/relative/path/that/is/long/enough/to/pass/the/length/gate","www.example.test/another/long/one/without/any/spaces/here","akeylikestringwithoutanywhitespaceatallbutlongenough","Forty-plus chars of real prose, with spaces.","forty-plus CHARS of real prose, with   spaces!"]</script>"#;
        assert_eq!(
            extract_islands(html).as_deref(),
            Some("Forty-plus chars of real prose, with spaces.")
        );
        assert_eq!(
            extract_islands("<div id=\"root\"></div><script>var a=1;</script>"),
            None
        );
        assert_eq!(
            extract_islands("<script type=\"application/json\">not json</script>"),
            None
        );
    }

    #[test]
    fn caps() {
        let strings: Vec<String> = (0..2_000)
            .map(|i| {
                format!("Paragraph number {i:04} of a synthetic island, long enough to count.")
            })
            .collect();
        let html = format!(
            "<script type=\"application/json\">{}</script>",
            serde_json::to_string(&strings).unwrap()
        );
        let out = extract_islands(&html).unwrap();
        assert_eq!(out.split("\n\n").count(), MAX_CHUNKS);
        assert!(
            out.ends_with("Paragraph number 0999 of a synthetic island, long enough to count.")
        );

        let big: Vec<String> = (0..600)
            .map(|i| format!("{i:04} {}", "x ".repeat(500)))
            .collect();
        let html = format!(
            "<script type=\"application/json\">{}</script>",
            serde_json::to_string(&big).unwrap()
        );
        let out = extract_islands(&html).unwrap();
        assert!(out.chars().count() <= MAX_CHARS + 2 * 600);
        assert!(out.split("\n\n").count() < 600);
    }

    #[test]
    fn fingerprint_collapses() {
        assert_eq!(fingerprint("  Hello,   World!  "), "hello world");
        assert_eq!(fingerprint("a-b c"), "ab c");
    }
}
