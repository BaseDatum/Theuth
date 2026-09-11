//! Snap compaction rasterizer (design doc 13 §2): serialise the folded span
//! to hard-wrapped lines, keep the last `budget` of them, draw them with the
//! embedded X.org misc `8x13.bdf` and encode one 8-bit grayscale PNG per
//! frame. Pure functions, no I/O.
//!
//! Font: `crates/agent/assets/8x13.bdf` from
//! <https://gitlab.freedesktop.org/xorg/font/misc-misc/-/raw/master/8x13.bdf>
//! (Markus Kuhn's ucs-fonts build, ISO10646-1, 3,703 glyphs; header:
//! `COPYRIGHT "Public domain font.  Share and enjoy."`). Every glyph is
//! `BBX 8 13 0 -2`, so the reader stores 13 rows per ENCODING and nothing
//! else. Codepoints without a glyph print `?`.

use std::collections::HashMap;
use std::sync::LazyLock;

use genai::chat::{ChatMessage, ChatRole, ContentPart};

const BDF: &str = include_str!("../assets/8x13.bdf");
const GLYPH_W: u32 = 8;
const GLYPH_H: usize = 13;

/// Tool-call arguments cap (13 §2.1).
const CALL_ARGS_CAP: usize = 200;
/// Tool-result head/tail kept around the omitted marker (13 §2.1; 11 §2.3 2k cap).
const RESULT_HEAD: usize = 1400;
const RESULT_TAIL: usize = 600;

/// ENCODING → 13 bitmap rows, MSB = leftmost pixel.
static GLYPHS: LazyLock<HashMap<u32, [u8; GLYPH_H]>> = LazyLock::new(|| {
    let mut map = HashMap::new();
    let mut code: Option<u32> = None;
    let mut bitmap: Option<([u8; GLYPH_H], usize)> = None;
    for line in BDF.lines() {
        if let Some(n) = line.strip_prefix("ENCODING ") {
            // `ENCODING -1` marks an unencoded glyph; it fails to parse and is skipped.
            code = n.trim().parse().ok();
        } else if line == "BITMAP" {
            bitmap = Some(([0; GLYPH_H], 0));
        } else if line == "ENDCHAR" {
            if let (Some(c), Some((rows, _))) = (code, bitmap) {
                map.insert(c, rows);
            }
            code = None;
            bitmap = None;
        } else if let Some((rows, i)) = bitmap.as_mut()
            && *i < GLYPH_H
        {
            rows[*i] = u8::from_str_radix(line.trim(), 16).unwrap_or(0);
            *i += 1;
        }
    }
    map
});

fn glyph(c: char) -> &'static [u8; GLYPH_H] {
    GLYPHS
        .get(&u32::from(c))
        .unwrap_or_else(|| &GLYPHS[&u32::from('?')])
}

/// Frame geometry for one provider family (13 §0.2, §2.3): glyphs are drawn
/// at the top-left of a `cell_w × cell_h` cell on a `frame`-wide square.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shape {
    /// Horizontal advance per character, px.
    pub cell_w: u32,
    /// Vertical pitch per line, px.
    pub cell_h: u32,
    /// Frame width (and maximum height), px.
    pub frame: u32,
    /// What the provider bills for one full frame.
    pub tokens_per_frame: u32,
}

/// The family's row of the legibility bench: `"anthropic"`, `"openai"`,
/// `"gemini"`; anything else takes the Anthropic row.
pub fn shape(adapter: &str) -> Shape {
    match adapter {
        "openai" => Shape {
            cell_w: 8,
            cell_h: 22,
            frame: 1568,
            tokens_per_frame: 2881,
        },
        "gemini" => Shape {
            cell_w: 8,
            cell_h: 22,
            frame: 2048,
            tokens_per_frame: 1120,
        },
        // Measured 2026-09-11 on claude-sonnet-4-5: four 1568² frames billed
        // ~18k input tokens (13 §1), not the 3,136 the 28 px patch formula gives.
        _ => Shape {
            cell_w: 11,
            cell_h: 16,
            frame: 1568,
            tokens_per_frame: 4500,
        },
    }
}

/// Characters per line.
pub fn cols(shape: &Shape) -> usize {
    (shape.frame / shape.cell_w) as usize
}

/// Lines per full frame.
pub fn rows(shape: &Shape) -> usize {
    (shape.frame / shape.cell_h) as usize
}

