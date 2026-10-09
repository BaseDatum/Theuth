//! Stage 7 (153 §4.2): dictionary rewrites over 1–3-word windows, longest
//! first, through [`Lookup`] so the stage runs against A2's matcher or a
//! test fake.

use crate::dictionary::{Dictionary, Match};

use super::RuleId;
use super::tokens::{Kind, Tok};

/// The one dictionary call the shaper makes (§5.4's `Dictionary::lookup`).
pub(crate) trait Lookup {
    /// The best accepted match for `heard` (one 1–3-token window).
    fn lookup(&self, heard: &str) -> Option<Match>;
}

impl Lookup for Dictionary {
    fn lookup(&self, heard: &str) -> Option<Match> {
        Dictionary::lookup(self, heard)
    }
}

/// A symbol that fuses its neighbours into one path/identifier.
fn joiner(t: &Tok) -> bool {
    t.kind == Kind::Sym
        && matches!(
            t.text.as_str(),
            "." | ".." | "/" | "\\" | "-" | "--" | "_" | "__" | "@" | "~" | "#"
        )
}

/// Free words not fused to a symbol (path pieces and identifiers stay as said).
fn eligible(toks: &[Tok], i: usize) -> bool {
    toks[i].is_free_word()
        && toks[i].replaced.is_none()
        && !(i > 0 && joiner(&toks[i - 1]) && toks[i - 1].glue_right)
        && !toks.get(i + 1).is_some_and(|n| joiner(n) && n.glue_left)
}

pub(super) fn dictionary(toks: &mut Vec<Tok>, dict: &dyn Lookup) {
    let mut i = 0;
    while i < toks.len() {
        if !eligible(toks, i) {
            i += 1;
            continue;
        }
        let mut run = 1;
        while run < 3 && i + run < toks.len() && eligible(toks, i + run) {
            run += 1;
        }
        let mut hit = None;
        for n in (1..=run).rev() {
            let heard = toks[i..i + n]
                .iter()
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            // Guard: a window already equal to its canonical is a completed occurrence, never re-expanded.
            if let Some(m) = dict.lookup(&heard).filter(|m| m.canonical != heard) {
                hit = Some((n, heard, m));
                break;
            }
        }
        let Some((n, heard, m)) = hit else {
            i += 1;
            continue;
        };
        let mut t = Tok::word(m.canonical);
        t.replaced = Some((heard, RuleId(m.entry.0)));
        toks.splice(i..i + n, [t]);
        i += 1;
    }
}
