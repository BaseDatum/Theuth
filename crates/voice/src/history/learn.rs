//! Correction-learning policy (153 §6.2, pure): which dictated → sent edits
//! are vocabulary, and when a pair is due for a suggestion card.

use super::Correction;

/// §6.2 `SINGLE_WORD`: a side longer than this is not a word.
pub const SINGLE_WORD_MAX_CHARS: usize = 40;
/// §6.2 `INFLECTIONS`: an edit that only adds or removes one of these is grammar, not vocabulary.
pub const INFLECTIONS: [&str; 6] = ["'s", "s", "es", "ies", "ed", "ing"];
/// §6.2 `SUGGEST_AFTER`: occurrences of a pair before a card is due.
pub const SUGGEST_AFTER: u32 = 2;
/// §6.2 `SUGGEST_AFTER`'s window and `DISMISS_COOLDOWN`, both 7 days.
pub const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1000;
/// §6.2 `IGNORES_PER_SESSION`.
pub const IGNORES_PER_SESSION: u32 = 3;
/// §6.4: a card left unanswered this long counts as ignored.
pub const CARD_TIMEOUT_MS: u64 = 20_000;

/// §6.3 step 1: the single-word edits between `heard` (delivered text) and `typed` (sent text).
///
/// Tokens are whitespace-split; an LCS alignment pairs each run of changed tokens, and a run that is
/// exactly one token on both sides is a candidate when it passes `SINGLE_WORD` and `INFLECTIONS`.
/// Insertions and deletions (typed-only or heard-only runs) are never vocabulary. `utterance` and
/// `at_ms` are `0`: the joined text has no utterance boundaries; the store stamps `at_ms`.
pub fn candidates(heard: &str, typed: &str) -> Vec<Correction> {
    let a: Vec<&str> = heard.split_whitespace().collect();
    let b: Vec<&str> = typed.split_whitespace().collect();
    // ponytail: O(n·m) LCS table; dictations are a few hundred tokens at most.
    let mut lcs = vec![vec![0u16; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < a.len() || j < b.len() {
        if i < a.len() && j < b.len() && a[i] == b[j] {
            i += 1;
            j += 1;
            continue;
        }
        // A changed run: advance both sides until the next common token.
        let (i0, j0) = (i, j);
        while i < a.len() || j < b.len() {
            if i < a.len() && j < b.len() && a[i] == b[j] {
                break;
            }
            if j >= b.len() || (i < a.len() && lcs[i + 1][j] >= lcs[i][j + 1]) {
                i += 1;
            } else {
                j += 1;
            }
        }
        if i - i0 == 1
            && j - j0 == 1
            && let Some((h, t)) = single_word(a[i0], b[j0])
        {
            out.push(Correction {
                utterance: 0,
                heard: h.to_owned(),
                typed: t.to_owned(),
                at_ms: 0,
            });
        }
    }
    out
}

/// `SINGLE_WORD` + `INFLECTIONS` on one token pair; returns the pair without surrounding punctuation.
fn single_word<'a>(heard: &'a str, typed: &'a str) -> Option<(&'a str, &'a str)> {
    let strip = |s: &'a str| s.trim_matches(|c: char| !c.is_alphanumeric());
    let (h, t) = (strip(heard), strip(typed));
    let word = |s: &str| {
        s.chars().count() <= SINGLE_WORD_MAX_CHARS
            && s.chars().filter(|c| c.is_alphanumeric()).count() >= 2
            && s.chars().any(char::is_alphabetic)
    };
    if h == t || !word(h) || !word(t) || inflection_only(h, t) {
        return None;
    }
    Some((h, t))
}

/// `a` and `b` differ only by one `INFLECTIONS` suffix (case-insensitive; `y` → `ies` included).
fn inflection_only(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_lowercase(), b.to_lowercase());
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    INFLECTIONS.iter().any(|suf| {
        long.strip_suffix(suf).is_some_and(|stem| {
            stem == short || (*suf == "ies" && short.strip_suffix('y') == Some(stem))
        })
    })
}

/// §6.3 step 3, the pure part: `SUGGEST_AFTER` occurrences of the pair (case-insensitive on `heard`,
/// exact on `typed`) within the last 7 days. Cooldown, never and the session's ignores live in the store.
pub fn due(heard: &str, typed: &str, history: &[Correction], now_ms: u64) -> bool {
    occurrences(heard, typed, history, now_ms) >= SUGGEST_AFTER
}

