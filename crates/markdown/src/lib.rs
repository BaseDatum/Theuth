//! Streaming markdown for any UI.
//!
//! A model streams text token by token. Re-parsing the whole message on every
//! delta flickers and is O(n²); rendering half-open constructs (`**bol`) is
//! wrong. [`Text`] splits the stream with `mdstream` into *committed* blocks
//! (never change again, rendered exactly once) and a *pending* tail (shown raw
//! until it completes). [`render`] turns one block into [`Line`]s of styled
//! [`Span`]s: no colours, no escape codes, so every frontend maps [`Style`]
//! onto its own text primitives.

use std::fmt;

use mdstream::{FootnotesMode, MdStream, Options};
use pulldown_cmark::{Alignment, CodeBlockKind, Event, Parser, Tag, TagEnd};
use unicode_width::UnicodeWidthStr;

/// How a run of text should look. All flags may combine.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    /// Strong emphasis, headings, table headers.
    pub bold: bool,
    /// Emphasis.
    pub italic: bool,
    /// Strikethrough.
    pub strike: bool,
    /// Inline code, code blocks, math.
    pub code: bool,
    /// Structural decoration: list bullets, quote bars, rules.
    pub dim: bool,
    /// Heading level 1–6 (0 = not a heading). Headings are also `bold`; a
    /// renderer gives them colour/underline rather than a `#` marker.
    pub heading: u8,
    /// Link or image target; render underlined / as OSC 8.
    pub link: Option<String>,
}

/// A run of text with one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Span {
    /// Never contains `\n`.
    pub text: String,
    /// How to draw it.
    pub style: Style,
}

/// One visual line; the renderer never emits `\n` inside a span.
pub type Line = Vec<Span>;

/// Render one complete markdown block (or a whole document) to lines. Bare
/// `http(s)://` URLs become links too (CommonMark only autolinks `<…>`).
pub fn render(md: &str) -> Vec<Line> {
    use pulldown_cmark::Options as O;
    let opts =
        O::ENABLE_STRIKETHROUGH | O::ENABLE_TABLES | O::ENABLE_TASKLISTS | O::ENABLE_FOOTNOTES;
    let mut r = Renderer::default();
    for ev in Parser::new_ext(md, opts) {
        r.event(ev);
    }
    let mut lines = r.finish();
    for l in &mut lines {
        autolink(l);
    }
    lines
}

/// Byte ranges of bare `http://` / `https://` URLs in `text`: up to the
/// next whitespace or `<>"'`, minus trailing punctuation and an unbalanced
/// closing bracket (`(https://x.y/z).` links `https://x.y/z`).
pub fn urls(text: &str) -> Vec<std::ops::Range<usize>> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = text[from..].find("http").map(|i| from + i) {
        let rest = &text[i..];
        let Some(after) = rest
            .strip_prefix("https://")
            .or(rest.strip_prefix("http://"))
        else {
            from = i + 4;
            continue;
        };
        let scheme = rest.len() - after.len();
        let body_end = after
            .find(|c: char| c.is_whitespace() || matches!(c, '<' | '>' | '"' | '\''))
            .unwrap_or(after.len());
        let mut body = &after[..body_end];
        while let Some(last) = body.chars().last() {
            let cut = match last {
                '.' | ',' | ';' | ':' | '!' | '?' | '*' | '_' | '`' => true,
                ')' => body.matches('(').count() < body.matches(')').count(),
                ']' => body.matches('[').count() < body.matches(']').count(),
                '}' => body.matches('{').count() < body.matches('}').count(),
                _ => false,
            };
            if !cut {
                break;
            }
            body = &body[..body.len() - last.len_utf8()];
        }
        // A bare scheme (`http://` alone) is not a link.
        if !body.is_empty() {
            out.push(i..i + scheme + body.len());
        }
        from = i + scheme + body.len().max(1);
    }
    out
}

/// Split spans that carry no link around the bare URLs they contain.
pub fn autolink(line: &mut Line) {
    if !line
        .iter()
        .any(|s| s.style.link.is_none() && s.text.contains("http"))
    {
        return;
    }
    let mut out: Line = Vec::with_capacity(line.len());
    for span in line.drain(..) {
        if span.style.link.is_some() {
            out.push(span);
            continue;
        }
        let mut at = 0;
        for r in urls(&span.text) {
            if r.start > at {
                out.push(Span {
                    text: span.text[at..r.start].to_owned(),
                    style: span.style.clone(),
                });
            }
            let url = &span.text[r.clone()];
            out.push(Span {
                text: url.to_owned(),
                style: Style {
                    link: Some(url.to_owned()),
                    ..span.style.clone()
                },
            });
            at = r.end;
        }
        if at < span.text.len() {
            out.push(Span {
                text: span.text[at..].to_owned(),
                style: span.style,
            });
        }
    }
    *line = out;
}

