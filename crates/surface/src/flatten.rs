//! Text-only channels (design doc 30 §9): rendered fences inside plain code blocks.

use crate::fences::fences;
use crate::render::plain_text;
use crate::{parse, spec_lines};

/// Replace every closed `json-render` fence that parses and fits `width` with its
/// rendering inside a plain ```` ``` ```` block; everything else — prose, Mermaid fences,
/// broken or unclosed fences — is byte-identical.
pub fn flatten(text: &str, width: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut pos = 0;
    for f in fences(text).filter(|f| f.closed && f.lang == "json-render") {
        let Ok(parsed) = parse(f.body) else {
            continue;
        };
        let Some(lines) = spec_lines(&parsed.spec, width) else {
            continue;
        };
        out.push_str(&text[pos..f.range.start]);
        out.push_str("```\n");
        out.push_str(&plain_text(&lines));
        out.push_str("\n```");
        pos = f.range.end;
    }
    out.push_str(&text[pos..]);
    out
}
