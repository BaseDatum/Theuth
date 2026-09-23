//! `Spec` → terminal lines (design doc 30 §7) and the `markdown` fence hook.
//!
//! Styles: box drawing, rules and prefixes `dim`; `Heading`, `Metric` values, `Alert`
//! titles and `Tabs`/`Accordion` labels `bold`; `Text{variant: code}` `code`;
//! `Text{variant: muted|caption}` dim; everything else `Style::default()`.

use std::time::Instant;

use markdown::{Line, Span, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::spec::{
    AlertType, BadgeVariant, Component, Direction, Gap, MetricProps, TextVariant, TimelineState,
    Trend,
};
use crate::{Spec, mermaid_lines, parse};

/// Render a spec to lines at `width` columns (0 = unbounded); `None` if any line would be
/// wider than `width`, so the caller falls back to the source block.
pub fn spec_lines(spec: &Spec, width: usize) -> Option<Vec<Line>> {
    let lines = element(spec, &spec.root, width);
    (width == 0 || lines.iter().all(|l| line_width(l) <= width)).then_some(lines)
}

/// The `markdown` fence hook: `mermaid` and `json-render` fences become their rendering;
/// a fence that cannot fit `width` or a Mermaid type the renderer does not draw returns
/// `None` (the plain code block); a fence with a syntax or validation error returns one dim
/// `◌ diagram omitted` line (30 §7 as-built: the correction rounds are the model's business,
/// the user is not shown a red card). Other languages return `None`. Logs
/// `fence{kind, ok, repaired, ms}` on target `surface` for every fence it handles.
pub fn fence(lang: &str, body: &str, width: usize) -> Option<Vec<Line>> {
    let start = Instant::now();
    let (lines, ok, repaired) = match lang {
        "mermaid" => match mermaid_lines(body, width) {
            Ok(Some(lines)) => (Some(lines), true, false),
            Ok(None) => (None, false, false),
            Err(msg) => {
                let first = msg.lines().next().unwrap_or_default();
                tracing::debug!(target: "surface", kind = lang, "omitted: {first}");
                (Some(omitted()), false, false)
            }
        },
        "json-render" => match parse(body) {
            Ok(p) => {
                let lines = spec_lines(&p.spec, width);
                (lines, true, p.repaired)
            }
            Err(errs) => {
                let first = errs.first().map(ToString::to_string).unwrap_or_default();
                tracing::debug!(target: "surface", kind = lang, "omitted: {first}");
                (Some(omitted()), false, false)
            }
        },
        _ => return None,
    };
    let ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::info!(target: "surface", kind = lang, ok, repaired, ms, "fence");
    lines
}

/// What stands in for a block that would not render: one dim line, nothing else. The source
/// stays in the transcript's raw view.
fn omitted() -> Vec<Line> {
    vec![vec![span("◌ diagram omitted", DIM)]]
}

/// Span texts concatenated per line, lines joined by `\n`; no styling.
pub(crate) fn plain_text(lines: &[Line]) -> String {
    let mut out = String::new();
    for (i, l) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        for s in l {
            out.push_str(&s.text);
        }
    }
    out
}

const PLAIN: Style = Style {
    bold: false,
    italic: false,
    strike: false,
    code: false,
    dim: false,
    heading: 0,
    link: None,
};
const DIM: Style = Style { dim: true, ..PLAIN };
const BOLD: Style = Style {
    bold: true,
    ..PLAIN
};
const CODE: Style = Style {
    code: true,
    ..PLAIN
};

fn span(text: impl Into<String>, style: Style) -> Span {
    Span {
        text: text.into(),
        style,
    }
}

fn line_width(l: &Line) -> usize {
    l.iter().map(|s| s.text.width()).sum()
}

fn block_width(lines: &[Line]) -> usize {
    lines.iter().map(line_width).max().unwrap_or(0)
}

/// Width left inside `n` columns of decoration; 0 stays unbounded.
fn sub(width: usize, n: usize) -> usize {
    if width == 0 {
        0
    } else {
        width.saturating_sub(n).max(1)
    }
}

/// Extend `line` with spaces up to `to` columns.
fn pad(line: &mut Line, to: usize) {
    let w = line_width(line);
    if w < to {
        line.push(span(" ".repeat(to - w), PLAIN));
    }
}

