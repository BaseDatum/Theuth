//! 201 §7.3 (P3): the per-kind draft line of a unit too long to be its own
//! line, and whether a model may improve on it (D8). Pure and deterministic;
//! every line is tagged with its kind and at most `node_bytes` long.

use super::guards::{cut_utf8, cut_words};
use super::tree::{Kind, Unit, mechanical_unit};

/// An echo over this many bytes that is not an error is worth a model line.
pub const ECHO_MODEL_OVER: usize = 2_048;

/// The draft line of `u` (only called for a unit whose tagged text does not
/// fit `node_bytes`).
pub fn draft_line(u: &Unit, node_bytes: u32) -> String {
    let max = node_bytes as usize;
    let line = match u.kind {
        Kind::Tool => tool_line(&u.text, max),
        Kind::Echo => echo_line(&u.text, max),
        Kind::User | Kind::Note => verbatim_line(u.kind, &u.text, max),
        Kind::Talk | Kind::Work => mechanical_unit(u, node_bytes).unwrap_or_else(|| u.tagged()),
    };
    cut_words(&line, max)
}

/// D8: whether the pump ever asks a model for `u`'s line. `tool`, `user`,
/// `note` never (the user's words are never paraphrased); `echo` only when
/// over [`ECHO_MODEL_OVER`] bytes and not an error; `talk` and `work` always.
pub fn needs_model(u: &Unit) -> bool {
    match u.kind {
        Kind::Talk | Kind::Work => true,
        Kind::Echo => u.text.len() > ECHO_MODEL_OVER && error_lines(&u.text).is_empty(),
        Kind::Tool | Kind::User | Kind::Note => false,
    }
}

/// One line of `s`: whitespace runs (newlines too) become one space.
fn flat(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// `tool: <name> <key>=<value>, …` from the unit text `name {json args}`;
/// each value cut to 60 bytes, then the whole line to `max`.
fn tool_line(text: &str, max: usize) -> String {
    const VALUE: usize = 60;
    let (name, args) = text.split_once(' ').unwrap_or((text, ""));
    let parts: Vec<String> = match serde_json::from_str::<serde_json::Value>(args) {
        Ok(serde_json::Value::Object(map)) => map
            .iter()
            .map(|(k, v)| {
                let v = match v {
                    serde_json::Value::String(s) => flat(s),
                    other => other.to_string(),
                };
                format!("{k}={}", cut_words(&v, VALUE))
            })
            .collect(),
        Ok(serde_json::Value::Null) => Vec::new(),
        Ok(other) => vec![cut_words(&other.to_string(), VALUE)],
        Err(_) if args.is_empty() => Vec::new(),
        Err(_) => vec![cut_words(&flat(args), VALUE)],
    };
    let mut line = format!("tool: {name}");
    for (k, p) in parts.iter().enumerate() {
        let sep = if k == 0 { " " } else { ", " };
        if line.len() + sep.len() + p.len() > max {
            break;
        }
        line.push_str(sep);
        line.push_str(p);
    }
    line
}

/// A line of tool output that reports a failure.
fn is_error(line: &str) -> bool {
    let l = line.to_ascii_lowercase();
    [
        "error",
        "panicked",
        "failed",
        "fatal",
        "exception",
        "traceback",
    ]
    .iter()
    .any(|w| l.contains(w))
}

fn error_lines(text: &str) -> Vec<&str> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && is_error(l))
        .collect()
}

/// `echo: <first> | <error lines> | <last> (N lines, M errors)`; each part
/// cut so the counts always fit.
fn echo_line(text: &str, max: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    let errors = error_lines(text);
    let counts = format!(" ({} lines, {} errors)", lines.len(), errors.len());
    let room = max.saturating_sub("echo: ".len() + counts.len());
    let part = (room / 3).max(16);
    let mut body: Vec<String> = Vec::new();
    let mut used = 0usize;
    let add = |s: &str, body: &mut Vec<String>, used: &mut usize| {
        let s = cut_words(&flat(s), part);
        let cost = s.len() + if body.is_empty() { 0 } else { 3 };
        if *used + cost <= room {
            *used += cost;
            body.push(s);
            true
        } else {
            false
        }
    };
    if let Some(first) = lines.first() {
        add(first, &mut body, &mut used);
    }
    let last = (lines.len() > 1).then(|| lines[lines.len() - 1]);
    // Keep room for the last line before the errors take it.
    let reserve = last.map_or(0, |l| cut_words(&flat(l), part).len() + 3);
    used += reserve;
    for e in errors
        .iter()
        .filter(|e| Some(**e) != lines.first().copied() && Some(**e) != last)
    {
        if !add(e, &mut body, &mut used) {
            break;
        }
    }
    used -= reserve;
    if let Some(last) = last {
        add(last, &mut body, &mut used);
    }
    format!("echo: {}{counts}", body.join(" | "))
}

