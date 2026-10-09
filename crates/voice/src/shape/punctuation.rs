//! Stage 2 (spoken punctuation, actions, the "literal" escape), stage 3
//! (context gates on the ambiguous words) and stage 6 (model-punctuation
//! clean-up) of 153 §4.2.

use super::tokens::{Kind, Tok};
use super::{Action, Ctx};

/// Spacing class of a converted symbol.
#[derive(Clone, Copy)]
enum Class {
    /// `, . ? ! : ;` — glued to the word on its left.
    Right,
    /// `( [ { “ $` — glued to the word on its right.
    Opener,
    /// `) ] }` — glued left.
    Closer,
    /// `+ = & | < > —` — a space on both sides.
    Spaced,
    /// `- _ / . @` — no space on either side.
    Tight,
    /// `# ~` — glued to what follows only.
    Prefix,
    /// `" \``: an opener, then a closer, alternating per utterance.
    Toggle,
}

impl Class {
    fn glue(self, toggled_open: bool) -> (bool, bool) {
        match self {
            Class::Right | Class::Closer => (true, false),
            Class::Opener | Class::Prefix => (false, true),
            Class::Spaced => (false, false),
            Class::Tight => (true, true),
            Class::Toggle => (!toggled_open, toggled_open),
        }
    }
}

/// Unconditional phrases (longest first within a position). The gated words
/// ("dot", "slash", "dash", operators) are stage 3's.
const PHRASES: &[(&[&str], &str, Class)] = &[
    (&["exclamation", "mark"], "!", Class::Right),
    (&["exclamation", "point"], "!", Class::Right),
    (&["question", "mark"], "?", Class::Right),
    (&["full", "stop"], ".", Class::Right),
    (&["open", "paren"], "(", Class::Opener),
    (&["open", "parenthesis"], "(", Class::Opener),
    (&["left", "paren"], "(", Class::Opener),
    (&["close", "paren"], ")", Class::Closer),
    (&["close", "parenthesis"], ")", Class::Closer),
    (&["right", "paren"], ")", Class::Closer),
    (&["open", "bracket"], "[", Class::Opener),
    (&["close", "bracket"], "]", Class::Closer),
    (&["open", "brace"], "{", Class::Opener),
    (&["close", "brace"], "}", Class::Closer),
    (&["open", "quote"], "\"", Class::Opener),
    (&["close", "quote"], "\"", Class::Closer),
    (&["dollar", "sign"], "$", Class::Opener),
    (&["euro", "sign"], "€", Class::Opener),
    (&["less", "than"], "<", Class::Spaced),
    (&["greater", "than"], ">", Class::Spaced),
    (&["em", "dash"], "—", Class::Spaced),
    (&["at", "sign"], "@", Class::Tight),
    (&["hash", "sign"], "#", Class::Prefix),
    (&["pound", "sign"], "#", Class::Prefix),
    (&["percent", "sign"], "%", Class::Right),
    (&["forward", "slash"], "/", Class::Tight),
    (&["back", "slash"], "\\", Class::Tight),
    (&["double", "underscore"], "__", Class::Tight),
    (&["comma"], ",", Class::Right),
    (&["period"], ".", Class::Right),
    (&["bang"], "!", Class::Right),
    (&["colon"], ":", Class::Right),
    (&["semicolon"], ";", Class::Right),
    (&["ampersand"], "&", Class::Spaced),
    (&["pipe"], "|", Class::Spaced),
    (&["hash"], "#", Class::Prefix),
    (&["tilde"], "~", Class::Prefix),
    (&["underscore"], "_", Class::Tight),
    (&["backslash"], "\\", Class::Tight),
    (&["asterisk"], "*", Class::Tight),
    (&["caret"], "^", Class::Tight),
    (&["quote"], "\"", Class::Toggle),
    (&["backtick"], "`", Class::Toggle),
];

const ACTIONS: &[(&[&str], Action)] = &[
    (&["new", "paragraph"], Action::NewParagraph),
    (&["new", "line"], Action::NewLine),
    (&["newline"], Action::NewLine),
    (&["tab"], Action::Tab),
];

/// Words whose conversion stage 3 decides.
const GATED: &[&str] = &[
    "dot", "point", "slash", "dash", "hyphen", "plus", "equals", "minus", "times",
];