/// A committed block and where its source ends in the raw buffer.
struct Block {
    lines: Vec<Line>,
    end: usize,
}

/// Streamed assistant text with a reveal cursor.
///
/// Deltas arrive in provider-sized chunks; a UI wants a steady character
/// flow. The cursor advances over the raw buffer at whatever pace the UI
/// ticks it ([`Text::reveal`]); text behind the cursor is visible, and a
/// committed block is drawn rendered only once the cursor has passed its
/// end - so the reader sees the same characters type out plain and then snap
/// into markdown.
pub struct Text {
    stream: MdStream,
    blocks: Vec<Block>,
    /// Byte offset in the raw buffer up to which committed blocks are mapped.
    mapped: usize,
    /// Byte offset of the reveal cursor (always a char boundary).
    revealed: usize,
    done: bool,
}

/// What to draw right now: rendered blocks, then plain text.
pub struct View<'a> {
    blocks: &'a [Block],
    /// Revealed text after the last shown block, verbatim.
    pub plain: &'a str,
}

impl View<'_> {
    /// Rendered lines with one empty line between blocks.
    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        static BLANK: Line = Vec::new();
        self.blocks
            .iter()
            .enumerate()
            .flat_map(|(i, b)| (i > 0).then_some(&BLANK).into_iter().chain(b.lines.iter()))
    }

    /// At least one rendered block is shown.
    pub fn has_blocks(&self) -> bool {
        !self.blocks.is_empty()
    }
}

impl Default for Text {
    fn default() -> Self {
        Self::new()
    }
}

impl Text {
    /// Empty, streaming, nothing revealed.
    pub fn new() -> Self {
        Self {
            stream: MdStream::new(Options {
                // A live UI cannot retract an already-rendered block.
                footnotes: FootnotesMode::Invalidate,
                ..Options::default()
            }),
            blocks: Vec::new(),
            mapped: 0,
            revealed: 0,
            done: false,
        }
    }

    /// Already complete and fully revealed (history replay).
    pub fn complete(md: &str) -> Self {
        let mut t = Self::new();
        t.push(md);
        t.finish();
        t.reveal_all();
        t
    }

    /// Append a delta; newly completed blocks are rendered now (shown later).
    pub fn push(&mut self, delta: &str) {
        let update = self.stream.append(delta);
        self.apply(update);
    }

    /// No more deltas: the tail becomes a committed block.
    pub fn finish(&mut self) {
        if self.done {
            return;
        }
        self.done = true;
        let update = self.stream.finalize();
        self.apply(update);
    }

    fn apply(&mut self, update: mdstream::Update) {
        if update.reset {
            self.blocks.clear();
            self.mapped = 0;
        }
        let raw = self.stream.buffer();
        for b in &update.committed {
            // Blocks are contiguous slices of the buffer (mdstream only
            // rewrites `\r`), so the search succeeds at or just after `mapped`.
            // Fail closed: an unmapped block shows once everything so far has.
            let end = raw[self.mapped..]
                .find(&b.raw)
                .map_or(raw.len(), |i| self.mapped + i + b.raw.len());
            self.mapped = end;
            self.blocks.push(Block {
                lines: render(&b.raw),
                end,
            });
        }
    }

    /// Advance the cursor by up to `chars` characters. `false` when there was
    /// nothing left to reveal.
    pub fn reveal(&mut self, chars: usize) -> bool {
        let rest = &self.stream.buffer()[self.revealed..];
        if rest.is_empty() {
            return false;
        }
        self.revealed += rest.chars().take(chars).map(char::len_utf8).sum::<usize>();
        true
    }

    /// Show everything received so far.
    pub fn reveal_all(&mut self) {
        self.revealed = self.stream.buffer().len();
    }

    /// Bytes received but not yet revealed.
    pub fn unrevealed(&self) -> usize {
        self.stream.buffer().len() - self.revealed
    }

