//! Surfaces (design doc 30): the `json-render` fences a model writes in its replies.
//!
//! [`parse`] turns one fence body into a typed [`Spec`] — strict JSON first, `jsonrepair`
//! on failure, then validation that reports every problem with a JSON Pointer path.
//! [`check`] runs that over every fence in a finished reply. The catalog the prompt and
//! the skill show ([`catalog_compact`], [`catalog_schema`]) is read off the same types.
//! UI-agnostic: the TUI, the text channels and (via ts-rs) the desktop consume this.

mod catalog;
mod spec;
mod validate;

pub use catalog::{catalog_compact, catalog_schema};
pub use spec::*;

use serde_json::Value;

use crate::validate::err;

/// Largest fence body [`parse`] accepts, in bytes.
pub const MAX_FENCE_BYTES: usize = 256 * 1024;
/// Most elements in one spec.
pub const MAX_ELEMENTS: usize = 256;
/// Deepest `children` nesting; `root` is depth 1.
pub const MAX_DEPTH: usize = 32;
/// Largest Mermaid source the renderer accepts, in bytes.
pub const MAX_MERMAID_BYTES: usize = 128 * 1024;

/// One located validation failure; `path` is a JSON Pointer into the fence body
/// (`""` = the whole body, `"/"` = the element graph as a whole).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpecError {
    /// JSON Pointer to the offending value.
    pub path: String,
    /// What is wrong there.
    pub message: String,
}

impl std::fmt::Display for SpecError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.path, self.message)
    }
}

/// A fence body that validated.
#[derive(Clone, Debug, PartialEq)]
pub struct Parsed {
    /// The typed spec.
    pub spec: Spec,
    /// Whether `jsonrepair` had to fix the JSON before it parsed.
    pub repaired: bool,
}

/// Parse and validate one `json-render` fence body.
///
/// Strict `serde_json` first; on failure `jsonrepair` then `serde_json` again (logged at
/// `info` on target `surface`); then typed validation collecting every error.
pub fn parse(body: &str) -> Result<Parsed, Vec<SpecError>> {
    if body.len() > MAX_FENCE_BYTES {
        return Err(vec![err(
            "",
            format!(
                "{} bytes, over the {MAX_FENCE_BYTES} byte limit",
                body.len()
            ),
        )]);
    }
    let (value, repaired) = match serde_json::from_str::<Value>(body) {
        Ok(v) => (v, false),
        Err(strict) => {
            let fixed = jsonrepair_rs::jsonrepair(body)
                .map_err(|e| vec![err("", format!("invalid JSON after repair: {e}"))])?;
            let v = serde_json::from_str::<Value>(&fixed)
                .map_err(|e| vec![err("", format!("invalid JSON after repair: {e}"))])?;
            tracing::info!(target: "surface", kind = "json-render", repaired = true, bytes = body.len(), strict_error = %strict, "fence repaired");
            (v, true)
        }
    };
    validate::validate(&value).map(|spec| Parsed { spec, repaired })
}

/// Validate every `json-render` fence in a markdown text; `(ordinal, errors)` for each
/// failing fence, ordinal 1-based over the `json-render` fences only. Empty = nothing to fix.
///
/// Fences are found the way CommonMark does: an opening line of three or more `` ` `` or
/// `~` (up to three spaces of indent) with the info string `json-render`, closed by a line
/// of at least as many of the same character. Fences inside another fenced block are
/// content, not fences. An unclosed fence is one error at path `""`.
pub fn check(text: &str) -> Vec<(usize, Vec<SpecError>)> {
    let mut lines = Vec::new();
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        lines.push((offset, line));
        offset += line.len();
    }
    let mut out = Vec::new();
    let mut ordinal = 0;
    let mut i = 0;
    while i < lines.len() {
        let Some((marker, len, info)) = opening(lines[i].1) else {
            i += 1;
            continue;
        };
        let json = info == "json-render";
        ordinal += usize::from(json);
        let Some(close) = (i + 1..lines.len()).find(|&j| closing(lines[j].1, marker, len)) else {
            if json {
                out.push((ordinal, vec![err("", "fence not closed")]));
            }
            break;
        };
        if json && let Err(errs) = parse(&text[lines[i + 1].0..lines[close].0]) {
            out.push((ordinal, errs));
        }
        i = close + 1;
    }
    out
}

/// The line with up to three leading spaces removed; `None` if indented further.
fn unindented(line: &str) -> Option<&str> {
    let n = line.bytes().take_while(|b| *b == b' ').count();
    (n <= 3).then_some(&line[n..])
}

/// `(marker, run length, trimmed info string)` if `line` opens a fence.
fn opening(line: &str) -> Option<(u8, usize, &str)> {
    let line = unindented(line)?;
    let marker = *line.as_bytes().first()?;
    if marker != b'`' && marker != b'~' {
        return None;
    }
    let len = line.bytes().take_while(|b| *b == marker).count();
    let rest = &line[len..];
    if len < 3 || (marker == b'`' && rest.contains('`')) {
        return None;
    }
    Some((marker, len, rest.trim()))
}