/// Occurrences of the pair within the 7-day window ending at `now_ms`.
pub fn occurrences(heard: &str, typed: &str, history: &[Correction], now_ms: u64) -> u32 {
    let since = now_ms.saturating_sub(WEEK_MS);
    history
        .iter()
        .filter(|c| {
            c.heard.eq_ignore_ascii_case(heard)
                && c.typed == typed
                && c.at_ms >= since
                && c.at_ms <= now_ms
        })
        .count() as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(heard: &str, typed: &str) -> Vec<(String, String)> {
        candidates(heard, typed)
            .into_iter()
            .map(|c| (c.heard, c.typed))
            .collect()
    }

    type Pairs = Vec<(String, String)>;

    fn one(h: &str, t: &str) -> Pairs {
        vec![(h.to_owned(), t.to_owned())]
    }

    #[test]
    fn candidates_single_word_non_inflection() {
        let table: &[(&str, &str, Pairs)] = &[
            // accepted
            (
                "i use github daily",
                "i use GitHub daily",
                one("github", "GitHub"),
            ),
            (
                "open the para keet model",
                "open the parakeet model",
                vec![],
            ), // 2→1 tokens: grammar
            (
                "run cargo clippy",
                "run Cargo clippy",
                one("cargo", "Cargo"),
            ),
            (
                "the file is in src",
                "the file is in lib",
                one("src", "lib"),
            ),
            ("ship it", "Ship it", one("ship", "Ship")),
            ("hello world.", "hello World.", one("world", "World")),
            (
                "use kubernetes",
                "use Kubernetes",
                one("kubernetes", "Kubernetes"),
            ),
            ("ok", "OK", one("ok", "OK")),
            // two independent single-word edits in one text
            (
                "github and kubernetes",
                "GitHub and Kubernetes",
                vec![
                    ("github".to_owned(), "GitHub".to_owned()),
                    ("kubernetes".to_owned(), "Kubernetes".to_owned()),
                ],
            ),
            // typed text has extra words around the dictation: insertions are not candidates
            ("fix the bug", "please fix the bug now", vec![]),
            // an edit adjacent to inserted text is ambiguous (which typed word is the heard one?): skipped
            ("fix the bug", "please fix the Bug now", vec![]),
            (
                "fix the bug now",
                "please fix the Bug now",
                one("bug", "Bug"),
            ),
            // rejected: inflections
            ("run the test", "run the tests", vec![]),
            ("run the tests", "run the test", vec![]),
            ("the box", "the boxes", vec![]),
            ("one party", "one parties", vec![]),
            ("they walk", "they walked", vec![]),
            ("they walk", "they walking", vec![]),
            ("the cat", "the cat's", vec![]),
            // rejected: not a word on one side
            ("send a note", "send 1 note", vec![]),
            ("a b", "a c", vec![]),
            ("version 12", "version 13", vec![]),
            // rejected: multi-word runs
            ("parakeet tee dee tee", "Parakeet TDT", vec![]),
            ("hello there", "goodbye now", vec![]),
            // rejected: deletions only
            ("send the very long message", "send the message", vec![]),
            // rejected: identical
            ("same text", "same text", vec![]),
            // rejected: over 40 chars
            (
                "x aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa y",
                "x bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb y",
                vec![],
            ),
        ];
        for (heard, typed, want) in table {
            assert_eq!(&pairs(heard, typed), want, "{heard:?} → {typed:?}");
        }
    }

    fn at(ms: u64) -> Correction {
        Correction {
            utterance: 0,
            heard: "Github".into(),
            typed: "GitHub".into(),
            at_ms: ms,
        }
    }

    #[test]
    fn due_after_2_in_7_days() {
        let now = 10 * WEEK_MS;
        assert!(!due("github", "GitHub", &[at(now)], now));
        assert!(due(
            "github",
            "GitHub",
            &[at(now - WEEK_MS + 1), at(now)],
            now
        ));
        // one of the two fell out of the window
        assert!(!due(
            "github",
            "GitHub",
            &[at(now - WEEK_MS - 1), at(now)],
            now
        ));
        // heard is case-insensitive, typed is exact
        assert!(due("GITHUB", "GitHub", &[at(now), at(now)], now));
        assert!(!due("github", "Github", &[at(now), at(now)], now));
        assert_eq!(
            occurrences("github", "GitHub", &[at(now), at(now), at(0)], now),
            2
        );
    }
}
