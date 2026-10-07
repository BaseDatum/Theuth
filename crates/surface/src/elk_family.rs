//! The ELK family on text surfaces (design 123 §4.4, W1): `StateMachine`, `Entities`,
//! `Swimlane` and `GraphDelta` as the typed text the TUI, Signal and the Slack fallback show,
//! plus the `stateDiagram-v2` / `erDiagram` lowering for channels that carry mermaid.
//! `GraphDelta`'s ledger is derived here by id (§0.9) — never authored.

use std::collections::{BTreeMap, BTreeSet};

use markdown::Line;
use unicode_width::UnicodeWidthStr;

use crate::graph::{self, mermaid_ids, quoted, truncate};
use crate::render::{BOLD, DIM, PLAIN, span};
use crate::spec::{
    EntitiesDetail, EntitiesProps, Entity, EntityKind, FieldKey, GraphDeltaProps, GraphDirection,
    GraphEdgeStyle, GraphNode, GraphProps, GraphSide, GraphState, MachineStateKind, RelationKind,
    StateMachineProps, SwimlaneProps,
};

/// Fields shown per entity before `+n more` (§4.3: the cap is 32, the renderer shows 12).
pub const SHOWN_FIELDS: usize = 12;

fn glyph(state: Option<GraphState>) -> &'static str {
    match state.unwrap_or(GraphState::Default) {
        GraphState::Active => "◉",
        GraphState::Done => "●",
        GraphState::Muted => "○",
        GraphState::Danger => "✗",
        GraphState::Default => "•",
    }
}

fn title_line(title: Option<&String>, out: &mut Vec<Line>) {
    if let Some(t) = title {
        out.push(vec![span(t, BOLD)]);
    }
}

// ---- StateMachine ----

/// `label [guard]` — the transition text the TUI, the desktop label mask and mermaid share.
pub fn transition_text(label: Option<&str>, guard: Option<&str>) -> String {
    match (
        label.filter(|l| !l.is_empty()),
        guard.filter(|g| !g.is_empty()),
    ) {
        (Some(l), Some(g)) => format!("{l} [{g}]"),
        (Some(l), None) => l.to_owned(),
        (None, Some(g)) => format!("[{g}]"),
        (None, None) => String::new(),
    }
}