fn phrase_at(toks: &[Tok], i: usize, words: &[&str]) -> bool {
    words.len() <= toks.len() - i && words.iter().enumerate().all(|(k, w)| toks[i + k].is(w))
}

/// "literal ‹phrase›": hide the escape word and mark the phrase (an end
/// phrase, a symbol phrase or one word) so no stage touches it.
pub(super) fn literal_escape(toks: &mut [Tok], ctx: &Ctx) {
    let lit = ctx.phrases.literal.as_str();
    if lit.is_empty() {
        return;
    }
    let mut i = 0;
    while i < toks.len() {
        if toks[i].is(lit) && i + 1 < toks.len() && toks[i + 1].kind == Kind::Word {
            toks[i].kind = Kind::Hidden;
            let mut n = 1;
            for p in ctx.phrases.send.iter().chain(&ctx.phrases.scratch) {
                let ws: Vec<&str> = p.split_whitespace().collect();
                if !ws.is_empty() && phrase_at(toks, i + 1, &ws) {
                    n = n.max(ws.len());
                }
            }
            for (ws, _, _) in PHRASES {
                if phrase_at(toks, i + 1, ws) {
                    n = n.max(ws.len());
                }
            }
            for (ws, _) in ACTIONS {
                if phrase_at(toks, i + 1, ws) {
                    n = n.max(ws.len());
                }
            }
            for t in &mut toks[i + 1..i + 1 + n] {
                t.literal = true;
            }
            i += 1 + n;
        } else {
            i += 1;
        }
    }
}

/// Stage 2: unconditional phrases → symbols, actions → `Act` tokens.
pub(super) fn spoken(toks: &mut Vec<Tok>) {
    let mut out = Vec::with_capacity(toks.len());
    let mut open: [bool; 2] = [false; 2]; // `"`, `` ` ``
    let mut i = 0;
    while i < toks.len() {
        if !toks[i].is_free_word() {
            out.push(toks[i].clone());
            i += 1;
            continue;
        }
        if let Some((ws, a)) = ACTIONS.iter().find(|(ws, _)| phrase_at(toks, i, ws)) {
            out.push(Tok::action(*a));
            i += ws.len();
            continue;
        }
        if let Some((ws, sym, class)) = PHRASES.iter().find(|(ws, _, _)| phrase_at(toks, i, ws)) {
            let toggled = if matches!(class, Class::Toggle) {
                let slot = usize::from(*sym == "`");
                open[slot] = !open[slot];
                open[slot]
            } else {
                false
            };
            let (l, r) = class.glue(toggled);
            out.push(Tok::sym(*sym, l, r));
            i += ws.len();
            continue;
        }
        out.push(toks[i].clone());
        i += 1;
    }
    *toks = out;
}

fn single_letter(t: &Tok) -> bool {
    t.is_free_word() && t.text.len() == 1 && t.text.as_bytes()[0].is_ascii_alphabetic()
}

/// Stage 3, spelled letters: `S H` → `SH`; after "dot"/"point" any case joins lowercase (`sh`).
pub(super) fn spelled_letters(toks: &mut Vec<Tok>) {
    let mut i = 0;
    while i < toks.len() {
        let after_dot = i > 0 && (toks[i - 1].is("dot") || toks[i - 1].is("point"));
        let ok =
            |t: &Tok| single_letter(t) && (after_dot || t.text.as_bytes()[0].is_ascii_uppercase());
        let mut j = i;
        while j < toks.len() && ok(&toks[j]) {
            j += 1;
        }
        let n = j - i;
        if n >= 2 || (n == 1 && after_dot) {
            let joined: String = toks[i..j].iter().map(|t| t.text.as_str()).collect();
            let mut t = Tok::word(if after_dot {
                joined.to_lowercase()
            } else {
                joined
            });
            t.spelled = true;
            toks.splice(i..j, [t]);
        }
        i += 1;
    }
}

fn numeric(ctx: &Ctx, t: &Tok) -> bool {
    t.is_free_word() && (t.text.chars().all(|c| c.is_ascii_digit()) || ctx.number_word(&t.text))
}

