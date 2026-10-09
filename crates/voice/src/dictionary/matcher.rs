//! The pure matcher (153 §5.4): candidates by exact spoken form and Double
//! Metaphone, scored with normalised Levenshtein, accepted against the W-k
//! thresholds in the parent module.
//!
//! Every form (canonical, spoken, heard) is keyed by [`norm`]: lowercase
//! alphanumerics with digit words folded to digits, so `git hub`, `GitHub`,
//! `python three` / `python3` and `k eight s` / `k8s` share one key without
//! storing derived spoken forms.

use std::collections::HashMap;

use rphonetic::DoubleMetaphone;

use super::{EDIT_MAX, Entry};

/// Candidate cap per window (§5.4 step 1).
const MAX_CANDIDATES: usize = 32;
/// Metaphone code length: longer than the default 4 so a 40k-term bucket stays small.
const METAPHONE_LEN: usize = 8;

/// Lowercase alphanumerics only; whole-word digit names → digits.
pub fn norm(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for tok in s.split(|c: char| !c.is_alphanumeric()) {
        if tok.is_empty() {
            continue;
        }
        let lc = tok.to_lowercase();
        out.push_str(digit(&lc).unwrap_or(&lc));
    }
    out
}

fn digit(w: &str) -> Option<&'static str> {
    Some(match w {
        "zero" => "0",
        "one" => "1",
        "two" => "2",
        "three" => "3",
        "four" => "4",
        "five" => "5",
        "six" => "6",
        "seven" => "7",
        "eight" => "8",
        "nine" => "9",
        _ => return None,
    })
}

/// Double Metaphone (primary, alternate) of a normalised form; empty codes for digit-only input.
/// `rphonetic` slices by byte, so only the ASCII letters are encoded (a CJK cask name gets no code).
pub fn metaphone(norm: &str) -> (String, String) {
    let ascii: String = norm.chars().filter(char::is_ascii_alphabetic).collect();
    if ascii.is_empty() {
        return (String::new(), String::new());
    }
    let r = DoubleMetaphone::new(Some(METAPHONE_LEN)).double_metaphone(&ascii);
    (r.primary(), r.alternate())
}

/// §5.4 step 2: `1 − lev(norm(a), norm(b)) / max(len)`, plus 0.1 for a phonetic hit within `EDIT_MAX`.
pub fn score(a: &str, b: &str) -> f32 {
    let (a, b) = (norm(a), norm(b));
    let (pa, pb) = (metaphone(&a), metaphone(&b));
    let phonetic = !pa.0.is_empty() && (pa.0 == pb.0 || pa.0 == pb.1 || pa.1 == pb.0);
    score_norm(&a, &b, phonetic)
}

/// [`score`] over already-normalised forms, with the phonetic hit decided by the caller; capped at 1.0.
pub fn score_norm(a: &str, b: &str, phonetic: bool) -> f32 {
    let max = a.chars().count().max(b.chars().count());
    if max == 0 {
        return 0.0;
    }
    let lev = strsim::levenshtein(a, b);
    let mut s = 1.0 - lev as f32 / max as f32;
    if phonetic && lev <= EDIT_MAX {
        s += 0.1;
    }
    s.min(1.0)
}

/// One candidate: entry position in the indexed slice, and which buckets found it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// Position in the indexed entry slice.
    pub idx: u32,
    /// A form normalises to the heard key (score 1.0 without scoring).
    pub exact: bool,
    /// Found through a metaphone bucket (the §5.4 +0.1 applies within `EDIT_MAX`).
    pub phonetic: bool,
}

/// The §5.4 step-7 buckets over one entry slice: `norm(form)` → entries and metaphone code → entries.
#[derive(Clone, Debug, Default)]
pub struct Index {
    exact: HashMap<String, Vec<u32>>,
    phon: HashMap<String, Vec<u32>>,
}

impl Index {
    /// Index canonical, spoken and heard forms of every entry.
    pub fn build(entries: &[Entry]) -> Index {
        let mut ix = Index::default();
        for (i, e) in entries.iter().enumerate() {
            let i = i as u32;
            ix.add(i, &norm(&e.canonical), None);
            for s in &e.spoken {
                ix.add(i, &norm(s), None);
            }
            for v in &e.variants {
                let codes = (!v.metaphone.0.is_empty()).then_some(&v.metaphone);
                ix.add(i, &norm(&v.heard), codes);
            }
        }
        ix
    }

    fn add(&mut self, idx: u32, key: &str, codes: Option<&(String, String)>) {
        if key.is_empty() {
            return;
        }
        push(&mut self.exact, key, idx);
        let computed;
        let codes = match codes {
            Some(c) => c,
            None => {
                computed = metaphone(key);
                &computed
            }
        };
        if !codes.0.is_empty() {
            push(&mut self.phon, &codes.0, idx);
        }
        if !codes.1.is_empty() && codes.1 != codes.0 {
            push(&mut self.phon, &codes.1, idx);
        }
    }

    /// Exact bucket first, then the two metaphone buckets; ≤ [`MAX_CANDIDATES`], each entry once.
    pub fn candidates(&self, key: &str, codes: &(String, String)) -> Vec<Candidate> {
        let mut out: Vec<Candidate> = Vec::new();
        let mut take = |bucket: Option<&Vec<u32>>, phonetic: bool| {
            for &idx in bucket.into_iter().flatten() {
                if out.len() >= MAX_CANDIDATES {
                    return;
                }
                match out.iter_mut().find(|c| c.idx == idx) {
                    Some(c) => c.phonetic |= phonetic,
                    None => out.push(Candidate {
                        idx,
                        exact: !phonetic,
                        phonetic,
                    }),
                }
            }
        };
        take(self.exact.get(key), false);
        if !codes.0.is_empty() {
            take(self.phon.get(&codes.0), true);
        }
        if !codes.1.is_empty() && codes.1 != codes.0 {
            take(self.phon.get(&codes.1), true);
        }
        out
    }
}

fn push(map: &mut HashMap<String, Vec<u32>>, key: &str, idx: u32) {
    let v = map.entry(key.to_owned()).or_default();
    if v.last() != Some(&idx) {
        v.push(idx);
    }
}
