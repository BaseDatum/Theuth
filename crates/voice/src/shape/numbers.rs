//! Stage 5 (153 §4.2): cardinals ≤ 6 digits, "point" decimals, percent,
//! ordinals only after "number". No wider inverse text normalisation.

use super::tokens::{Kind, Tok};

const UNITS: &[&str] = &[
    "zero", "one", "two", "three", "four", "five", "six", "seven", "eight", "nine",
];
const TEENS: &[&str] = &[
    "ten",
    "eleven",
    "twelve",
    "thirteen",
    "fourteen",
    "fifteen",
    "sixteen",
    "seventeen",
    "eighteen",
    "nineteen",
];
const TENS: &[&str] = &[
    "twenty", "thirty", "forty", "fifty", "sixty", "seventy", "eighty", "ninety",
];
const ORDINALS: &[(&str, &str)] = &[
    ("first", "1st"),
    ("second", "2nd"),
    ("third", "3rd"),
    ("fourth", "4th"),
    ("fifth", "5th"),
    ("sixth", "6th"),
    ("seventh", "7th"),
    ("eighth", "8th"),
    ("ninth", "9th"),
    ("tenth", "10th"),
    ("eleventh", "11th"),
    ("twelfth", "12th"),
];
const MAX: u64 = 999_999;

/// `w` is a number word (hyphenated pairs like "twenty-three" included).
pub(super) fn is_number_word(w: &str) -> bool {
    w.split('-').all(|p| {
        let p = p.to_ascii_lowercase();
        UNITS.contains(&p.as_str())
            || TEENS.contains(&p.as_str())
            || TENS.contains(&p.as_str())
            || matches!(p.as_str(), "hundred" | "thousand" | "million")
    })
}

/// Compose a run of number words; `None` when the run is not a well-formed
/// cardinal ≤ 6 digits ("eighteen hundred" stays, times are not numbers).
fn compose(words: &[&str]) -> Option<u64> {
    let parts: Vec<String> = words
        .iter()
        .flat_map(|w| w.split('-'))
        .map(str::to_ascii_lowercase)
        .collect();
    // A string of single digits ("one five three") is read digit by digit.
    if parts.len() >= 2 && parts.iter().all(|p| UNITS.contains(&p.as_str())) {
        let s: Option<String> = parts
            .iter()
            .map(|p| {
                UNITS
                    .iter()
                    .position(|u| u == p)
                    .map(|d| char::from(b'0' + d as u8))
            })
            .collect();
        return s.filter(|s| s.len() <= 6).and_then(|s| s.parse().ok());
    }
    let mut total = 0u64;
    let mut cur = 0u64;
    let mut last_small = 0u64; // the small number most recently added (for ordering checks)
    for p in &parts {
        let p = p.as_str();
        if let Some(u) = UNITS.iter().position(|w| *w == p) {
            if last_small != 0 && last_small < 20 {
                return None; // "three four"
            }
            cur += u as u64;
            last_small = u as u64;
        } else if let Some(t) = TEENS.iter().position(|w| *w == p) {
            if last_small != 0 {
                return None;
            }
            cur += 10 + t as u64;
            last_small = 10 + t as u64;
        } else if let Some(t) = TENS.iter().position(|w| *w == p) {
            if last_small != 0 {
                return None;
            }
            cur += 20 + 10 * t as u64;
            last_small = cur % 100;
        } else if p == "hundred" {
            if cur == 0 || cur > 9 {
                return None;
            }
            cur *= 100;
            last_small = 0;
        } else {
            let scale = if p == "thousand" { 1_000 } else { 1_000_000 };
            if cur == 0 || (total != 0 && scale >= total) {
                return None;
            }
            total += cur * scale;
            cur = 0;
            last_small = 0;
        }
    }
    let n = total + cur;
    (n <= MAX).then_some(n)
}

/// Digits of a decimal part: "twenty five" → "25", "zero five" → "05".
fn fraction(words: &[&str]) -> Option<String> {
    let parts: Vec<String> = words.iter().map(|w| w.to_ascii_lowercase()).collect();
    if parts.iter().all(|p| UNITS.contains(&p.as_str())) {
        return parts
            .iter()
            .map(|p| {
                UNITS
                    .iter()
                    .position(|u| u == p)
                    .map(|d| char::from(b'0' + d as u8))
            })
            .collect();
    }
    compose(words).map(|n| n.to_string())
}

fn run_end(toks: &[Tok], start: usize) -> usize {
    let mut j = start;
    while j < toks.len()
        && toks[j].is_free_word()
        && !toks[j].spelled
        && is_number_word(&toks[j].text)
    {
        j += 1;
    }
    j
}

fn is_digits(t: &Tok) -> bool {
    t.kind == Kind::Word
        && !t.text.is_empty()
        && t.text.chars().all(|c| c.is_ascii_digit() || c == '.')
}

pub(super) fn numbers(toks: &mut Vec<Tok>) {
    let mut i = 0;
    while i < toks.len() {
        // Ordinals: only after "number".
        if i > 0
            && toks[i - 1].is("number")
            && let Some((_, d)) = ORDINALS.iter().find(|(w, _)| toks[i].is(w))
        {
            toks[i].text = (*d).to_owned();
            i += 1;
            continue;
        }
        // "percent" after a number → `%`.
        if toks[i].is("percent") && i > 0 && is_digits(&toks[i - 1]) {
            toks[i] = Tok::sym("%", true, false);
            i += 1;
            continue;
        }
        let j = run_end(toks, i);
        if j == i {
            i += 1;
            continue;
        }
        let words: Vec<&str> = toks[i..j].iter().map(|t| t.text.as_str()).collect();
        // 201 (shaper audit): a lone number word is prose ("one or two of them", "for one
        // thing", "the two of us"); the model writes real quantities as digits itself. It
        // converts only after "number" or beside a spoken symbol (`#`, `.`, an operator),
        // "point" or "percent".
        let beside_sym = |t: &Tok| t.kind == Kind::Sym && !t.asr && (t.glue_right || !t.glue_left);
        // A hyphenated compound ("twenty-three") is two number words in one token, not a lone one.
        let lone = words.len() == 1
            && !words[0].contains('-')
            && !(i > 0
                && ((toks[i - 1].kind == Kind::Sym && !toks[i - 1].asr)
                    || toks[i - 1].is("number")))
            && !toks
                .get(j)
                .is_some_and(|t| beside_sym(t) || t.is("point") || t.is("percent"));
        let Some(n) = compose(&words).filter(|_| !lone) else {
            i = j;
            continue;
        };
        let mut text = n.to_string();
        let mut end = j;
        // "‹int› point ‹digits›" or a converted tight `.` between runs → decimal.
        let point = toks
            .get(j)
            .is_some_and(|t| t.is("point") || (t.is_sym(".") && t.glue_right));
        if point {
            let k = run_end(toks, j + 1);
            let frac: Vec<&str> = toks[j + 1..k].iter().map(|t| t.text.as_str()).collect();
            if let Some(f) = fraction(&frac).filter(|_| k > j + 1) {
                text = format!("{n}.{f}");
                end = k;
            }
        }
        let mut t = Tok::word(text);
        t.glue_left = toks[i].glue_left;
        t.glue_right = toks[end - 1].glue_right;
        toks.splice(i..end, [t]);
        i += 1;
    }
}
