//! 123 W2: the chart family's text lowering (design 123 §4.4) and its markdown form (§4.5).
//! Every TUI form opens with one dim header `Name: title — n units` — the line the desktop
//! reproduces as the svg `<desc>`, so the a11y text and the terminal agree.

use std::fmt::Write as _;

use markdown::Line;
use unicode_width::UnicodeWidthStr;

use super::{BOLD, DIM, PLAIN, bar, span, sparkline};
use crate::spec::*;
use crate::validate::iso_datetime;

/// One glyph run per series (`▓ ▒ ░ …`), cycling past eight.
const SERIES_GLYPHS: [char; 8] = ['▓', '▒', '░', '█', '▚', '▞', '▙', '▟'];
const UNBOUNDED_CELLS: usize = 30;

/// `Name: title — 2 series` / `Name: 6 tasks` (dim).
fn header(name: &str, title: Option<&str>, n: usize, one: &str, many: &str) -> Line {
    vec![span(describe(name, title, n, one, many), DIM)]
}

/// The header text, shared with the markdown form and mirrored by `lib/diagram/chart/text.ts`.
pub(crate) fn describe(name: &str, title: Option<&str>, n: usize, one: &str, many: &str) -> String {
    let count = format!("{n} {}", if n == 1 { one } else { many });
    match title {
        Some(t) if !t.is_empty() => format!("{name}: {t} — {count}"),
        _ => format!("{name}: {count}"),
    }
}

/// Integers plain, otherwise up to two decimals with trailing zeros trimmed.
pub(crate) fn num(v: f64) -> String {
    if (v - v.round()).abs() < 1e-9 {
        format!("{}", v.round() as i64)
    } else {
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_owned()
    }
}

fn with_unit(v: f64, unit: Option<&str>) -> String {
    match unit {
        Some(u) => format!("{} {u}", num(v)),
        None => num(v),
    }
}

/// Columns left for a bar after `fixed` columns of text; 0 width stays unbounded.
fn cells(width: usize, fixed: usize) -> usize {
    if width == 0 {
        UNBOUNDED_CELLS
    } else {
        width.saturating_sub(fixed).max(10)
    }
}

fn padded(s: &str, to: usize) -> String {
    format!("{s}{}", " ".repeat(to.saturating_sub(s.width())))
}

fn cell(s: impl Into<String>, style: markdown::Style) -> Line {
    vec![span(s, style)]
}

// ---------------------------------------------------------------- BarChart

/// Rows per label, one glyph run per series, a legend line.
pub(super) fn bar_chart(p: &BarChartProps, width: usize) -> Vec<Line> {
    let mut out = vec![header(
        "BarChart",
        p.title.as_deref(),
        p.series.len(),
        "series",
        "series",
    )];
    let unit = p.unit.as_deref();
    let lw = p.labels.iter().map(|l| l.width()).max().unwrap_or(0);
    if p.variant == Some(BarVariant::Dumbbell) && p.series.len() == 2 {
        for (i, label) in p.labels.iter().enumerate() {
            let (a, b) = (p.series[0].values[i], p.series[1].values[i]);
            out.push(vec![
                span(format!("{}  ", padded(label, lw)), PLAIN),
                span(format!("{} → {}", num(a), with_unit(b, unit)), PLAIN),
                span(format!("  ({:+})", num_signed(b - a)), DIM),
            ]);
        }
    } else {
        let values: Vec<String> = (0..p.labels.len())
            .map(|i| {
                let vs: Vec<String> = p.series.iter().map(|s| num(s.values[i])).collect();
                match unit {
                    Some(u) => format!("{} {u}", vs.join(" · ")),
                    None => vs.join(" · "),
                }
            })
            .collect();
        let vw = values.iter().map(|v| v.width()).max().unwrap_or(0);
        let scale = (0..p.labels.len())
            .map(|i| p.series.iter().map(|s| s.values[i].abs()).sum::<f64>())
            .fold(0.0, f64::max);
        let n = cells(width, lw + vw + 4);
        for (i, label) in p.labels.iter().enumerate() {
            let mut line = vec![span(format!("{}  ", padded(label, lw)), PLAIN)];
            let mut cum = 0.0;
            let mut drawn = 0;
            for (k, s) in p.series.iter().enumerate() {
                cum += s.values[i].abs();
                let end = if scale > 0.0 {
                    ((cum / scale) * n as f64).round() as usize
                } else {
                    0
                };
                let run = end.saturating_sub(drawn).min(n - drawn);
                if run > 0 {
                    line.push(span(
                        SERIES_GLYPHS[k % SERIES_GLYPHS.len()]
                            .to_string()
                            .repeat(run),
                        PLAIN,
                    ));
                }
                drawn += run;
            }
            line.push(span(" ".repeat(n - drawn), PLAIN));
            line.push(span(format!("  {}", values[i]), PLAIN));
            out.push(line);
        }
    }
    out.push(legend(&p.series));
    out
}

