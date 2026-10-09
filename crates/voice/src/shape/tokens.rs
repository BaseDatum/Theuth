//! Stage 1 (tokenise) and the renderer: the token list every stage rewrites,
//! then the join with spacing classes (153 §4.2).

use super::{Action, Replaced, RuleId};

/// What a token is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    /// A word (also a joined identifier, number or path piece).
    Word,
    /// A punctuation symbol: the model's own (`asr`) or a converted one.
    Sym,
    /// A W-j action, rendered as `\n` / `\n\n` / `\t`.
    Act(Action),
    /// Consumed by an escape ("literal"): kept for the end parser, never rendered.
    Hidden,
}

/// One token; `glue_*` are the spacing class (no space on that side).
#[derive(Clone, Debug)]
pub(super) struct Tok {
    pub text: String,
    pub kind: Kind,
    /// The model wrote this mark itself.
    pub asr: bool,
    pub glue_left: bool,
    pub glue_right: bool,
    /// Escaped by "literal": later stages leave it alone.
    pub literal: bool,
    /// Joined from spelled single letters (`S H` → `SH`).
    pub spelled: bool,
    /// A dictionary rewrite: what was heard and which rule.
    pub replaced: Option<(String, RuleId)>,
}

impl Tok {
    pub fn word(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            kind: Kind::Word,
            asr: false,
            glue_left: false,
            glue_right: false,
            literal: false,
            spelled: false,
            replaced: None,
        }
    }

    pub fn sym(text: impl Into<String>, glue_left: bool, glue_right: bool) -> Self {
        Self {
            kind: Kind::Sym,
            glue_left,
            glue_right,
            ..Self::word(text)
        }
    }

    pub fn action(a: Action) -> Self {
        let text = match a {
            Action::NewLine => "\n",
            Action::NewParagraph => "\n\n",
            Action::Tab => "\t",
        };
        Self {
            kind: Kind::Act(a),
            glue_left: true,
            glue_right: true,
            ..Self::word(text)
        }
    }

    /// A plain, unescaped word this stage may rewrite.
    pub fn is_free_word(&self) -> bool {
        self.kind == Kind::Word && !self.literal
    }

    /// Case-insensitive word equality.
    pub fn is(&self, w: &str) -> bool {
        self.is_free_word() && self.text.eq_ignore_ascii_case(w)
    }

    /// A converted (not model-written) symbol with this text.
    pub fn is_sym(&self, s: &str) -> bool {
        self.kind == Kind::Sym && !self.asr && self.text == s
    }

    /// Letters, digits, `_`, `'`, `-` only (a word an identifier or path can attach to).
    pub fn alnum(&self) -> bool {
        self.kind == Kind::Word
            && !self.text.is_empty()
            && self
                .text
                .chars()
                .all(|c| c.is_alphanumeric() || matches!(c, '_' | '\'' | '-'))
    }
}

const RIGHT_ATTACHED: &str = ".,?!:;";
const CLOSERS: &str = ")]}”’\"'";
const OPENERS: &str = "([{“‘\"'";

/// Split `raw` into words and the model's own marks (`asr: true`), peeled
/// from the edges of each whitespace chunk; interior marks stay in the word
/// (`I'll`, `2.5`, `deploy.sh`).
pub(super) fn tokenize(raw: &str) -> Vec<Tok> {
    let mut out = Vec::new();
    for chunk in raw.split_whitespace() {
        let mut lead = Vec::new();
        let mut rest = chunk;
        while let Some(c) = rest.chars().next() {
            if OPENERS.contains(c) && rest.chars().count() > 1 {
                lead.push(c);
                rest = &rest[c.len_utf8()..];
            } else {
                break;
            }
        }
        let mut trail = Vec::new();
        while let Some(c) = rest.chars().next_back() {
            if (RIGHT_ATTACHED.contains(c) || CLOSERS.contains(c)) && rest.chars().count() > 1 {
                trail.push(c);
                rest = &rest[..rest.len() - c.len_utf8()];
            } else {
                break;
            }
        }
        for c in lead {
            out.push(asr_mark(c, true));
        }
        if !rest.is_empty() {
            if rest.chars().count() == 1 {
                let c = rest.chars().next().unwrap_or(' ');
                if RIGHT_ATTACHED.contains(c) || CLOSERS.contains(c) || OPENERS.contains(c) {
                    out.push(asr_mark(c, false));
                    continue;
                }
            }
            out.push(Tok::word(rest));
        }
        for c in trail.into_iter().rev() {
            out.push(asr_mark(c, false));
        }
    }
    out
}

fn asr_mark(c: char, leading: bool) -> Tok {
    let opener = OPENERS.contains(c) && (leading || !CLOSERS.contains(c));
    let mut t = Tok::sym(c.to_string(), !opener, opener);
    t.asr = true;
    t
}

/// Join the tokens: a space between two unless the left glues right or the
/// right glues left; sentence case after "new line" (table row 21; row 22
/// keeps the case after a paragraph break) or after `prev`'s trailing newline;
/// `Replaced` byte ranges from the rewritten tokens.
pub(super) fn render(toks: &[Tok], prev: &str) -> (String, Vec<Action>, Vec<Replaced>) {
    let mut text = String::new();
    let mut actions = Vec::new();
    let mut replaced = Vec::new();
    let mut cap_next = prev.ends_with('\n');
    let mut last: Option<&Tok> = None;
    for t in toks.iter().filter(|t| t.kind != Kind::Hidden) {
        if let Some(l) = last
            && !(l.glue_right || t.glue_left)
        {
            text.push(' ');
        }
        let start = text.len();
        match t.kind {
            Kind::Act(a) => {
                actions.push(a);
                cap_next = a == Action::NewLine;
                text.push_str(&t.text);
            }
            Kind::Word if cap_next => {
                let mut cs = t.text.chars();
                if let Some(c) = cs.next() {
                    text.extend(c.to_uppercase());
                    text.push_str(cs.as_str());
                }
                cap_next = false;
            }
            _ => text.push_str(&t.text),
        }
        if let Some((heard, rule)) = &t.replaced {
            replaced.push(Replaced {
                start,
                end: text.len(),
                heard: heard.clone(),
                rule: rule.clone(),
            });
        }
        last = Some(t);
    }
    (text, actions, replaced)
}
