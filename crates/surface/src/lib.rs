//! Surfaces (design doc 30): the `json-render` and `mermaid` fences a model writes in its
//! replies.
//!
//! [`parse`] turns one fence body into a typed [`Spec`] — strict JSON first, `jsonrepair`
//! on failure, then validation that reports every problem with a JSON Pointer path.
//! [`check`] runs that over every fence in a finished reply. [`spec_lines`] and
//! [`mermaid_lines`] render to [`markdown::Line`]s at a width; [`fence`] is the
//! `markdown` hook over both, and [`flatten`] replaces renderable fences in a text for
//! channels that only carry text. The catalog the prompt and the skill show
//! ([`catalog_compact`], [`catalog_schema`]) is read off the same types.
//! UI-agnostic: the TUI, the text channels and (via ts-rs) the desktop consume this.

pub mod anchor;
mod catalog;
pub mod export;
mod fences;
mod flatten;
mod graph;
mod mermaid;
pub mod prompt;
mod render;
mod sequence;
pub mod slack;
mod spec;
mod validate;

pub use catalog::{catalog_compact, catalog_schema};
pub use fences::json_render_fences;
pub use flatten::flatten;
pub use mermaid::mermaid_lines;
pub use prompt::*;
pub use render::{fence, spec_lines};
pub use slack::{SlackBlocksError, slack_blocks};
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
/// Most nodes in one `Graph`.
pub const MAX_GRAPH_NODES: usize = 120;
/// Most edges in one `Graph`.
pub const MAX_GRAPH_EDGES: usize = 240;
/// Most groups in one `Graph`.
pub const MAX_GRAPH_GROUPS: usize = 12;
/// Longest `Graph` node, edge or group label, in characters.
pub const MAX_GRAPH_LABEL_CHARS: usize = 80;
/// Fewest and most `Sequence` actors.
pub const SEQUENCE_ACTORS: std::ops::RangeInclusive<usize> = 2..=12;
/// Fewest and most `Sequence` steps.
pub const SEQUENCE_STEPS: std::ops::RangeInclusive<usize> = 1..=40;
/// Most lines in one `Excerpt`.
pub const MAX_EXCERPT_LINES: usize = 120;
/// Largest `Excerpt.text`, in bytes.
pub const MAX_EXCERPT_BYTES: usize = 16 * 1024;

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

/// What a streaming `show` call is doing, read off its cumulative partial argument JSON
/// (design doc 35 §6.1).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PartialShow {
    /// `name`, else the file name of `path`; `None` until either has streamed in.
    pub name: Option<String>,
    /// A `content` key is present: the model is writing the file, not showing an existing one.
    pub writing: bool,
}

/// `jsonrepair` the cumulative partial tool-argument JSON and read `name`/`path`/`content`.
/// `None` when it does not parse yet.
pub fn partial_show(partial_json: &str) -> Option<PartialShow> {
    let fixed = jsonrepair_rs::jsonrepair(partial_json).ok()?;
    let v = serde_json::from_str::<Value>(&fixed).ok()?;
    let name = match v.get("name").and_then(Value::as_str) {
        Some(n) if !n.is_empty() => Some(n.to_owned()),
        _ => v
            .get("path")
            .and_then(Value::as_str)
            .and_then(|p| std::path::Path::new(p).file_name())
            .map(|f| f.to_string_lossy().into_owned()),
    };
    Some(PartialShow {
        name,
        writing: v.get("content").is_some(),
    })
}

/// Validate every `json-render` fence in a markdown text; `(ordinal, errors)` for each
/// failing fence, ordinal 1-based over the `json-render` fences only. Empty = nothing to fix.
///
/// Fences are found the way CommonMark does: an opening line of three or more `` ` `` or
/// `~` (up to three spaces of indent) with the info string `json-render`, closed by a line
/// of at least as many of the same character. Fences inside another fenced block are
/// content, not fences. An unclosed fence is one error at path `""`.
pub fn check(text: &str) -> Vec<(usize, Vec<SpecError>)> {
    let mut out = Vec::new();
    for (i, f) in fences::fences(text)
        .filter(|f| f.lang == "json-render")
        .enumerate()
    {
        let ordinal = i + 1;
        if !f.closed {
            out.push((ordinal, vec![err("", "fence not closed")]));
        } else if let Err(errs) = parse(f.body) {
            out.push((ordinal, errs));
        }
    }
    out
}

/// One reply fence that would not render (30 §5): what a correction round names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FailedFence {
    /// Fence info string: `json-render`, `mermaid`, `openui`.
    pub lang: String,
    /// 1-based over the fences of that language in the reply.
    pub ordinal: usize,
    /// Located errors; for `mermaid` `path` is `line:N` or `""`.
    pub errors: Vec<SpecError>,
}