/// Word wrap at `width` columns (0 = no wrap); `\n` in `text` always breaks. Words wider
/// than `width` are broken by character.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in text.split('\n') {
        if width == 0 {
            out.push(para.to_owned());
            continue;
        }
        let mut line = String::new();
        let mut lw = 0;
        for word in para.split_whitespace() {
            let ww = word.width();
            if lw > 0 && lw + 1 + ww > width {
                out.push(std::mem::take(&mut line));
                lw = 0;
            }
            if lw > 0 {
                line.push(' ');
                lw += 1;
            }
            if ww <= width {
                line.push_str(word);
                lw += ww;
                continue;
            }
            for c in word.chars() {
                let cw = c.width().unwrap_or(0);
                if lw > 0 && lw + cw > width {
                    out.push(std::mem::take(&mut line));
                    lw = 0;
                }
                line.push(c);
                lw += cw;
            }
        }
        out.push(line);
    }
    out
}

/// `text` wrapped at `width`, one span per line in `style`.
fn text_lines(text: &str, width: usize, style: Style) -> Vec<Line> {
    wrap(text, width)
        .into_iter()
        .map(|l| {
            if l.is_empty() {
                Line::new()
            } else {
                vec![span(l, style.clone())]
            }
        })
        .collect()
}

/// Blocks one under another, a blank line between when `gap`.
fn stack(blocks: Vec<Vec<Line>>, gap: bool) -> Vec<Line> {
    let mut out = Vec::new();
    for (i, b) in blocks.into_iter().enumerate() {
        if i > 0 && gap {
            out.push(Line::new());
        }
        out.extend(b);
    }
    out
}

/// Blocks side by side: column `i` starts after `widths[..i]` plus two spaces per gap;
/// nothing is padded after the last column with content on a row.
fn beside(blocks: &[Vec<Line>], widths: &[usize]) -> Vec<Line> {
    let rows = blocks.iter().map(Vec::len).max().unwrap_or(0);
    (0..rows)
        .map(|r| {
            let mut line = Line::new();
            let mut start = 0;
            for (b, w) in blocks.iter().zip(widths) {
                if let Some(l) = b.get(r)
                    && !l.is_empty()
                {
                    pad(&mut line, start);
                    line.extend(l.iter().cloned());
                }
                start += w + 2;
            }
            line
        })
        .collect()
}

/// Every line prefixed with two spaces; blank lines stay blank.
fn indent(lines: Vec<Line>) -> Vec<Line> {
    lines
        .into_iter()
        .map(|mut l| {
            if !l.is_empty() {
                l.insert(0, span("  ", PLAIN));
            }
            l
        })
        .collect()
}

/// Bold `labels[i]` over the indented `children[i]`, a blank line between sections
/// (`Tabs`, `Accordion`).
fn sections(labels: &[String], children: Vec<Vec<Line>>) -> Vec<Line> {
    let mut out = Vec::new();
    for (i, (label, child)) in labels.iter().zip(children).enumerate() {
        if i > 0 {
            out.push(Line::new());
        }
        out.push(vec![span(label, BOLD)]);
        out.extend(indent(child));
    }
    out
}

/// `frac` of `cells` as `█` then dim `░`.
fn bar(frac: f64, cells: usize) -> [Span; 2] {
    let filled = ((frac * cells as f64).round() as usize).min(cells);
    [
        span("█".repeat(filled), PLAIN),
        span("░".repeat(cells - filled), DIM),
    ]
}