    /// Blocks the cursor has passed, then the revealed remainder as plain text.
    pub fn view(&self) -> View<'_> {
        let shown = self.blocks.partition_point(|b| b.end <= self.revealed);
        let from = self.blocks[..shown].last().map_or(0, |b| b.end);
        View {
            blocks: &self.blocks[..shown],
            plain: self.stream.buffer()[from..self.revealed].trim_start_matches('\n'),
        }
    }

    /// Everything received so far, verbatim.
    pub fn raw(&self) -> &str {
        self.stream.buffer()
    }

    /// `finish` was called.
    pub fn is_done(&self) -> bool {
        self.done
    }
}

impl fmt::Debug for Text {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Text")
            .field("blocks", &self.blocks.len())
            .field("revealed", &self.revealed)
            .field("len", &self.stream.buffer().len())
            .field("done", &self.done)
            .finish()
    }
}

/// Left margin of the current line: quote bars and list indents. `once`
/// entries (list markers) print on their first line, then turn into spaces.
struct Prefix {
    text: String,
    once: bool,
}

/// Open table: cells are buffered until `TagEnd::Table`, when column widths
/// are known. `rows[0]` is the header.
struct Table {
    align: Vec<Alignment>,
    rows: Vec<Vec<Line>>,
    row: Vec<Line>,
}

#[derive(Default)]
struct Renderer {
    lines: Vec<Line>,
    /// Open line, `None` between blocks.
    cur: Option<Line>,
    prefixes: Vec<Prefix>,
    /// Open lists: next ordinal, or `None` for bullets.
    lists: Vec<Option<u64>>,
    bold: u8,
    italic: u8,
    strike: u8,
    code: bool,
    heading: u8,
    link: Option<String>,
    table_head: bool,
    table: Option<Table>,
}

impl Renderer {
    fn style(&self) -> Style {
        Style {
            bold: self.bold > 0 || self.table_head,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code: self.code,
            dim: false,
            heading: self.heading,
            link: self.link.clone(),
        }
    }

    fn open(&mut self) -> &mut Line {
        if self.cur.is_none() {
            let mut line = Line::new();
            for p in &mut self.prefixes {
                push_span(
                    &mut line,
                    &p.text,
                    Style {
                        dim: true,
                        ..Style::default()
                    },
                );
                if p.once {
                    p.text = " ".repeat(p.text.chars().count());
                    p.once = false;
                }
            }
            self.cur = Some(line);
        }
        self.cur.as_mut().expect("just opened")
    }

    fn text(&mut self, s: &str) {
        let style = self.style();
        push_span(self.open(), s, style);
    }

    fn dim(&mut self, s: &str) {
        push_span(
            self.open(),
            s,
            Style {
                dim: true,
                ..Style::default()
            },
        );
    }

    /// Close the open line.
    fn flush(&mut self) {
        if let Some(line) = self.cur.take() {
            self.lines.push(line);
        }
    }

    /// Close the line and, at top level, separate from the next block.
    fn end_block(&mut self) {
        self.flush();
        if self.prefixes.is_empty() && self.lists.is_empty() && !self.lines.is_empty() {
            self.lines.push(Line::new());
        }
    }