/// One line per transition: `◉ Draft → Review  submit [complete]` — the source state's glyph
/// (`◉` initial, `◎` final, `◇` choice, else the state glyph), then the event and guard dim.
/// States no transition touches are listed after. Lines are cut to `width` with `…`.
pub fn state_machine_lines(p: &StateMachineProps, width: usize) -> Vec<Line> {
    let by_id: BTreeMap<&str, &crate::spec::MachineState> =
        p.states.iter().map(|s| (s.id.as_str(), s)).collect();
    let mark = |id: &str| -> &'static str {
        match by_id.get(id) {
            Some(s) => match s.kind {
                Some(MachineStateKind::Initial) => "◉",
                Some(MachineStateKind::Final) => "◎",
                Some(MachineStateKind::Choice) => "◇",
                _ => glyph(s.state),
            },
            None => "•",
        }
    };
    let name = |id: &str| -> String {
        by_id
            .get(id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    let mut out = Vec::new();
    title_line(p.title.as_ref(), &mut out);
    let mut touched = BTreeSet::new();
    for t in &p.transitions {
        touched.insert(t.from.as_str());
        touched.insert(t.to.as_str());
        let arrow = match (t.state, t.style) {
            (Some(GraphState::Danger), _) => "✗→",
            (_, Some(GraphEdgeStyle::Dashed)) => "⇢",
            _ => "→",
        };
        let head = format!(
            "{} {} {arrow} {}",
            mark(&t.from),
            name(&t.from),
            name(&t.to)
        );
        let text = transition_text(t.label.as_deref(), t.guard.as_deref());
        if text.is_empty() {
            out.push(vec![span(truncate(head, width), PLAIN)]);
        } else {
            let room = width.saturating_sub(head.width() + 2);
            let tail = if width == 0 || head.width() + 2 + text.width() <= width {
                format!("  {text}")
            } else if room > 1 {
                format!("  {}", truncate(text, room))
            } else {
                String::new()
            };
            out.push(vec![span(truncate(head, width), PLAIN), span(tail, DIM)]);
        }
    }
    for s in p.states.iter().filter(|s| !touched.contains(s.id.as_str())) {
        let mut text = format!("{} {}", mark(&s.id), s.label);
        if let Some(sub) = &s.sub {
            text.push_str(&format!(" ({sub})"));
        }
        out.push(vec![span(truncate(text, width), PLAIN)]);
    }
    if out.is_empty() {
        out.push(vec![span("(empty state machine)", DIM)]);
    }
    out
}

/// The machine as `stateDiagram-v2`: `[*] --> initial`, `final --> [*]`, `<<choice>>` states,
/// `from --> to : event [guard]`; `direction LR|TB` first.
pub fn state_machine_mermaid(p: &StateMachineProps) -> String {
    let ids = mermaid_ids(p.states.iter().map(|s| s.id.as_str()));
    let id = |raw: &str| ids.get(raw).map(String::as_str).unwrap_or("_");
    let dir = match p.direction.unwrap_or(GraphDirection::LR) {
        GraphDirection::LR => "LR",
        GraphDirection::TB => "TB",
    };
    let mut out = format!("stateDiagram-v2\n  direction {dir}\n");
    for s in &p.states {
        let label = one_line(&s.label);
        match s.kind {
            Some(MachineStateKind::Choice) => {
                out.push_str(&format!("  state {} <<choice>>\n", id(&s.id)));
            }
            _ => out.push_str(&format!("  state {} as {}\n", quoted(&label), id(&s.id))),
        }
    }
    for s in &p.states {
        match s.kind {
            Some(MachineStateKind::Initial) => {
                out.push_str(&format!("  [*] --> {}\n", id(&s.id)));
            }
            Some(MachineStateKind::Final) => {
                out.push_str(&format!("  {} --> [*]\n", id(&s.id)));
            }
            _ => {}
        }
    }
    for t in &p.transitions {
        let text = one_line(&transition_text(t.label.as_deref(), t.guard.as_deref()));
        if text.is_empty() {
            out.push_str(&format!("  {} --> {}\n", id(&t.from), id(&t.to)));
        } else {
            out.push_str(&format!("  {} --> {} : {text}\n", id(&t.from), id(&t.to)));
        }
    }
    out
}

fn one_line(text: &str) -> String {
    text.split(['\n', '\r'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .replace(':', "꞉")
}

// ---- Entities ----

fn key_text(key: Option<FieldKey>, nullable: Option<bool>) -> String {
    let mut parts = Vec::new();
    match key {
        Some(FieldKey::Pk) => parts.push("pk"),
        Some(FieldKey::Fk) => parts.push("fk"),
        Some(FieldKey::Uk) => parts.push("uk"),
        None => {}
    }
    if nullable == Some(true) {
        parts.push("null");
    }
    parts.join(" ")
}

/// The relation glyph between the two names: crow's-foot ends for the cardinalities, UML marks
/// for the class relations.
fn relation_glyph(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::OneOne => "┼──┼",
        RelationKind::OneMany => "┼──<",
        RelationKind::ManyMany => ">──<",
        RelationKind::Inherits => "──▷",
        RelationKind::Implements => "┄┄▷",
        RelationKind::Composes => "◆──",
        RelationKind::Aggregates => "◇──",
        RelationKind::Depends => "┄┄>",
        RelationKind::References => "──>",
    }
}

fn kind_name(kind: RelationKind) -> &'static str {
    match kind {
        RelationKind::OneOne => "one-one",
        RelationKind::OneMany => "one-many",
        RelationKind::ManyMany => "many-many",
        RelationKind::Inherits => "inherits",
        RelationKind::Implements => "implements",
        RelationKind::Composes => "composes",
        RelationKind::Aggregates => "aggregates",
        RelationKind::Depends => "depends",
        RelationKind::References => "references",
    }
}

/// The rows of one entity's table: `field  type  key` (`+n more` after `SHOWN_FIELDS`), then
/// the methods as `name(sig)` rows when the diagram is `detail: class`.
fn entity_rows(e: &Entity, detail: EntitiesDetail) -> Vec<Vec<Line>> {
    let mut rows = vec![vec![
        vec![span(&e.label, BOLD)],
        vec![span(
            match e.kind.unwrap_or(EntityKind::Entity) {
                EntityKind::Entity => "",
                EntityKind::Table => "table",
                EntityKind::Class => "class",
                EntityKind::Interface => "interface",
                EntityKind::Enum => "enum",
            },
            DIM,
        )],
        Vec::new(),
    ]];
    for f in e.fields.iter().take(SHOWN_FIELDS) {
        let ty = if detail == EntitiesDetail::Model {
            String::new()
        } else {
            f.r#type.clone().unwrap_or_default()
        };
        rows.push(vec![
            vec![span(&f.name, PLAIN)],
            vec![span(ty, DIM)],
            vec![span(key_text(f.key, f.nullable), DIM)],
        ]);
    }
    if e.fields.len() > SHOWN_FIELDS {
        rows.push(vec![vec![span(
            format!("+{} more", e.fields.len() - SHOWN_FIELDS),
            DIM,
        )]]);
    }
    if detail == EntitiesDetail::Class {
        for m in e.methods.iter().flatten() {
            rows.push(vec![vec![span(
                format!("{}{}", m.name, m.sig.as_deref().unwrap_or("()")),
                PLAIN,
            )]]);
        }
    }
    rows
}

/// One table per entity (`field  type  key`), then one relation line each:
/// `users ┼──< sessions  (one-many: id → user_id)`. Tables wider than `width` fall back to
/// `field  type` rows without the rule.
pub fn entities_lines(p: &EntitiesProps, width: usize) -> Vec<Line> {
    let detail = p.detail.unwrap_or(EntitiesDetail::Model);
    let label_of: BTreeMap<&str, &str> = p
        .entities
        .iter()
        .map(|e| (e.id.as_str(), e.label.as_str()))
        .collect();
    let name = |id: &str| -> String {
        label_of
            .get(id)
            .map(|l| (*l).to_owned())
            .unwrap_or_else(|| id.to_owned())
    };
    let mut out = Vec::new();
    title_line(p.title.as_ref(), &mut out);
    for (i, e) in p.entities.iter().enumerate() {
        if i > 0 || !out.is_empty() {
            out.push(Line::new());
        }
        let table = markdown::layout_table(entity_rows(e, detail), &[]);
        if width == 0 || table.iter().all(|l| line_width(l) <= width) {
            out.extend(table);
        } else {
            out.push(vec![span(truncate(e.label.clone(), width), BOLD)]);
            for f in e.fields.iter().take(SHOWN_FIELDS) {
                let mut text = format!("  {}", f.name);
                if let Some(t) = f
                    .r#type
                    .as_deref()
                    .filter(|_| detail != EntitiesDetail::Model)
                {
                    text.push_str(&format!("  {t}"));
                }
                out.push(vec![span(truncate(text, width), PLAIN)]);
            }
            if e.fields.len() > SHOWN_FIELDS {
                out.push(vec![span(
                    format!("  +{} more", e.fields.len() - SHOWN_FIELDS),
                    DIM,
                )]);
            }
        }
    }
    if !p.relations.is_empty() {
        out.push(Line::new());
    }
    for r in &p.relations {
        let head = format!(
            "{} {} {}",
            name(&r.from),
            relation_glyph(r.kind),
            name(&r.to)
        );
        let mut detail = kind_name(r.kind).to_owned();
        if let (Some(a), Some(b)) = (&r.from_field, &r.to_field) {
            detail.push_str(&format!(": {a} → {b}"));
        }
        if let Some(l) = r.label.as_deref().filter(|l| !l.is_empty()) {
            detail.push_str(&format!(", {l}"));
        }
        let tail = format!("  ({detail})");
        if width == 0 || head.width() + tail.width() <= width {
            out.push(vec![span(head, PLAIN), span(tail, DIM)]);
        } else {
            out.push(vec![span(truncate(head, width), PLAIN)]);
        }
    }
    if out.is_empty() {
        out.push(vec![span("(no entities)", DIM)]);
    }
    out
}

/// `erDiagram` source (`classDiagram` when `detail: class`): entities with their fields, then
/// relations with mermaid's crow's-foot (`||--o{`) or UML (`<|--`, `*--`, `o--`, `..>`) marks.
pub fn entities_mermaid(p: &EntitiesProps) -> String {
    let ids = mermaid_ids(p.entities.iter().map(|e| e.id.as_str()));
    let id = |raw: &str| ids.get(raw).map(String::as_str).unwrap_or("_");
    let class = p.detail == Some(EntitiesDetail::Class);
    let mut out = String::from(if class {
        "classDiagram\n"
    } else {
        "erDiagram\n"
    });
    for e in &p.entities {
        if class {
            out.push_str(&format!("  class {} {{\n", id(&e.id)));
            if let Some(k) = e
                .kind
                .filter(|k| matches!(k, EntityKind::Interface | EntityKind::Enum))
            {
                out.push_str(&format!(
                    "    <<{}>>\n",
                    if k == EntityKind::Interface {
                        "interface"
                    } else {
                        "enumeration"
                    }
                ));
            }
            for f in &e.fields {
                out.push_str(&format!(
                    "    {}{}\n",
                    f.r#type
                        .as_deref()
                        .map(|t| format!("{t} "))
                        .unwrap_or_default(),
                    f.name
                ));
            }
            for m in e.methods.iter().flatten() {
                out.push_str(&format!(
                    "    {}{}\n",
                    m.name,
                    m.sig.as_deref().unwrap_or("()")
                ));
            }
            out.push_str("  }\n");
        } else {
            out.push_str(&format!("  {} {{\n", id(&e.id)));
            for f in &e.fields {
                let key = match f.key {
                    Some(FieldKey::Pk) => " PK",
                    Some(FieldKey::Fk) => " FK",
                    Some(FieldKey::Uk) => " UK",
                    None => "",
                };
                out.push_str(&format!(
                    "    {} {}{key}\n",
                    f.r#type.as_deref().unwrap_or("string").replace(' ', "_"),
                    f.name
                ));
            }
            out.push_str("  }\n");
        }
    }
    for r in &p.relations {
        let mark = if class {
            match r.kind {
                RelationKind::Inherits => "--|>",
                RelationKind::Implements => "..|>",
                RelationKind::Composes => "*--",
                RelationKind::Aggregates => "o--",
                RelationKind::Depends => "..>",
                RelationKind::OneOne | RelationKind::OneMany | RelationKind::ManyMany => {
                    "\"1\" --> \"*\""
                }
                RelationKind::References => "-->",
            }
        } else {
            match r.kind {
                RelationKind::OneOne => "||--||",
                RelationKind::OneMany => "||--o{",
                RelationKind::ManyMany => "}o--o{",
                RelationKind::Inherits | RelationKind::Implements => "||--||",
                _ => "}o--||",
            }
        };
        let label = r
            .label
            .as_deref()
            .filter(|l| !l.is_empty())
            .map(one_line)
            .unwrap_or_else(|| kind_name(r.kind).to_owned());
        out.push_str(&format!(
            "  {} {mark} {} : {}\n",
            id(&r.from),
            id(&r.to),
            if class { label } else { quoted(&label) }
        ));
    }
    out
}

// ---- Swimlane ----

/// Bold lane headers, each lane's steps indented under it (`◇` decision, `▶` start, `■` end,
/// `▱` data), then one `→` line per edge that crosses lanes. Lines are cut to `width`.
pub fn swimlane_lines(p: &SwimlaneProps, width: usize) -> Vec<Line> {
    let lane_of: BTreeMap<&str, &str> = p
        .steps
        .iter()
        .map(|s| (s.id.as_str(), s.lane.as_str()))
        .collect();
    let label_of: BTreeMap<&str, &str> = p
        .steps
        .iter()
        .map(|s| (s.id.as_str(), s.label.as_str()))
        .collect();
    let name = |id: &str| -> String {
        label_of
            .get(id)
            .map(|l| (*l).to_owned())
            .unwrap_or_else(|| id.to_owned())
    };
    let mut out = Vec::new();
    title_line(p.title.as_ref(), &mut out);
    for lane in &p.lanes {
        out.push(vec![span(truncate(lane.label.clone(), width), BOLD)]);
        for s in p.steps.iter().filter(|s| s.lane == lane.id) {
            let mark = match s.kind {
                Some(crate::spec::LaneStepKind::Decision) => "◇",
                Some(crate::spec::LaneStepKind::Start) => "▶",
                Some(crate::spec::LaneStepKind::End) => "■",
                Some(crate::spec::LaneStepKind::Data) => "▱",
                _ => glyph(s.state),
            };
            let mut text = format!("  {mark} {}", s.label);
            if let Some(sub) = &s.sub {
                text.push_str(&format!(" ({sub})"));
            }
            let targets: Vec<String> = p
                .edges
                .iter()
                .filter(|e| e.from == s.id && lane_of.get(e.to.as_str()) == Some(&lane.id.as_str()))
                .map(|e| match e.label.as_deref().filter(|l| !l.is_empty()) {
                    Some(l) => format!("{} ({l})", name(&e.to)),
                    None => name(&e.to),
                })
                .collect();
            if !targets.is_empty() {
                text.push_str(" → ");
                text.push_str(&targets.join(", "));
            }
            out.push(vec![span(truncate(text, width), PLAIN)]);
        }
    }
    let crossing: Vec<Line> = p
        .edges
        .iter()
        .filter(|e| lane_of.get(e.from.as_str()) != lane_of.get(e.to.as_str()))
        .map(|e| {
            let arrow = match e.style {
                Some(GraphEdgeStyle::Dashed) => "⇢",
                _ => "→",
            };
            let mut text = format!("{} {arrow} {}", name(&e.from), name(&e.to));
            if let Some(l) = e.label.as_deref().filter(|l| !l.is_empty()) {
                text.push_str(&format!("  {l}"));
            }
            vec![span(truncate(text, width), PLAIN)]
        })
        .collect();
    if !crossing.is_empty() {
        out.push(Line::new());
        out.extend(crossing);
    }
    if out.is_empty() {
        out.push(vec![span("(empty swimlane)", DIM)]);
    }
    out
}

// ---- GraphDelta ----

/// The derived ledger of a `GraphDelta` (§0.9): ids only in `after`, only in `before`, and in
/// both with a different `label`, `kind` or `group` (what changed, in that order).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Ledger {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    /// `(id, what)` — `what` is a comma-joined subset of `label`, `kind`, `group`.
    pub changed: Vec<(String, String)>,
}