/// Every `lang` fence of `text` as `(ordinal, body)`, ordinal 1-based over that language;
/// `body` is `None` for an unclosed fence. What the loop hands to the registered checkers
/// for languages this crate cannot validate itself.
pub fn fences_of<'a>(text: &'a str, lang: &str) -> Vec<(usize, Option<&'a str>)> {
    fences::fences(text)
        .filter(|f| f.lang == lang)
        .enumerate()
        .map(|(i, f)| (i + 1, f.closed.then_some(f.body)))
        .collect()
}

/// `line 4: A --> B[` for a `line:4` path into `body`; the last line when the parser counts
/// past the end (mermaid does). `None` for any other path.
pub fn quote_line(body: &str, path: &str) -> Option<String> {
    let n: usize = path.strip_prefix("line:")?.parse().ok()?;
    let lines: Vec<&str> = body.lines().collect();
    let line = lines.get(n.checked_sub(1)?).or(lines.last())?;
    Some(format!("line {n}: {}", line.trim()))
}

/// The mermaid source of a diagram component — `flowchart` for a `Graph`, `sequenceDiagram`
/// for a `Sequence` (`ref`s dropped) — for channels that carry mermaid but not `json-render`
/// (Slack, Signal, markdown export). `None` for every other component.
pub fn to_mermaid(c: &Component) -> Option<String> {
    match c {
        Component::Graph(g) => Some(graph::to_mermaid(g)),
        Component::Sequence(s) => Some(sequence::to_mermaid(s)),
        _ => None,
    }
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

    const VALID: [(&str, &str); 34] = [
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
        fixture!("graph-architecture"),
        fixture!("graph-dependency-lr"),
        fixture!("graph-pipeline-tb"),
        fixture!("valid-steps"),
        fixture!("valid-callout"),
        fixture!("valid-code"),
        fixture!("valid-diff"),
        fixture!("valid-tree"),
        fixture!("valid-glossary"),
        fixture!("valid-quote"),
        fixture!("valid-image"),
        fixture!("valid-links"),
        fixture!("valid-tags"),
        fixture!("valid-gauge"),
        fixture!("valid-sparkline"),
        fixture!("valid-heatmap"),
        fixture!("valid-person"),
        fixture!("valid-files"),
        fixture!("valid-kanban"),
        fixture!("valid-calendar"),
        fixture!("valid-excerpt"),
        fixture!("valid-sequence"),
    ];

    const REPAIRABLE: [(&str, &str); 4] = [
        fixture!("broken-trailing-comma"),
        fixture!("broken-single-quotes"),
        fixture!("broken-comments"),
        fixture!("broken-python-literals"),
    ];

    const TRUNCATED: (&str, &str) = fixture!("broken-truncated");

    const LOCATED: [((&str, &str), &str); 14] = [
        (fixture!("broken-unknown-type"), "/elements/t/type"),
        (fixture!("broken-missing-child"), "/elements/r/children/1"),
        (fixture!("broken-shared-child"), "/"),
        (
            fixture!("broken-prop-type"),
            "/elements/m/props/items/0/value",
        ),
        (
            fixture!("graph-broken-unknown-endpoint"),
            "/elements/g/props/edges/1/to",
        ),
        (
            fixture!("graph-broken-dup-id"),
            "/elements/g/props/nodes/2/id",
        ),
        (
            fixture!("graph-broken-dup-id"),
            "/elements/g/props/nodes/2/group",
        ),
        (
            fixture!("broken-heatmap-ragged"),
            "/elements/h/props/cells/1",
        ),
        (fixture!("broken-image-src"), "/elements/i/props/src"),
        (fixture!("broken-link-url"), "/elements/l/props/items/0/url"),
        (fixture!("broken-excerpt-mark"), "/elements/x/props/mark/1"),
        (
            fixture!("broken-sequence-actor"),
            "/elements/s/props/steps/1/to",
        ),
        (
            fixture!("broken-ref-scheme"),
            "/elements/g/props/nodes/0/ref",
        ),
        (
            fixture!("broken-ref-scheme"),
            "/elements/s/props/steps/0/ref",
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

    #[test]
    fn fences_of_hands_out_closed_bodies_by_language_and_quote_line_locates() {
        let text = "```mermaid\ngraph TD\n  A --> B[\n```\n\n```openui\nroot = Stack([])\n```\n\n```mermaid\nsequenceDiagram\n";
        assert_eq!(
            fences_of(text, "mermaid"),
            vec![(1, Some("graph TD\n  A --> B[\n")), (2, None)]
        );
        assert_eq!(
            fences_of(text, "openui"),
            vec![(1, Some("root = Stack([])\n"))]
        );
        assert!(fences_of(text, "json-render").is_empty());
        let body = "graph TD\n  A --> B[\n";
        assert_eq!(
            quote_line(body, "line:2").as_deref(),
            Some("line 2: A --> B[")
        );
        // mermaid counts one past the end for a trailing newline: the last line is quoted.
        assert_eq!(
            quote_line(body, "line:3").as_deref(),
            Some("line 3: A --> B[")
        );
        assert_eq!(quote_line(body, "line:0"), None);
        assert_eq!(quote_line(body, "/root"), None);
    }
}