/// The folded span as wrapped lines (13 §2.1): `== epoch head ==` + the
/// previous head's text parts, then one logical line per part —
/// `[user] …`, `[assistant] …`, `[tool_call NAME {args}]`, `[tool NAME] …`,
/// `[image frame omitted]`. Reasoning and thought signatures are skipped.
/// Tabs → 4 spaces, `\r` dropped, hard wrap at `cols` with a two-space
/// continuation indent.
pub fn serialise(head: Option<&ChatMessage>, folded: &[ChatMessage], cols: usize) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(h) = head {
        wrap("== epoch head ==", cols, &mut out);
        for t in h.content.texts() {
            wrap(t, cols, &mut out);
        }
    }
    let mut names: HashMap<&str, &str> = HashMap::new();
    for m in folded {
        let tag = match m.role {
            ChatRole::System => "system",
            ChatRole::User => "user",
            ChatRole::Assistant => "assistant",
            ChatRole::Tool => "tool",
        };
        for p in m.content.iter() {
            match p {
                ContentPart::Text(t) if !t.trim().is_empty() => {
                    wrap(&format!("[{tag}] {t}"), cols, &mut out)
                }
                ContentPart::Binary(_) => wrap("[image frame omitted]", cols, &mut out),
                ContentPart::ToolCall(c) => {
                    names.insert(&c.call_id, &c.fn_name);
                    let args = cap(&c.fn_arguments.to_string(), CALL_ARGS_CAP);
                    wrap(&format!("[tool_call {} {args}]", c.fn_name), cols, &mut out);
                }
                ContentPart::ToolResponse(r) => {
                    let name = names
                        .get(r.call_id.as_str())
                        .copied()
                        .or(r.fn_name.as_deref())
                        .unwrap_or("?");
                    wrap(
                        &format!("[tool {name}] {}", elide(&r.content)),
                        cols,
                        &mut out,
                    );
                }
                // Empty text, reasoning, thought signatures, custom parts: never re-sent.
                _ => {}
            }
        }
    }
    out
}

/// Keep the last `frames_max × rows` lines for `budget_tokens` (13 §2.2);
/// when lines were dropped, line 0 is `[… n earlier lines dropped …]` and
/// the count is returned (the caller adds the message-count caption).
pub fn budget_lines(
    mut lines: Vec<String>,
    budget_tokens: u32,
    shape: &Shape,
) -> (Vec<String>, usize) {
    let frames_max = (budget_tokens / shape.tokens_per_frame).max(1) as usize;
    let rows_max = frames_max * rows(shape);
    if lines.len() <= rows_max {
        return (lines, 0);
    }
    let dropped = lines.len() - rows_max + 1; // one row for the marker
    lines.drain(..dropped);
    lines.insert(0, format!("[… {dropped} earlier lines dropped …]"));
    (lines, dropped)
}

/// PNG frames (13 §2.3): chunks of `rows(shape)` lines; width `frame`,
/// height `lines × cell_h`; black glyphs on white, no anti-aliasing.
/// Deterministic. Empty `lines` → empty vec.
pub fn rasterize(lines: &[String], shape: &Shape) -> Vec<Vec<u8>> {
    let w = shape.frame;
    let cols = cols(shape);
    lines
        .chunks(rows(shape).max(1))
        .map(|chunk| {
            let h = chunk.len() as u32 * shape.cell_h;
            let mut px = vec![255u8; (w * h) as usize];
            for (li, line) in chunk.iter().enumerate() {
                let y0 = li as u32 * shape.cell_h;
                for (ci, c) in line.chars().take(cols).enumerate() {
                    let x0 = ci as u32 * shape.cell_w;
                    for (r, bits) in glyph(c).iter().enumerate() {
                        let row = ((y0 + r as u32) * w + x0) as usize;
                        for b in 0..GLYPH_W {
                            if bits & (0x80 >> b) != 0 {
                                px[row + b as usize] = 0;
                            }
                        }
                    }
                }
            }
            encode(w, h, &px)
        })
        .collect()
}

fn encode(w: u32, h: u32, px: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = png::Encoder::new(&mut out, w, h);
    enc.set_color(png::ColorType::Grayscale);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    writer.write_image_data(px).expect("png data");
    writer.finish().expect("png finish");
    out
}

/// Normalise (`\t` → 4 spaces, drop `\r`) and hard-wrap every line of
/// `text` at `cols`; continuation lines carry a two-space indent.
fn wrap(text: &str, cols: usize, out: &mut Vec<String>) {
    let cols = cols.max(3);
    for logical in text.replace('\r', "").replace('\t', "    ").lines() {
        let mut chars = logical.chars();
        let mut first = true;
        loop {
            let (indent, n) = if first { ("", cols) } else { ("  ", cols - 2) };
            let chunk: String = chars.by_ref().take(n).collect();
            if chunk.is_empty() && !first {
                break;
            }
            let full = chunk.chars().count() == n;
            out.push(format!("{indent}{chunk}"));
            first = false;
            if !full {
                break;
            }
        }
    }
}

/// First `n` chars, `…` appended when cut.
fn cap(s: &str, n: usize) -> String {
    match s.char_indices().nth(n) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_owned(),
    }
}