pub fn ledger(p: &GraphDeltaProps) -> Ledger {
    let before: BTreeMap<&str, &GraphNode> =
        p.before.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let after: BTreeMap<&str, &GraphNode> =
        p.after.nodes.iter().map(|n| (n.id.as_str(), n)).collect();
    let mut l = Ledger::default();
    for n in &p.after.nodes {
        match before.get(n.id.as_str()) {
            None => l.added.push(n.id.clone()),
            Some(b) => {
                let mut what = Vec::new();
                if b.label != n.label {
                    what.push("label");
                }
                if b.kind != n.kind {
                    what.push("kind");
                }
                if b.group != n.group {
                    what.push("group");
                }
                if !what.is_empty() {
                    l.changed.push((n.id.clone(), what.join(", ")));
                }
            }
        }
    }
    for n in &p.before.nodes {
        if !after.contains_key(n.id.as_str()) {
            l.removed.push(n.id.clone());
        }
    }
    l
}

pub(crate) fn side_as_graph(s: &GraphSide, direction: Option<GraphDirection>) -> GraphProps {
    GraphProps {
        nodes: s.nodes.clone(),
        edges: s.edges.clone(),
        groups: s.groups.clone(),
        direction,
        legend: None,
        title: None,
    }
}

/// `Before: n nodes · After: m nodes`, the ledger as `+ added  − removed  ~ changed (what)`
/// (labels from the side that has the node), then the `after` adjacency list.
pub fn graph_delta_lines(p: &GraphDeltaProps, width: usize) -> Vec<Line> {
    let label = |id: &str| -> String {
        p.after
            .nodes
            .iter()
            .chain(p.before.nodes.iter())
            .find(|n| n.id == id)
            .map(|n| n.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    let mut out = Vec::new();
    title_line(p.title.as_ref(), &mut out);
    out.push(vec![span(
        format!(
            "Before: {} nodes · After: {} nodes",
            p.before.nodes.len(),
            p.after.nodes.len()
        ),
        DIM,
    )]);
    let l = ledger(p);
    for id in &l.added {
        out.push(vec![
            span("+ ", PLAIN),
            span(truncate(label(id), width.saturating_sub(2)), PLAIN),
        ]);
    }
    for id in &l.removed {
        out.push(vec![
            span("− ", PLAIN),
            span(truncate(label(id), width.saturating_sub(2)), PLAIN),
        ]);
    }
    for (id, what) in &l.changed {
        out.push(vec![
            span("~ ", PLAIN),
            span(truncate(label(id), width.saturating_sub(2)), PLAIN),
            span(format!(" ({what})"), DIM),
        ]);
    }
    if l.added.is_empty() && l.removed.is_empty() && l.changed.is_empty() {
        out.push(vec![span("= no node changes", DIM)]);
    }
    out.push(Line::new());
    out.extend(graph::adjacency_lines(
        &side_as_graph(&p.after, p.direction),
        width,
    ));
    out
}

// ---- Export (72 §3): the §4.4 forms as GFM ----

/// `- **Draft** → Review: submit [complete]` per transition.
pub(crate) fn state_machine_md(p: &StateMachineProps, out: &mut String) {
    use std::fmt::Write as _;
    let name = |id: &str| -> String {
        p.states
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    if let Some(t) = &p.title {
        let _ = writeln!(out, "**{t}**\n");
    }
    let _ = writeln!(
        out,
        "StateMachine: {} states, {} transitions\n",
        p.states.len(),
        p.transitions.len()
    );
    for t in &p.transitions {
        let _ = write!(out, "- **{}** → {}", name(&t.from), name(&t.to));
        let text = transition_text(t.label.as_deref(), t.guard.as_deref());
        if !text.is_empty() {
            let _ = write!(out, ": {text}");
        }
        out.push('\n');
    }
    out.push('\n');
}

/// One GFM table per entity (`field | type | key`), then `- from → to (kind)` relations.
pub(crate) fn entities_md(p: &EntitiesProps, out: &mut String) {
    use std::fmt::Write as _;
    let detail = p.detail.unwrap_or(EntitiesDetail::Model);
    let name = |id: &str| -> String {
        p.entities
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    if let Some(t) = &p.title {
        let _ = writeln!(out, "**{t}**\n");
    }
    for e in &p.entities {
        let _ = writeln!(out, "**{}**\n", e.label);
        if !e.fields.is_empty() {
            let _ = writeln!(out, "| field | type | key |\n| --- | --- | --- |");
            for f in e.fields.iter().take(SHOWN_FIELDS) {
                let ty = if detail == EntitiesDetail::Model {
                    ""
                } else {
                    f.r#type.as_deref().unwrap_or("")
                };
                let _ = writeln!(
                    out,
                    "| {} | {} | {} |",
                    f.name.replace('|', "\\|"),
                    ty.replace('|', "\\|"),
                    key_text(f.key, f.nullable)
                );
            }
            if e.fields.len() > SHOWN_FIELDS {
                let _ = writeln!(out, "| +{} more | | |", e.fields.len() - SHOWN_FIELDS);
            }
            out.push('\n');
        }
        if detail == EntitiesDetail::Class {
            for m in e.methods.iter().flatten() {
                let _ = writeln!(out, "- `{}{}`", m.name, m.sig.as_deref().unwrap_or("()"));
            }
            if e.methods.as_ref().is_some_and(|m| !m.is_empty()) {
                out.push('\n');
            }
        }
    }
    for r in &p.relations {
        let _ = write!(
            out,
            "- {} → {} ({}",
            name(&r.from),
            name(&r.to),
            kind_name(r.kind)
        );
        if let (Some(a), Some(b)) = (&r.from_field, &r.to_field) {
            let _ = write!(out, ": {a} → {b}");
        }
        if let Some(l) = r.label.as_deref().filter(|l| !l.is_empty()) {
            let _ = write!(out, ", {l}");
        }
        out.push_str(")\n");
    }
    if !p.relations.is_empty() {
        out.push('\n');
    }
}

/// `**Lane**` headings with their steps as bullets, then the cross-lane edges.
pub(crate) fn swimlane_md(p: &SwimlaneProps, out: &mut String) {
    use std::fmt::Write as _;
    let name = |id: &str| -> String {
        p.steps
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    let lane_of = |id: &str| p.steps.iter().find(|s| s.id == id).map(|s| s.lane.as_str());
    if let Some(t) = &p.title {
        let _ = writeln!(out, "**{t}**\n");
    }
    for lane in &p.lanes {
        let _ = writeln!(out, "**{}**\n", lane.label);
        for s in p.steps.iter().filter(|s| s.lane == lane.id) {
            let _ = write!(out, "- {}", s.label);
            if let Some(sub) = &s.sub {
                let _ = write!(out, " ({sub})");
            }
            out.push('\n');
        }
        out.push('\n');
    }
    let crossing: Vec<_> = p
        .edges
        .iter()
        .filter(|e| lane_of(&e.from) != lane_of(&e.to))
        .collect();
    for e in &crossing {
        let _ = write!(out, "- {} → {}", name(&e.from), name(&e.to));
        if let Some(l) = e.label.as_deref().filter(|l| !l.is_empty()) {
            let _ = write!(out, ": {l}");
        }
        out.push('\n');
    }
    if !crossing.is_empty() {
        out.push('\n');
    }
}

/// The counts line and the ledger as bullets; the caller appends the `after` graph.
pub(crate) fn graph_delta_md(p: &GraphDeltaProps, out: &mut String) {
    use std::fmt::Write as _;
    let label = |id: &str| -> String {
        p.after
            .nodes
            .iter()
            .chain(p.before.nodes.iter())
            .find(|n| n.id == id)
            .map(|n| n.label.clone())
            .unwrap_or_else(|| id.to_owned())
    };
    if let Some(t) = &p.title {
        let _ = writeln!(out, "**{t}**\n");
    }
    let _ = writeln!(
        out,
        "Before: {} nodes · After: {} nodes\n",
        p.before.nodes.len(),
        p.after.nodes.len()
    );
    let l = ledger(p);
    for id in &l.added {
        let _ = writeln!(out, "- **+** {}", label(id));
    }
    for id in &l.removed {
        let _ = writeln!(out, "- **−** {}", label(id));
    }
    for (id, what) in &l.changed {
        let _ = writeln!(out, "- **~** {} ({what})", label(id));
    }
    if !(l.added.is_empty() && l.removed.is_empty() && l.changed.is_empty()) {
        out.push('\n');
    }
}

fn line_width(l: &Line) -> usize {
    l.iter().map(|s| s.text.width()).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::plain_text;
    use crate::{Spec, parse};

    const STATE: &str = include_str!("../fixtures/json-render/valid-state-machine.json");
    const STATE_LOOP: &str =
        include_str!("../fixtures/json-render/valid-state-machine-self-loop.json");
    const ENTITIES: &str = include_str!("../fixtures/json-render/valid-entities.json");
    const CLASSES: &str = include_str!("../fixtures/json-render/valid-entities-class.json");
    const SWIM: &str = include_str!("../fixtures/json-render/valid-swimlane.json");
    const SWIM_TB: &str = include_str!("../fixtures/json-render/valid-swimlane-tb.json");
    const DELTA: &str = include_str!("../fixtures/json-render/valid-graph-delta.json");
    const DELTA_GROUPS: &str =
        include_str!("../fixtures/json-render/valid-graph-delta-groups.json");

    fn root(body: &str) -> crate::spec::Component {
        let Spec { root, elements } = parse(body).expect("valid fixture").spec;
        elements[&root].component.clone()
    }

    macro_rules! props {
        ($body:expr, $variant:ident) => {
            match root($body) {
                crate::spec::Component::$variant(p) => p,
                other => panic!("not a {}: {other:?}", stringify!($variant)),
            }
        };
    }

    #[test]
    fn state_machine_lists_transitions_with_marks_guards_and_untouched_states() {
        let p = props!(STATE, StateMachine);
        let text = plain_text(&state_machine_lines(&p, 0));
        assert_eq!(
            text,
            "Task lifecycle\n\
             ◉ Todo → Doing  start\n\
             ◉ Doing ⇢ Blocked  wait [dependency open]\n\
             ✗ Blocked → Doing  unblock\n\
             ◉ Doing → Review  submit\n\
             • Review → Checks?  run checks\n\
             ◇ Checks? → Done  pass [all green]\n\
             ◇ Checks? ✗→ Doing  fail [any red]"
        );
        let p = props!(STATE_LOOP, StateMachine);
        let text = plain_text(&state_machine_lines(&p, 0));
        assert!(text.contains("• Polling → Polling  tick"), "{text}");
        assert!(
            text.ends_with("◎ Stopped"),
            "untouched final state listed: {text}"
        );
        // Narrow: the guard is cut, the line fits.
        let narrow = state_machine_lines(&props!(STATE, StateMachine), 30);
        assert!(narrow.iter().all(|l| line_width(l) <= 30));
    }

    #[test]
    fn state_machine_mermaid_is_state_diagram_v2() {
        let p = props!(STATE, StateMachine);
        let m = state_machine_mermaid(&p);
        assert!(m.starts_with("stateDiagram-v2\n  direction LR\n"), "{m}");
        assert!(m.contains("  state gate <<choice>>\n"), "{m}");
        assert!(m.contains("  state \"Doing\" as doing\n"), "{m}");
        assert!(m.contains("  [*] --> todo\n"), "{m}");
        assert!(m.contains("  done --> [*]\n"), "{m}");
        assert!(m.contains("  gate --> done : pass [all green]\n"), "{m}");
        assert!(
            crate::mermaid_lines(&m, 0).is_ok(),
            "merman parses the state source"
        );
    }

    #[test]
    fn entities_tables_relations_and_more_marker() {
        let p = props!(ENTITIES, Entities);
        let text = plain_text(&entities_lines(&p, 100));
        assert!(text.starts_with("Sessions schema\n\nusers"), "{text}");
        assert!(text.contains("id         │ uuid        │ pk"), "{text}");
        assert!(text.contains("title    │ text  │ null"), "{text}");
        assert!(
            text.contains("users ┼──< sessions  (one-many: id → user_id)"),
            "{text}"
        );
        assert!(text.contains("turns ┄┄> Runner  (depends)"), "{text}");
        let c = props!(CLASSES, Entities);
        let text = plain_text(&entities_lines(&c, 0));
        assert!(text.contains("+2 more"), "{text}");
        assert!(
            text.contains("run(turn) -> Result"),
            "methods in class detail: {text}"
        );
        assert!(text.contains("Worker ──▷ Runner  (inherits)"), "{text}");
    }

    #[test]
    fn entities_mermaid_er_and_class() {
        let m = entities_mermaid(&props!(ENTITIES, Entities));
        assert!(
            m.starts_with("erDiagram\n  user {\n    uuid id PK\n"),
            "{m}"
        );
        assert!(m.contains("  user ||--o{ session : \"one-many\"\n"), "{m}");
        assert!(m.contains("  session ||--o{ turn : \"has\"\n"), "{m}");
        let c = entities_mermaid(&props!(CLASSES, Entities));
        assert!(c.starts_with("classDiagram\n"), "{c}");
        assert!(c.contains("    <<interface>>\n"), "{c}");
        assert!(c.contains("  worker --|> runner : inherits\n"), "{c}");
    }

    #[test]
    fn swimlane_headers_steps_and_crossings() {
        let p = props!(SWIM, Swimlane);
        let text = plain_text(&swimlane_lines(&p, 0));
        assert_eq!(
            text,
            "Pull request flow\n\
             Developer\n  ▶ Push branch\n  ✗ Fix\n\
             CI\n  • Build + test (just check) → Green?\n  ◇ Green?\n\
             Reviewer\n  ◉ Review → Merge\n  ■ Merge\n\
             \n\
             Push branch → Build + test\n\
             Green? ⇢ Fix  no\n\
             Fix → Build + test\n\
             Green? → Review  yes"
        );
        let tb = props!(SWIM_TB, Swimlane);
        assert!(
            plain_text(&swimlane_lines(&tb, 40)).contains("▱ "),
            "data step mark"
        );
    }

    #[test]
    fn graph_delta_ledger_is_derived_by_id() {
        let p = props!(DELTA, GraphDelta);
        assert_eq!(
            ledger(&p),
            Ledger {
                added: vec!["cache".into()],
                removed: vec![],
                changed: vec![],
            }
        );
        let text = plain_text(&graph_delta_lines(&p, 0));
        assert!(
            text.starts_with("Add a cache\nBefore: 3 nodes · After: 4 nodes\n+ Redis\n\nData\n"),
            "{text}"
        );
        let g = props!(DELTA_GROUPS, GraphDelta);
        let l = ledger(&g);
        assert_eq!(l.removed, vec!["legacy".to_owned()]);
        assert_eq!(
            l.changed,
            vec![
                ("db".to_owned(), "kind, group".to_owned()),
                ("api".to_owned(), "label".to_owned())
            ]
        );
        let text = plain_text(&graph_delta_lines(&g, 0));
        assert!(
            text.contains("− Legacy worker\n~ Postgres (kind, group)\n~ API v2 (label)\n"),
            "{text}"
        );
    }
}
