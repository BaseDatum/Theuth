//! User-cell fences of a markdown text and the channel "Run in theuth" line (design 171
//! §14.3). A cell fence is a closed fence whose language is runnable and whose body starts
//! with a `#|` header ([`crate::cell::parse`]); every other fence — snippets, 164's picture
//! fences — is left alone. Channels show the code and a link; they never run anything.

use std::borrow::Cow;
use std::ops::Range;

use crate::cell::{self, Cell};
use crate::fences::fences;

/// Every closed user-cell fence of `text` in order: the fence lines' range and the parsed
/// cell. The position in the list (1-based) is the cell's ordinal (§3.4).
pub fn cells(text: &str) -> Vec<(Range<usize>, Cell)> {
    fences(text)
        .filter(|f| f.closed)
        .filter_map(|f| cell::parse(f.lang, f.body).map(|c| (f.range, c)))
        .collect()
}

/// The streaming form of [`append_run_links`]: `delta` with a `Run in theuth:` line after
/// each cell fence whose closing line ends inside it, given everything already sent for
/// this message in `before` (links included — they never read as fences). Concatenating
/// the results of successive calls equals [`append_run_links`] of the whole message.
pub fn run_links<'a>(before: &str, delta: &'a str, session: &str) -> Cow<'a, str> {
    // A closing line completes only in a delta carrying its fence character.
    if !delta.contains(['`', '~']) {
        return Cow::Borrowed(delta);
    }
    let full = format!("{before}{delta}");
    let mut out = String::new();
    let mut at = 0;
    for (ordinal, (range, cell)) in cells(&full).into_iter().enumerate() {
        if range.end <= before.len() {
            continue;
        }
        let end = range.end - before.len();
        out.push_str(&delta[at..end]);
        at = end;
        let id = cell
            .options
            .name
            .unwrap_or_else(|| (ordinal + 1).to_string());
        out.push_str(&format!("\nRun in theuth: theuth://c/{session}?cell={id}"));
    }
    if at == 0 {
        return Cow::Borrowed(delta);
    }
    out.push_str(&delta[at..]);
    Cow::Owned(out)
}

/// `markdown` with one `Run in theuth: theuth://c/<session>?cell=<name or ordinal>` line
/// after each cell fence (§14.3).
pub fn append_run_links(markdown: &str, session: &str) -> String {
    run_links("", markdown, session).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    const S: &str = "0193c9a0-0000-7000-8000-000000000001";

    #[test]
    fn a_link_follows_each_cell_fence_only() {
        let md = "Two steps.\n\n```bash\n#| name: preview-ir-prod1\n#| cwd: /tmp\npulumi preview\n```\n\n\
```bash\necho snippet\n```\n\n```mermaid\ngraph LR\n```\n\n```python\n#| cwd: /tmp\nprint(1)\n```";
        let got = append_run_links(md, S);
        assert_eq!(
            got,
            format!(
                "Two steps.\n\n```bash\n#| name: preview-ir-prod1\n#| cwd: /tmp\npulumi preview\n```\n\
Run in theuth: theuth://c/{S}?cell=preview-ir-prod1\n\n```bash\necho snippet\n```\n\n```mermaid\ngraph LR\n```\n\n\
```python\n#| cwd: /tmp\nprint(1)\n```\nRun in theuth: theuth://c/{S}?cell=2"
            )
        );
    }

    #[test]
    fn unclosed_and_headerless_fences_are_untouched() {
        for md in [
            "```bash\n#| cwd: /tmp\nls\n",
            "```bash\nls\n```\n",
            "```text\n#| cwd: /tmp\n```\n",
        ] {
            assert_eq!(append_run_links(md, S), md);
        }
    }

    #[test]
    fn streaming_in_any_split_equals_the_whole() {
        let md =
            "a\n```bash\n#| name: one\n#| cwd: /tmp\nls\n```\nb\n```sh\n#| cwd: /x\npwd\n```\n";
        let whole = append_run_links(md, S);
        for step in 1..=md.len() {
            let mut sent = String::new();
            let mut i = 0;
            while i < md.len() {
                let j = (i + step).min(md.len());
                let add = run_links(&sent, &md[i..j], S).into_owned();
                sent.push_str(&add);
                i = j;
            }
            assert_eq!(sent, whole, "step {step}");
        }
    }
}
