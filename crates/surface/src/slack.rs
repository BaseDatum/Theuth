//! Slack Block Kit (design doc 110 §8): a validated `json-render` fence as native blocks.
//!
//! Pure: fence body in, blocks out — or an error so the caller keeps today's
//! [`crate::flatten`] for the whole fence. Shapes and caps follow the Block Kit reference
//! (docs.slack.dev/reference/block-kit/blocks); strings over a cap are clipped with `…`.
//! Inside a `container` Slack allows only `context`, `divider`, `header`, `image`,
//! `rich_text` and `section`, so nested cards hoist their children under a `header`, and a
//! nested `Table`/`Bars`/`Alert` takes its text form.

use std::collections::BTreeSet;

use markdown::{Line, Style};
use serde_json::{Value, json};

use crate::SpecError;
use crate::render::{plain_text, span};
use crate::spec::*;

/// Most top-level blocks one fence may render to (one `appendStream` chunk).
pub const MAX_BLOCKS: usize = 50;
/// Most `Table` rows that become a `data_table`; more → preformatted text.
pub const MAX_TABLE_ROWS: usize = 15;
const TABLE_COLS: usize = 20;
const TABLE_CELL: usize = 2000;
const TABLE_CHARS: usize = 20_000;
const CHART_POINTS: usize = 20;
const CHART_LABEL: usize = 20;
const CHART_TITLE: usize = 50;
const CHARTS_PER_MESSAGE: usize = 2;
const HEADER: usize = 150;
const ALERT: usize = 200;
const SECTION: usize = 3000;
const FIELD: usize = 2000;
const FIELDS: usize = 10;
const CONTAINER_TITLE: usize = 150;
const CHILD_BLOCKS: usize = 10;
const URL: usize = 3000;
const ALT: usize = 2000;

/// Why a fence has no Block Kit form.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlackBlocksError {
    /// The body did not parse or validate ([`crate::parse`]'s errors).
    Invalid(Vec<SpecError>),
    /// A component with no block shape; the caller flattens the whole fence.
    Unsupported(&'static str),
    /// More than [`MAX_BLOCKS`] top-level blocks.
    TooManyBlocks(usize),
}

impl std::fmt::Display for SlackBlocksError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(errs) => {
                let s: Vec<String> = errs.iter().map(ToString::to_string).collect();
                write!(f, "invalid fence: {}", s.join("; "))
            }
            Self::Unsupported(c) => write!(f, "unsupported component {c}"),
            Self::TooManyBlocks(n) => write!(f, "{n} blocks, over the {MAX_BLOCKS} block limit"),
        }
    }
}

impl std::error::Error for SlackBlocksError {}

/// Render one `json-render` fence body as Slack blocks (design doc 110 §8's table).
pub fn slack_blocks(fence_json: &str) -> Result<Vec<Value>, SlackBlocksError> {
    let parsed = crate::parse(fence_json).map_err(SlackBlocksError::Invalid)?;
    let mut out = Vec::new();
    Render {
        spec: &parsed.spec,
        charts: 0,
    }
    .element(&parsed.spec.root, false, &mut out)?;
    if out.len() > MAX_BLOCKS {
        return Err(SlackBlocksError::TooManyBlocks(out.len()));
    }
    Ok(out)
}

struct Render<'a> {
    spec: &'a Spec,
    /// `data_visualization` blocks emitted so far (Slack allows two per message).
    charts: usize,
}

