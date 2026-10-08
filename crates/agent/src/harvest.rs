//! The 136 amendments to the per-turn harvest (design doc 136 §0.14, §15
//! "07"): titles ≤ 120 chars with the statement in the body, `record` and
//! `warning` types, `evidence: [this session]` and `status: fresh` on every
//! put. The instruction and the parser themselves live in
//! `knowledge::harvest` (W1's crate); this module appends to the one and
//! shapes what the other returns, so the agent owns the whole change.

use knowledge::types::Meta;

/// Longest title a harvest put may carry (136 §5.1).
pub const TITLE_MAX: usize = 120;

/// Appended to `knowledge::harvest::instruction` (136 §15 "07"); the
/// `type:` line of the template above it still lists the five old types.
pub const AMENDMENT: &str = "\
Titles are at most 120 characters: the first clause only; the statement itself goes in the body.
`type` may also be `record` (what happened on a date — a landing, a machine's state that morning,
an incident) or `warning` (a one-line never/always whose title is the whole payload).";

/// The harvest instruction with the 136 amendment.
pub fn instruction(related: &[Meta], cwd: &str) -> String {
    format!(
        "{}\n\n{AMENDMENT}",
        knowledge::harvest::instruction(related, cwd)
    )
}

/// Enforce [`TITLE_MAX`] on a parsed concept: a title within the cap is
/// returned with `body` unchanged; a longer one is cut at its first clause
/// boundary (the last `. `, `; `, `: ` or ` — ` inside the cap; else the last
/// space, with `…`) and the rest becomes a `Summary:` paragraph before the
/// body — nothing is lost, the hint shelf gets a readable line.
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
        .filter_map(|sep| head.rfind(sep).map(|at| (at, sep.len())))
        .max_by_key(|(at, _)| *at);
    let (short, rest) = match clause {
        Some((at, len)) => (
            title[..at].trim_end_matches(['.', ';', ':']).to_owned(),
            title[at + len..].trim(),
        ),
        None => {
            let at = head.rfind(' ').unwrap_or(cap);
            (format!("{}…", title[..at].trim_end()), title)
        }
    };
    let body = body.trim_start();
    let body = if body.is_empty() {
        format!("Summary: {rest}\n")
    } else {
        format!("Summary: {rest}\n\n{body}")
    };
    (short, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instruction_keeps_the_first_line_and_appends_the_amendment() {
        let s = instruction(&[], "/tmp/x");
        assert!(s.starts_with(
            knowledge::harvest::HARVEST_INSTRUCTION
                .lines()
                .next()
                .unwrap()
        ));
        assert!(s.ends_with(AMENDMENT));
        assert!(s.contains("/tmp/x"));
        assert!(!s.contains("{cwd}") && !s.contains("{related}"));
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
        assert!(b.starts_with("Summary: Every worker wraps each cargo call"));
        assert!(b.ends_with("\n\nBody text."));
        // Nothing lost: every word of the title is in title + body.
        for w in long.split_whitespace() {
            assert!(t.contains(w) || b.contains(w), "{w}");
        }
    }

    #[test]
    fn no_clause_boundary_cuts_at_a_space_and_keeps_the_whole_statement() {
        let long = "word ".repeat(60);
        let (t, b) = split_title(&long, "");
        assert!(t.ends_with('…'));
        assert!(t.chars().count() <= TITLE_MAX);
        assert_eq!(b, format!("Summary: {}\n", long.trim()));
    }
}