    fn event(&mut self, ev: Event<'_>) {
        match ev {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(s) if self.code => {
                let mut first = true;
                for part in s.split('\n') {
                    if !first {
                        self.flush();
                    }
                    first = false;
                    if !part.is_empty() {
                        self.text(part);
                    } else {
                        self.open();
                    }
                }
            }
            Event::Text(s) | Event::Html(s) | Event::InlineHtml(s) => self.text(&s),
            Event::Code(s) | Event::InlineMath(s) | Event::DisplayMath(s) => {
                let style = Style {
                    code: true,
                    ..self.style()
                };
                push_span(self.open(), &s, style);
            }
            Event::FootnoteReference(name) => self.dim(&format!("[^{name}]")),
            Event::SoftBreak => self.text(" "),
            Event::HardBreak => self.flush(),
            Event::Rule => {
                self.flush();
                self.dim("────────");
                self.end_block();
            }
            Event::TaskListMarker(done) => self.dim(if done { "[x] " } else { "[ ] " }),
        }
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {
                self.flush();
            }
            Tag::Heading { level, .. } => {
                self.flush();
                self.heading = level as u8;
                self.bold += 1;
            }
            Tag::BlockQuote(_) => {
                self.flush();
                self.prefixes.push(Prefix {
                    text: "│ ".into(),
                    once: false,
                });
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                if let CodeBlockKind::Fenced(lang) = kind
                    && !lang.is_empty()
                {
                    self.dim(&format!("╭ {lang}"));
                    self.flush();
                }
                self.code = true;
            }
            Tag::List(start) => {
                self.flush();
                self.lists.push(start);
            }
            Tag::Item => {
                self.flush();
                let marker = match self.lists.last_mut() {
                    Some(Some(n)) => {
                        *n += 1;
                        format!("{}. ", *n - 1)
                    }
                    _ => "• ".into(),
                };
                self.prefixes.push(Prefix {
                    text: marker,
                    once: true,
                });
            }
            Tag::FootnoteDefinition(name) => {
                self.flush();
                self.prefixes.push(Prefix {
                    text: format!("[^{name}]: "),
                    once: true,
                });
            }
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } | Tag::Image { dest_url, .. } => {
                self.link = Some(dest_url.into_string());
            }
            Tag::Table(align) => {
                self.flush();
                self.table = Some(Table {
                    align,
                    rows: Vec::new(),
                    row: Vec::new(),
                });
            }
            Tag::TableHead => self.table_head = true,
            // A bare line, not `open()`: prefixes are added when the table is laid out.
            Tag::TableCell => self.cur = Some(Line::new()),
            Tag::TableRow => {}
            Tag::HtmlBlock | Tag::MetadataBlock(_) => self.flush(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock | TagEnd::MetadataBlock(_) => {
                self.end_block();
            }
            TagEnd::Table => {
                if let Some(t) = self.table.take() {
                    self.layout_table(t);
                }
                self.end_block();
            }
            TagEnd::Heading(_) => {
                self.bold -= 1;
                self.heading = 0;
                self.end_block();
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.prefixes.pop();
                if self.prefixes.is_empty() {
                    self.end_block();
                }
            }
            TagEnd::CodeBlock => {
                self.code = false;
                // The text ends with '\n', leaving an empty open line.
                if self
                    .cur
                    .as_ref()
                    .is_some_and(|l| l.iter().all(|s| s.style.dim))
                {
                    self.cur = None;
                }
                self.end_block();
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.end_block();
                }
            }
            TagEnd::Item | TagEnd::FootnoteDefinition => {
                self.flush();
                self.prefixes.pop();
            }
            TagEnd::Emphasis => self.italic -= 1,
            TagEnd::Strong => self.bold -= 1,
            TagEnd::Strikethrough => self.strike -= 1,
            TagEnd::Link | TagEnd::Image => self.link = None,
            TagEnd::TableCell => {
                let cell = self.cur.take().unwrap_or_default();
                if let Some(t) = &mut self.table {
                    t.row.push(cell);
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                self.table_head = false;
                if let Some(t) = &mut self.table {
                    let row = std::mem::take(&mut t.row);
                    t.rows.push(row);
                }
            }
            _ => {}
        }
    }

    /// Emit a buffered table: cells padded to their column's width, ` │ `
    /// between columns, a `─┼─` rule under the header.
    fn layout_table(&mut self, t: Table) {
        let width = |l: &Line| l.iter().map(|s| s.text.width()).sum::<usize>();
        let cols = t.rows.iter().map(Vec::len).max().unwrap_or(0);
        let widths: Vec<usize> = (0..cols)
            .map(|c| {
                t.rows
                    .iter()
                    .filter_map(|r| r.get(c))
                    .map(width)
                    .max()
                    .unwrap_or(0)
            })
            .collect();
        for (i, row) in t.rows.into_iter().enumerate() {
            self.open();
            for (c, w) in widths.iter().enumerate() {
                if c > 0 {
                    self.dim(" │ ");
                }
                let cell = row.get(c).cloned().unwrap_or_default();
                let pad = w - width(&cell);
                let (left, right) = match t.align.get(c) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    // No trailing blanks after the last column.
                    _ if c + 1 == cols => (0, 0),
                    _ => (0, pad),
                };
                self.text(&" ".repeat(left));
                for s in cell {
                    push_span(self.open(), &s.text, s.style);
                }
                self.text(&" ".repeat(right));
            }
            self.flush();
            if i == 0 {
                let rule: Vec<String> = widths.iter().map(|w| "─".repeat(*w)).collect();
                self.dim(&rule.join("─┼─"));
                self.flush();
            }
        }
    }

    fn finish(mut self) -> Vec<Line> {
        self.flush();
        while self.lines.last().is_some_and(Vec::is_empty) {
            self.lines.pop();
        }
        self.lines
    }
}