impl Render<'_> {
    /// Push the blocks of element `id`; `nested` = inside a `container`.
    fn element(
        &mut self,
        id: &str,
        nested: bool,
        out: &mut Vec<Value>,
    ) -> Result<(), SlackBlocksError> {
        let Some(el) = self.spec.elements.get(id) else {
            return Ok(());
        };
        match &el.component {
            Component::Card(p) => {
                let mut kids = Vec::new();
                self.children(el, true, &mut kids)?;
                if nested || p.title.is_none() || kids.is_empty() {
                    if let Some(t) = &p.title {
                        out.push(header(t, None));
                    }
                    if let Some(d) = &p.description {
                        out.push(context(d));
                    }
                    out.extend(kids);
                } else {
                    container(
                        p.title.as_deref().unwrap_or_default(),
                        p.description.as_deref(),
                        &kids,
                        false,
                        out,
                    );
                }
            }
            Component::Stack(_) | Component::Grid(_) => self.children(el, nested, out)?,
            Component::Accordion(p) => self.panes(&p.titles, el, nested, out)?,
            Component::Tabs(p) => self.panes(&p.labels, el, nested, out)?,
            Component::Separator(_) => out.push(json!({"type": "divider"})),
            Component::Heading(p) => out.push(header(&p.text, p.level)),
            Component::Text(p) => match p.variant {
                Some(TextVariant::Muted | TextVariant::Caption) => out.push(context(&p.text)),
                Some(TextVariant::Code) => out.push(section(&format!("```\n{}\n```", p.text))),
                _ => out.push(section(&p.text)),
            },
            Component::Badge(p) => out.push(context(&format!("`{}`", p.text))),
            Component::Alert(p) => {
                let level = match p.kind.unwrap_or(AlertType::Info) {
                    AlertType::Info => "info",
                    AlertType::Success => "success",
                    AlertType::Warning => "warning",
                    AlertType::Error => "error",
                };
                out.push(alert(nested, Some(level), p.title.as_deref(), &p.message));
            }
            Component::Callout(p) => out.push(alert(nested, None, p.title.as_deref(), &p.text)),
            Component::Table(p) => out.push(table(p, nested)),
            Component::KeyValueList(p) => {
                fields(
                    p.items.iter().map(|i| format!("*{}*\n{}", i.key, i.value)),
                    out,
                );
            }
            Component::Metric(p) => fields(std::iter::once(metric(p)), out),
            Component::MetricGrid(p) => fields(p.items.iter().map(metric), out),
            Component::Bars(p) => {
                let points: Vec<(String, f64)> =
                    p.items.iter().map(|b| (b.label.clone(), b.value)).collect();
                out.push(self.chart(nested, "bar", "Values", points));
            }
            Component::Sparkline(p) => {
                // ponytail: a line chart holds 20 points, so a longer sparkline shows its last 20.
                let skip = p.values.len().saturating_sub(CHART_POINTS);
                let points: Vec<(String, f64)> = p
                    .values
                    .iter()
                    .enumerate()
                    .skip(skip)
                    .map(|(i, v)| ((i + 1).to_string(), *v))
                    .collect();
                out.push(self.chart(
                    nested,
                    "line",
                    p.label.as_deref().unwrap_or("Trend"),
                    points,
                ));
            }
            Component::Timeline(p) => out.push(list(
                "bullet",
                p.items.iter().map(|i| {
                    let glyph = match i.state.unwrap_or(TimelineState::Pending) {
                        TimelineState::Done => "✓",
                        TimelineState::Active => "▶",
                        TimelineState::Pending => "○",
                    };
                    let mut row = vec![text(&format!("{glyph} {} — ", i.when)), bold(&i.title)];
                    if let Some(d) = &i.detail {
                        row.push(text(&format!(" — {d}")));
                    }
                    row
                }),
            )),
            Component::Checklist(p) => out.push(list(
                "bullet",
                p.items.iter().map(|i| {
                    vec![text(&format!(
                        "{} {}",
                        if i.done { "✓" } else { "○" },
                        i.text
                    ))]
                }),
            )),
            Component::Steps(p) => out.push(list(
                "ordered",
                p.items.iter().map(|s| {
                    let mut row = vec![bold(&s.title)];
                    if let Some(d) = &s.detail {
                        row.push(text(&format!(" — {d}")));
                    }
                    row
                }),
            )),
            Component::Glossary(p) => out.push(list(
                "bullet",
                p.items
                    .iter()
                    .map(|d| vec![bold(&d.term), text(&format!(" — {}", d.text))]),
            )),
            Component::Tags(p) => {
                let tags: Vec<String> = p.items.iter().map(|t| format!("`{}`", t.text)).collect();
                out.push(context(&tags.join(" · ")));
            }
            Component::Code(p) => {
                let mut elements = Vec::new();
                if let Some(t) = &p.title {
                    elements.push(json!({"type": "rich_text_section", "elements": [bold(t)]}));
                }
                elements.push(preformatted(&p.text));
                out.push(rich_text(elements));
            }
            Component::Quote(p) => {
                let mut elements = vec![text(&p.text)];
                if let Some(s) = &p.source {
                    elements.push(json!({"type": "text", "text": format!(" — {s}"), "style": {"italic": true}}));
                }
                out.push(rich_text(vec![
                    json!({"type": "rich_text_quote", "elements": elements}),
                ]));
            }
            Component::Image(p) => {
                // Slack fetches the picture itself: http(s) only, and a clipped URL is no URL.
                if !is_http(&p.src) || p.src.chars().count() > URL {
                    return Err(SlackBlocksError::Unsupported("Image"));
                }
                let mut b =
                    json!({"type": "image", "image_url": p.src, "alt_text": clip(&p.alt, ALT)});
                if let Some(c) = &p.caption {
                    b["title"] = plain(c, FIELD);
                }
                out.push(b);
            }
            Component::LinkList(p) => out.push(list(
                "bullet",
                p.items.iter().map(|l| {
                    let mut row = vec![json!({"type": "link", "url": l.url, "text": l.title})];
                    if let Some(n) = &l.note {
                        row.push(text(&format!(" — {n}")));
                    }
                    row
                }),
            )),
            Component::Graph(_) => return Err(SlackBlocksError::Unsupported("Graph")),
            Component::Sequence(_) => return Err(SlackBlocksError::Unsupported("Sequence")),
            Component::Heatmap(_) => return Err(SlackBlocksError::Unsupported("Heatmap")),
            Component::Kanban(_) => return Err(SlackBlocksError::Unsupported("Kanban")),
            Component::Calendar(_) => return Err(SlackBlocksError::Unsupported("Calendar")),
            Component::Gauge(_) => return Err(SlackBlocksError::Unsupported("Gauge")),
            Component::Excerpt(_) => return Err(SlackBlocksError::Unsupported("Excerpt")),
            Component::Progress(_) => return Err(SlackBlocksError::Unsupported("Progress")),
            Component::Person(_) => return Err(SlackBlocksError::Unsupported("Person")),
            Component::FileList(_) => return Err(SlackBlocksError::Unsupported("FileList")),
            Component::Tree(_) => return Err(SlackBlocksError::Unsupported("Tree")),
            Component::Diff(_) => return Err(SlackBlocksError::Unsupported("Diff")),
        }
        Ok(())
    }

    fn children(
        &mut self,
        el: &Element,
        nested: bool,
        out: &mut Vec<Value>,
    ) -> Result<(), SlackBlocksError> {
        for id in &el.children {
            self.element(id, nested, out)?;
        }
        Ok(())
    }

    /// `Accordion`/`Tabs`: one collapsed `container` per titled child.
    fn panes(
        &mut self,
        titles: &[String],
        el: &Element,
        nested: bool,
        out: &mut Vec<Value>,
    ) -> Result<(), SlackBlocksError> {
        for (title, id) in titles.iter().zip(&el.children) {
            let mut kids = Vec::new();
            self.element(id, true, &mut kids)?;
            if nested || kids.is_empty() {
                out.push(header(title, None));
                out.extend(kids);
            } else {
                container(title, None, &kids, true, out);
            }
        }
        Ok(())
    }

    /// One-series `data_visualization`, or a `label: value` listing where Slack would refuse
    /// the chart (nested, empty, over 20 points, clipped labels colliding, third chart).
    fn chart(
        &mut self,
        nested: bool,
        kind: &str,
        title: &str,
        points: Vec<(String, f64)>,
    ) -> Value {
        let labels: Vec<String> = points.iter().map(|(l, _)| clip(l, CHART_LABEL)).collect();
        let unique = labels.iter().collect::<BTreeSet<_>>().len() == labels.len();
        if nested
            || points.is_empty()
            || points.len() > CHART_POINTS
            || !unique
            || self.charts >= CHARTS_PER_MESSAGE
        {
            let rows: Vec<String> = points.iter().map(|(l, v)| format!("{l}: {v}")).collect();
            return rich_text(vec![preformatted(&rows.join("\n"))]);
        }
        self.charts += 1;
        let data: Vec<Value> = labels
            .iter()
            .zip(&points)
            .map(|(l, (_, v))| json!({"label": l, "value": v}))
            .collect();
        json!({
            "type": "data_visualization",
            "title": clip(title, CHART_TITLE),
            "chart": {
                "type": kind,
                "series": [{"name": "Value", "data": data}],
                "axis_config": {"categories": labels},
            },
        })
    }
}

