//! Search controls, previews and inspection chrome; the shell supplies the input widget.
use iocraft::prelude::*;
use super::state::{self as search, Focus, Preview};
use crate::components::{EditorConfig, View as PanelView};
use crate::model::short;
use crate::ui::{BONE, BONE_DIM, SAND, OCHRE, MALACHITE, CARNELIAN, FAIENCE, line};
use crate::ui::OCHRE as HEADING;

pub(super) fn layout(
    s: &search::State,
    width: u16,
    budget: usize,
) -> (usize, usize, Vec<search::ChipRow>, bool) {
    // Border 2 + padding 2; rows stay one cell short of the inner width so
    // taffy never wraps them (see `menu_box_is_tight_at_any_width`).
    let inner = usize::from(width).saturating_sub(5).max(8);
    let chips = search::chip_rows(s, inner);
    let rows = budget.max(3) - 2;
    let area = if s.previews.is_empty() {
        rows.saturating_sub(3 + chips.len()).max(1)
    } else {
        rows - 1
    };
    (inner, area, chips, area < 4)
}

pub(super) fn search_box(s: &search::State, attach: bool, editor: Option<EditorConfig>, width: u16, budget: usize) -> PanelView {
    let (inner, area, chips, compact) = layout(s, width, budget);
    let dim = SAND;
    let border = OCHRE;
    if let Some(p) = s.previews.last() {
        let title = search::fit(&p.title(), inner);
        let rows: Vec<AnyElement<'static>> = match p.rows(inner) {
            Some(rows) => rows
                .into_iter()
                .skip(p.scroll())
                .take(area)
                .map(|r| {
                    let color = match (p, r.chars().next()) {
                        (Preview::Changes { .. }, Some('+')) => MALACHITE,
                        (Preview::Changes { .. }, Some('-')) => CARNELIAN,
                        (Preview::Changes { .. }, Some('@')) => FAIENCE,
                        _ => BONE_DIM,
                    };
                    element!(Text(content: search::fit(&r, inner), wrap: TextWrap::NoWrap, color))
                        .into_any()
                })
                .collect(),
            None => match p {
                Preview::Artifact { body, scroll, .. } => body
                    .view()
                    .lines()
                    .skip(*scroll)
                    .take(area)
                    .map(line)
                    .collect(),
                _ => Vec::new(),
            },
        };
        let height = u16::try_from((2 + 1 + rows.len()).min(budget.max(3))).unwrap_or(u16::MAX);
        let element = element! {
            View(border_style: BorderStyle::Round, border_color: border, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1, width, height, overflow: Overflow::Hidden) {
                Text(content: title, color: dim, wrap: TextWrap::NoWrap)
                #(rows)
            }
        }
        .into_any();
        return PanelView::IntrinsicElement { rows: usize::from(height), replace: false, element };
    }
    // Every row in order, and the one the focus sits on: a box too short
    // for them all drops leading rows until that one is in view.
    let mut body: Vec<AnyElement<'static>> = Vec::with_capacity(3 + chips.len() + s.rows.len());
    let mut target = 0;
    let query_focus = s.focus == Focus::Query;
    // Chips: selected ones carry a check and colour; the focused one inverts.
    for row in &chips {
        let mut contents = Vec::with_capacity(row.len() * 2);
        for (n, (i, text)) in row.iter().enumerate() {
            if n > 0 {
                contents.push(MixedTextContent::new(" "));
            }
            let on = text.contains('✓');
            let mut c = MixedTextContent::new(text).color(if on { FAIENCE } else { BONE_DIM });
            if s.focus == Focus::Filters(*i) {
                c = c.invert().weight(Weight::Bold);
                target = body.len() + 1;
            }
            contents.push(c);
        }
        body.push(element!(MixedText(contents, wrap: TextWrap::NoWrap)).into_any());
    }
    let status = search::fit(&s.status(), inner);
    body.push(
        element!(Text(content: status, color: if s.notice.is_some() { OCHRE } else { dim }, wrap: TextWrap::NoWrap))
            .into_any(),
    );
    // Result rows: the window starting at the clamped scroll.
    let first = search::first_row(&s.rows, s.scroll, s.cursor, area, compact);
    let text_fit = inner.saturating_sub(2);
    let mut used = 0;
    for (r, row) in s.rows.iter().enumerate().skip(first) {
        let h = row.height(compact);
        if used + h > area {
            break;
        }
        used += h;
        let here = r == s.cursor && row.selectable();
        if here && s.focus == Focus::Results {
            target = body.len() + 1;
        }
        let mark = if here { "› " } else { "  " };
        let color = if here { BONE } else { BONE_DIM };
        let weight = if here { Weight::Bold } else { Weight::Normal };
        match row {
            search::Row::Header(t) => body.push(
                element!(Text(content: search::fit(t, inner), color: dim, weight: Weight::Bold, wrap: TextWrap::NoWrap)).into_any(),
            ),
            search::Row::Note(t) => body.push(
                element!(Text(content: format!("  {}", search::fit(t, text_fit)), color: dim, italic: true, wrap: TextWrap::NoWrap)).into_any(),
            ),
            search::Row::More(g) => body.push(
                element!(Text(content: format!("{mark}{}", s.more_label(*g)), color: if here { BONE } else { FAIENCE }, weight, wrap: TextWrap::NoWrap)).into_any(),
            ),
            search::Row::Recent(i) => {
                let Some(sess) = s.recent.as_ref().and_then(|r| r.get(*i)) else { continue };
                let text = format!(
                    "{mark}{}  {}  {}",
                    short(sess.id),
                    sess.updated_at.with_timezone(&chrono::Local).format("%m-%d %H:%M"),
                    sess.title
                );
                body.push(element!(Text(content: search::fit(&text, inner), color, weight, wrap: TextWrap::NoWrap)).into_any());
            }
            search::Row::Hit(g, i) => {
                let Some(hit) = s.groups.get(*g).and_then(|g| g.hits.get(*i)) else { continue };
                let (title, prov) = (hit.title(), hit.provenance());
                let room = text_fit.saturating_sub(3);
                let prov_room = (room / 2).min(search::cells(&prov));
                let title = search::fit(&title, room.saturating_sub(prov_room + if prov_room > 0 { 2 } else { 0 }));
                let prov = search::fit(&prov, room.saturating_sub(search::cells(&title) + 2));
                body.push(
                    element!(MixedText(contents: vec![
                        MixedTextContent::new(format!("{mark}{title}")).color(color).weight(weight),
                        MixedTextContent::new(format!("  {prov}")).color(dim),
                    ], wrap: TextWrap::NoWrap))
                    .into_any(),
                );
                if !compact {
                    let (label, snippet, ranges) = hit.snippet();
                    let mut contents = vec![MixedTextContent::new(format!("    {label}")).color(dim)];
                    let max = text_fit.saturating_sub(2 + search::cells(label));
                    for (text, hot) in search::runs(snippet, ranges, max) {
                        let mut c = MixedTextContent::new(text).color(if hot { HEADING } else { BONE_DIM });
                        if hot {
                            c = c.weight(Weight::Bold);
                        }
                        contents.push(c);
                    }
                    body.push(element!(MixedText(contents, wrap: TextWrap::NoWrap)).into_any());
                }
            }
        }
    }
    // Footer: the actions the footer focus walks, the key legend before
    // them when the row has the room.
    let legend: &str = if attach {
        "⇥ filters · ↑↓ results · ⏎ attach · esc close  "
    } else {
        "⇥ filters · ↑↓ results · ⏎ open · esc close  "
    };
    let actions: usize = search::FOOTER.iter().map(|a| search::cells(a) + 3).sum();
    let mut footer = Vec::with_capacity(1 + 2 * search::FOOTER.len());
    if search::cells(legend) + actions <= inner {
        footer.push(MixedTextContent::new(legend).color(dim));
    }
    for (i, action) in search::FOOTER.iter().enumerate() {
        let mut c = MixedTextContent::new(format!("[{action}]")).color(BONE_DIM);
        if s.focus == Focus::Footer(i) {
            c = c.invert().weight(Weight::Bold);
        }
        footer.push(c);
        footer.push(MixedTextContent::new(" "));
    }
    if matches!(s.focus, Focus::Footer(_)) {
        target = body.len() + 1;
    }
    body.push(element!(MixedText(contents: footer, wrap: TextWrap::NoWrap)).into_any());
    // Never taller than the transcript rows on offer: a short terminal
    // scrolls the box to the focus rather than the frame overflowing (49 §6.2).
    let shown = budget.max(3) - 2;
    let skip = (target + 1).saturating_sub(shown);
    let height = u16::try_from(2 + (body.len() + 1).min(shown)).unwrap_or(u16::MAX);
    PanelView::Editor { editor: editor.expect("query view requires a shell editor"), rows: usize::from(height), replace: false, render: Box::new(move |input| {
        let query = element! {
            View(flex_direction: FlexDirection::Row, height: 1) {
                Text(content: "Query: ", color: if query_focus { BONE } else { dim }, weight: Weight::Bold)
                View(flex_grow: 1.0_f32, height: 1) { #([input]) }
            }
        }.into_any();
        element! {
            View(border_style: BorderStyle::Round, border_color: border, flex_direction: FlexDirection::Column, padding_left: 1, padding_right: 1, width, height, overflow: Overflow::Hidden) {
                #(std::iter::once(query).chain(body).skip(skip))
            }
        }.into_any()
    }) }
}

pub(super) fn inspect_hint(i: &search::Inspect) -> PanelView {
    let text = format!("earlier messages · {} · message {}{}   ↑↓ scroll  ⏎/j jump to latest  esc/b back to results", short(i.session), i.seq, if i.found { "" } else { " (not on this page)" });
    PanelView::IntrinsicElement { rows: 1, replace: false, element: element!(View(height: 1, padding_left: 1) {
        Text(content: text, color: OCHRE, wrap: TextWrap::NoWrap)
    }).into_any() }
}
