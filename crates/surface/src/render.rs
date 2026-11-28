//! `Spec` → terminal lines (design doc 30 §7) and the `markdown` fence hook.
//!
//! Styles: box drawing, rules and prefixes `dim`; `Heading`, `Metric` values, `Alert`
//! titles and `Tabs`/`Accordion` labels `bold`; `Text{variant: code}` `code`;
//! `Text{variant: muted|caption}` dim; everything else `Style::default()`.

use std::time::Instant;

use markdown::{Line, Span, Style};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::spec::{
    AlertType, BadgeVariant, CalloutVariant, Component, Direction, FishboneProps, Gap, GraphState,
    HeatmapPalette, JourneyProps, KanbanCard, KanbanProps, KanbanState, Layer, LayersDirection,
    LayersProps, LoopProps, MetricProps, SankeyProps, TextVariant, TimelineState, TreemapCell,
    TreemapProps, Trend, VennProps,
};
use crate::{Spec, elk_family, graph, mermaid_lines, parse, sequence};

/// 123 W2: the chart family's lowering (`render/charts.rs`).
pub(crate) mod charts;

/// Render a spec to lines at `width` columns (0 = unbounded); `None` if any line would be
/// wider than `width`, so the caller falls back to the source block.
pub fn spec_lines(spec: &Spec, width: usize) -> Option<Vec<Line>> {
    lines(spec, width, false)
}

/// [`spec_lines`] for text channels (65 §3.3): a `Graph` is always its adjacency list, never
/// the ASCII drawing (box drawing survives, a 40-node picture does not fit a phone).
pub(crate) fn spec_lines_text(spec: &Spec, width: usize) -> Option<Vec<Line>> {
    lines(spec, width, true)
}

fn lines(spec: &Spec, width: usize, text_only: bool) -> Option<Vec<Line>> {
    let lines = element(spec, &spec.root, width, text_only);
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

pub(crate) const PLAIN: Style = Style {
    bold: false,
    italic: false,
    strike: false,
    code: false,
    dim: false,
    heading: 0,
    link: None,
};
pub(crate) const DIM: Style = Style { dim: true, ..PLAIN };
pub(crate) const BOLD: Style = Style {
    bold: true,
    ..PLAIN
};
const CODE: Style = Style {
    code: true,
    ..PLAIN
};

pub(crate) fn span(text: impl Into<String>, style: Style) -> Span {
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
            Trend::Flat => " ▸",
        };
        value.push(span(glyph, PLAIN));
    }
    if let Some(d) = &p.delta {
        value.push(span(format!(" {d}"), DIM));
    }
    if let Some(s) = &p.spark {
        value.push(span(format!(" {}", sparkline(s)), DIM));
    }
    vec![vec![span(&p.label, DIM)], value]
}

/// Values as one block glyph each, `▁` for the smallest to `█` for the largest.
fn sparkline(values: &[f64]) -> String {
    const GLYPHS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    values
        .iter()
        .map(|v| {
            let f = if hi > lo { (v - lo) / (hi - lo) } else { 0.5 };
            GLYPHS[((f * 7.0).round() as usize).min(7)]
        })
        .collect()
}

/// `[label]` per item with the `Badge` mark, space-separated on one line.
fn tag(text: &str, variant: Option<BadgeVariant>) -> String {
    let mark = match variant {
        Some(BadgeVariant::Success) => "✓ ",
        Some(BadgeVariant::Destructive) => "✗ ",
        Some(BadgeVariant::Warning) => "! ",
        _ => "",
    };
    format!("[{mark}{text}]")
}

/// Bold `title` (or nothing) over the wrapped `text`, every line prefixed with `prefix`.
fn titled(prefix: &str, title: Option<&str>, text: &str, width: usize) -> Vec<Line> {
    let mut out = Vec::new();
    if let Some(t) = title {
        out.push(vec![span(prefix, DIM), span(t, BOLD)]);
    }
    out.extend(
        wrap(text, sub(width, prefix.width()))
            .into_iter()
            .map(|l| vec![span(prefix, DIM), span(l, PLAIN)]),
    );
    out
}

/// Bar length when `width` is unbounded.
const BAR_UNBOUNDED: usize = 20;