/// Head `RESULT_HEAD` + `[… n chars omitted …]` + tail `RESULT_TAIL` chars.
fn elide(s: &str) -> String {
    let n = s.chars().count();
    if n <= RESULT_HEAD + RESULT_TAIL {
        return s.to_owned();
    }
    let head_end = s
        .char_indices()
        .nth(RESULT_HEAD)
        .map_or(s.len(), |(i, _)| i);
    let tail_start = s
        .char_indices()
        .nth(n - RESULT_TAIL)
        .map_or(s.len(), |(i, _)| i);
    format!(
        "{}\n[… {} chars omitted …]\n{}",
        &s[..head_end],
        n - RESULT_HEAD - RESULT_TAIL,
        &s[tail_start..]
    )
}

#[cfg(test)]
mod tests {
    use genai::chat::{MessageContent, ToolCall, ToolResponse};

    use super::*;

    #[test]
    fn bdf_parses_all_printable_ascii() {
        for c in 0x20u32..=0x7E {
            assert!(GLYPHS.contains_key(&c), "missing glyph U+{c:04X}");
        }
        assert!(glyph('A').iter().any(|r| *r != 0));
        assert!(glyph(' ').iter().all(|r| *r == 0));
        assert_eq!(glyph('\u{10FFFF}'), glyph('?'));
    }

    #[test]
    fn rasterize_is_deterministic_and_hugs_rows() {
        let shape = shape("anthropic");
        let lines = vec!["hello world".to_owned(), "second line".to_owned()];
        let a = rasterize(&lines, &shape);
        let b = rasterize(&lines, &shape);
        assert_eq!(a, b);
        assert_eq!(a.len(), 1);
        assert!(rasterize(&[], &shape).is_empty());

        let mut reader = png::Decoder::new(&a[0][..]).read_info().unwrap();
        let info = reader.info();
        assert_eq!((info.width, info.height), (shape.frame, 2 * shape.cell_h));
        let mut px = vec![0u8; reader.output_buffer_size()];
        reader.next_frame(&mut px).unwrap();
        let glyph_rows = &px[..(shape.frame as usize) * GLYPH_H];
        assert!(glyph_rows.contains(&0));
        assert!(
            px[(shape.frame * (shape.cell_h - 1)) as usize..(shape.frame * shape.cell_h) as usize]
                .iter()
                .all(|p| *p == 255)
        );
    }

    #[test]
    fn budget_keeps_the_tail() {
        let shape = shape("anthropic");
        let lines: Vec<String> = (0..300).map(|i| format!("line {i}")).collect();
        let (kept, dropped) = budget_lines(lines, shape.tokens_per_frame, &shape);
        assert_eq!(kept.len(), rows(&shape));
        assert_eq!(dropped, 300 - rows(&shape) + 1);
        assert_eq!(kept[0], format!("[… {dropped} earlier lines dropped …]"));
        assert_eq!(kept.last().unwrap(), "line 299");

        let (kept, dropped) = budget_lines(vec!["a".to_owned()], 0, &shape);
        assert_eq!((kept.len(), dropped), (1, 0));
    }

    #[test]
    fn serialise_wraps_and_caps_tool_results() {
        let cols = 100;
        let call = ToolCall {
            call_id: "c1".into(),
            fn_name: "read_file".into(),
            fn_arguments: serde_json::json!({"path": "x".repeat(300)}),
            thought_signatures: None,
        };
        let folded = vec![
            ChatMessage::user("a".repeat(500)),
            ChatMessage::assistant(MessageContent::from_parts(vec![
                ContentPart::ReasoningContent("hidden".into()),
                ContentPart::ToolCall(call),
            ])),
            ChatMessage::tool(ToolResponse::new("c1", "b".repeat(5000))),
        ];
        let head = ChatMessage::user("prev\tsummary\r\n");
        let lines = serialise(Some(&head), &folded, cols);

        assert_eq!(lines[0], "== epoch head ==");
        assert_eq!(lines[1], "prev    summary");
        assert_eq!(lines[2].chars().take(8).collect::<String>(), "[user] a");
        // "[user] " + 500 a's = 507 chars: 100 + ceil(407 / 98) = 6 lines.
        let user: Vec<&String> = lines
            .iter()
            .filter(|l| l.starts_with("[user] ") || l.starts_with("  a"))
            .collect();
        assert_eq!(user.len(), 6);
        assert!(
            user[1..]
                .iter()
                .all(|l| l.starts_with("  ") && l.chars().count() <= cols)
        );
        assert!(user[..5].iter().all(|l| l.chars().count() == cols));
        assert!(!lines.iter().any(|l| l.contains("hidden")));
        assert!(lines.iter().any(|l| l.starts_with("[tool read_file] b")));
        assert!(lines.iter().any(|l| l.contains("[… 3000 chars omitted …]")));

        // Unwrapped: args capped at 200 chars + "…".
        let wide = serialise(None, &folded, 10_000);
        let call = wide
            .iter()
            .find(|l| l.starts_with("[tool_call read_file "))
            .unwrap();
        assert!(call.ends_with("…]"));
        assert_eq!(
            call.chars().count(),
            "[tool_call read_file ".len() + CALL_ARGS_CAP + 2
        );
    }
}
