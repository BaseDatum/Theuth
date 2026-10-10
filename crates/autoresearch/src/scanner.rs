//! W1 MEASURE — the output grammar (design doc 140 §5.1; 159 §5.3 CASE v2 and SPEND):
//! `METRIC <name>=<number>`, `CASE <id> <name>=<number> [<name>=<number> …]`,
//! `SPEND <usd> [<label>]`, `ASI <key>=<text>`, scanned line by line while streaming (O(line)
//! memory), last occurrence wins (`SPEND` adds up), `nan`/`inf` → crash, CRLF tolerated, ASI
//! capped at 2 KiB per sample, 16 pairs per `CASE` line, 2 000 case ids per sample.

use std::collections::BTreeMap;

/// Total bytes of `ASI` text kept per sample (140 §5.1).
pub const ASI_CAP: usize = 2048;
/// Pairs read from one `CASE` line (159 §5.3); more are ignored.
pub const CASE_PAIRS: usize = 16;
/// Case ids kept per sample (159 §5.3); more are ignored.
pub const CASE_IDS: usize = 2000;
/// `SPEND` label length kept.
pub const SPEND_LABEL: usize = 40;

/// A partial line longer than this without a newline is dropped: no contract
/// line is that long, and holding it would break the O(line) promise.
const MAX_LINE: usize = 1 << 20;

/// What one sample's output said.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Scanned {
    /// Every `METRIC` line, last wins.
    pub metrics: BTreeMap<String, f64>,
    /// Every `CASE` line keyed by id, last wins.
    pub cases: BTreeMap<String, f64>,
    /// 159 §5.3: every `CASE` pair, `id → name → value`, last wins.
    pub case_values: BTreeMap<String, BTreeMap<String, f64>>,
    /// 159 §5.3: Σ `SPEND` lines, USD nanos.
    pub spend_nanos: u64,
    /// 159 §5.3: `SPEND` labels in order (≤ 40 chars each), for the card.
    pub spend_labels: Vec<String>,
    /// Every `ASI` line, capped.
    pub asi: BTreeMap<String, String>,
    /// A `METRIC` value that was not finite (→ crash).
    pub bad_number: Option<String>,
}

/// Incremental scanner: feed bytes as they arrive, read the result at the end.
#[derive(Debug, Default)]
pub struct Scanner {
    pending: Vec<u8>,
    out: Scanned,
}

impl Scanner {
    /// Empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk; complete lines are scanned at once, the tail is kept.
    pub fn feed(&mut self, chunk: &[u8]) {
        let mut rest = chunk;
        while let Some(nl) = rest.iter().position(|&b| b == b'\n') {
            let (head, after) = rest.split_at(nl);
            rest = &after[1..];
            if self.pending.is_empty() {
                self.line(head);
            } else {
                self.pending.extend_from_slice(head);
                let line = std::mem::take(&mut self.pending);
                self.line(&line);
            }
        }
        if self.pending.len() + rest.len() > MAX_LINE {
            // ponytail: an over-long partial line is dropped whole; a contract line never is.
            self.pending.clear();
            self.pending.push(b' '); // poison: the rest of this line is not a contract line either
        } else {
            self.pending.extend_from_slice(rest);
        }
    }

    fn line(&mut self, raw: &[u8]) {
        if let Ok(s) = std::str::from_utf8(raw) {
            scan_line(s, &mut self.out);
        }
    }

    /// Scan the pending partial line and return everything seen.
    pub fn finish(mut self) -> Scanned {
        let line = std::mem::take(&mut self.pending);
        self.line(&line);
        self.out
    }
}