/// The unit's own words, one line, cut at sentence boundaries: a short head
/// and as much of the end as fits (the request usually comes last).
fn verbatim_line(kind: Kind, text: &str, max: usize) -> String {
    const GAP: &str = " … ";
    let tag = format!("{}: ", kind.as_str());
    let text = flat(text);
    if tag.len() + text.len() <= max {
        return format!("{tag}{text}");
    }
    let room = max.saturating_sub(tag.len() + GAP.len());
    // Head: up to a third, ending at a sentence end when one falls in it.
    let head = cut_utf8(&text, room / 3);
    let head = match sentence_end(head) {
        Some(p) if p >= head.len() / 3 => &head[..p],
        _ => head.rfind(' ').map_or(head, |p| &head[..p]),
    };
    let tail_room = room.saturating_sub(head.len());
    let mut from = text.len().saturating_sub(tail_room);
    while !text.is_char_boundary(from) {
        from += 1;
    }
    let window = &text[from..];
    // Tail: starts at a sentence start when one falls in its first half.
    let start = sentence_start(window)
        .filter(|p| *p <= window.len() / 2)
        .or_else(|| window.find(' ').map(|p| p + 1))
        .unwrap_or(0);
    format!("{tag}{}{GAP}{}", head.trim_end(), &window[start..])
}

/// Byte just past the last `. ` / `! ` / `? ` in `s` (the sentence end).
fn sentence_end(s: &str) -> Option<usize> {
    s.match_indices(['.', '!', '?'])
        .filter(|(p, _)| s[p + 1..].starts_with(' ') || p + 1 == s.len())
        .map(|(p, _)| p + 1)
        .next_back()
}

/// Byte of the first sentence start in `s` (after `. ` / `! ` / `? `).
fn sentence_start(s: &str) -> Option<usize> {
    s.match_indices(". ")
        .chain(s.match_indices("! "))
        .chain(s.match_indices("? "))
        .map(|(p, _)| p + 2)
        .min()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(kind: Kind, text: &str) -> Unit {
        Unit {
            i: 0,
            seq: 0,
            part: 0,
            kind,
            text: text.to_owned(),
            size: (kind.as_str().len() + 2 + text.len()) as u32,
            at: None,
        }
    }

    const NB: u32 = 200;

    fn line(kind: Kind, text: &str) -> String {
        let u = unit(kind, text);
        let l = draft_line(&u, NB);
        assert!(l.len() <= NB as usize, "{kind:?} line over node_bytes: {l}");
        assert_eq!(l, draft_line(&u, NB), "deterministic");
        assert!(
            l.starts_with(&format!("{}: ", kind.as_str())),
            "tagged: {l}"
        );
        assert!(!l.contains('\n'), "one line: {l}");
        l
    }

    #[test]
    fn tool_line_names_the_call_and_its_key_arguments() {
        let big = "x".repeat(5_000);
        let l = line(
            Kind::Tool,
            &format!(r#"edit {{"path":"src/main.rs","old":"{big}","new":"fn main() {{}}"}}"#),
        );
        assert!(l.starts_with("tool: edit "), "{l}");
        assert!(l.contains("path=src/main.rs"), "{l}");
        assert!(!l.contains(&"x".repeat(100)), "long value cut: {l}");
        assert!(!needs_model(&unit(Kind::Tool, "edit {}")));
    }

    #[test]
    fn echo_line_keeps_first_errors_counts_and_last() {
        let mut text = String::from("Compiling agent v0.1.0\n");
        for k in 0..400 {
            text.push_str(&format!("   checking module number {k} of the crate\n"));
        }
        text.push_str("error[E0308]: mismatched types in pump.rs\n");
        text.push_str(
            "warning: unused import\nerror: could not compile `agent`\nFinished in 3.2s\n",
        );
        let l = line(Kind::Echo, &text);
        assert!(l.starts_with("echo: Compiling agent"), "{l}");
        assert!(l.contains("error[E0308]"), "{l}");
        assert!(l.contains("Finished in 3.2s"), "{l}");
        assert!(l.ends_with("(404 lines, 2 errors)"), "{l}");
        assert!(
            !needs_model(&unit(Kind::Echo, &text)),
            "an error echo is final"
        );
        let quiet = "all good\n".repeat(400);
        assert!(needs_model(&unit(Kind::Echo, &quiet)), "long, no error");
        assert!(
            !needs_model(&unit(Kind::Echo, &"ok\n".repeat(100))),
            "short"
        );
    }

    #[test]
    fn user_and_note_lines_are_verbatim_with_the_end_kept() {
        let mut text = String::from("Here is some background on the project. ");
        text.push_str(&"It has many parts that matter less. ".repeat(40));
        text.push_str("Please fix the retry delay in pump.rs now.");
        for kind in [Kind::User, Kind::Note] {
            let l = line(kind, &text);
            assert!(l.contains("Here is some background"), "{l}");
            assert!(
                l.ends_with("Please fix the retry delay in pump.rs now."),
                "{l}"
            );
            assert!(!needs_model(&unit(kind, &text)));
        }
    }

    #[test]
    fn talk_and_work_lines_are_the_mechanical_cut() {
        let text = "word ".repeat(400);
        for kind in [Kind::Talk, Kind::Work] {
            let u = unit(kind, &text);
            assert_eq!(line(kind, &text), mechanical_unit(&u, NB).unwrap());
            assert!(needs_model(&u));
        }
    }
}
