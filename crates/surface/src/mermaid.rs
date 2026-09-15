//! Mermaid source → terminal lines through `merman` (bake-off winner, `fixtures/mermaid/BAKEOFF.md`).

use markdown::{Line, Span, Style};
use merman::ascii::{AsciiError, HeadlessAsciiError, HeadlessAsciiRenderer};
use unicode_width::UnicodeWidthStr;

use crate::MAX_MERMAID_BYTES;

/// Render a Mermaid source to lines at `width` columns (0 = unbounded).
///
/// `Ok(Some)` rendered; `Ok(None)` the diagram type or a construct is unsupported, the
/// source is over [`MAX_MERMAID_BYTES`], or the drawing is wider than `width` — the caller
/// falls back to the source block; `Err` is the parser's message for a syntax error.
///
/// Box drawing, block and arrow characters are `dim`, everything else plain.
pub fn mermaid_lines(src: &str, width: usize) -> Result<Option<Vec<Line>>, String> {
    if src.len() > MAX_MERMAID_BYTES {
        return Ok(None);
    }
    // ponytail: a pre-pass quoting labels with `()[]{}` waits for the log to show it (doc 30 §4).
    let text = match HeadlessAsciiRenderer::new().render_ascii_sync(src) {
        Ok(Some(text)) => text,
        Ok(None) => return Ok(None),
        Err(
            HeadlessAsciiError::Parse(merman::Error::UnsupportedDiagram { .. })
            | HeadlessAsciiError::Ascii(
                AsciiError::UnsupportedDiagram { .. }
                | AsciiError::UnsupportedFeature { .. }
                | AsciiError::RenderLimitExceeded { .. },
            ),
        ) => return Ok(None),
        // ponytail: merman 0.7 rejects the valid ER cardinality `o|` (BAKEOFF.md); treat every
        // ER parse error as unsupported so the model is not asked to fix correct syntax.
        // Drop this arm once upstream accepts 06-er.mmd.
        Err(HeadlessAsciiError::Parse(merman::Error::DiagramParse { diagram_type, .. }))
            if diagram_type == "er" =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e.to_string()),
    };
    let lines: Vec<Line> = text.lines().map(|l| l.trim_end()).map(styled).collect();
    if width > 0
        && lines
            .iter()
            .any(|l| l.iter().map(|s| s.text.width()).sum::<usize>() > width)
    {
        return Ok(None);
    }
    Ok(Some(lines))
}

/// Box drawing, block elements, arrows and geometric shapes: the structural characters.
fn is_drawing(c: char) -> bool {
    matches!(c, '\u{2190}'..='\u{21FF}' | '\u{2500}'..='\u{259F}' | '\u{25A0}'..='\u{25FF}')
}

/// One line split into runs of drawing (dim) and other (plain) characters.
fn styled(line: &str) -> Line {
    let mut out = Line::new();
    let mut run = String::new();
    let mut drawing = false;
    for c in line.chars() {
        let d = is_drawing(c);
        if d != drawing && !run.is_empty() {
            out.push(span(std::mem::take(&mut run), drawing));
        }
        drawing = d;
        run.push(c);
    }
    if !run.is_empty() {
        out.push(span(run, drawing));
    }
    out
}

fn span(text: String, dim: bool) -> Span {
    Span {
        text,
        style: Style {
            dim,
            ..Style::default()
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    macro_rules! mmd {
        ($name:literal) => {
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/fixtures/mermaid/",
                $name,
                ".mmd"
            ))
        };
    }

    #[test]
    fn flowchart_renders_at_100_and_falls_back_at_40() {
        let lines = mermaid_lines(mmd!("01-flowchart-lr"), 100)
            .expect("parses")
            .expect("fits 100");
        assert!(
            lines
                .iter()
                .any(|l| l.iter().any(|s| s.text.contains("Agent")))
        );
        assert!(
            lines.iter().flatten().any(|s| s.style.dim),
            "box drawing is dim"
        );
        assert!(
            lines
                .iter()
                .flatten()
                .all(|s| !s.style.dim || s.text.chars().all(is_drawing)),
            "only drawing characters are dim"
        );
        assert_eq!(
            mermaid_lines(mmd!("01-flowchart-lr"), 40).expect("parses"),
            None
        );
    }

    #[test]
    fn unsupported_and_er_fall_back_syntax_errors_report() {
        assert_eq!(mermaid_lines(mmd!("04-state"), 100), Ok(None));
        assert!(mermaid_lines(mmd!("06-er"), 100).is_ok(), "ER never errors");
        let err = mermaid_lines(mmd!("11-syntax-error"), 100).expect_err("unquoted parens");
        assert!(err.contains("parse error"), "{err}");
        assert_eq!(
            mermaid_lines(&"x".repeat(MAX_MERMAID_BYTES + 1), 0),
            Ok(None)
        );
    }
}