/// A short operand for "plus"/"equals"/…: a number, a single letter or a
/// code-looking identifier ≤ 12 chars — never a plain prose word.
fn operand(ctx: &Ctx, t: &Tok) -> bool {
    if !t.is_free_word() || t.text.chars().count() > 12 {
        return false;
    }
    numeric(ctx, t)
        || single_letter(t)
        || t.text.contains(['_', '.', '/'])
        || t.text.chars().any(|c| c.is_ascii_digit())
        || t.text.chars().skip(1).any(|c| c.is_uppercase())
}

fn path_like(t: &Tok) -> bool {
    match t.kind {
        Kind::Sym => matches!(t.text.as_str(), "~" | "." | ".."),
        Kind::Word => {
            t.text.starts_with(['~', '/'])
                || t.text == "."
                || t.text == ".."
                || t.text.contains('/')
        }
        _ => false,
    }
}

/// Stage 3: the gated words. `is_final` false → a rule that needs the right
/// neighbour requires one more token after it (the partial's last word may
/// still be growing).
pub(super) fn gates(toks: &mut Vec<Tok>, ctx: &Ctx, is_final: bool) {
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if !t.is_free_word() || !GATED.iter().any(|g| t.is(g)) {
            i += 1;
            continue;
        }
        let right_ok = i + 1 < toks.len() && (is_final || i + 2 < toks.len());
        let left = i.checked_sub(1).map(|l| &toks[l]);
        let right = right_ok.then(|| &toks[i + 1]);
        let word = t.text.to_ascii_lowercase();
        match word.as_str() {
            "dot" | "point" => {
                let l_ok = left.is_some_and(|l| l.alnum());
                // "point" between numbers is stage 5's decimal.
                let r_ok = right.is_some_and(|r| {
                    r.spelled
                        || ctx.is_tld(&r.text)
                        || ctx.is_ext(&r.text)
                        || (word == "dot" && numeric(ctx, r))
                });
                if l_ok && r_ok {
                    toks[i] = Tok::sym(".", true, true);
                }
            }
            "slash" => {
                let chain = i >= 2 && (toks[i - 2].is_sym("/") || toks[i - 2].is_sym("."));
                let l_dot = left.is_some_and(|l| l.is("dot"));
                let l_ok = left.is_some_and(path_like) || chain || l_dot;
                let r_ok = right.is_some_and(path_like);
                if l_ok || r_ok {
                    toks[i] = Tok::sym("/", true, true);
                    if l_dot {
                        // "dot slash" → `./`, "dot dot slash" → `../`
                        let dd = i >= 2 && toks[i - 2].is("dot");
                        toks[i - 1] = Tok::sym(if dd { ".." } else { "." }, false, true);
                        if dd {
                            toks.remove(i - 2);
                            i -= 1;
                        }
                    }
                }
            }
            "dash" | "hyphen" => {
                let double = right.is_some_and(|r| r.is("dash") || r.is("hyphen"));
                let l_word = left.is_some_and(|l| l.alnum());
                let r_word = right.is_some_and(|r| r.alnum());
                if double && !l_word {
                    toks[i] = Tok::sym("--", false, true);
                    toks.remove(i + 1);
                } else if l_word && r_word && !double {
                    toks[i] = Tok::sym("-", true, true);
                } else if !l_word && r_word {
                    toks[i] = Tok::sym("-", false, true);
                }
            }
            _ => {
                let sym = match word.as_str() {
                    "plus" => "+",
                    "equals" => "=",
                    "minus" => "-",
                    _ => "*",
                };
                if left.is_some_and(|l| operand(ctx, l)) && right.is_some_and(|r| operand(ctx, r)) {
                    toks[i] = Tok::sym(sym, false, false);
                }
            }
        }
        i += 1;
    }
}

/// Stage 6: a model mark beside a converted symbol, or after an action, is
/// dropped (no `, ,`, no `\n.`); one before an action stays (`done.\n\n`).
pub(super) fn cleanup(toks: &mut Vec<Tok>) {
    let converted = |t: &Tok| t.kind == Kind::Sym && !t.asr;
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        let drop = t.asr
            && t.kind == Kind::Sym
            && ((i > 0 && (converted(&toks[i - 1]) || matches!(toks[i - 1].kind, Kind::Act(_))))
                || toks.get(i + 1).is_some_and(converted));
        if drop {
            toks.remove(i);
        } else {
            i += 1;
        }
    }
}
