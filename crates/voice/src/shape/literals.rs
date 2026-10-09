//! Stage 4 (153 §4.2): `/commands`, `@mentions`, flag joins after `-`/`--`
//! and the spoken casing formatters.

use super::Ctx;
use super::tokens::Tok;

/// A flag join stops here (function words and the words that describe a flag).
// ponytail: fixed stop list; the real fix is a POS gate, add when "--foo bar" joins wrongly in the field.
const STOP: &[&str] = &[
    "and", "then", "to", "the", "a", "an", "on", "in", "for", "with", "of", "is", "it", "that",
    "this", "or", "but", "at", "by", "from", "flag", "flags", "option", "options", "switch",
    "argument", "arg", "args",
];

const FORMATTERS: &[(&[&str], Case)] = &[
    (&["camel", "case"], Case::Camel),
    (&["snake", "case"], Case::Snake),
    (&["kebab", "case"], Case::Kebab),
    (&["pascal", "case"], Case::Pascal),
    (&["constant", "case"], Case::Constant),
    (&["all", "caps"], Case::AllCaps),
    (&["dotted"], Case::Dotted),
];

/// Longest formatter run.
const MAX_WORDS: usize = 4;

#[derive(Clone, Copy)]
enum Case {
    Camel,
    Snake,
    Kebab,
    Pascal,
    Constant,
    AllCaps,
    Dotted,
}

fn cap(w: &str) -> String {
    let mut cs = w.chars();
    cs.next()
        .map(|c| {
            c.to_uppercase()
                .chain(cs.flat_map(char::to_lowercase))
                .collect()
        })
        .unwrap_or_default()
}

fn apply(case: Case, words: &[&str]) -> String {
    let lower = || words.iter().map(|w| w.to_lowercase()).collect::<Vec<_>>();
    match case {
        Case::Camel => words
            .iter()
            .enumerate()
            .map(|(i, w)| if i == 0 { w.to_lowercase() } else { cap(w) })
            .collect(),
        Case::Pascal => words.iter().map(|w| cap(w)).collect(),
        Case::Snake => lower().join("_"),
        Case::Kebab => lower().join("-"),
        Case::Dotted => lower().join("."),
        Case::Constant => lower().join("_").to_uppercase(),
        Case::AllCaps => words.join(" ").to_uppercase(),
    }
}

fn lowercase_word(t: &Tok) -> bool {
    t.is_free_word() && t.alnum() && t.text.chars().all(|c| !c.is_uppercase())
}

pub(super) fn literals(toks: &mut Vec<Tok>, ctx: &Ctx) {
    let mut i = 0;
    while i < toks.len() {
        // "slash ‹command›" at the start or after a lead-in.
        if toks[i].is("slash")
            && toks
                .get(i + 1)
                .is_some_and(|r| r.is_free_word() && ctx.is_command(&r.text))
            && (i == 0 || ["type", "run", "send"].iter().any(|w| toks[i - 1].is(w)))
        {
            let cmd = format!("/{}", toks[i + 1].text);
            toks.splice(i..i + 2, [Tok::word(cmd)]);
            i += 1;
            continue;
        }
        // "at ‹agent›" → @agent; "at theuth's" is not the name.
        if toks[i].is("at")
            && toks
                .get(i + 1)
                .is_some_and(|r| r.is_free_word() && ctx.is_mention(&r.text))
        {
            let m = format!("@{}", toks[i + 1].text);
            toks.splice(i..i + 2, [Tok::word(m)]);
            i += 1;
            continue;
        }
        // Flags: after a prefix `-`/`--`, the next 1–3 lowercase words join with `-`.
        if (toks[i].is_sym("-") || toks[i].is_sym("--")) && toks[i].glue_right && !toks[i].glue_left
        {
            let mut j = i + 1;
            while j < toks.len()
                && j - (i + 1) < 3
                && lowercase_word(&toks[j])
                && !STOP.iter().any(|s| toks[j].is(s))
            {
                j += 1;
            }
            if j - (i + 1) >= 2 {
                let joined = toks[i + 1..j]
                    .iter()
                    .map(|t| t.text.as_str())
                    .collect::<Vec<_>>()
                    .join("-");
                toks.splice(i + 1..j, [Tok::word(joined)]);
            }
            i += 1;
            continue;
        }
        // Casing formatters: consume ≤ 4 following words, stop at anything else.
        if let Some((ws, case)) = FORMATTERS.iter().find(|(ws, _)| {
            ws.iter()
                .enumerate()
                .all(|(k, w)| toks.get(i + k).is_some_and(|t| t.is(w)))
        }) {
            let start = i + ws.len();
            let mut j = start;
            while j < toks.len()
                && j - start < MAX_WORDS
                && toks[j].is_free_word()
                && toks[j].alnum()
            {
                j += 1;
            }
            if j > start {
                let words: Vec<&str> = toks[start..j].iter().map(|t| t.text.as_str()).collect();
                let out = Tok::word(apply(*case, &words));
                toks.splice(i..j, [out]);
            }
            i += 1;
            continue;
        }
        i += 1;
    }
}
