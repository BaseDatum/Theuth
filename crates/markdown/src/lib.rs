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
use pulldown_cmark::{CodeBlockKind, Event, Parser, Tag, TagEnd};

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
    /// Structural decoration: heading markers, list bullets, quote bars, rules.
    pub dim: bool,
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

/// Render one complete markdown block (or a whole document) to lines.
pub fn render(md: &str) -> Vec<Line> {
    use pulldown_cmark::Options as O;
    let opts =
        O::ENABLE_STRIKETHROUGH | O::ENABLE_TABLES | O::ENABLE_TASKLISTS | O::ENABLE_FOOTNOTES;
    let mut r = Renderer::default();
    for ev in Parser::new_ext(md, opts) {
        r.event(ev);
    }
    r.finish()
}

/// Streamed assistant text: rendered committed blocks plus the raw tail.
pub struct Text {
    stream: MdStream,
    blocks: Vec<Vec<Line>>,
    pending: String,
    done: bool,
}

impl Default for Text {
    fn default() -> Self {
        Self::new()
    }
}

impl Text {
    /// Empty, streaming.
    pub fn new() -> Self {
        Self {
            stream: MdStream::new(Options {
                // A live UI cannot retract an already-rendered block.
                footnotes: FootnotesMode::Invalidate,
                ..Options::default()
            }),
            blocks: Vec::new(),
            pending: String::new(),
            done: false,
        }
    }

    /// Already complete (history replay).
    pub fn complete(md: &str) -> Self {
        let mut t = Self::new();
        t.push(md);
        t.finish();
        t
    }

    /// Append a delta; newly completed blocks are rendered now.
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
        self.pending.clear();
    }

    fn apply(&mut self, update: mdstream::Update) {
        if update.reset {
            self.blocks.clear();
        }
        self.blocks
            .extend(update.committed.iter().map(|b| render(&b.raw)));
        self.pending = update.pending.map(|b| b.raw).unwrap_or_default();
    }

    /// Rendered, immutable blocks in order.
    pub fn blocks(&self) -> &[Vec<Line>] {
        &self.blocks
    }

    /// Raw tail not yet committed; empty once finished.
    pub fn pending(&self) -> &str {
        &self.pending
    }

    /// Everything received so far, verbatim.
    pub fn raw(&self) -> &str {
        self.stream.buffer()
    }

    /// Committed lines with one empty line between blocks: what a UI draws
    /// above the pending tail.
    pub fn lines(&self) -> impl Iterator<Item = &Line> {
        static BLANK: Line = Vec::new();
        self.blocks
            .iter()
            .enumerate()
            .flat_map(|(i, b)| (i > 0).then_some(&BLANK).into_iter().chain(b.iter()))
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
            .field("pending", &self.pending)
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
    link: Option<String>,
    table_head: bool,
}

impl Renderer {
    fn style(&self) -> Style {
        Style {
            bold: self.bold > 0 || self.table_head,
            italic: self.italic > 0,
            strike: self.strike > 0,
            code: self.code,
            dim: false,
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
                self.dim(&format!("{} ", "#".repeat(level as usize)));
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
            Tag::Table(_) => self.flush(),
            Tag::TableHead => self.table_head = true,
            Tag::TableRow => {}
            Tag::TableCell => {
                if self.cur.as_ref().is_some_and(|l| !l.is_empty()) {
                    self.dim(" │ ");
                }
            }
            Tag::HtmlBlock | Tag::MetadataBlock(_) => self.flush(),
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph | TagEnd::HtmlBlock | TagEnd::Table | TagEnd::MetadataBlock(_) => {
                self.end_block();
            }
            TagEnd::Heading(_) => {
                self.bold -= 1;
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
            TagEnd::TableHead => {
                self.table_head = false;
                self.flush();
            }
            TagEnd::TableRow => self.flush(),
            _ => {}
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
    fn renders_structure_with_prefixes_and_styles() {
        let lines = render(
            "# Title\n\nSome **bold** and `code`.\n\n- one\n- two\n  1. nested\n\n> quoted\n\n```rust\nlet x = 1;\n\nlet y = 2;\n```\n",
        );
        assert_eq!(
            plain(&lines),
            [
                "# Title",
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
        assert!(title[0].style.dim && title[1].style.bold);
        let para = &lines[2];
        assert_eq!(para[1].text, "bold");
        assert!(para[1].style.bold && !para[1].style.code);
        assert!(para[3].style.code);
        assert!(lines[11][0].style.code);
    }

    #[test]
    fn streams_plain_then_renders_completed_blocks() {
        let mut t = Text::new();
        t.push("Hello **wor");
        assert!(t.blocks().is_empty());
        assert_eq!(t.pending(), "Hello **wor");
        t.push("ld**\n\nNext para");
        assert_eq!(t.blocks().len(), 1);
        assert_eq!(t.blocks()[0][0][1].text, "world");
        assert!(t.blocks()[0][0][1].style.bold);
        assert_eq!(t.pending(), "Next para");
        t.finish();
        assert_eq!(t.blocks().len(), 2);
        assert_eq!(t.pending(), "");
        assert_eq!(t.raw(), "Hello **world**\n\nNext para");
        assert_eq!(plain(&Text::complete(t.raw()).blocks()[1]), ["Next para"]);
    }
}