/// `value / max` clamped to `0..=1`; 0 when `max` is not positive.
fn frac(value: f64, max: f64) -> f64 {
    if max > 0.0 {
        (value / max).clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Dim `label` over bold `value`, trend glyph and dim delta beside it.
fn metric(p: &MetricProps) -> Vec<Line> {
    let mut value = vec![span(&p.value, BOLD)];
    if let Some(t) = p.trend {
        let glyph = match t {
            Trend::Up => " ▲",
            Trend::Down => " ▼",
            Trend::Flat => " ▶",
        };
        value.push(span(glyph, PLAIN));
    }
    if let Some(d) = &p.delta {
        value.push(span(format!(" {d}"), DIM));
    }
    vec![vec![span(&p.label, DIM)], value]
}

/// Bar length when `width` is unbounded.
const BAR_UNBOUNDED: usize = 20;

/// One element and its subtree at `width`.
fn element(spec: &Spec, id: &str, width: usize) -> Vec<Line> {
    let Some(el) = spec.elements.get(id) else {
        return Vec::new();
    };
    let kids =
        |w: usize| -> Vec<Vec<Line>> { el.children.iter().map(|c| element(spec, c, w)).collect() };
    match &el.component {
        Component::Card(p) => {
            let inner_w = sub(width, 4);
            let mut inner = Vec::new();
            if let Some(d) = &p.description {
                inner.extend(text_lines(d, inner_w, DIM));
            }
            let body = stack(kids(inner_w), true);
            if !inner.is_empty() && !body.is_empty() {
                inner.push(Line::new());
            }
            inner.extend(body);
            let title = p.title.as_deref().unwrap_or("");
            let title_w = if title.is_empty() {
                0
            } else {
                title.width() + 2
            };
            let w = block_width(&inner).max(title_w);
            let mut out = Vec::with_capacity(inner.len() + 2);
            if title.is_empty() {
                out.push(vec![span(format!("╭{}╮", "─".repeat(w + 2)), DIM)]);
            } else {
                out.push(vec![
                    span("╭─ ", DIM),
                    span(title, PLAIN),
                    span(format!(" {}╮", "─".repeat(w + 2 - title.width() - 3)), DIM),
                ]);
            }
            for mut l in inner {
                l.insert(0, span("│ ", DIM));
                pad(&mut l, w + 2);
                l.push(span(" │", DIM));
                out.push(l);
            }
            out.push(vec![span(format!("╰{}╯", "─".repeat(w + 2)), DIM)]);
            out
        }
        Component::Stack(p) => {
            let blocks = kids(width);
            if p.direction == Some(Direction::Horizontal) {
                let widths: Vec<usize> = blocks.iter().map(|b| block_width(b)).collect();
                let total = widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1);
                if width == 0 || total <= width {
                    return beside(&blocks, &widths);
                }
            }
            stack(blocks, p.gap != Some(Gap::None))
        }
        Component::Grid(p) => {
            let cols = usize::from(p.columns.max(1));
            let cw = if width == 0 {
                0
            } else {
                (sub(width, 2 * (cols - 1)) / cols).max(1)
            };
            let cells = kids(cw);
            let cw = if cw == 0 {
                cells.iter().map(|c| block_width(c)).max().unwrap_or(0)
            } else {
                cw
            };
            let widths = vec![cw; cols];
            cells
                .chunks(cols)
                .flat_map(|row| beside(row, &widths))
                .collect()
        }
        Component::Separator(_) => {
            let n = if width == 0 { 40 } else { width.min(40) };
            vec![vec![span("─".repeat(n), DIM)]]
        }
        Component::Heading(p) => {
            let style = Style {
                heading: u8::from(p.level.unwrap_or(2) == 1),
                ..BOLD
            };
            text_lines(&p.text, width, style)
        }
        Component::Text(p) => {
            let style = match p.variant {
                Some(TextVariant::Code) => CODE,
                Some(TextVariant::Muted | TextVariant::Caption) => DIM,
                _ => PLAIN,
            };
            text_lines(&p.text, width, style)
        }
        Component::Badge(p) => {
            let mark = match p.variant {
                Some(BadgeVariant::Success) => "✓ ",
                Some(BadgeVariant::Destructive) => "✗ ",
                Some(BadgeVariant::Warning) => "! ",
                _ => "",
            };
            vec![vec![span(format!("[{mark}{}]", p.text), PLAIN)]]
        }
        Component::Alert(p) => {
            let glyph = match p.kind.unwrap_or(AlertType::Info) {
                AlertType::Info => "ℹ ",
                AlertType::Success => "✓ ",
                AlertType::Warning => "⚠ ",
                AlertType::Error => "✗ ",
            };
            let mut msg = wrap(&p.message, sub(width, 2)).into_iter();
            let first = match &p.title {
                Some(t) => span(t, BOLD),
                None => span(msg.next().unwrap_or_default(), PLAIN),
            };
            let mut out = vec![vec![span(glyph, PLAIN), first]];
            out.extend(msg.map(|m| vec![span("│ ", DIM), span(m, PLAIN)]));
            out
        }
        Component::Table(p) => {
            let cell = |c: &String, style: Style| -> Line { vec![span(c, style)] };
            let rows = std::iter::once(p.columns.iter().map(|c| cell(c, BOLD)).collect())
                .chain(
                    p.rows
                        .iter()
                        .map(|r| r.iter().map(|c| cell(c, PLAIN)).collect()),
                )
                .collect();
            markdown::layout_table(rows, &[])
        }
        Component::KeyValueList(p) => {
            let k = p.items.iter().map(|i| i.key.width()).max().unwrap_or(0);
            p.items
                .iter()
                .map(|i| {
                    vec![
                        span(format!("{}{}", " ".repeat(k - i.key.width()), i.key), DIM),
                        span(format!("  {}", i.value), PLAIN),
                    ]
                })
                .collect()
        }
        Component::Metric(p) => metric(p),
        Component::MetricGrid(p) => {
            let mut rows: Vec<(Vec<Vec<Line>>, Vec<usize>)> = Vec::new();
            let mut total = 0;
            for b in p.items.iter().map(metric) {
                let w = block_width(&b);
                match rows.last_mut() {
                    Some((row, widths)) if width == 0 || total + 2 + w <= width => {
                        total += 2 + w;
                        row.push(b);
                        widths.push(w);
                    }
                    _ => {
                        total = w;
                        rows.push((vec![b], vec![w]));
                    }
                }
            }
            stack(
                rows.iter()
                    .map(|(row, widths)| beside(row, widths))
                    .collect(),
                true,
            )
        }
        Component::Progress(p) => {
            let max = p.max.unwrap_or(100.0);
            let frac = frac(p.value, max);
            let nums = format!("{}/{}  {}%", p.value, max, (frac * 100.0).round());
            let label = p.label.as_deref().unwrap_or("");
            let cells = if width == 0 {
                BAR_UNBOUNDED
            } else {
                width
                    .saturating_sub(label.width() + nums.width() + 4)
                    .max(10)
            };
            let mut line = Line::new();
            if !label.is_empty() {
                line.push(span(format!("{label}  "), PLAIN));
            }
            line.extend(bar(frac, cells));
            line.push(span(format!("  {nums}"), PLAIN));
            vec![line]
        }
        Component::Bars(p) => {
            let lw = p.items.iter().map(|i| i.label.width()).max().unwrap_or(0);
            let values: Vec<String> = p.items.iter().map(|i| i.value.to_string()).collect();
            let vw = values.iter().map(|v| v.width()).max().unwrap_or(0);
            let max = p
                .max
                .unwrap_or_else(|| p.items.iter().map(|i| i.value).fold(0.0, f64::max));
            let cells = if width == 0 {
                BAR_UNBOUNDED
            } else {
                width.saturating_sub(lw + vw + 4).max(10)
            };
            p.items
                .iter()
                .zip(values)
                .map(|(i, v)| {
                    let mut line = vec![span(
                        format!("{}{}  ", i.label, " ".repeat(lw - i.label.width())),
                        PLAIN,
                    )];
                    line.extend(bar(frac(i.value, max), cells));
                    line.push(span(format!("  {}{v}", " ".repeat(vw - v.width())), PLAIN));
                    line
                })
                .collect()
        }
        Component::Timeline(p) => {
            let mut out = Vec::new();
            for i in &p.items {
                let glyph = match i.state {
                    Some(TimelineState::Active) => "◉",
                    Some(TimelineState::Pending) => "○",
                    _ => "●",
                };
                out.push(vec![
                    span(format!("{glyph} {}  ", i.when), PLAIN),
                    span(&i.title, PLAIN),
                ]);
                if let Some(d) = &i.detail {
                    let indent = 2 + i.when.width() + 2;
                    let prefix = format!("│{}", " ".repeat(indent - 1));
                    out.extend(
                        wrap(d, sub(width, indent))
                            .into_iter()
                            .map(|l| vec![span(&prefix, DIM), span(l, PLAIN)]),
                    );
                }
            }
            out
        }
        Component::Checklist(p) => {
            let mut out = Vec::new();
            for i in &p.items {
                let mut lines = wrap(&i.text, sub(width, 4)).into_iter();
                out.push(vec![
                    span(if i.done { "[x] " } else { "[ ] " }, DIM),
                    span(lines.next().unwrap_or_default(), PLAIN),
                ]);
                out.extend(lines.map(|l| vec![span(format!("    {l}"), PLAIN)]));
            }
            out
        }
        Component::Tabs(p) => sections(&p.labels, kids(sub(width, 2))),
        Component::Accordion(p) => sections(&p.titles, kids(sub(width, 2))),
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;

    use super::*;
    use crate::flatten;

    const READINESS: &str = include_str!("../fixtures/json-render/valid-release-readiness.json");
    const BROKEN: &str = include_str!("../fixtures/json-render/broken-unknown-type.json");

    fn spec(body: &str) -> Spec {
        parse(body).expect("valid fixture").spec
    }

    /// `-want` / `+got` for the lines that differ.
    fn diff(want: &str, got: &str) -> String {
        let (w, g): (Vec<_>, Vec<_>) = (want.lines().collect(), got.lines().collect());
        let mut out = String::new();
        for i in 0..w.len().max(g.len()) {
            match (w.get(i), g.get(i)) {
                (Some(a), Some(b)) if a == b => out.push_str(&format!("  {a}\n")),
                (a, b) => {
                    if let Some(a) = a {
                        out.push_str(&format!("- {a}\n"));
                    }
                    if let Some(b) = b {
                        out.push_str(&format!("+ {b}\n"));
                    }
                }
            }
        }
        out
    }

    /// Every valid fixture at 60 and 100 columns against `tests/golden/<name>-w<width>.txt`.
    /// `UPDATE_GOLDEN=1` rewrites the files instead.
    #[test]
    fn valid_fixtures_match_golden_files() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let update = std::env::var_os("UPDATE_GOLDEN").is_some();
        let mut failures = Vec::new();
        let mut fixtures: Vec<_> = fs::read_dir(root.join("fixtures/json-render"))
            .expect("fixtures dir")
            .map(|e| e.expect("dir entry").path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("valid-"))
            })
            .collect();
        fixtures.sort();
        assert!(!fixtures.is_empty());
        for path in fixtures {
            let name = path
                .file_stem()
                .and_then(|n| n.to_str())
                .expect("utf-8 name");
            let spec = spec(&fs::read_to_string(&path).expect("fixture"));
            for width in [60, 100] {
                let lines = spec_lines(&spec, width)
                    .unwrap_or_else(|| panic!("{name} does not fit {width} columns"));
                let got = format!("{}\n", plain_text(&lines));
                let golden = root.join(format!("tests/golden/{name}-w{width}.txt"));
                if update {
                    fs::write(&golden, &got).expect("write golden");
                    continue;
                }
                let want = fs::read_to_string(&golden)
                    .unwrap_or_else(|e| panic!("{}: {e}", golden.display()));
                if want != got {
                    failures.push(format!("{name} at {width}:\n{}", diff(&want, &got)));
                }
            }
        }
        assert!(failures.is_empty(), "\n{}", failures.join("\n"));
    }

    #[test]
    fn width_gates_the_render() {
        let s = spec(READINESS);
        assert!(spec_lines(&s, 100).is_some());
        assert!(spec_lines(&s, 40).is_some(), "28 columns of metrics fit 40");
        assert_eq!(spec_lines(&s, 20), None, "the title alone needs 23");
        let wide = format!(
            r#"{{"root":"g","elements":{{"g":{{"type":"Grid","props":{{"columns":2}},"children":["a","b"]}},
            "a":{{"type":"Table","props":{{"columns":["k"],"rows":[["{}"]]}}}},
            "b":{{"type":"Badge","props":{{"text":"x"}}}}}}}}"#,
            "x".repeat(200)
        );
        let g = spec(&wide);
        assert_eq!(spec_lines(&g, 80), None);
        assert!(spec_lines(&g, 0).is_some());
    }

    #[test]
    fn fence_hook_omits_failures_quietly_and_ignores_other_langs() {
        assert_eq!(fence("rust", "let x = 1;", 80), None);
        let lines = fence("json-render", BROKEN, 80).expect("failed block is shown");
        assert_eq!(lines.len(), 1);
        assert_eq!(plain_text(&lines), "◌ diagram omitted");
        assert!(lines[0][0].style.dim);
        let ok = fence("json-render", READINESS, 100).expect("renders");
        assert!(plain_text(&ok).starts_with("╭─ Release readiness "));
        let err = fence("mermaid", "flowchart LR\n  A[f(x)] --> B\n", 80).expect("shown");
        assert_eq!(plain_text(&err), "◌ diagram omitted");
        assert_eq!(fence("mermaid", "stateDiagram-v2\n  [*] --> A\n", 80), None);
    }

    #[test]
    fn flatten_replaces_only_renderable_json_render_fences() {
        const OPEN: &str =
            "```json-render\n{\"root\":\"s\",\"elements\":{\"s\":{\"type\":\"Separator\"}}}\n";
        let text = format!(
            "Takeaway.\n\n```json-render\n{READINESS}```\n\nBroken:\n\n```json-render\n{BROKEN}```\n\n```mermaid\nflowchart LR\n  A --> B\n```\n\n{OPEN}"
        );
        let rendered = plain_text(&spec_lines(&spec(READINESS), 60).expect("fits"));
        let want = format!(
            "Takeaway.\n\n```\n{rendered}\n```\n\nBroken:\n\n```json-render\n{BROKEN}```\n\n```mermaid\nflowchart LR\n  A --> B\n```\n\n{OPEN}"
        );
        assert_eq!(flatten(&text, 60), want);
        assert_eq!(flatten("no fences\n", 60), "no fences\n");
    }

    #[test]
    fn wrap_breaks_on_words_then_characters() {
        assert_eq!(wrap("a bb ccc", 5), ["a bb", "ccc"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap("x\n\ny", 10), ["x", "", "y"]);
        assert_eq!(wrap("a b", 0), ["a b"]);
    }
}