/// Append text, merging into the previous span when the style matches.
fn push_span(line: &mut Line, text: &str, style: Style) {
    if text.is_empty() {
        return;
    }
    match line.last_mut() {
        Some(last) if last.style == style => last.text.push_str(text),
        _ => line.push(Span {
            text: text.to_owned(),
            style,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(lines: &[Line]) -> Vec<String> {
        lines
            .iter()
            .map(|l| l.iter().map(|s| s.text.as_str()).collect())
            .collect()
    }

    #[test]
    fn bare_urls_become_links_with_tidy_edges() {
        let t =
            "see https://a.io/x_(y). or (http://b.io/q?z=1), not http:// alone; `https://c.io/d`";
        let got: Vec<&str> = urls(t).into_iter().map(|r| &t[r]).collect();
        assert_eq!(
            got,
            ["https://a.io/x_(y)", "http://b.io/q?z=1", "https://c.io/d"]
        );
        assert!(urls("httpx and https:// and http").is_empty());

        // Through the renderer: the URL span carries the link, an explicit
        // markdown link is left alone (its span already links elsewhere).
        let lines = render("go to https://a.io/x now, or [here](https://b.io)");
        let links: Vec<(&str, Option<&str>)> = lines[0]
            .iter()
            .map(|s| (s.text.as_str(), s.style.link.as_deref()))
            .collect();
        assert_eq!(
            links,
            [
                ("go to ", None),
                ("https://a.io/x", Some("https://a.io/x")),
                (" now, or ", None),
                ("here", Some("https://b.io")),
            ]
        );
    }

    #[test]
    fn renders_structure_with_prefixes_and_styles() {
        let lines = render(
            "# Title\n\nSome **bold** and `code`.\n\n- one\n- two\n  1. nested\n\n> quoted\n\n```rust\nlet x = 1;\n\nlet y = 2;\n```\n",
        );
        assert_eq!(
            plain(&lines),
            [
                "Title",
                "",
                "Some bold and code.",
                "",
                "• one",
                "• two",
                "  1. nested",
                "",
                "│ quoted",
                "",
                "╭ rust",
                "let x = 1;",
                "",
                "let y = 2;",
            ]
        );
        let title = &lines[0];
        assert!(title[0].style.heading == 1 && title[0].style.bold);
        let para = &lines[2];
        assert_eq!(para[1].text, "bold");
        assert!(para[1].style.bold && !para[1].style.code);
        assert!(para[3].style.code);
        assert!(lines[11][0].style.code);
    }

    #[test]
    fn tables_align_columns() {
        let lines = render(
            "| Name | N | Note |\n|:--|--:|:-:|\n| Alice | 3 | ok |\n| Bo | 1234 | 日本 |\n",
        );
        assert_eq!(
            plain(&lines),
            [
                "Name  │    N │ Note",
                "──────┼──────┼─────",
                "Alice │    3 │  ok ",
                "Bo    │ 1234 │ 日本",
            ]
        );
        assert!(lines[0][0].style.bold);
        assert!(!lines[2][0].style.bold);
    }

    fn shown(t: &Text) -> (Vec<String>, String) {
        let v = t.view();
        (
            v.lines()
                .map(|l| l.iter().map(|s| s.text.as_str()).collect())
                .collect(),
            v.plain.to_owned(),
        )
    }

    #[test]
    fn typewriter_reveals_plain_then_snaps_to_rendered_blocks() {
        let mut t = Text::new();
        t.push("Hello **wor");
        assert_eq!(shown(&t), (vec![], String::new()));
        assert!(t.reveal(5));
        assert_eq!(shown(&t), (vec![], "Hello".into()));
        // The first block is committed but the cursor is still inside it: plain.
        t.push("ld**\n\nNext para");
        assert!(t.reveal(9));
        assert_eq!(shown(&t), (vec![], "Hello **world*".into()));
        // The block's source includes its separator newlines: revealing `**`
        // alone keeps it plain, the newlines after it snap it to rendered.
        t.reveal(1);
        assert_eq!(shown(&t), (vec![], "Hello **world**".into()));
        t.reveal(2);
        assert_eq!(shown(&t), (vec!["Hello world".to_owned()], String::new()));
        assert!(t.view().lines().next().unwrap()[1].style.bold);
        t.reveal(4);
        assert_eq!(shown(&t).1, "Next");
        assert_eq!(t.unrevealed(), " para".len());
        t.finish();
        t.reveal(100);
        assert!(!t.reveal(1));
        assert_eq!(
            shown(&t),
            (
                ["Hello world", "", "Next para"].map(str::to_owned).to_vec(),
                String::new()
            )
        );
        assert_eq!(t.raw(), "Hello **world**\n\nNext para");
        assert_eq!(
            shown(&Text::complete(t.raw())).0,
            ["Hello world", "", "Next para"]
        );
    }
}