/// Scan one line (the unit `feed` applies); `None` when it is not a contract line.
pub fn scan_line(line: &str, into: &mut Scanned) -> bool {
    let line = line.strip_suffix('\r').unwrap_or(line);
    if let Some(rest) = line.strip_prefix("METRIC ") {
        let Some((name, value)) = kv(rest) else {
            return false;
        };
        match number(value) {
            Some(v) => {
                into.metrics.insert(name.to_owned(), v);
            }
            None => into.bad_number = Some(format!("{name}={value}")),
        }
        true
    } else if let Some(rest) = line.strip_prefix("CASE ") {
        let rest = rest.trim_start();
        let Some((id, kv_part)) = rest.split_once(char::is_whitespace) else {
            return false;
        };
        // 159 §5.3 CASE v2: one or more `name=number` pairs. A line with one `=` is a v1 line
        // and takes the old path exactly (`kv` trims around the `=`); more pairs split on
        // whitespace. `cases` keeps the id's last value as today; `case_values` keeps every name.
        let known = into.cases.contains_key(id);
        if !known && into.cases.len() >= CASE_IDS {
            return true;
        }
        let pairs: Vec<&str> = if kv_part.matches('=').count() <= 1 {
            vec![kv_part]
        } else {
            kv_part.split_whitespace().take(CASE_PAIRS).collect()
        };
        for pair in pairs {
            let Some((name, value)) = kv(pair) else {
                return false;
            };
            match number(value) {
                Some(v) => {
                    into.cases.insert(id.to_owned(), v);
                    into.case_values
                        .entry(id.to_owned())
                        .or_default()
                        .insert(name.to_owned(), v);
                }
                None => {
                    into.bad_number = Some(format!("{id} {kv_part}"));
                    return true;
                }
            }
        }
        true
    } else if let Some(rest) = line.strip_prefix("SPEND ") {
        // 159 §5.3: `SPEND <usd> [<label>]`, finite and ≥ 0, lines add up; a bad number is a crash.
        let rest = rest.trim();
        let (usd, label) = rest
            .split_once(char::is_whitespace)
            .map_or((rest, ""), |(u, l)| (u, l.trim()));
        match number(usd).filter(|v| *v >= 0.0) {
            Some(v) => {
                into.spend_nanos = into.spend_nanos.saturating_add((v * 1e9).round() as u64);
                let mut label = label.to_owned();
                if label.len() > SPEND_LABEL {
                    let mut cut = SPEND_LABEL;
                    while !label.is_char_boundary(cut) {
                        cut -= 1;
                    }
                    label.truncate(cut);
                }
                into.spend_labels.push(label);
            }
            None => into.bad_number = Some(format!("SPEND {rest}")),
        }
        true
    } else if let Some(rest) = line.strip_prefix("ASI ") {
        let Some((key, text)) = kv(rest) else {
            return false;
        };
        let used: usize = into
            .asi
            .iter()
            .map(|(k, v)| k.len() + v.len())
            .sum::<usize>()
            - into.asi.get(key).map_or(0, |v| key.len() + v.len());
        let room = ASI_CAP.saturating_sub(used + key.len());
        if room == 0 {
            return true;
        }
        let mut text = text.to_owned();
        if text.len() > room {
            let mut cut = room;
            while !text.is_char_boundary(cut) {
                cut -= 1;
            }
            text.truncate(cut);
        }
        into.asi.insert(key.to_owned(), text);
        true
    } else {
        false
    }
}

/// `name=value` with a non-empty name (both trimmed).
fn kv(s: &str) -> Option<(&str, &str)> {
    let (name, value) = s.split_once('=')?;
    let name = name.trim();
    (!name.is_empty()).then(|| (name, value.trim()))
}