fn num_signed(v: f64) -> String {
    if v < 0.0 {
        format!("-{}", num(-v))
    } else {
        format!("+{}", num(v))
    }
}

/// `▓ Passed  ▒ Failed` (dim).
fn legend(series: &[Series]) -> Line {
    let text: Vec<String> = series
        .iter()
        .enumerate()
        .map(|(k, s)| format!("{} {}", SERIES_GLYPHS[k % SERIES_GLYPHS.len()], s.name))
        .collect();
    vec![span(text.join("  "), DIM)]
}

// --------------------------------------------------------------- LineChart

/// Sparkline per series with first/last values; `slope` as `a  12 → 18  b`; `bump` as rank columns.
pub(super) fn line_chart(p: &LineChartProps, width: usize) -> Vec<Line> {
    let mut out = vec![header(
        "LineChart",
        p.title.as_deref(),
        p.series.len(),
        "series",
        "series",
    )];
    let unit = p.unit.as_deref();
    let nw = p.series.iter().map(|s| s.name.width()).max().unwrap_or(0);
    match p.variant {
        Some(LineVariant::Slope) if p.labels.len() == 2 => {
            let (a, b) = (&p.labels[0], &p.labels[1]);
            for s in &p.series {
                out.push(vec![
                    span(format!("{}  ", padded(&s.name, nw)), PLAIN),
                    span(format!("{a}  "), DIM),
                    span(
                        format!("{} → {}", num(s.values[0]), with_unit(s.values[1], unit)),
                        PLAIN,
                    ),
                    span(format!("  {b}"), DIM),
                ]);
            }
        }
        Some(LineVariant::Bump) => {
            let head = std::iter::once(cell("", BOLD))
                .chain(p.labels.iter().map(|l| cell(l, BOLD)))
                .collect();
            let rows = std::iter::once(head)
                .chain(p.series.iter().map(|s| {
                    std::iter::once(cell(&s.name, PLAIN))
                        .chain(s.values.iter().map(|v| cell(format!("#{}", num(*v)), PLAIN)))
                        .collect()
                }))
                .collect();
            out.extend(markdown::layout_table(rows, &[]));
        }
        _ => {
            let ends: Vec<String> = p
                .series
                .iter()
                .map(|s| match (s.values.first(), s.values.last()) {
                    (Some(a), Some(b)) => format!("{} → {}", num(*a), with_unit(*b, unit)),
                    _ => String::new(),
                })
                .collect();
            let ew = ends.iter().map(|e| e.width()).max().unwrap_or(0);
            let avail = cells(width, nw + ew + 4).max(2);
            let step = p.labels.len().div_ceil(avail).max(1);
            for (s, end) in p.series.iter().zip(&ends) {
                let sampled: Vec<f64> = s.values.iter().copied().step_by(step).collect();
                out.push(vec![
                    span(format!("{}  ", padded(&s.name, nw)), PLAIN),
                    span(sparkline(&sampled), PLAIN),
                    span(format!("  {end}"), PLAIN),
                ]);
            }
            if let (Some(a), Some(b)) = (p.labels.first(), p.labels.last()) {
                let caption = if p.labels.len() > 1 {
                    format!("{a} … {b}")
                } else {
                    a.clone()
                };
                out.push(vec![span(" ".repeat(nw + 2), PLAIN), span(caption, DIM)]);
            }
        }
    }
    out
}

// ------------------------------------------------------------ ScatterChart

