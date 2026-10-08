//! W1 MEASURE — the output grammar (design doc 140 §5.1): `METRIC <name>=<number>`,
//! `CASE <id> <name>=<number>`, `ASI <key>=<text>`, scanned line by line while
//! streaming (O(line) memory), last occurrence wins, `nan`/`inf` → crash,
//! CRLF tolerated, ASI capped at 2 KiB per sample.

use std::collections::BTreeMap;

/// Total bytes of `ASI` text kept per sample (140 §5.1).
pub const ASI_CAP: usize = 2048;

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
        let Some((_name, value)) = kv(kv_part) else {
            return false;
        };
        match number(value) {
            Some(v) => {
                into.cases.insert(id.to_owned(), v);
            }
            None => into.bad_number = Some(format!("{id} {kv_part}")),
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