/// Whether `line` closes a fence opened by `len` × `marker`.
fn closing(line: &str, marker: u8, len: usize) -> bool {
    unindented(line).is_some_and(|l| {
        let n = l.bytes().take_while(|b| *b == marker).count();
        n >= len && l[n..].trim().is_empty()
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    macro_rules! fixture {
        ($name:literal) => {
            (
                $name,
                include_str!(concat!(
                    env!("CARGO_MANIFEST_DIR"),
                    "/fixtures/json-render/",
                    $name,
                    ".json"
                )),
            )
        };
    }

    const VALID: [(&str, &str); 12] = [
        fixture!("valid-release-readiness"),
        fixture!("valid-status-board"),
        fixture!("valid-stack-text"),
        fixture!("valid-grid-badges"),
        fixture!("valid-alerts"),
        fixture!("valid-key-value"),
        fixture!("valid-metrics"),
        fixture!("valid-bars"),
        fixture!("valid-timeline"),
        fixture!("valid-tabs"),
        fixture!("valid-accordion"),
        fixture!("valid-bare"),
    ];

    const REPAIRABLE: [(&str, &str); 4] = [
        fixture!("broken-trailing-comma"),
        fixture!("broken-single-quotes"),
        fixture!("broken-comments"),
        fixture!("broken-python-literals"),
    ];

    const TRUNCATED: (&str, &str) = fixture!("broken-truncated");

    const LOCATED: [((&str, &str), &str); 4] = [
        (fixture!("broken-unknown-type"), "/elements/t/type"),
        (fixture!("broken-missing-child"), "/elements/r/children/1"),
        (fixture!("broken-shared-child"), "/"),
        (
            fixture!("broken-prop-type"),
            "/elements/m/props/items/0/value",
        ),
    ];

    /// `broken-unknown-type`: fails at `/elements/t/type`.
    const BROKEN: &str = (LOCATED[0].0).1;

    fn component_name(c: &Component) -> String {
        let v = serde_json::to_value(c).expect("component serializes");
        v["type"].as_str().expect("adjacent tag").to_owned()
    }

    #[test]
    fn valid_fixtures_parse_strictly_cover_the_catalog_and_round_trip() {
        let mut seen = BTreeSet::new();
        for (name, body) in VALID {
            let parsed = parse(body).unwrap_or_else(|e| panic!("{name}: {e:#?}"));
            assert!(!parsed.repaired, "{name} needed repair");
            seen.extend(
                parsed
                    .spec
                    .elements
                    .values()
                    .map(|e| component_name(&e.component)),
            );
            let again = parse(&serde_json::to_string(&parsed.spec).expect("spec serializes"))
                .unwrap_or_else(|e| panic!("{name} round trip: {e:#?}"));
            assert_eq!(again.spec, parsed.spec, "{name}");
        }
        let catalog: BTreeSet<String> = catalog_compact()
            .split(" · ")
            .map(|c| c[..c.find('(').expect("name(")].to_owned())
            .collect();
        assert_eq!(seen, catalog, "every component appears in a valid fixture");
    }

    #[test]
    fn repairable_fixtures_parse_after_repair() {
        for (name, body) in REPAIRABLE {
            let parsed = parse(body).unwrap_or_else(|e| panic!("{name}: {e:#?}"));
            assert!(
                parsed.repaired,
                "{name} parsed strictly; the fixture is not broken"
            );
        }
    }

    /// Cut inside the second checklist item, after `"text"`. Repair closes the brackets,
    /// so either the item comes back without `done` — a located error at
    /// `/elements/l/props/items/1/done` — or the repair yields a whole spec (`repaired`).
    #[test]
    fn truncated_fixture_repairs_or_locates() {
        let (name, body) = TRUNCATED;
        match parse(body) {
            Ok(parsed) => assert!(parsed.repaired, "{name}"),
            Err(errs) => assert!(
                errs.iter().all(|e| !e.path.is_empty()),
                "{name}: unlocated {errs:#?}"
            ),
        }
    }

    #[test]
    fn broken_fixtures_yield_the_expected_path() {
        for ((name, body), path) in LOCATED {
            let errs = parse(body).expect_err(name);
            assert!(
                errs.iter().any(|e| e.path == path),
                "{name}: want {path} in {errs:#?}"
            );
        }
    }

    #[test]
    fn check_reports_only_failing_fences_by_ordinal() {
        let text = format!(
            "Takeaway first.\n\n```json-render\n{}\n```\n\nThen a broken one:\n\n```json-render\n{}\n```\n\nDone.\n",
            VALID[0].1, BROKEN
        );
        let out = check(&text);
        assert_eq!(out.len(), 1, "{out:#?}");
        assert_eq!(out[0].0, 2);
        assert_eq!(out[0].1[0].path, "/elements/t/type");
        assert!(check("no fences here\n").is_empty());
    }

    #[test]
    fn check_ignores_fences_nested_in_another_block() {
        let text = format!("~~~markdown\n```json-render\n{BROKEN}\n```\n~~~\n");
        assert!(check(&text).is_empty());
        let text = format!("````md\n```json-render\n{BROKEN}\n```\n````\n");
        assert!(check(&text).is_empty());
    }

    #[test]
    fn check_reports_an_unclosed_fence() {
        let out = check("```json-render\n{\"root\": \"r\"\n");
        assert_eq!(out, vec![(1, vec![err("", "fence not closed")])]);
        let out = check("```json-render\n{}\n``\n");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].1[0].path, "");
    }

    #[test]
    fn unrepairable_json_is_one_error_at_the_root() {
        let errs = parse("").expect_err("empty body");
        assert_eq!(errs.len(), 1, "{errs:#?}");
        assert_eq!(errs[0].path, "");
        assert!(
            errs[0].message.starts_with("invalid JSON after repair: "),
            "{errs:#?}"
        );
    }
}