/// `data_table` when it fits Slack's table, else the TUI table as preformatted text.
fn table(p: &TableProps, nested: bool) -> Value {
    let cells = || p.rows.iter().flatten();
    let chars: usize = cells().chain(&p.columns).map(|c| c.chars().count()).sum();
    let fits = !nested
        && !p.rows.is_empty()
        && p.rows.len() <= MAX_TABLE_ROWS
        && p.columns.len() <= TABLE_COLS
        && chars <= TABLE_CHARS
        && cells().all(|c| c.chars().count() <= TABLE_CELL);
    if !fits {
        let line = |c: &String, style: Style| -> Line { vec![span(c, style)] };
        let bold = Style {
            bold: true,
            ..Style::default()
        };
        let rows = std::iter::once(p.columns.iter().map(|c| line(c, bold.clone())).collect())
            .chain(
                p.rows
                    .iter()
                    .map(|r| r.iter().map(|c| line(c, Style::default())).collect()),
            )
            .collect();
        return rich_text(vec![preformatted(&plain_text(&markdown::layout_table(
            rows,
            &[],
        )))]);
    }
    let cell = |c: &String| -> Value {
        match c.trim().parse::<f64>() {
            Ok(n) if n.is_finite() => json!({"type": "raw_number", "value": n, "text": c}),
            _ => json!({"type": "raw_text", "text": clip(c, TABLE_CELL)}),
        }
    };
    let header: Vec<Value> = p
        .columns
        .iter()
        .map(|c| json!({"type": "raw_text", "text": clip(c, TABLE_CELL)}))
        .collect();
    let rows: Vec<Vec<Value>> = std::iter::once(header)
        .chain(p.rows.iter().map(|r| r.iter().map(cell).collect()))
        .collect();
    json!({"type": "data_table", "caption": "Table", "rows": rows})
}

