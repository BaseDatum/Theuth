//! Harvest instruction, reply parser, and hint rendering (design doc 10 §4.1, §5.3, §5.5).
//! Pure functions, no I/O.

use super::HINT_DESCRIPTION_CHARS;
use super::types::Meta;

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
type: fact            # fact | entity | decision | procedure | project
title: one line
description: one line, what a reader needs to decide whether to open this
tags: [a, b]
scope: {cwd}          # only when the fact is specific to this directory or project; omit otherwise
---
The knowledge itself, with the evidence that established it (the command, path, error text,
or the user's words). Link related concepts as [title](/other/id.md).
</concept>

Ids are lowercase `a-z0-9-` segments, directory/slug. Prefer updating an existing concept over
adding one. If nothing qualifies, reply `none`.";

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
/// clipped at `HINT_DESCRIPTION_CHARS`.
pub fn render_hints(metas: &[Meta]) -> String {
    let mut s =
        String::from("Concepts that may be relevant; call knowledge_get before relying on one:\n");
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
    if cid.segments().len() > 3 {
        return Err("more than three segments".into());
    }
    if matches!(cid.name(), "index" | "log") {
        return Err("reserved file name".into());
    }
    let mut doc = okf::Document::parse(body).map_err(|e| e.to_string())?;
    doc.validate_conformance().map_err(|e| e.to_string())?;
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
        let s = render_hints(&[meta("facts/long", &long), meta("facts/empty", "")]);
        let mut lines = s.lines();
        assert_eq!(
            lines.next().unwrap(),
            "Concepts that may be relevant; call knowledge_get before relying on one:"
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
