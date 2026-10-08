//! W1 MEASURE — the output grammar (design doc 140 §5.1): `METRIC <name>=<number>`,
//! `CASE <id> <name>=<number>`, `ASI <key>=<text>`, scanned line by line while
//! streaming (O(line) memory), last occurrence wins, `nan`/`inf` → crash,
//! CRLF tolerated, ASI capped at 2 KiB per sample.

use std::collections::BTreeMap;

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
    _pending: Vec<u8>,
    _out: Scanned,
}

impl Scanner {
    /// Empty.
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed a chunk; complete lines are scanned at once, the tail is kept.
    pub fn feed(&mut self, _chunk: &[u8]) {
        // stub: W1
    }

    /// Scan the pending partial line and return everything seen.
    pub fn finish(self) -> Scanned {
        // stub: W1
        self._out
    }
}

/// Scan one line (the unit `feed` applies); `None` when it is not a contract line.
pub fn scan_line(_line: &str, _into: &mut Scanned) -> bool {
    // stub: W1
    false
}