/// `*label*` over value, trend glyph and delta.
fn metric(p: &MetricProps) -> String {
    let mut s = format!("*{}*\n{}", p.label, p.value);
    if let Some(t) = p.trend {
        s.push_str(match t {
            Trend::Up => " ↑",
            Trend::Down => " ↓",
            Trend::Flat => " →",
        });
    }
    if let Some(d) = &p.delta {
        s.push(' ');
        s.push_str(d);
    }
    s
}

/// `section` blocks of ≤ 10 `mrkdwn` fields each.
fn fields(items: impl Iterator<Item = String>, out: &mut Vec<Value>) {
    let items: Vec<Value> = items.map(|s| mrkdwn(&s, FIELD)).collect();
    for chunk in items.chunks(FIELDS) {
        out.push(json!({"type": "section", "fields": chunk}));
    }
}

/// `alert` (≤ 200 chars) at the top level; a `section` inside a container.
fn alert(nested: bool, level: Option<&str>, title: Option<&str>, body: &str) -> Value {
    let text = match title {
        Some(t) => format!("*{t}*\n{body}"),
        None => body.to_owned(),
    };
    if nested {
        return section(&text);
    }
    let mut b = json!({"type": "alert", "text": mrkdwn(&text, ALERT)});
    if let Some(l) = level {
        b["level"] = l.into();
    }
    b
}

/// `container`s of ≤ 10 children each, all with the same title.
fn container(
    title: &str,
    subtitle: Option<&str>,
    kids: &[Value],
    collapsible: bool,
    out: &mut Vec<Value>,
) {
    for chunk in kids.chunks(CHILD_BLOCKS) {
        let mut b = json!({"type": "container", "title": plain(title, CONTAINER_TITLE), "child_blocks": chunk});
        if let Some(s) = subtitle {
            b["subtitle"] = plain(s, CONTAINER_TITLE);
        }
        if collapsible {
            b["is_collapsible"] = true.into();
            b["default_collapsed"] = true.into();
        }
        out.push(b);
    }
}

fn header(text: &str, level: Option<u8>) -> Value {
    let mut b = json!({"type": "header", "text": plain(text, HEADER)});
    if let Some(l) = level {
        b["level"] = l.into();
    }
    b
}

fn section(text: &str) -> Value {
    json!({"type": "section", "text": mrkdwn(text, SECTION)})
}

fn context(text: &str) -> Value {
    json!({"type": "context", "elements": [mrkdwn(text, FIELD)]})
}

fn plain(text: &str, cap: usize) -> Value {
    json!({"type": "plain_text", "text": clip(text, cap)})
}