/// Table `label  x  y [size] [group]`.
pub(super) fn scatter_chart(p: &ScatterChartProps) -> Vec<Line> {
    let mut out = vec![header(
        "ScatterChart",
        p.title.as_deref(),
        p.points.len(),
        "point",
        "points",
    )];
    let sized = p.points.iter().any(|pt| pt.size.is_some());
    let groups = p.groups.as_deref().filter(|g| !g.is_empty());
    let group_label = |id: &str| -> String {
        groups
            .and_then(|g| g.iter().find(|g| g.id == id))
            .map(|g| g.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    let mut head = vec![cell("label", BOLD), cell(&p.x_label, BOLD), cell(&p.y_label, BOLD)];
    if sized {
        head.push(cell("size", BOLD));
    }
    if groups.is_some() {
        head.push(cell("group", BOLD));
    }
    let rows = std::iter::once(head)
        .chain(p.points.iter().map(|pt| {
            let mut row = vec![
                cell(pt.label.as_deref().unwrap_or("·"), PLAIN),
                cell(num(pt.x), PLAIN),
                cell(num(pt.y), PLAIN),
            ];
            if sized {
                row.push(cell(pt.size.map(num).unwrap_or_default(), PLAIN));
            }
            if groups.is_some() {
                row.push(cell(
                    pt.group.as_deref().map(group_label).unwrap_or_default(),
                    PLAIN,
                ));
            }
            row
        }))
        .collect();
    out.extend(markdown::layout_table(rows, &[]));
    out
}

// -------------------------------------------------------------- RadarChart

/// Table `axis  s1  s2`, `max` in the header.
pub(super) fn radar_chart(p: &RadarChartProps) -> Vec<Line> {
    let mut out = vec![header(
        "RadarChart",
        p.title.as_deref(),
        p.series.len(),
        "series",
        "series",
    )];
    let axis_head = match p.max {
        Some(m) => format!("axis (max {})", num(m)),
        None => "axis".to_owned(),
    };
    let head = std::iter::once(cell(axis_head, BOLD))
        .chain(p.series.iter().map(|s| cell(&s.name, BOLD)))
        .collect();
    let rows = std::iter::once(head)
        .chain(p.axes.iter().enumerate().map(|(i, a)| {
            std::iter::once(cell(a, PLAIN))
                .chain(p.series.iter().map(|s| cell(num(s.values[i]), PLAIN)))
                .collect()
        }))
        .collect();
    out.extend(markdown::layout_table(rows, &[]));
    out
}

// --------------------------------------------------------------- Waterfall

/// The running total after each step: deltas add, `total`/`subtotal` set the level.
pub(crate) fn running_total(steps: &[WaterfallStep]) -> Vec<f64> {
    let mut cur = 0.0;
    steps
        .iter()
        .map(|s| {
            match s.kind {
                Some(WaterfallKind::Total | WaterfallKind::Subtotal) => cur = s.value,
                _ => cur += s.value,
            }
            cur
        })
        .collect()
}

fn is_total(s: &WaterfallStep) -> bool {
    matches!(
        s.kind,
        Some(WaterfallKind::Total | WaterfallKind::Subtotal)
    )
}

/// Table `step  Δ  running total`, totals bold.
pub(super) fn waterfall(p: &WaterfallProps) -> Vec<Line> {
    let mut out = vec![header(
        "Waterfall",
        p.title.as_deref(),
        p.steps.len(),
        "step",
        "steps",
    )];
    let unit = p.unit.as_deref();
    let totals = running_total(&p.steps);
    let head = vec![cell("step", BOLD), cell("Δ", BOLD), cell("total", BOLD)];
    let rows = std::iter::once(head)
        .chain(p.steps.iter().zip(&totals).map(|(s, t)| {
            let style = if is_total(s) { BOLD } else { PLAIN };
            let delta = if is_total(s) {
                "=".to_owned()
            } else {
                num_signed(s.value)
            };
            vec![
                cell(&s.label, style.clone()),
                cell(delta, style.clone()),
                cell(with_unit(*t, unit), style),
            ]
        }))
        .collect();
    out.extend(markdown::layout_table(rows, &[]));
    out
}

// ------------------------------------------------------------------ Funnel

/// Conversion from the previous stage, `25%`, when both have values.
fn conversion(stages: &[FunnelStage], i: usize) -> Option<String> {
    let prev = stages.get(i.checked_sub(1)?)?.value?;
    let cur = stages[i].value?;
    (prev > 0.0).then(|| format!("{}%", (cur / prev * 100.0).round() as i64))
}

/// Rows `label  value  ████░░  25%`, bars proportional to the largest value (equal without values).
pub(super) fn funnel(p: &FunnelProps, width: usize) -> Vec<Line> {
    let mut out = vec![header(
        "Funnel",
        p.title.as_deref(),
        p.stages.len(),
        "stage",
        "stages",
    )];
    let unit = p.unit.as_deref();
    let lw = p.stages.iter().map(|s| s.label.width()).max().unwrap_or(0);
    let values: Vec<String> = p
        .stages
        .iter()
        .map(|s| s.value.map(|v| with_unit(v, unit)).unwrap_or_default())
        .collect();
    let vw = values.iter().map(|v| v.width()).max().unwrap_or(0);
    let pcts: Vec<String> = (0..p.stages.len())
        .map(|i| conversion(&p.stages, i).unwrap_or_default())
        .collect();
    let pw = pcts.iter().map(|v| v.width()).max().unwrap_or(0);
    let max = p
        .stages
        .iter()
        .filter_map(|s| s.value)
        .fold(0.0, f64::max);
    let nw = p
        .stages
        .iter()
        .filter_map(|s| s.note.as_deref())
        .map(|n| n.width() + 2)
        .max()
        .unwrap_or(0);
    let n = cells(width, lw + vw + pw + nw + if vw > 0 { 6 } else { 4 });
    for (i, s) in p.stages.iter().enumerate() {
        let frac = match s.value {
            Some(v) if max > 0.0 => (v / max).clamp(0.0, 1.0),
            _ => 1.0,
        };
        let mut line = vec![span(format!("{}  ", padded(&s.label, lw)), PLAIN)];
        if vw > 0 {
            line.push(span(format!("{}  ", padded(&values[i], vw)), PLAIN));
        }
        line.extend(bar(frac, n));
        if !pcts[i].is_empty() {
            line.push(span(format!("  {}", padded(&pcts[i], pw)), DIM));
        }
        if let Some(note) = &s.note {
            line.push(span(format!("  {note}"), DIM));
        }
        out.push(line);
    }
    out
}

// ------------------------------------------------------------------- Gantt

/// Days since 1970-01-01 (fractional for datetimes) — Howard Hinnant's `days_from_civil`.
fn days(t: [u32; 6]) -> f64 {
    let [y, m, d, h, min, s] = t;
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    days as f64 + (f64::from(h) * 3600.0 + f64::from(min) * 60.0 + f64::from(s)) / 86_400.0
}

fn day_of(s: &str) -> Option<f64> {
    iso_datetime(s).map(days)
}

/// `2026-10-01 → 10-14` (end abbreviated inside the same year); datetimes show `HH:MM`, the end's
/// date dropped when it is the start's.
fn date_span(start: &str, end: &str) -> String {
    fn date(s: &str) -> &str {
        &s[..s.len().min(10)]
    }
    fn clock(s: &str) -> Option<&str> {
        s.get(11..16).filter(|_| s.len() >= 16)
    }
    let (sd, ed) = (date(start), date(end));
    let same_year = sd.len() == 10 && ed.len() == 10 && sd[..4] == ed[..4];
    match (clock(start), clock(end)) {
        (Some(sc), Some(ec)) if sd == ed => format!("{sd} {sc} → {ec}"),
        (Some(sc), Some(ec)) if same_year => format!("{sd} {sc} → {} {ec}", &ed[5..]),
        (Some(sc), Some(ec)) => format!("{sd} {sc} → {ed} {ec}"),
        _ if same_year => format!("{sd} → {}", &ed[5..]),
        _ => format!("{sd} → {ed}"),
    }
}

/// A task's end instant: a date-only `end` is inclusive, so it runs to the next midnight.
fn end_of(end: &str) -> Option<f64> {
    day_of(end).map(|d| if end.len() == 10 { d + 1.0 } else { d })
}

/// `label  2026-10-01 → 10-14  ▓▓▓▓░░░░` on one scale fitted to the width; `│` the today column.
pub(super) fn gantt(p: &GanttProps, width: usize) -> Vec<Line> {
    let mut out = vec![header(
        "Gantt",
        p.title.as_deref(),
        p.tasks.len(),
        "task",
        "tasks",
    )];
    let starts: Vec<f64> = p.tasks.iter().filter_map(|t| day_of(&t.start)).collect();
    let ends: Vec<f64> = p
        .tasks
        .iter()
        .filter_map(|t| end_of(&t.end))
        .collect();
    let lo = starts.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = ends.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let span_days = (hi - lo).max(1.0);
    let grouped = p.tasks.iter().any(|t| t.group.is_some());
    let indent = if grouped { 2 } else { 0 };
    let lw = p.tasks.iter().map(|t| t.label.width()).max().unwrap_or(0);
    let dates: Vec<String> = p.tasks.iter().map(|t| date_span(&t.start, &t.end)).collect();
    let dw = dates.iter().map(|d| d.width()).max().unwrap_or(0);
    let n = cells(width, indent + lw + dw + 6);
    let col = |day: f64| -> usize { (((day - lo) / span_days) * n as f64).floor() as usize };
    let today = p.today.as_deref().and_then(day_of).map(col);
    let mut groups: Vec<&str> = Vec::new();
    for t in &p.tasks {
        if let Some(g) = &t.group
            && !groups.contains(&g.as_str())
        {
            groups.push(g);
        }
    }
    let row = |t: &GanttTask, date: &str, out: &mut Vec<Line>| {
        let (Some(s), Some(e)) = (day_of(&t.start), end_of(&t.end)) else {
            return;
        };
        let from = col(s).min(n - 1);
        let to = col(e).clamp(from + 1, n);
        let len = to - from;
        let fill = t
            .progress
            .unwrap_or(if t.state == Some(GraphState::Done) { 1.0 } else { 0.0 });
        let done = ((fill * len as f64).round() as usize).min(len);
        let mut cells: Vec<char> = vec![' '; n];
        for (i, c) in cells.iter_mut().enumerate().take(to).skip(from) {
            *c = if i - from < done { '▓' } else { '░' };
        }
        if let Some(tc) = today
            && tc < n
            && cells[tc] == ' '
        {
            cells[tc] = '│';
        }
        let text: String = cells.into_iter().collect::<String>().trim_end().to_owned();
        out.push(vec![
            span(
                format!("{}{}  ", " ".repeat(indent), padded(&t.label, lw)),
                PLAIN,
            ),
            span(format!("{}  ", padded(date, dw)), DIM),
            span(text, PLAIN),
        ]);
    };
    for g in &groups {
        out.push(vec![span(*g, BOLD)]);
        for (t, d) in p.tasks.iter().zip(&dates) {
            if t.group.as_deref() == Some(g) {
                row(t, d, &mut out);
            }
        }
    }
    for (t, d) in p.tasks.iter().zip(&dates) {
        if t.group.is_none() {
            row(t, d, &mut out);
        }
    }
    out
}

// ---------------------------------------------------------------- Quadrant

/// Quadrant index of an item: 0 tl, 1 tr, 2 bl, 3 br (`x`, `y` ≥ 0.5 are right / top).
pub(crate) fn quadrant_of(x: f64, y: f64) -> usize {
    usize::from(x >= 0.5) + 2 * usize::from(y < 0.5)
}

fn truncate(s: &str, to: usize) -> String {
    if s.width() <= to {
        return s.to_owned();
    }
    let mut out = String::new();
    for c in s.chars() {
        if out.width() + 2 > to {
            break;
        }
        out.push(c);
    }
    out.push('…');
    out
}

/// A 2×2 box-drawing grid listing the items of each quadrant; the axis labels as the caption.
pub(super) fn quadrant(p: &QuadrantProps, width: usize) -> Vec<Line> {
    let mut out = vec![header(
        "Quadrant",
        p.title.as_deref(),
        p.items.len(),
        "item",
        "items",
    )];
    const NAMES: [&str; 4] = ["top-left", "top-right", "bottom-left", "bottom-right"];
    let names: Vec<&str> = match p.quadrants.as_deref() {
        Some(q) if q.len() == 4 => q.iter().map(String::as_str).collect(),
        _ => NAMES.to_vec(),
    };
    let mut cells: [Vec<&str>; 4] = Default::default();
    for it in &p.items {
        cells[quadrant_of(it.x, it.y)].push(&it.label);
    }
    let content = names
        .iter()
        .map(|n| n.width())
        .chain(p.items.iter().map(|i| i.label.width()))
        .max()
        .unwrap_or(0);
    let inner = if width == 0 {
        content
    } else {
        content.min(width.saturating_sub(7) / 2).max(4)
    };
    let rule = |l: &str, m: &str, r: &str| -> Line {
        let seg = "─".repeat(inner + 2);
        vec![span(format!("{l}{seg}{m}{seg}{r}"), DIM)]
    };
    let text_rows = |left: usize, right: usize, out: &mut Vec<Line>| {
        let rows = cells[left].len().max(cells[right].len()) + 1;
        for r in 0..rows {
            let mut line = Line::new();
            for (k, q) in [left, right].into_iter().enumerate() {
                line.push(span(if k == 0 { "│ " } else { " │ " }, DIM));
                let (text, style) = if r == 0 {
                    (truncate(names[q], inner), BOLD)
                } else {
                    (
                        cells[q]
                            .get(r - 1)
                            .map(|s| truncate(s, inner))
                            .unwrap_or_default(),
                        PLAIN,
                    )
                };
                line.push(span(padded(&text, inner), style));
            }
            line.push(span(" │", DIM));
            out.push(line);
        }
    };
    out.push(rule("┌", "┬", "┐"));
    text_rows(0, 1, &mut out);
    out.push(rule("├", "┼", "┤"));
    text_rows(2, 3, &mut out);
    out.push(rule("└", "┴", "┘"));
    let axis = |label: &str, ticks: Option<&Vec<String>>| -> String {
        match ticks {
            Some(t) if t.len() == 2 => format!("{label}: {} → {}", t[0], t[1]),
            _ => format!("{label} →"),
        }
    };
    out.push(vec![span(
        format!(
            "{} · {}",
            axis(&p.x_label, p.x_ticks.as_ref()),
            axis(&p.y_label, p.y_ticks.as_ref())
        ),
        DIM,
    )]);
    out
}

// ---------------------------------------------------------------- Timeline

/// Items in date order when `scale: time` (stable for equal instants), else as authored.
pub(super) fn timeline_order(p: &TimelineProps) -> Vec<&TimelineItem> {
    let mut items: Vec<&TimelineItem> = p.items.iter().collect();
    if p.scale == Some(TimelineScale::Time) {
        items.sort_by(|a, b| {
            iso_datetime(&a.when)
                .partial_cmp(&iso_datetime(&b.when))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    items
}

/// `when` or `when → until`; an `until` on the same date as a datetime `when` shows its time only.
pub(super) fn when(i: &TimelineItem) -> String {
    match &i.until {
        Some(u) if i.when.len() > 10 && u.len() > 10 && u[..10] == i.when[..10] => {
            format!("{} → {}", i.when, &u[11..])
        }
        Some(u) => format!("{} → {u}", i.when),
        None => i.when.clone(),
    }
}

// ---------------------------------------------------------------- markdown

/// The GFM form (§4.5) of one chart component; `None` for other components.
pub(crate) fn markdown(c: &Component) -> Option<String> {
    let mut out = String::new();
    let mut table = |title: Option<&str>, columns: Vec<String>, rows: Vec<Vec<String>>| {
        if let Some(t) = title.filter(|t| !t.is_empty()) {
            let _ = writeln!(out, "**{t}**\n");
        }
        let cell = |s: &str| s.replace('|', "\\|").replace('\n', " ");
        let row = |cells: &[String], out: &mut String| {
            let _ = writeln!(
                out,
                "| {} |",
                cells.iter().map(|c| cell(c)).collect::<Vec<_>>().join(" | ")
            );
        };
        row(&columns, &mut out);
        let _ = writeln!(out, "|{}", " --- |".repeat(columns.len()));
        for r in &rows {
            row(r, &mut out);
        }
        out.push('\n');
    };
    let series_table = |first: &str, labels: &[String], series: &[Series], unit: Option<&str>| {
        let mut columns = vec![first.to_owned()];
        columns.extend(series.iter().map(|s| match unit {
            Some(u) => format!("{} ({u})", s.name),
            None => s.name.clone(),
        }));
        let rows = labels
            .iter()
            .enumerate()
            .map(|(i, l)| {
                std::iter::once(l.clone())
                    .chain(series.iter().map(|s| num(s.values[i])))
                    .collect()
            })
            .collect();
        (columns, rows)
    };
    match c {
        Component::BarChart(p) => {
            let (c, r) = series_table(
                p.x_label.as_deref().unwrap_or(""),
                &p.labels,
                &p.series,
                p.unit.as_deref(),
            );
            table(p.title.as_deref(), c, r);
        }
        Component::LineChart(p) => {
            let (c, r) = series_table(
                p.x_label.as_deref().unwrap_or(""),
                &p.labels,
                &p.series,
                p.unit.as_deref(),
            );
            table(p.title.as_deref(), c, r);
        }
        Component::RadarChart(p) => {
            let (c, r) = series_table("axis", &p.axes, &p.series, None);
            table(p.title.as_deref(), c, r);
        }
        Component::ScatterChart(p) => {
            let sized = p.points.iter().any(|pt| pt.size.is_some());
            let mut columns = vec!["label".to_owned(), p.x_label.clone(), p.y_label.clone()];
            if sized {
                columns.push("size".to_owned());
            }
            if p.groups.is_some() {
                columns.push("group".to_owned());
            }
            let rows = p
                .points
                .iter()
                .map(|pt| {
                    let mut row = vec![
                        pt.label.clone().unwrap_or_default(),
                        num(pt.x),
                        num(pt.y),
                    ];
                    if sized {
                        row.push(pt.size.map(num).unwrap_or_default());
                    }
                    if p.groups.is_some() {
                        row.push(pt.group.clone().unwrap_or_default());
                    }
                    row
                })
                .collect();
            table(p.title.as_deref(), columns, rows);
        }
        Component::Waterfall(p) => {
            let totals = running_total(&p.steps);
            let rows = p
                .steps
                .iter()
                .zip(&totals)
                .map(|(s, t)| {
                    let label = if is_total(s) {
                        format!("**{}**", s.label)
                    } else {
                        s.label.clone()
                    };
                    let delta = if is_total(s) {
                        "=".to_owned()
                    } else {
                        num_signed(s.value)
                    };
                    vec![label, delta, with_unit(*t, p.unit.as_deref())]
                })
                .collect();
            table(
                p.title.as_deref(),
                vec!["step".into(), "Δ".into(), "total".into()],
                rows,
            );
        }
        Component::Funnel(p) => {
            let rows = p
                .stages
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    vec![
                        s.label.clone(),
                        s.value
                            .map(|v| with_unit(v, p.unit.as_deref()))
                            .unwrap_or_default(),
                        conversion(&p.stages, i).unwrap_or_default(),
                        s.note.clone().unwrap_or_default(),
                    ]
                })
                .collect();
            table(
                p.title.as_deref(),
                vec!["stage".into(), "value".into(), "conversion".into(), "note".into()],
                rows,
            );
        }
        Component::Gantt(p) => {
            let rows = p
                .tasks
                .iter()
                .map(|t| {
                    vec![
                        t.label.clone(),
                        t.start.clone(),
                        t.end.clone(),
                        t.group.clone().unwrap_or_default(),
                        t.progress
                            .map(|f| format!("{}%", (f * 100.0).round() as i64))
                            .unwrap_or_default(),
                    ]
                })
                .collect();
            table(
                p.title.as_deref(),
                vec![
                    "task".into(),
                    "start".into(),
                    "end".into(),
                    "group".into(),
                    "progress".into(),
                ],
                rows,
            );
        }
        Component::Quadrant(p) => {
            const NAMES: [&str; 4] = ["top-left", "top-right", "bottom-left", "bottom-right"];
            let names: Vec<String> = match p.quadrants.as_deref() {
                Some(q) if q.len() == 4 => q.to_vec(),
                _ => NAMES.iter().map(|s| (*s).to_owned()).collect(),
            };
            let rows = names
                .iter()
                .enumerate()
                .map(|(q, n)| {
                    let items: Vec<&str> = p
                        .items
                        .iter()
                        .filter(|i| quadrant_of(i.x, i.y) == q)
                        .map(|i| i.label.as_str())
                        .collect();
                    vec![n.clone(), items.join(", ")]
                })
                .collect();
            table(
                p.title.as_deref(),
                vec![
                    format!("quadrant ({} × {})", p.x_label, p.y_label),
                    "items".into(),
                ],
                rows,
            );
        }
        _ => return None,
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::parse;
    use crate::render::plain_text;

    fn root(body: &str) -> Component {
        let spec = parse(body).expect("valid").spec;
        spec.elements[&spec.root].component.clone()
    }

    const WATERFALL: &str = include_str!("../../fixtures/json-render/valid-waterfall.json");
    const GANTT: &str = include_str!("../../fixtures/json-render/valid-gantt.json");
    const QUADRANT: &str = include_str!("../../fixtures/json-render/valid-quadrant.json");
    const BAR: &str = include_str!("../../fixtures/json-render/valid-bar-chart.json");

    /// The desktop's `runningTotal()` (`lib/diagram/chart/geometry.ts`) pins the same sequence.
    #[test]
    fn waterfall_running_total_matches_the_renderer() {
        let Component::Waterfall(p) = root(WATERFALL) else {
            panic!()
        };
        assert_eq!(
            running_total(&p.steps),
            [100.0, 112.0, 107.0, 107.0, 111.0, 111.0]
        );
        let lines = waterfall(&p);
        assert_eq!(
            plain_text(&lines[..1]),
            "Waterfall: Headcount 2026 — 6 steps"
        );
        assert!(lines[3][0].style.bold, "Jan is a total");
        assert!(!lines[4][0].style.bold, "Hires is a delta");
    }

    #[test]
    fn gantt_fits_the_width_and_marks_today() {
        let Component::Gantt(p) = root(GANTT) else {
            panic!()
        };
        for width in [60, 100] {
            let lines = gantt(&p, width);
            let text = plain_text(&lines);
            assert!(
                lines.iter().all(|l| l.iter().map(|s| s.text.width()).sum::<usize>() <= width),
                "{text}"
            );
            assert!(text.contains("2026-10-07 → 10-08"), "{text}");
            assert!(text.contains('│'), "today column:\n{text}");
            assert!(text.contains("▓") && text.contains("░"), "{text}");
        }
        assert_eq!(plain_text(&gantt(&p, 60)[..1]), "Gantt: Wave 123 — 6 tasks");
    }

    #[test]
    fn quadrant_places_items_in_a_two_by_two_grid() {
        let Component::Quadrant(p) = root(QUADRANT) else {
            panic!()
        };
        let text = plain_text(&quadrant(&p, 60));
        assert!(text.contains("┌") && text.contains("┼") && text.contains("┘"), "{text}");
        let top = text.lines().nth(2).unwrap();
        assert!(top.contains("Do first") && top.contains("Plan"), "{top}");
        assert_eq!(quadrant_of(0.2, 0.8), 0);
        assert_eq!(quadrant_of(0.5, 0.6), 1, "the midline is right / top");
        assert_eq!(quadrant_of(0.3, 0.3), 2);
        assert_eq!(quadrant_of(0.8, 0.2), 3);
        assert!(text.ends_with("Effort: low → high · Impact: low → high"), "{text}");
    }

    #[test]
    fn bar_chart_rows_scale_to_the_width_and_end_with_a_legend() {
        let Component::BarChart(p) = root(BAR) else {
            panic!()
        };
        let lines = bar_chart(&p, 60);
        let text = plain_text(&lines);
        assert!(text.starts_with("BarChart: CI runs per day — 2 series\n"), "{text}");
        assert!(text.contains("Thu  ▓"), "{text}");
        assert!(text.contains("60 · 2 runs"), "{text}");
        assert_eq!(plain_text(&lines[lines.len() - 1..]), "▓ Passed  ▒ Failed");
        assert!(lines.iter().all(|l| l.iter().map(|s| s.text.width()).sum::<usize>() <= 60));
    }

    #[test]
    fn numbers_format_compactly() {
        assert_eq!(num(42.0), "42");
        assert_eq!(num(0.25), "0.25");
        assert_eq!(num(1.5), "1.5");
        assert_eq!(num_signed(-5.0), "-5");
        assert_eq!(date_span("2026-10-01", "2026-10-14"), "2026-10-01 → 10-14");
        assert_eq!(date_span("2026-12-30T10:00:00Z", "2027-01-02"), "2026-12-30 → 2027-01-02");
        assert_eq!(date_span("2026-10-08T09:00:00Z", "2026-10-08T10:00:00Z"), "2026-10-08 09:00 → 10:00");
        assert_eq!(date_span("2026-10-08T16:00:00Z", "2026-10-09T09:00:00Z"), "2026-10-08 16:00 → 10-09 09:00");
        assert_eq!(end_of("2026-10-08"), Some(days([2026, 10, 9, 0, 0, 0])));
        assert_eq!(end_of("2026-10-08T10:00:00Z"), Some(days([2026, 10, 8, 10, 0, 0])));
        assert_eq!(days([1970, 1, 1, 0, 0, 0]), 0.0);
        assert_eq!(days([2026, 10, 8, 12, 0, 0]), 20734.5);
    }

    #[test]
    fn markdown_forms_are_tables_with_a_title() {
        let md = markdown(&root(WATERFALL)).unwrap();
        assert!(md.starts_with("**Headcount 2026**\n\n| step | Δ | total |"), "{md}");
        assert!(md.contains("| **Jan** | = | 100 people |"), "{md}");
        assert!(md.contains("| Hires | +12 | 112 people |"), "{md}");
        let md = markdown(&root(QUADRANT)).unwrap();
        assert!(md.contains("| Do first | Cache |"), "{md}");
        assert!(md.contains("| Plan | Rewrite, Tests |"), "{md}");
    }
}
