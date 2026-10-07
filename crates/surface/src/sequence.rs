//! `Sequence` on text surfaces (design doc 71 §2.4): the numbered step list the TUI and the
//! text channels show — every step keeps its `ref`, which is the point of the component — and
//! the `sequenceDiagram` lowering for channels that carry mermaid (`ref`s dropped).

use std::collections::BTreeMap;

use markdown::Line;
use unicode_width::UnicodeWidthStr;

use crate::graph::mermaid_ids;
use crate::render::{BOLD, DIM, PLAIN, span};
use crate::spec::{GraphState, SequenceProps, SequenceStyle};

/// A participant label or message text on one mermaid line: no line breaks, and `;` (a
/// statement separator in sequence diagrams) as `,`.
fn one_line(text: &str) -> String {
    text.split(['\n', '\r'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
        .replace(';', ",")
}

/// The sequence as `sequenceDiagram` source: one `participant` per actor, one message per
/// step — `->>` call, `-->>` return, `->` async (mermaid's `-)` open head is not drawn by merman
/// 0.7, and a solid line with no head is the nearest mark). `ref` has no mermaid form and is dropped.
pub fn to_mermaid(p: &SequenceProps) -> String {
    let ids = mermaid_ids(p.actors.iter().map(|a| a.id.as_str()));
    let id = |raw: &str| ids.get(raw).map(String::as_str).unwrap_or("_");
    let mut out = String::from("sequenceDiagram\n");
    if let Some(t) = p.title.as_deref().filter(|t| !t.trim().is_empty()) {
        out.push_str(&format!("  title {}\n", one_line(t)));
    }
    for a in &p.actors {
        out.push_str(&format!(
            "  participant {} as {}\n",
            id(&a.id),
            one_line(&a.label)
        ));
    }
    for s in &p.steps {
        let arrow = match s.style.unwrap_or(SequenceStyle::Call) {
            SequenceStyle::Call => "->>",
            SequenceStyle::Return => "-->>",
            SequenceStyle::Async => "->",
        };
        out.push_str(&format!(
            "  {}{arrow}{}: {}\n",
            id(&s.from),
            id(&s.to),
            one_line(&s.label)
        ));
    }
    out
}

/// The arrow between two actor labels: `→` call, `⇢` return (dashed), `⇾` async (open head).
fn arrow(style: Option<SequenceStyle>) -> &'static str {
    match style.unwrap_or(SequenceStyle::Call) {
        SequenceStyle::Call => "→",
        SequenceStyle::Return => "⇢",
        SequenceStyle::Async => "⇾",
    }
}

/// The sequence as text: the bold title, then one numbered line per step —
/// `1. parser → validator  check(spec)   crates/surface/src/validate.rs:12-40` — the actor
/// labels joined by the style's arrow, two spaces, the step label, then the dim `ref`. When the
/// `ref` would push the line past `width` it moves to its own indented line. 123 §4.4 (W3):
/// `[alt: label]` … `[else]` … `[end]` dim lines frame the steps of a fragment (the framed
/// steps indent two columns), a step's `note` follows it as an indented `· note`, and a
/// non-default `state` puts its glyph before the label (`✗` danger, `● ` done, `◉` active,
/// `○` muted).
pub fn steps_lines(p: &SequenceProps, width: usize) -> Vec<Line> {
    let label_of: BTreeMap<&str, &str> = p
        .actors
        .iter()
        .map(|a| (a.id.as_str(), a.label.as_str()))
        .collect();
    let name = |id: &'_ str| -> String { label_of.get(id).copied().unwrap_or(id).to_owned() };
    let fragments = p.fragments.as_deref().unwrap_or_default();
    let mut out = Vec::new();
    if let Some(t) = &p.title {
        out.push(vec![span(t, BOLD)]);
    }
    let digits = p.steps.len().to_string().len();
    for (i, s) in p.steps.iter().enumerate() {
        for f in fragments {
            if f.from == i {
                let kind = format!("{:?}", f.kind).to_lowercase();
                let text = match f.label.as_deref().filter(|l| !l.is_empty()) {
                    Some(l) => format!("[{kind}: {l}]"),
                    None => format!("[{kind}]"),
                };
                out.push(vec![span(text, DIM)]);
            }
            if f.r#else == Some(i) && f.from < i && i <= f.to {
                out.push(vec![span("[else]", DIM)]);
            }
        }
        let framed = fragments.iter().any(|f| f.from <= i && i <= f.to);
        let indent = if framed { "  " } else { "" };
        let head = format!("{indent}{:>digits$}. ", i + 1);
        let glyph = match s.state {
            Some(GraphState::Danger) => "✗ ",
            Some(GraphState::Done) => "● ",
            Some(GraphState::Active) => "◉ ",
            Some(GraphState::Muted) => "○ ",
            _ => "",
        };
        let mut line = vec![
            span(&head, DIM),
            span(
                format!(
                    "{} {} {}  {glyph}{}",
                    name(&s.from),
                    arrow(s.style),
                    name(&s.to),
                    s.label
                ),
                PLAIN,
            ),
        ];
        match s.r#ref.as_deref().filter(|r| !r.is_empty()) {
            None => out.push(line),
            Some(r) => {
                let used: usize = line.iter().map(|sp| sp.text.width()).sum();
                if width == 0 || used + 3 + r.width() <= width {
                    line.push(span(format!("   {r}"), DIM));
                    out.push(line);
                } else {
                    out.push(line);
                    out.push(vec![span(format!("{}{r}", " ".repeat(head.width())), DIM)]);
                }
            }
        }
        if let Some(n) = s.note.as_deref().filter(|n| !n.is_empty()) {
            out.push(vec![span(
                format!("{}· {n}", " ".repeat(head.width())),
                DIM,
            )]);
        }
        if fragments.iter().any(|f| f.to == i) {
            out.push(vec![span("[end]", DIM)]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render::plain_text;
    use crate::spec::{SequenceActor, SequenceStep};
    use crate::{Component, mermaid_lines, parse};

    const VALID: &str = include_str!("../fixtures/json-render/valid-sequence.json");

    fn sequence(body: &str) -> SequenceProps {
        let spec = parse(body).expect("valid fixture").spec;
        spec.elements
            .values()
            .find_map(|e| match &e.component {
                Component::Sequence(s) => Some(s.clone()),
                _ => None,
            })
            .expect("a Sequence element")
    }

    fn actor(id: &str, label: &str) -> SequenceActor {
        SequenceActor {
            id: id.into(),
            label: label.into(),
        }
    }

    fn step(
        from: &str,
        to: &str,
        label: &str,
        style: Option<SequenceStyle>,
        r: Option<&str>,
    ) -> SequenceStep {
        SequenceStep {
            from: from.into(),
            to: to.into(),
            label: label.into(),
            style,
            r#ref: r.map(Into::into),
            note: None,
            state: None,
        }
    }

    /// The lowering is the fixture oracle: the valid fixture draws under merman.
    #[test]
    fn fixture_lowers_to_mermaid_that_merman_draws() {
        let src = to_mermaid(&sequence(VALID));
        assert!(src.starts_with("sequenceDiagram\n  title "), "{src}");
        assert!(!src.contains("crates/"), "refs are dropped:\n{src}");
        let lines = mermaid_lines(&src, 0)
            .unwrap_or_else(|e| panic!("{e}\n{src}"))
            .unwrap_or_else(|| panic!("merman declined\n{src}"));
        assert!(!lines.is_empty());
    }

    #[test]
    fn lowering_ids_arrows_and_one_line_labels() {
        let p = SequenceProps {
            title: Some("Round;\ntrip".into()),
            actors: vec![actor("end", "Caller"), actor("a-b", "Store; db")],
            steps: vec![
                step("end", "a-b", "put(k, v)", None, Some("x.rs#L1")),
                step("a-b", "end", "ok", Some(SequenceStyle::Return), None),
                step("end", "end", "flush", Some(SequenceStyle::Async), None),
            ],
            fragments: None,
        };
        assert_eq!(
            to_mermaid(&p),
            "sequenceDiagram\n  title Round, trip\n  participant end_ as Caller\n  participant a_b as Store, db\n  end_->>a_b: put(k, v)\n  a_b-->>end_: ok\n  end_->end_: flush\n"
        );
        assert!(mermaid_lines(&to_mermaid(&p), 0).expect("parses").is_some());
    }

    #[test]
    fn step_list_numbers_arrows_and_wraps_the_ref() {
        let p = SequenceProps {
            title: None,
            actors: vec![actor("p", "parser"), actor("v", "validator")],
            steps: vec![
                step(
                    "p",
                    "v",
                    "check(spec)",
                    None,
                    Some("crates/surface/src/validate.rs:12-40"),
                ),
                step("v", "p", "errors", Some(SequenceStyle::Return), None),
                step("p", "p", "repair", Some(SequenceStyle::Async), Some("a.rs")),
            ],
            fragments: None,
        };
        assert_eq!(
            plain_text(&steps_lines(&p, 0)),
            "1. parser → validator  check(spec)   crates/surface/src/validate.rs:12-40\n2. validator ⇢ parser  errors\n3. parser ⇾ parser  repair   a.rs"
        );
        let narrow = steps_lines(&p, 50);
        assert_eq!(
            plain_text(&narrow),
            "1. parser → validator  check(spec)\n   crates/surface/src/validate.rs:12-40\n2. validator ⇢ parser  errors\n3. parser ⇾ parser  repair   a.rs"
        );
        assert!(narrow[1][0].style.dim && !narrow[0][1].style.dim);
    }

    /// 123 §4.4 (W3): fragments frame their steps, notes indent under theirs, states glyph.
    #[test]
    fn fragments_notes_and_states_lower_around_the_steps() {
        const FRAGMENTS: &str =
            include_str!("../fixtures/json-render/valid-sequence-fragments.json");
        let lines = steps_lines(&sequence(FRAGMENTS), 0);
        let text = plain_text(&lines);
        assert!(
            text.contains("check(text)   crates/surface/src/lib.rs#L154\n[alt: errors]\n  3. surface ⇢ Loop  ✗ located errors\n"),
            "{text}"
        );
        assert!(text.contains("\n     · path + message\n  4. Loop → Model  <surface_error>\n[else]\n  5. Model → Loop  ● corrected block\n[end]\n[loop: ≤ 3 rounds]\n  6. Loop ⇾ Loop  give up after 3\n     · final=true\n[end]"), "{text}");
        assert!(
            text.contains("\n1. Model → Loop  reply with fence\n2. Loop →"),
            "unframed steps stay flush:\n{text}"
        );
        assert!(
            lines
                .iter()
                .flatten()
                .filter(|s| s.text.starts_with('['))
                .all(|s| s.style.dim)
        );
    }
}