/// A finite `f64`, else `None` (`nan`, `inf`, text, units).
fn number(s: &str) -> Option<f64> {
    s.parse::<f64>().ok().filter(|v| v.is_finite())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(text: &str) -> Scanned {
        let mut s = Scanner::new();
        s.feed(text.as_bytes());
        s.finish()
    }

    #[test]
    fn grammar_last_wins_and_crlf() {
        let out = scan(
            "hello\r\nMETRIC time_ms=42.5\r\nMETRIC time_ms=41.0\nCASE n500 time_ms=3\nCASE n500 time_ms=4\nASI cache=hit 90%\nMETRIC peak_kb = 1024 \nnot a line\n",
        );
        assert_eq!(out.metrics["time_ms"], 41.0);
        assert_eq!(out.metrics["peak_kb"], 1024.0);
        assert_eq!(out.cases["n500"], 4.0);
        assert_eq!(out.asi["cache"], "hit 90%");
        assert_eq!(out.bad_number, None);
    }

    /// 159 §5.3: a v1 line is a v2 line with one pair; names are kept; several pairs per id.
    #[test]
    fn case_v2_keeps_names_and_v1_lines_unchanged() {
        let out = scan(
            "CASE n500 time_ms=3\nCASE n500 time_ms= 4 \nCASE n900 time_ms=7 peak_kb=12 pass=1\nCASE n900 pass=0\n",
        );
        assert_eq!(out.cases["n500"], 4.0, "v1: last value, trimmed around `=`");
        assert_eq!(out.case_values["n500"]["time_ms"], 4.0);
        assert_eq!(out.cases["n900"], 0.0, "the id's last value");
        assert_eq!(out.case_values["n900"]["time_ms"], 7.0);
        assert_eq!(out.case_values["n900"]["peak_kb"], 12.0);
        assert_eq!(out.case_values["n900"]["pass"], 0.0);
        assert_eq!(out.bad_number, None);
        let bad = scan("CASE n1 a=1 b=nan\n");
        assert_eq!(bad.bad_number.as_deref(), Some("n1 a=1 b=nan"));
        let many = (0..CASE_PAIRS + 4)
            .map(|i| format!("p{i}={i}"))
            .collect::<Vec<_>>()
            .join(" ");
        let capped = scan(&format!("CASE x {many}\n"));
        assert_eq!(capped.case_values["x"].len(), CASE_PAIRS);
    }

    /// 159 §5.3: `SPEND` sums, keeps ≤ 40-char labels, and a bad or negative number is a crash.
    #[test]
    fn spend_sums_caps_labels_and_crashes_on_bad_numbers() {
        let out = scan(
            "SPEND 0.25 judge\nSPEND 0.5\nSPEND 0.125 a-very-long-label-that-goes-on-and-on-past-forty-chars\n",
        );
        assert_eq!(out.spend_nanos, 875_000_000);
        assert_eq!(out.spend_labels.len(), 3);
        assert_eq!(out.spend_labels[0], "judge");
        assert_eq!(out.spend_labels[1], "");
        assert_eq!(out.spend_labels[2].len(), SPEND_LABEL);
        assert_eq!(out.bad_number, None);
        assert_eq!(scan("SPEND -1\n").bad_number.as_deref(), Some("SPEND -1"));
        assert_eq!(
            scan("SPEND lots\n").bad_number.as_deref(),
            Some("SPEND lots")
        );
        assert_eq!(scan("METRIC t=1\n").spend_nanos, 0);
    }

    #[test]
    fn nan_and_inf_are_bad_numbers() {
        assert_eq!(scan("METRIC t=nan\n").bad_number.as_deref(), Some("t=nan"));
        assert_eq!(scan("METRIC t=inf\n").bad_number.as_deref(), Some("t=inf"));
        assert_eq!(
            scan("METRIC t=12ms\n").bad_number.as_deref(),
            Some("t=12ms")
        );
        assert!(scan("METRIC t=nan\n").metrics.is_empty());
    }

    #[test]
    fn partial_lines_across_chunks_and_final_line_without_newline() {
        let mut s = Scanner::new();
        s.feed(b"METR");
        s.feed(b"IC a=1\nMETRIC b");
        s.feed(b"=2");
        let out = s.finish();
        assert_eq!(out.metrics["a"], 1.0);
        assert_eq!(out.metrics["b"], 2.0);
    }

    #[test]
    fn asi_capped_at_2kib() {
        let big = "x".repeat(3000);
        let out = scan(&format!("ASI a={big}\nASI b=more\n"));
        let total: usize = out.asi.iter().map(|(k, v)| k.len() + v.len()).sum();
        assert!(total <= ASI_CAP, "{total}");
        assert_eq!(out.asi["a"].len(), ASI_CAP - 1);
        assert!(!out.asi.contains_key("b"));
    }

    #[test]
    fn bounded_memory_on_a_huge_stream() {
        // 200 MB of noise with one contract line at the end: the scanner holds
        // at most one line (and never a line past MAX_LINE).
        let mut s = Scanner::new();
        let noise = vec![b'z'; 1 << 16];
        for _ in 0..(200 * 16) {
            s.feed(&noise);
            assert!(s.pending.len() <= MAX_LINE + 1);
        }
        s.feed(b"\nMETRIC t=7\n");
        let out = s.finish();
        assert_eq!(out.metrics["t"], 7.0);
    }

    #[test]
    fn not_contract_lines() {
        let mut out = Scanned::default();
        assert!(!scan_line("METRICS t=1", &mut out));
        assert!(!scan_line("METRIC =1", &mut out));
        assert!(!scan_line("CASE n500", &mut out));
        assert!(!scan_line("ASI novalue", &mut out));
        assert_eq!(out, Scanned::default());
    }
}