fn mrkdwn(text: &str, cap: usize) -> Value {
    json!({"type": "mrkdwn", "text": clip(text, cap)})
}

fn rich_text(elements: Vec<Value>) -> Value {
    json!({"type": "rich_text", "elements": elements})
}

fn preformatted(text: &str) -> Value {
    json!({"type": "rich_text_preformatted", "elements": [self::text(text)]})
}

/// A `rich_text` block holding one list; each item is a row of rich text elements.
fn list(style: &str, items: impl Iterator<Item = Vec<Value>>) -> Value {
    let elements: Vec<Value> = items
        .map(|row| json!({"type": "rich_text_section", "elements": row}))
        .collect();
    rich_text(vec![
        json!({"type": "rich_text_list", "style": style, "elements": elements}),
    ])
}

fn text(s: &str) -> Value {
    json!({"type": "text", "text": s})
}

fn bold(s: &str) -> Value {
    json!({"type": "text", "text": s, "style": {"bold": true}})
}

fn is_http(url: &str) -> bool {
    let lower = url.get(..8).unwrap_or(url).to_ascii_lowercase();
    lower.starts_with("http://") || lower.starts_with("https://")
}

/// At most `cap` characters, the last one `…` when clipped; `—` for an empty string
/// (every Slack text field has a minimum length of 1).
fn clip(s: &str, cap: usize) -> String {
    if s.is_empty() {
        return "—".to_owned();
    }
    match s.char_indices().nth(cap - 1) {
        Some((i, _)) if s[i..].chars().nth(1).is_some() => format!("{}…", &s[..i]),
        _ => s.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;

    fn blocks(body: &str) -> Vec<Value> {
        slack_blocks(body).expect("renders")
    }

    fn doc(root: &str, elements: &str) -> String {
        format!(r#"{{"root":"{root}","elements":{{{elements}}}}}"#)
    }

    /// Every `fixtures/slack/<name>.json` against `<name>.blocks.json`: the block array, or
    /// `{"error": …}` for a refused fence. `UPDATE_GOLDEN=1` rewrites the files instead.
    #[test]
    fn fixtures_match_golden_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let update = std::env::var_os("UPDATE_GOLDEN").is_some();
        let mut fixtures: Vec<_> = fs::read_dir(root.join("fixtures/slack"))
            .expect("fixtures dir")
            .map(|e| e.expect("dir entry").path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".json") && !n.ends_with(".blocks.json"))
            })
            .collect();
        fixtures.sort();
        assert!(!fixtures.is_empty());
        let mut failures = Vec::new();
        for path in fixtures {
            let body = fs::read_to_string(&path).expect("fixture");
            let got = match slack_blocks(&body) {
                Ok(blocks) => Value::Array(blocks),
                Err(e) => json!({"error": e.to_string()}),
            };
            let got = format!("{}\n", serde_json::to_string_pretty(&got).expect("json"));
            let golden = path.with_extension("blocks.json");
            if update {
                fs::write(&golden, &got).expect("write golden");
                continue;
            }
            let want =
                fs::read_to_string(&golden).unwrap_or_else(|e| panic!("{}: {e}", golden.display()));
            if want != got {
                failures.push(format!(
                    "{}:\n--- want\n{want}\n--- got\n{got}",
                    path.display()
                ));
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    #[test]
    fn container_children_cap_at_ten() {
        let kids: Vec<String> = (0..23).map(|i| format!("s{i}")).collect();
        let mut els = format!(
            r#""c":{{"type":"Card","props":{{"title":"Many"}},"children":{}}}"#,
            serde_json::to_string(&kids).unwrap()
        );
        for k in &kids {
            els.push_str(&format!(r#","{k}":{{"type":"Separator"}}"#));
        }
        let out = blocks(&doc("c", &els));
        let sizes: Vec<usize> = out
            .iter()
            .map(|b| b["child_blocks"].as_array().unwrap().len())
            .collect();
        assert_eq!(sizes, [10, 10, 3]);
        assert!(out.iter().all(|b| b["title"]["text"] == "Many"));
    }

    #[test]
    fn nested_card_hoists_under_a_header_and_nested_table_is_text() {
        let body = doc(
            "outer",
            r#""outer":{"type":"Card","props":{"title":"Outer"},"children":["inner"]},
               "inner":{"type":"Card","props":{"title":"Inner","description":"d"},"children":["t","a"]},
               "t":{"type":"Table","props":{"columns":["k"],"rows":[["v"]]}},
               "a":{"type":"Alert","props":{"message":"m"}}"#,
        );
        let out = blocks(&body);
        assert_eq!(out.len(), 1);
        let kinds: Vec<&str> = out[0]["child_blocks"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["type"].as_str().unwrap())
            .collect();
        assert_eq!(kinds, ["header", "context", "rich_text", "section"]);
    }

    #[test]
    fn fields_split_at_ten_per_section() {
        let items: Vec<Value> = (0..12)
            .map(|i| json!({"key": format!("k{i}"), "value": "v"}))
            .collect();
        let body = doc(
            "kv",
            &format!(
                r#""kv":{{"type":"KeyValueList","props":{{"items":{}}}}}"#,
                Value::Array(items)
            ),
        );
        let out = blocks(&body);
        let sizes: Vec<usize> = out
            .iter()
            .map(|b| b["fields"].as_array().unwrap().len())
            .collect();
        assert_eq!(sizes, [10, 2]);
    }

    #[test]
    fn urls_are_http_only() {
        let data = doc(
            "i",
            r#""i":{"type":"Image","props":{"src":"data:image/png;base64,AAAA","alt":"x"}}"#,
        );
        assert_eq!(
            slack_blocks(&data),
            Err(SlackBlocksError::Unsupported("Image"))
        );
        let http = doc(
            "i",
            r#""i":{"type":"Image","props":{"src":"HTTP://example.com/a.png","alt":"x"}}"#,
        );
        assert_eq!(blocks(&http)[0]["image_url"], "HTTP://example.com/a.png");
    }

    #[test]
    fn strings_clip_at_slack_caps_with_an_ellipsis() {
        assert_eq!(clip("héllo", 5), "héllo");
        assert_eq!(clip("héllo!", 5), "héll…");
        assert_eq!(clip("", 5), "—");
        let long = "x".repeat(300);
        let body = doc(
            "s",
            &format!(
                r#""s":{{"type":"Stack","children":["h","a"]}},
                   "h":{{"type":"Heading","props":{{"text":"{long}"}}}},
                   "a":{{"type":"Alert","props":{{"title":"t","message":"{long}","type":"error"}}}}"#
            ),
        );
        let out = blocks(&body);
        assert_eq!(
            out[0]["text"]["text"].as_str().unwrap().chars().count(),
            HEADER
        );
        let alert = out[1]["text"]["text"].as_str().unwrap();
        assert_eq!(alert.chars().count(), ALERT);
        assert!(alert.starts_with("*t*\nxxx") && alert.ends_with('…'));
        assert_eq!(out[1]["level"], "error");
    }

    #[test]
    fn third_chart_and_long_series_fall_back_to_text() {
        let values: Vec<f64> = (1..=25).map(f64::from).collect();
        let body = doc(
            "s",
            &format!(
                r#""s":{{"type":"Stack","children":["a","b","c","d"]}},
                   "a":{{"type":"Sparkline","props":{{"values":{v}}}}},
                   "b":{{"type":"Sparkline","props":{{"values":[1,2]}}}},
                   "c":{{"type":"Sparkline","props":{{"values":[3,4]}}}},
                   "d":{{"type":"Bars","props":{{"items":[{{"label":"a","value":1}},{{"label":"a","value":2}}]}}}}"#,
                v = Value::from(values)
            ),
        );
        let out = blocks(&body);
        let kinds: Vec<&str> = out.iter().map(|b| b["type"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "data_visualization",
                "data_visualization",
                "rich_text",
                "rich_text"
            ]
        );
        let cats = out[0]["chart"]["axis_config"]["categories"]
            .as_array()
            .unwrap();
        assert_eq!(cats.len(), CHART_POINTS);
        assert_eq!(
            (cats[0].as_str().unwrap(), cats[19].as_str().unwrap()),
            ("6", "25")
        );
        assert_eq!(
            out[3]["elements"][0]["elements"][0]["text"], "a: 1\na: 2",
            "duplicate labels cannot be axis categories"
        );
    }

    #[test]
    fn invalid_body_is_an_error() {
        assert!(matches!(
            slack_blocks("{"),
            Err(SlackBlocksError::Invalid(_))
        ));
    }
}
