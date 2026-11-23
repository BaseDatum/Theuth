//! Harvest instruction, reply parser, and hint rendering (design doc 10 §4.1, §5.3, §5.5).
//! Pure functions, no I/O.

use crate::HINT_DESCRIPTION_CHARS;
use crate::types::Meta;

/// The one user message appended to the finished frame (10 §5.3), with
/// `{related}` and `{cwd}` placeholders for [`instruction`].
pub const HARVEST_INSTRUCTION: &str = "\
Review this conversation for knowledge worth keeping across sessions: facts about the user's
environment (hosts, paths, toolchains, accounts, conventions), decisions and the reasons behind
them, procedures that worked, and entities (projects, services, people) and how they relate.
Not: transient task state, todo progress, file contents that live in a repository, or anything
already recorded below unless it changed.

Existing concepts that may already cover this — update by reusing the id; never create a
near-duplicate:
{related}

Reply with text only — do not call tools. Emit zero or more documents, each exactly:

<concept id=\"facts/example-slug\">
---
type: fact            # fact | entity | decision | procedure | project | record | warning
title: one line, at most 120 characters (the first clause; the statement goes in the body)
description: one line, what a reader needs to decide whether to open this
tags: [a, b]
scope: {cwd}          # only when the fact is specific to this directory or project; omit otherwise
---
The knowledge itself, with the evidence that established it (the command, path, error text,
or the user's words). Link related concepts as [title](/other/id.md).
</concept>

Ids are lowercase `a-z0-9-` segments, directory/slug. `record` is what happened on a date (a landing,
a machine's state that morning, an incident); `warning` is a one-line never/always whose title is the
whole payload. Prefer updating an existing concept over adding one. If nothing qualifies, reply `none`.";

/// Longest title a harvest put may carry (136 §5.1).
pub const TITLE_MAX: usize = 120;

/// Enforce [`TITLE_MAX`] on a parsed concept (136 §15 "07"): a title within
/// the cap is returned with `body` unchanged; a longer one is cut at its
/// first clause boundary (the last `. `, `; `, `: ` or ` — ` inside the cap;
/// else the last space, with `…`) and the rest becomes a leading
/// `## Summary` paragraph — the shape [`crate::Knowledge::retitle`] writes —
/// so nothing is lost and the hint shelf gets a readable line.
pub fn split_title(title: &str, body: &str) -> (String, String) {
    let title = title.trim();
    if title.chars().count() <= TITLE_MAX {
        return (title.to_owned(), body.to_owned());
    }
    let cap = title
        .char_indices()
        .nth(TITLE_MAX)
        .map_or(title.len(), |(i, _)| i);
    let head = &title[..cap];
    let clause = [". ", "; ", ": ", " — ", " – "]
        .iter()
        .filter_map(|sep| head.rfind(*sep).map(|at| (at, sep.len())))
        .max_by_key(|(at, _)| *at);
    let (short, rest) = match clause {
        Some((at, len)) => (
            title[..at].trim_end_matches(['.', ';', ':']).to_owned(),
            title[at + len..].trim().to_owned(),
        ),
        // No clause boundary: a fragment in the body would read wrong, so
        // the whole statement is the summary. No space either: cut one
        // char short so the `…` keeps the title within the cap (136 R-45).
        None => {
            let at = head.rfind(' ').unwrap_or_else(|| {
                head.char_indices()
                    .nth(TITLE_MAX - 1)
                    .map_or(head.len(), |(i, _)| i)
            });
            (format!("{}…", title[..at].trim_end()), title.to_owned())
        }
    };
    let body = body.trim_start();
    let body = if body.is_empty() {
        format!("## Summary\n\n{rest}\n")
    } else {
        format!("## Summary\n\n{rest}\n\n{body}")
    };
    (short, body)
}

/// `HARVEST_INSTRUCTION` with `{related}` = [`render_related`] of `related`
/// (or `none`) and `{cwd}` substituted.
pub fn instruction(related: &[Meta], cwd: &str) -> String {
    // cwd first: a related description could itself contain `{cwd}`.
    HARVEST_INSTRUCTION
        .replace("{cwd}", cwd)
        .replace("{related}", &render_related(related))
}

/// Body of a `<knowledge>` note (10 §4.1): the lead sentence plus one
/// `` - `id` — title [type]: description `` line per meta, description
/// clipped at `HINT_DESCRIPTION_CHARS`. `peek` (`knowledge.peek`, 212 §15): the lead
/// sentence offers `peek_memory`; off, it names `knowledge_get` only.
pub fn render_hints(metas: &[Meta], peek: bool) -> String {
    let mut s = String::from(if peek {
        "Concepts that may be relevant; not sure one helps? `await peek_memory(question=\"…\", ids=[…])` checks them against your question locally and returns only those that answer it; `await knowledge_get(id)` reads one outright (python):\n"
    } else {
        "Concepts that may be relevant; read one with `await knowledge_get(id)` in python before relying on it:\n"
    });
    for m in metas {
        line(&mut s, m);
    }
    s
}

/// The `{related}` block: hint lines without the lead sentence, or `none`.
pub fn render_related(metas: &[Meta]) -> String {
    if metas.is_empty() {
        return "none".into();
    }
    let mut s = String::new();
    for m in metas {
        line(&mut s, m);
    }
    s
}

fn line(s: &mut String, m: &Meta) {
    s.push_str(&format!("- `{}` — {} [{}]", m.id, m.title, m.type_));
    let d = m.description.trim();
    if !d.is_empty() {
        s.push_str(": ");
        match d.char_indices().nth(HINT_DESCRIPTION_CHARS) {
            Some((cut, _)) => {
                s.push_str(&d[..cut]);
                s.push('…');
            }
            None => s.push_str(d),
        }
    }
    s.push('\n');
}

/// Extract `<concept id="…">…</concept>` blocks (10 §5.5): lowercase the id,
/// parse the document, drop `timestamp`/`source` the model wrote. Invalid
/// blocks are skipped with a `warn!`; a `none` reply yields nothing.
pub fn parse_harvest(text: &str) -> Vec<(String, okf::Document)> {
    const OPEN: &str = "<concept id=\"";
    const CLOSE: &str = "</concept>";
    let mut out = Vec::new();
    if text.trim().eq_ignore_ascii_case("none") {
        return out;
    }
    let mut rest = text;
    while let Some(at) = rest.find(OPEN) {
        let after = &rest[at + OPEN.len()..];
        let Some(id_end) = after.find('"') else { break };
        let id = after[..id_end].trim().to_ascii_lowercase();
        let after = &after[id_end + 1..];
        let Some(tag_end) = after.find('>') else {
            break;
        };
        let after = &after[tag_end + 1..];
        let Some(body_end) = after.find(CLOSE) else {
            break;
        };
        let body = after[..body_end].trim();
        rest = &after[body_end + CLOSE.len()..];
        match document(&id, body) {
            Ok(doc) => out.push((id, doc)),
            Err(reason) => tracing::warn!(id, reason, "harvest: skipped concept"),
        }
    }
    out
}

fn document(id: &str, body: &str) -> Result<okf::Document, String> {
    let cid = okf::ConceptId::parse(id).map_err(|e| e.to_string())?;
    if !cid
        .segments()
        .iter()
        .all(|s| okf::concept_id::is_portable_segment(s))
    {
        return Err("segment outside [a-z0-9_][a-z0-9_.-]*".into());
    }
    if cid.segments().len() > 3 {
        return Err("more than three segments".into());
    }
    if matches!(cid.name(), "index" | "log") {
        return Err("reserved file name".into());
    }
    let mut doc = okf::Document::parse(body).map_err(|e| e.to_string())?;
    doc.validate().map_err(|e| e.to_string())?;
    let fm = doc.frontmatter.as_mapping_mut();
    fm.remove("timestamp");
    fm.remove("source");
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(id: &str, description: &str) -> Meta {
        Meta {
            id: id.into(),
            type_: "fact".into(),
            title: "T".into(),
            description: description.into(),
            tags: vec![],
            scope: None,
            source: None,
            timestamp: None,
            owner: crate::types::USER_OWNER.into(),
            visibility: Default::default(),
            status: Default::default(),
            hint: true,
            evidence: vec![],
            verified: None,
            valid_until: None,
            superseded_by: None,
            merged_into: None,
            consolidated_at: None,
        }
    }

    #[test]
    fn short_titles_pass_and_long_ones_split_at_a_clause() {
        let (t, b) = split_title("Short title", "body");
        assert_eq!((t.as_str(), b.as_str()), ("Short title", "body"));

        let long = format!(
            "The build lock serialises cargo across waves. {}",
            "Every worker wraps each cargo call so that two targets never compile at once and the laptop stays usable. ".repeat(4)
        );
        assert!(long.chars().count() > 400);
        let (t, b) = split_title(&long, "Body text.");
        assert_eq!(t, "The build lock serialises cargo across waves");
        assert!(t.chars().count() <= TITLE_MAX);
        assert!(b.starts_with("## Summary\n\nEvery worker wraps each cargo call"));
        assert!(b.ends_with("\n\nBody text."));
        // Nothing lost: every word of the title is in title + body (the
        // clause separator itself is the one character dropped).
        for w in long.split_whitespace().map(|w| w.trim_end_matches('.')) {
            assert!(t.contains(w) || b.contains(w), "{w}");
        }
    }

    #[test]
    fn no_clause_boundary_cuts_at_a_space_and_keeps_the_whole_statement() {
        let long = "word ".repeat(60);
        let (t, b) = split_title(&long, "");
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= TITLE_MAX);
        assert_eq!(b, format!("## Summary\n\n{}\n", long.trim()));
    }

    /// 136 R-45: a title with no space at all (a URL, a hash, a CJK run) is
    /// still within the cap after the `…`.
    #[test]
    fn no_space_at_all_stays_within_the_cap() {
        for long in ["x".repeat(300), "日".repeat(300)] {
            let (t, b) = split_title(&long, "");
            assert!(t.ends_with('…'));
            assert_eq!(t.chars().count(), TITLE_MAX, "{t}");
            assert_eq!(b, format!("## Summary\n\n{long}\n"));
        }
    }

    #[test]
    fn parses_two_blocks_lowercases_and_strips_harness_keys() {
        let text = "Some preamble.\n<concept id=\"Facts/A\">\n---\ntype: fact\ntitle: a\ntimestamp: 2020-01-01T00:00:00Z\nsource: model\n---\nbody a\n</concept>\n\
                    <concept id=\"procedures/b\">\n---\ntype: procedure\n---\nbody b\n</concept>\ntrailing";
        let docs = parse_harvest(text);
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].0, "facts/a");
        assert_eq!(docs[1].0, "procedures/b");
        assert_eq!(docs[0].1.frontmatter.timestamp(), None);
        assert!(docs[0].1.frontmatter.get("source").is_none());
        assert_eq!(docs[0].1.frontmatter.title().as_deref(), Some("a"));
        assert_eq!(docs[0].1.body.trim(), "body a");
    }

    #[test]
    fn skips_invalid_blocks_keeps_valid() {
        let text = "<concept id=\"bad id!\">\n---\ntype: fact\n---\nx\n</concept>\
                    <concept id=\"a/b/c/d\">\n---\ntype: fact\n---\nx\n</concept>\
                    <concept id=\"facts/index\">\n---\ntype: fact\n---\nx\n</concept>\
                    <concept id=\"facts/notype\">\n---\ntitle: t\n---\nx\n</concept>\
                    <concept id=\"facts/ok\">\n---\ntype: fact\n---\nx\n</concept>\
                    <concept id=\"facts/unclosed\">\n---\ntype: fact\n---\nx";
        let docs = parse_harvest(text);
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].0, "facts/ok");
    }

    #[test]
    fn none_and_empty_yield_nothing() {
        assert!(parse_harvest(" None\n").is_empty());
        assert!(parse_harvest("nothing to see here").is_empty());
    }

    #[test]
    fn hints_clip_description_and_omit_empty() {
        let long = "x".repeat(200);
        let s = render_hints(&[meta("facts/long", &long), meta("facts/empty", "")], true);
        let mut lines = s.lines();
        assert_eq!(
            lines.next().unwrap(),
            "Concepts that may be relevant; not sure one helps? `await peek_memory(question=\"…\", ids=[…])` checks them against your question locally and returns only those that answer it; `await knowledge_get(id)` reads one outright (python):"
        );
        let clipped = lines.next().unwrap();
        assert_eq!(
            clipped,
            format!(
                "- `facts/long` — T [fact]: {}…",
                "x".repeat(HINT_DESCRIPTION_CHARS)
            )
        );
        assert_eq!(lines.next().unwrap(), "- `facts/empty` — T [fact]");
        assert_eq!(lines.next(), None);
        assert_eq!(render_related(&[]), "none");
    }

    /// 212 §15: with `knowledge.peek` off the lead sentence is the pre-212 one, no `peek_memory`.
    #[test]
    fn hints_without_peek_name_knowledge_get_only() {
        let s = render_hints(&[meta("facts/a", "d")], false);
        assert_eq!(
            s,
            "Concepts that may be relevant; read one with `await knowledge_get(id)` in python before relying on it:\n\
             - `facts/a` — T [fact]: d\n"
        );
    }

    #[test]
    fn instruction_substitutes_placeholders() {
        let s = instruction(&[meta("facts/a", "d")], "/tmp/w");
        assert!(!s.contains("{related}"));
        assert!(!s.contains("{cwd}"));
        assert!(s.contains("- `facts/a` — T [fact]: d\n"));
        assert!(s.contains("scope: /tmp/w "));
        assert!(instruction(&[], "/x").contains("near-duplicate:\nnone\n"));
    }
}