/// One element and its subtree at `width`; `text_only` picks the text form of a `Graph`.
fn element(spec: &Spec, id: &str, width: usize, text_only: bool) -> Vec<Line> {
    let Some(el) = spec.elements.get(id) else {
        return Vec::new();
    };
    let kids = |w: usize| -> Vec<Vec<Line>> {
        el.children
            .iter()
            .map(|c| element(spec, c, w, text_only))
            .collect()
    };
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
        Component::Badge(p) => vec![vec![span(tag(&p.text, p.variant), PLAIN)]],
        Component::Alert(p) => {
            let glyph = match p.kind.unwrap_or(AlertType::Info) {
                AlertType::Info => "i ",
                AlertType::Success => "✓ ",
                AlertType::Warning => "! ",
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
            for i in charts::timeline_order(p) {
                let glyph = match i.state {
                    Some(TimelineState::Active) => "◉",
                    Some(TimelineState::Pending) => "○",
                    _ => "●",
                };
                let when = charts::when(i);
                out.push(vec![
                    span(format!("{glyph} {when}  "), PLAIN),
                    span(&i.title, PLAIN),
                ]);
                if let Some(d) = &i.detail {
                    let indent = 2 + when.width() + 2;
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
        // 65 §3.3: merman's ASCII drawing when it parses and fits; the adjacency list otherwise.
        Component::Graph(p) => {
            if !text_only && let Ok(Some(lines)) = mermaid_lines(&graph::to_mermaid(p), width) {
                return lines;
            }
            graph::adjacency_lines(p, width)
        }
        // 71 §2.4: the numbered step list on every text surface; the steps keep their `ref`s.
        Component::Sequence(p) => sequence::steps_lines(p, width),
        Component::Steps(p) => {
            let mut out = Vec::new();
            for (n, s) in p.items.iter().enumerate() {
                let head = format!("{}. ", n + 1);
                out.push(vec![span(&head, DIM), span(&s.title, BOLD)]);
                if let Some(d) = &s.detail {
                    let pad = " ".repeat(head.width());
                    out.extend(
                        wrap(d, sub(width, head.width()))
                            .into_iter()
                            .map(|l| vec![span(format!("{pad}{l}"), PLAIN)]),
                    );
                }
            }
            out
        }
        Component::Callout(p) => {
            let glyph = match p.variant.unwrap_or(CalloutVariant::Note) {
                CalloutVariant::Note => "▍ ",
                CalloutVariant::Tip => "▍◇ ",
                CalloutVariant::Question => "▍? ",
                CalloutVariant::Important => "▍! ",
            };
            let mut out = titled("▍ ", p.title.as_deref(), &p.text, width);
            if let Some(first) = out.first_mut() {
                first[0] = span(glyph, DIM);
            }
            out
        }
        Component::Code(p) => {
            let mut out = Vec::new();
            let head = [p.title.as_deref(), p.lang.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            if !head.is_empty() {
                out.push(vec![span(head, DIM)]);
            }
            out.extend(p.text.lines().map(|l| vec![span(l, CODE)]));
            out
        }
        Component::Diff(p) => {
            let side = |title: &str, text: &str| -> Vec<Line> {
                let mut b = vec![vec![span(title, BOLD)]];
                b.extend(text.lines().map(|l| vec![span(l, CODE)]));
                b
            };
            let blocks = [
                side(p.from.as_deref().unwrap_or("Before"), &p.before),
                side(p.to.as_deref().unwrap_or("After"), &p.after),
            ];
            let widths: Vec<usize> = blocks.iter().map(|b| block_width(b)).collect();
            if width == 0 || widths.iter().sum::<usize>() + 2 <= width {
                beside(&blocks, &widths)
            } else {
                let [a, b] = blocks;
                stack(vec![a, b], true)
            }
        }
        // 71 §2.5: `path:start-end · symbol · lang` over a numbered gutter, `▸` on marked rows,
        // the caption under.
        Component::Excerpt(p) => {
            let start = p.start.unwrap_or(1);
            let lines: Vec<&str> = p.text.lines().collect();
            let end = start + u32::try_from(lines.len().saturating_sub(1)).unwrap_or(u32::MAX);
            let marked = |n: u32| {
                p.mark
                    .iter()
                    .flatten()
                    .any(|[from, to]| (*from..=*to).contains(&n))
            };
            let head = [
                Some(format!("{}:{start}-{end}", p.path)),
                p.symbol.clone(),
                p.lang.clone(),
            ]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(" · ");
            let mut out = vec![vec![span(head, DIM)]];
            let digits = end.to_string().len();
            for (i, l) in lines.iter().enumerate() {
                let n = start + u32::try_from(i).unwrap_or(u32::MAX);
                out.push(vec![
                    span(
                        format!("{}{n:>digits$} │ ", if marked(n) { "▸ " } else { "  " }),
                        DIM,
                    ),
                    span(*l, CODE),
                ]);
            }
            if let Some(c) = &p.caption {
                out.extend(text_lines(c, width, DIM));
            }
            out
        }
        Component::Tree(p) => p
            .items
            .iter()
            .map(|i| {
                let depth = usize::from(i.depth.unwrap_or(0));
                let mut line = vec![
                    span(format!("{}├ ", "│ ".repeat(depth)), DIM),
                    span(&i.label, PLAIN),
                ];
                if let Some(d) = &i.detail {
                    line.push(span(format!("  {d}"), DIM));
                }
                line
            })
            .collect(),
        Component::Glossary(p) => {
            let mut out = Vec::new();
            for i in &p.items {
                out.push(vec![span(&i.term, BOLD)]);
                out.extend(
                    wrap(&i.text, sub(width, 2))
                        .into_iter()
                        .map(|l| vec![span(format!("  {l}"), PLAIN)]),
                );
            }
            out
        }
        Component::Quote(p) => {
            let mut out: Vec<Line> = wrap(&p.text, sub(width, 2))
                .into_iter()
                .map(|l| vec![span("┃ ", DIM), span(l, PLAIN)])
                .collect();
            if let Some(s) = &p.source {
                out.push(vec![span(format!("  — {s}"), DIM)]);
            }
            out
        }
        Component::Image(p) => {
            let mut out = vec![vec![span("▣ ", PLAIN), span(&p.alt, PLAIN)]];
            if let Some(c) = &p.caption {
                out.extend(text_lines(c, width, DIM));
            }
            out
        }
        Component::LinkList(p) => {
            let mut out = Vec::new();
            for l in &p.items {
                out.push(vec![
                    span("• ", DIM),
                    span(&l.title, BOLD),
                    span(format!("  {}", l.url), DIM),
                ]);
                if let Some(n) = &l.note {
                    out.extend(indent(text_lines(n, sub(width, 2), PLAIN)));
                }
            }
            out
        }
        Component::Tags(p) => {
            let tags: Vec<String> = p.items.iter().map(|t| tag(&t.text, t.variant)).collect();
            wrap(&tags.join(" "), width)
                .into_iter()
                .map(|l| vec![span(l, PLAIN)])
                .collect()
        }
        Component::Gauge(p) => {
            let min = p.min.unwrap_or(0.0);
            let max = p.max.unwrap_or(100.0);
            let f = frac(p.value - min, max - min);
            let nums = format!("{}  ({}–{})", p.value, min, max);
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
            line.extend(bar(f, cells));
            line.push(span(format!("  {nums}"), PLAIN));
            vec![line]
        }
        Component::Sparkline(p) => {
            let mut line = Vec::new();
            if let Some(l) = &p.label {
                line.push(span(format!("{l}  "), DIM));
            }
            line.push(span(sparkline(&p.values), PLAIN));
            vec![line]
        }
        // 123 §4.1 (W3): `palette: diverging` shades by |v| about 0 and prints signed values;
        // `title` above the grid.
        Component::Heatmap(p) => {
            const SHADES: [&str; 5] = ["·", "░", "▒", "▓", "█"];
            let diverging = p.palette == Some(HeatmapPalette::Diverging);
            let max = p.max.unwrap_or_else(|| {
                p.cells
                    .iter()
                    .flatten()
                    .map(|v| if diverging { v.abs() } else { *v })
                    .fold(0.0, f64::max)
            });
            let cell = |c: &String, style: Style| -> Line { vec![span(c, style)] };
            let head = std::iter::once(String::new())
                .chain(p.cols.iter().cloned())
                .collect::<Vec<_>>();
            let rows = std::iter::once(head.iter().map(|c| cell(c, BOLD)).collect())
                .chain(p.rows.iter().zip(&p.cells).map(|(r, cs)| {
                    std::iter::once(vec![span(r, BOLD)])
                        .chain(cs.iter().map(|v| {
                            let i = ((frac(v.abs(), max) * 4.0).round() as usize).min(4);
                            let text = if diverging && *v > 0.0 {
                                format!("{} +{}", SHADES[i], num(*v))
                            } else {
                                format!("{} {}", SHADES[i], num(*v))
                            };
                            vec![span(text, if diverging && *v < 0.0 { DIM } else { PLAIN })]
                        }))
                        .collect()
                }))
                .collect();
            let mut out = Vec::new();
            if let Some(t) = p.title.as_deref().filter(|t| !t.is_empty()) {
                out.push(vec![span(t, BOLD)]);
            }
            out.extend(markdown::layout_table(rows, &[]));
            out
        }
        Component::Person(p) => {
            let initials: String = p
                .name
                .split_whitespace()
                .take(2)
                .filter_map(|w| w.chars().next())
                .flat_map(char::to_uppercase)
                .collect();
            let mut line = vec![span(format!("({initials}) "), DIM), span(&p.name, BOLD)];
            for extra in [&p.role, &p.contact].into_iter().flatten() {
                line.push(span(format!(" · {extra}"), DIM));
            }
            vec![line]
        }
        Component::FileList(p) => {
            let pw = p.items.iter().map(|i| i.path.width()).max().unwrap_or(0);
            p.items
                .iter()
                .map(|i| {
                    let folder = i.kind.as_deref() == Some("dir") || i.path.ends_with('/');
                    let mut line = vec![
                        span(if folder { "▸ " } else { "· " }, DIM),
                        span(
                            format!("{}{}", i.path, " ".repeat(pw - i.path.width())),
                            CODE,
                        ),
                    ];
                    for extra in [&i.kind, &i.size].into_iter().flatten() {
                        line.push(span(format!("  {extra}"), DIM));
                    }
                    line
                })
                .collect()
        }
        Component::Kanban(p) => kanban_lines(p, width),
        Component::Calendar(p) => {
            let mut out = Vec::new();
            if let Some(t) = &p.title {
                out.push(vec![span(t, BOLD)]);
            }
            // One day per line when the strip would not fit: each day is a two-span cell.
            let cells: Vec<Line> = p
                .days
                .iter()
                .map(|d| {
                    let glyph = match d.state {
                        Some(TimelineState::Done) => "●",
                        Some(TimelineState::Active) => "◉",
                        Some(TimelineState::Pending) => "○",
                        None => "·",
                    };
                    let mut cell = vec![span(format!("{glyph} {}", d.day), PLAIN)];
                    if let Some(l) = &d.label {
                        cell.push(span(format!(" {l}"), DIM));
                    }
                    cell
                })
                .collect();
            let total: usize =
                cells.iter().map(line_width).sum::<usize>() + 2 * cells.len().saturating_sub(1);
            if width == 0 || total <= width {
                let mut line = Line::new();
                for cell in cells {
                    if !line.is_empty() {
                        line.push(span("  ", PLAIN));
                    }
                    line.extend(cell);
                }
                out.push(line);
            } else {
                out.extend(cells);
            }
            out
        }
        // ---- 123 ELK family (W1) ----
        Component::StateMachine(p) => elk_family::state_machine_lines(p, width),
        Component::Entities(p) => elk_family::entities_lines(p, width),
        Component::Swimlane(p) => elk_family::swimlane_lines(p, width),
        Component::GraphDelta(p) => elk_family::graph_delta_lines(p, width),
        // ---- 123 chart family (W2) ----
        Component::BarChart(p) => charts::bar_chart(p, width),
        Component::LineChart(p) => charts::line_chart(p, width),
        Component::ScatterChart(p) => charts::scatter_chart(p),
        Component::RadarChart(p) => charts::radar_chart(p),
        Component::Waterfall(p) => charts::waterfall(p),
        Component::Funnel(p) => charts::funnel(p, width),
        Component::Gantt(p) => charts::gantt(p, width),
        Component::Quadrant(p) => charts::quadrant(p, width),
        // ---- 123 part-of-whole + misc (W3) ----
        Component::Treemap(p) => treemap_lines(p, width),
        Component::Sankey(p) => sankey_lines(p, width),
        Component::Venn(p) => venn_lines(p, width),
        Component::Loop(p) => loop_lines(p, width),
        Component::Layers(p) => layers_lines(p, width),
        Component::Journey(p) => journey_lines(p, width),
        Component::Fishbone(p) => fishbone_lines(p, width),
        // ---- 229 Deck-scoped interactive controls (inert static fallback in TUI) ----
        Component::ActionButton(p) => vec![vec![span(format!("[Action: {}]", p.label), DIM)]],
        Component::StreamControls(p) => vec![vec![span(
            format!(
                "[Stream controls: {}]",
                p.title.as_deref().unwrap_or("Deck")
            ),
            DIM,
        )]],
        Component::ResultBanner(p) => vec![vec![span(
            p.empty_text
                .as_deref()
                .unwrap_or("Action results appear in the Deck view"),
            DIM,
        )]],
    }
}

// ---- 123 part-of-whole + misc (W3): the §4.4 lowerings ----

/// `6 items` / `1 item` — the TS `countOf` twin (`lib/diagram/a11y.ts`).
pub(crate) fn count_of(n: usize, unit: &str) -> String {
    if n == 1 {
        let one = unit
            .strip_suffix("ies")
            .map(|s| format!("{s}y"))
            .or_else(|| unit.strip_suffix('s').map(str::to_owned))
            .unwrap_or_else(|| unit.to_owned());
        format!("{n} {one}")
    } else {
        format!("{n} {unit}")
    }
}

/// The first line of every W3 lowering — `Treemap: Lines by crate — 6 items` — which is also the
/// svg `<desc>` on the desktop (`describe()` in `lib/diagram/a11y.ts`), so the a11y text and the
/// terminal agree.
pub(crate) fn header(name: &str, title: Option<&str>, n: usize, unit: &str) -> Line {
    let count = count_of(n, unit);
    match title.filter(|t| !t.trim().is_empty()) {
        Some(t) => vec![
            span(format!("{name}: "), DIM),
            span(t, BOLD),
            span(format!(" — {count}"), DIM),
        ],
        None => vec![span(format!("{name}: {count}"), DIM)],
    }
}

/// `12.5`, `3`, `0.33` — integers without a point, else at most two decimals.
pub(crate) fn num(v: f64) -> String {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        format!("{}", v as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

/// `text` cut to `max` columns with `…`; unchanged when it fits or `max` is 0.
fn fit(text: &str, max: usize) -> String {
    if max == 0 || text.width() <= max {
        return text.to_owned();
    }
    let mut out = String::new();
    let mut w = 0;
    for c in text.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw + 1 > max {
            break;
        }
        out.push(c);
        w += cw;
    }
    out.push('…');
    out
}

/// Glyph for a unit state; none for `default` or unset (the Graph vocabulary, `graph.rs`).
fn state_glyph(state: Option<GraphState>) -> Option<&'static str> {
    match state? {
        GraphState::Active => Some("◉"),
        GraphState::Done => Some("●"),
        GraphState::Muted => Some("○"),
        GraphState::Danger => Some("✗"),
        GraphState::Default => None,
    }
}

/// Treemap: `label  value  share%` descending inside each group (groups by total, descending),
/// group headers bold with their total and share. Labels shrink to the width.
fn treemap_lines(p: &TreemapProps, width: usize) -> Vec<Line> {
    let total: f64 = p.items.iter().map(|c| c.value).sum();
    let share = |v: f64| {
        if total > 0.0 {
            format!("{}%", (v / total * 100.0).round() as i64)
        } else {
            "–".to_owned()
        }
    };
    // Groups in first-appearance order, then sorted by total; ungrouped cells form a trailing group.
    let mut groups: Vec<(Option<&str>, Vec<&TreemapCell>)> = Vec::new();
    for c in &p.items {
        let key = c.group.as_deref();
        match groups.iter_mut().find(|(k, _)| *k == key) {
            Some((_, cells)) => cells.push(c),
            None => groups.push((key, vec![c])),
        }
    }
    let sum = |cells: &[&TreemapCell]| cells.iter().map(|c| c.value).sum::<f64>();
    groups.sort_by(|a, b| {
        b.0.is_some()
            .cmp(&a.0.is_some())
            .then(sum(&b.1).total_cmp(&sum(&a.1)))
    });
    let headed = groups.iter().any(|(k, _)| k.is_some());
    let indent = if headed { 2 } else { 0 };
    let mut out = vec![header(
        "Treemap",
        p.title.as_deref(),
        p.items.len(),
        "items",
    )];
    for (key, mut cells) in groups {
        cells.sort_by(|a, b| b.value.total_cmp(&a.value));
        if headed {
            let t = sum(&cells);
            out.push(vec![
                span(key.unwrap_or("(ungrouped)"), BOLD),
                span(format!("  {} · {}", num(t), share(t)), DIM),
            ]);
        }
        let values: Vec<String> = cells.iter().map(|c| num(c.value)).collect();
        let shares: Vec<String> = cells.iter().map(|c| share(c.value)).collect();
        let vw = values.iter().map(|v| v.width()).max().unwrap_or(0);
        let sw = shares.iter().map(|s| s.width()).max().unwrap_or(0);
        let label_max = sub(width, indent + 2 + vw + 2 + sw + 2);
        let lw = cells
            .iter()
            .map(|c| fit(&c.label, label_max).width())
            .max()
            .unwrap_or(0);
        for ((c, v), s) in cells.iter().zip(values).zip(shares) {
            let label = fit(&c.label, label_max);
            let mut line = Vec::with_capacity(4);
            if indent > 0 {
                line.push(span("  ", PLAIN));
            }
            line.push(span(
                format!("{label}{}", " ".repeat(lw - label.width())),
                PLAIN,
            ));
            line.push(span(format!("  {}{v}", " ".repeat(vw - v.width())), PLAIN));
            line.push(span(format!("  {}{s}", " ".repeat(sw - s.width())), DIM));
            if let Some(g) = state_glyph(c.state) {
                line.push(span(format!("  {g}"), DIM));
            }
            out.push(line);
        }
    }
    out
}

/// Column of every Sankey node: the authored `stage`, else the longest path from a source
/// (d3-sankey's `sankeyJustify` depth for sources; sinks keep their depth, which is what the
/// text order needs). Cycles cannot reach here (`validate.rs`), but the walk is bounded anyway.
fn sankey_stages(p: &SankeyProps) -> Vec<usize> {
    let index = |id: &str| p.nodes.iter().position(|n| n.id == id);
    let mut depth = vec![0usize; p.nodes.len()];
    for _ in 0..p.nodes.len() {
        let mut changed = false;
        for f in &p.flows {
            if let (Some(a), Some(b)) = (index(&f.from), index(&f.to))
                && depth[b] < depth[a] + 1
            {
                depth[b] = depth[a] + 1;
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    p.nodes
        .iter()
        .zip(depth)
        .map(|(n, d)| n.stage.unwrap_or(d))
        .collect()
}

/// Sankey: nodes in stage order as bold headers with their total (`max(in, out)`, d3-sankey's
/// `value`), each followed by its outgoing `→ to  value` lines; a blank line between stages.
fn sankey_lines(p: &SankeyProps, width: usize) -> Vec<Line> {
    let unit = p
        .unit
        .as_deref()
        .map(|u| format!(" {u}"))
        .unwrap_or_default();
    let label = |id: &str| -> String {
        p.nodes
            .iter()
            .find(|n| n.id == id)
            .map(|n| n.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    let stages = sankey_stages(p);
    let mut order: Vec<usize> = (0..p.nodes.len()).collect();
    order.sort_by_key(|&i| stages[i]);
    let mut out = vec![header("Sankey", p.title.as_deref(), p.flows.len(), "flows")];
    let mut last_stage = None;
    for i in order {
        let n = &p.nodes[i];
        if last_stage.is_some_and(|s| s != stages[i]) {
            out.push(Line::new());
        }
        last_stage = Some(stages[i]);
        let inflow: f64 = p
            .flows
            .iter()
            .filter(|f| f.to == n.id)
            .map(|f| f.value)
            .sum();
        let outflow: f64 = p
            .flows
            .iter()
            .filter(|f| f.from == n.id)
            .map(|f| f.value)
            .sum();
        let mut head = vec![
            span(&n.label, BOLD),
            span(format!("  {}{unit}", num(inflow.max(outflow))), DIM),
        ];
        if let Some(g) = state_glyph(n.state) {
            head.push(span(format!("  {g}"), DIM));
        }
        out.push(head);
        for f in p.flows.iter().filter(|f| f.from == n.id) {
            let mut line = vec![
                span("  → ", DIM),
                span(label(&f.to), PLAIN),
                span(format!("  {}{unit}", num(f.value)), PLAIN),
            ];
            if let Some(l) = f.label.as_deref().filter(|l| !l.is_empty()) {
                let extra = format!("  ({l})");
                if width == 0 || line_width(&line) + extra.width() <= width {
                    line.push(span(extra, DIM));
                }
            }
            out.push(line);
        }
    }
    out
}

/// Venn: one `● label (size)` line per set, then `A ∩ B: label` per overlap.
fn venn_lines(p: &VennProps, width: usize) -> Vec<Line> {
    let label = |id: &str| -> String {
        p.sets
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    let mut out = vec![header("Venn", p.title.as_deref(), p.sets.len(), "sets")];
    for s in &p.sets {
        let mut line = vec![span("● ", DIM), span(&s.label, PLAIN)];
        if let Some(n) = s.size {
            line.push(span(format!(" ({})", num(n)), DIM));
        }
        out.push(line);
    }
    for o in p.overlaps.as_deref().unwrap_or_default() {
        let names: Vec<String> = o.sets.iter().map(|id| label(id)).collect();
        let head = format!("{}: ", names.join(" ∩ "));
        let mut lines = wrap(&o.label, sub(width, head.width())).into_iter();
        out.push(vec![
            span(&head, DIM),
            span(lines.next().unwrap_or_default(), PLAIN),
        ]);
        let pad = " ".repeat(head.width());
        out.extend(lines.map(|l| vec![span(format!("{pad}{l}"), PLAIN)]));
    }
    out
}

/// Loop: `1. step → 2. step → … ↺ 1.` packed to the width (continuations indented), the hub
/// line, then the notes as dim `n. · note` lines.
fn loop_lines(p: &LoopProps, width: usize) -> Vec<Line> {
    let mut out = vec![header("Loop", p.title.as_deref(), p.steps.len(), "steps")];
    let mut parts: Vec<String> = p
        .steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let g = state_glyph(s.state)
                .map(|g| format!("{g} "))
                .unwrap_or_default();
            format!("{}. {g}{}", i + 1, s.label)
        })
        .collect();
    if !parts.is_empty() {
        parts.push("↺ 1.".to_owned());
    }
    let mut line = String::new();
    for (i, part) in parts.iter().enumerate() {
        let piece = if i == 0 {
            part.clone()
        } else {
            format!(" → {part}")
        };
        if !line.is_empty() && width > 0 && line.width() + piece.width() > width {
            out.push(vec![span(std::mem::take(&mut line), PLAIN)]);
            line = format!("   → {part}");
        } else {
            line.push_str(&piece);
        }
    }
    if !line.is_empty() {
        out.push(vec![span(line, PLAIN)]);
    }
    if let Some(h) = p.hub.as_deref().filter(|h| !h.is_empty()) {
        out.push(vec![span("⟳ ", DIM), span(h, BOLD)]);
    }
    for (i, s) in p.steps.iter().enumerate() {
        if let Some(n) = s.note.as_deref().filter(|n| !n.is_empty()) {
            out.push(vec![span(format!("{}. · {n}", i + 1), DIM)]);
        }
    }
    out
}

/// Layers: one framed row per layer (`bottom-up` reverses the order), the label bold, items
/// comma-joined and wrapped beside it, the note dim at the end.
fn layers_lines(p: &LayersProps, width: usize) -> Vec<Line> {
    let mut out = vec![header(
        "Layers",
        p.title.as_deref(),
        p.layers.len(),
        "layers",
    )];
    let mut layers: Vec<&Layer> = p.layers.iter().collect();
    if p.direction == Some(LayersDirection::BottomUp) {
        layers.reverse();
    }
    if layers.is_empty() {
        return out;
    }
    let lw = layers.iter().map(|l| l.label.width()).max().unwrap_or(0);
    // `│ ` + label + `  ` + body + ` │`
    let body_w = sub(width, lw + 6);
    let rows: Vec<Vec<Line>> = layers
        .iter()
        .map(|l| {
            let note = l
                .note
                .as_deref()
                .filter(|n| !n.is_empty())
                .map(|n| format!("· {n}"));
            let mut body = l.items.as_deref().unwrap_or_default().join(", ");
            if let Some(n) = &note {
                if !body.is_empty() {
                    body.push_str("  ");
                }
                body.push_str(n);
            }
            let mut lines: Vec<Line> = wrap(&body, body_w)
                .into_iter()
                .enumerate()
                .map(|(i, text)| {
                    let label = if i == 0 {
                        format!("{}{}", l.label, " ".repeat(lw - l.label.width()))
                    } else {
                        " ".repeat(lw)
                    };
                    let mut line = vec![span(label, BOLD), span("  ", PLAIN)];
                    match note.as_deref().and_then(|n| text.find(n)) {
                        Some(at) => {
                            line.push(span(text[..at].to_owned(), PLAIN));
                            line.push(span(text[at..].to_owned(), DIM));
                        }
                        None => line.push(span(text, PLAIN)),
                    }
                    line
                })
                .collect();
            if lines.is_empty() {
                lines.push(vec![span(&l.label, BOLD)]);
            }
            if let Some(g) = state_glyph(l.state) {
                lines[0].push(span(format!("  {g}"), DIM));
            }
            lines
        })
        .collect();
    let inner = rows.iter().map(|r| block_width(r)).max().unwrap_or(0);
    out.push(vec![span(format!("┌{}┐", "─".repeat(inner + 2)), DIM)]);
    for (i, row) in rows.into_iter().enumerate() {
        if i > 0 {
            out.push(vec![span(format!("├{}┤", "─".repeat(inner + 2)), DIM)]);
        }
        for mut l in row {
            pad(&mut l, inner);
            l.insert(0, span("│ ", DIM));
            l.push(span(" │", DIM));
            out.push(l);
        }
    }
    out.push(vec![span(format!("└{}┘", "─".repeat(inner + 2)), DIM)]);
    out
}

/// Mood glyph: `☹` below −⅓, `☺` above ⅓, `○` between.
fn mood_glyph(m: f64) -> &'static str {
    if m < -1.0 / 3.0 {
        "☹"
    } else if m > 1.0 / 3.0 {
        "☺"
    } else {
        "○"
    }
}

/// `prefix` (dim) before `text` wrapped to the width, continuations aligned under the text.
fn prefixed(prefix: &str, text: &str, width: usize) -> Vec<Line> {
    wrap(text, sub(width, prefix.width()))
        .into_iter()
        .enumerate()
        .map(|(j, l)| {
            let lead = if j == 0 {
                prefix.to_owned()
            } else {
                " ".repeat(prefix.width())
            };
            vec![span(lead, DIM), span(l, PLAIN)]
        })
        .collect()
}

/// Journey: a stage × row table with a `☹ ○ ☺` mood row when the table fits, else one bold
/// stage per section with `row: cell` lines; pains as `▲ stage: text` footnotes either way.
fn journey_lines(p: &JourneyProps, width: usize) -> Vec<Line> {
    let mut out = vec![header(
        "Journey",
        p.title.as_deref(),
        p.stages.len(),
        "stages",
    )];
    let cell = |s: &str| -> Line { vec![span(s, PLAIN)] };
    let mut rows: Vec<Vec<Line>> = vec![
        std::iter::once(Line::new())
            .chain(p.stages.iter().map(|s| vec![span(s, BOLD)]))
            .collect(),
    ];
    for r in &p.rows {
        rows.push(
            std::iter::once(vec![span(&r.label, BOLD)])
                .chain((0..p.stages.len()).map(|i| cell(r.cells.get(i).map_or("", String::as_str))))
                .collect(),
        );
    }
    if let Some(m) = &p.mood {
        rows.push(
            std::iter::once(vec![span("Mood", BOLD)])
                .chain((0..p.stages.len()).map(|i| cell(m.get(i).map_or("", |v| mood_glyph(*v)))))
                .collect(),
        );
    }
    let table = markdown::layout_table(rows, &[]);
    if width == 0 || block_width(&table) <= width {
        out.extend(table);
    } else {
        for (i, stage) in p.stages.iter().enumerate() {
            let mut head = vec![span(stage, BOLD)];
            if let Some(m) = p.mood.as_ref().and_then(|m| m.get(i)) {
                head.push(span(format!("  {}", mood_glyph(*m)), DIM));
            }
            out.push(head);
            for r in &p.rows {
                if let Some(c) = r.cells.get(i).filter(|c| !c.is_empty()) {
                    out.extend(prefixed(&format!("  {}: ", r.label), c, width));
                }
            }
        }
    }
    for pain in p.pains.as_deref().unwrap_or_default() {
        let stage = p.stages.get(pain.stage).map_or("?", String::as_str);
        out.extend(prefixed(&format!("▲ {stage}: "), &pain.text, width));
    }
    out
}

/// Fishbone: the effect as a `→ effect` header, one bold category per bone, items indented.
fn fishbone_lines(p: &FishboneProps, width: usize) -> Vec<Line> {
    let mut out = vec![header(
        "Fishbone",
        p.title.as_deref(),
        p.causes.len(),
        "causes",
    )];
    out.push(vec![span("→ ", DIM), span(&p.effect, BOLD)]);
    for c in &p.causes {
        out.push(vec![span("├ ", DIM), span(&c.category, BOLD)]);
        for item in &c.items {
            out.extend(
                wrap(item, sub(width, 4))
                    .into_iter()
                    .enumerate()
                    .map(|(j, l)| {
                        vec![
                            span(if j == 0 { "│ · " } else { "│   " }, DIM),
                            span(l, PLAIN),
                        ]
                    }),
            );
        }
    }
    out
}

/// One Kanban card line: `⊘` before a blocked card, `✓` before a done one, the dim `[tag]`,
/// the detail on a dim second line.
fn kanban_card(card: &KanbanCard) -> Vec<Line> {
    let mark = match card.state {
        Some(KanbanState::Blocked) => "⊘ ",
        Some(KanbanState::Done) => "✓ ",
        _ => "• ",
    };
    let mut line = vec![span(mark, DIM), span(&card.title, PLAIN)];
    if let Some(t) = &card.tag {
        line.push(span(format!(" [{t}]"), DIM));
    }
    let mut out = vec![line];
    if let Some(d) = &card.detail {
        out.push(vec![span(format!("  {d}"), DIM)]);
    }
    out
}

/// Kanban (123 §4.1 W3): the title, column headers `title (n)` — `(n/limit ⚠)` over a WIP
/// limit — then the cards side by side when they fit, stacked otherwise. With `rows` (a story
/// map) each row is a sub-header over the columns' cards in that row, a `────` release line
/// follows row `cut`, and cards without a row close the board.
fn kanban_lines(p: &KanbanProps, width: usize) -> Vec<Line> {
    let mut out = Vec::new();
    if let Some(t) = p.title.as_deref().filter(|t| !t.is_empty()) {
        out.push(vec![span(t, BOLD)]);
    }
    let heads: Vec<Line> = p
        .columns
        .iter()
        .map(|c| {
            let n = c.cards.len();
            let mut line = vec![span(&c.title, BOLD)];
            match c.limit {
                Some(l) if n > l => line.push(span(format!(" ({n}/{l} ⚠)"), BOLD)),
                Some(l) => line.push(span(format!(" ({n}/{l})"), DIM)),
                None => line.push(span(format!(" ({n})"), DIM)),
            }
            line
        })
        .collect();
    let Some(rows) = p.rows.as_deref() else {
        let blocks: Vec<Vec<Line>> = p
            .columns
            .iter()
            .zip(heads)
            .map(|(c, head)| {
                std::iter::once(head)
                    .chain(c.cards.iter().flat_map(kanban_card))
                    .collect()
            })
            .collect();
        let widths: Vec<usize> = blocks.iter().map(|b| block_width(b)).collect();
        let total = widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1);
        if width == 0 || total <= width {
            out.extend(beside(&blocks, &widths));
        } else {
            out.extend(stack(blocks, true));
        }
        return out;
    };
    // Row `rows.len()` collects the cards with no row or one past the end.
    let section = |r: usize| -> Vec<Vec<Line>> {
        p.columns
            .iter()
            .map(|c| {
                c.cards
                    .iter()
                    .filter(|card| card.row.map_or(rows.len(), |i| i.min(rows.len())) == r)
                    .flat_map(kanban_card)
                    .collect()
            })
            .collect()
    };
    let sections: Vec<Vec<Vec<Line>>> = (0..=rows.len()).map(section).collect();
    let widths: Vec<usize> = (0..p.columns.len())
        .map(|c| {
            sections
                .iter()
                .map(|s| block_width(&s[c]))
                .chain(std::iter::once(line_width(&heads[c])))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let total = widths.iter().sum::<usize>() + 2 * widths.len().saturating_sub(1);
    let fits = width == 0 || total <= width;
    let head_blocks: Vec<Vec<Line>> = heads.into_iter().map(|h| vec![h]).collect();
    if fits {
        out.extend(beside(&head_blocks, &widths));
    }
    for (r, blocks) in sections.into_iter().enumerate() {
        if blocks.iter().all(Vec::is_empty) {
            continue;
        }
        match rows.get(r) {
            Some(name) => out.push(vec![span("▸ ", DIM), span(name, BOLD)]),
            None => out.push(vec![span("▸ ", DIM), span("(no row)", DIM)]),
        }
        if fits {
            out.extend(beside(&blocks, &widths));
        } else {
            for (c, b) in blocks.into_iter().enumerate() {
                if b.is_empty() {
                    continue;
                }
                out.push(head_blocks[c][0].clone());
                out.extend(b);
            }
        }
        if p.cut == Some(r) {
            let n = if width == 0 { total } else { total.min(width) };
            out.push(vec![span("─".repeat(n.max(4)), DIM)]);
        }
    }
    out
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
    fn flatten_replaces_every_picture_fence() {
        const OPEN: &str =
            "```json-render\n{\"root\":\"s\",\"elements\":{\"s\":{\"type\":\"Separator\"}}}\n";
        let text = format!(
            "Takeaway.\n\n```json-render\n{READINESS}```\n\nBroken:\n\n```json-render\n{BROKEN}```\n\n```mermaid\nflowchart LR\n  A[f(x)] --> B\n```\n\n{OPEN}"
        );
        let rendered = plain_text(&spec_lines(&spec(READINESS), 60).expect("fits"));
        let placeholder = "[diagram — open theuth to see it]";
        let want = format!(
            "Takeaway.\n\n```\n{rendered}\n```\n\nBroken:\n\n{placeholder}\n\n{placeholder}\n\n{placeholder}"
        );
        assert_eq!(flatten(&text, 60), want);
        assert_eq!(flatten("no fences\n", 60), "no fences\n");
    }

    /// 65 §3.3: the TUI gets merman's drawing at 100 columns and the adjacency list at 40; text
    /// channels always get the list.
    #[test]
    fn graph_draws_wide_lists_narrow_and_flattens_to_the_list() {
        const GRAPH: &str = include_str!("../fixtures/json-render/graph-architecture.json");
        let s = spec(GRAPH);
        let wide = spec_lines(&s, 100).expect("fits 100");
        assert!(
            wide.iter()
                .flatten()
                .any(|sp| sp.style.dim && sp.text.contains('─')),
            "box drawing at 100:\n{}",
            plain_text(&wide)
        );
        let narrow = spec_lines(&s, 40).expect("list fits 40");
        assert!(
            plain_text(&narrow).starts_with("Edge\n  • Browser → API (HTTPS), CDN\n"),
            "{}",
            plain_text(&narrow)
        );
        let flat = flatten(&format!("```json-render\n{GRAPH}```\n"), 100);
        assert!(flat.starts_with("```\nEdge\n  • Browser"), "{flat}");
    }

    #[test]
    fn wrap_breaks_on_words_then_characters() {
        assert_eq!(wrap("a bb ccc", 5), ["a bb", "ccc"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap("x\n\ny", 10), ["x", "", "y"]);
        assert_eq!(wrap("a b", 0), ["a b"]);
    }

    // ---- 123 part-of-whole + misc (W3) ----

    fn fixture(name: &str) -> Spec {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/json-render")
            .join(format!("{name}.json"));
        spec(&fs::read_to_string(path).expect("fixture"))
    }

    fn text(name: &str, width: usize) -> String {
        plain_text(&spec_lines(&fixture(name), width).expect("fits"))
    }

    #[test]
    fn header_counts_like_the_desktop_desc() {
        assert_eq!(count_of(1, "items"), "1 item");
        assert_eq!(count_of(1, "entities"), "1 entity");
        assert_eq!(count_of(2, "stages"), "2 stages");
        assert_eq!(
            plain_text(&[header("Treemap", Some("Lines"), 6, "items")]),
            "Treemap: Lines — 6 items"
        );
        assert_eq!(
            plain_text(&[header("Loop", None, 1, "steps")]),
            "Loop: 1 step"
        );
        assert_eq!(num(12.5), "12.5");
        assert_eq!(num(3.0), "3");
        assert_eq!(num(1.0 / 3.0), "0.33");
        assert_eq!(fit("abcdef", 4), "abc…");
        assert_eq!(fit("abc", 4), "abc");
    }

    #[test]
    fn treemap_sorts_groups_and_cells_descending_with_shares() {
        let t = text("valid-treemap", 0);
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "Treemap: Lines by crate — 6 items");
        assert!(lines[1].starts_with("crates  53400 · 64%"), "{t}");
        assert!(
            lines[2].starts_with("  agent") && lines[2].ends_with("44%  ◉"),
            "{t}"
        );
        assert!(lines[3].starts_with("  surface"), "{t}");
        let groups: Vec<&&str> = lines.iter().filter(|l| !l.starts_with(' ')).collect();
        assert_eq!(groups.len(), 4, "header + 3 groups by total:\n{t}");
        // Narrow widths shrink the labels rather than refusing the render.
        assert!(spec_lines(&fixture("valid-treemap"), 36).is_some());
    }

    #[test]
    fn sankey_groups_nodes_by_stage_with_totals() {
        let t = text("valid-sankey", 0);
        assert!(t.starts_with("Sankey: Spend by model and session — 4 flows\nOpus  16.7 USD\n  → Design 123  12.5 USD\n"), "{t}");
        assert!(
            t.contains("\n\nDesign 123  12.5 USD  ◉\n"),
            "stage break then the sinks:\n{t}"
        );
        assert!(t.contains("→ Docs  2.4 USD  (drafts)"), "{t}");
        // Without authored stages the depth decides the order.
        let p = parse(r#"{"root":"s","elements":{"s":{"type":"Sankey","props":{"nodes":[{"id":"c","label":"C"},{"id":"a","label":"A"},{"id":"b","label":"B"}],"flows":[{"from":"a","to":"b","value":1},{"from":"b","to":"c","value":1}]}}}}"#).unwrap().spec;
        let Component::Sankey(s) = &p.elements["s"].component else {
            panic!()
        };
        assert_eq!(sankey_stages(s), [2, 0, 1]);
    }

    #[test]
    fn venn_and_fishbone_and_layers_forms() {
        let v = text("valid-venn", 60);
        assert!(v.contains("● Has tests (41)\n"), "{v}");
        assert!(
            v.ends_with("Has tests ∩ Has docs ∩ Reviewed: Release-ready"),
            "{v}"
        );
        let f = text("valid-fishbone", 60);
        assert!(
            f.contains("\n→ Slow replies\n├ Model\n│ · long context\n"),
            "{f}"
        );
        let l = text("valid-layers", 60);
        assert!(
            l.contains("│ Surfaces       desktop, TUI, Slack, Signal"),
            "{l}"
        );
        assert!(l.contains("· one catalog │"), "note dim at the end:\n{l}");
        let p = parse(r#"{"root":"l","elements":{"l":{"type":"Layers","props":{"direction":"bottom-up","layers":[{"label":"A"},{"label":"B"}]}}}}"#).unwrap().spec;
        let up = plain_text(&spec_lines(&p, 0).unwrap());
        assert!(up.find("│ B").unwrap() < up.find("│ A").unwrap(), "{up}");
    }

    #[test]
    fn loop_packs_the_chain_to_the_width() {
        let wide = text("valid-loop", 100);
        assert!(wide.contains("1. Measure → 2. Decide → 3. ◉ Build → 4. Ship → 5. ○ Observe → ↺ 1.\n⟳ Learning\n1. · 20 prompts"), "{wide}");
        let narrow = text("valid-loop", 30);
        assert!(narrow.lines().all(|l| l.width() <= 30), "{narrow}");
        assert!(
            narrow.contains("\n   → "),
            "continuations indent:\n{narrow}"
        );
    }

    #[test]
    fn journey_is_a_table_when_it_fits_and_sections_otherwise() {
        let wide = text("valid-journey", 100);
        assert!(wide.contains("Mood   │ ○             │ ☺"), "{wide}");
        assert!(
            wide.ends_with("▲ Install: notarisation wait\n▲ First board: board takes 40 s"),
            "{wide}"
        );
        let narrow = text("valid-journey", 60);
        assert!(
            narrow.contains(
                "\nInstall  ○\n  Does: downloads dmg\n  Thinks: is it safe?\nFirst chat  ☺\n"
            ),
            "{narrow}"
        );
        assert_eq!(mood_glyph(-0.5), "☹");
    }

    #[test]
    fn kanban_marks_limits_states_rows_and_the_cut() {
        let t = text("valid-kanban-rows", 100);
        assert!(t.starts_with("Story map\nOnboard (3/2 ⚠)  Chat (3/2 ⚠)    Boards (1)\n▸ Must\n✓ Install        ✓ Ask\n"), "{t}");
        assert!(t.contains("⊘ Draw a chart"), "{t}");
        let lines: Vec<&str> = t.lines().collect();
        let cut = lines
            .iter()
            .position(|l| l.starts_with("────"))
            .expect("release line");
        assert_eq!(lines[cut + 1], "▸ Could", "the cut follows row 1:\n{t}");
        // Too narrow for the columns: one stacked column per row.
        let narrow = text("valid-kanban-rows", 30);
        assert!(
            narrow.contains("▸ Must\nOnboard (3/2 ⚠)\n✓ Install\n✓ Sign in\nChat (3/2 ⚠)\n✓ Ask\n"),
            "{narrow}"
        );
        // The plain board keeps its shape.
        assert!(
            text("valid-kanban", 100)
                .lines()
                .next()
                .unwrap()
                .contains(" (")
        );
    }

    #[test]
    fn heatmap_diverging_signs_values_and_shades_by_magnitude() {
        let t = text("valid-heatmap-diverging", 100);
        assert!(t.starts_with("Latency delta vs last week (ms)\n"), "{t}");
        assert!(
            t.contains("▒ -12") && t.contains("█ +20") && t.contains("· 0 "),
            "{t}"
        );
        let plain = text("valid-heatmap", 100);
        assert!(!plain.contains('+'), "sequential stays unsigned:\n{plain}");
    }
}
