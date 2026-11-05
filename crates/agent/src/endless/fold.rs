//! 137 §7: the slack fold (pure) and the view renderer. The fold is a pure
//! function of `(tree, T, cfg)`: [`Fold::rebuild`] at load reproduces the live
//! view that [`Fold::append`] + [`Fold::fit`] maintained.

use super::tree::{Tree, exists, id_n};
use super::{ViewLine, tree::Unit};

/// Fold parameters (clamped by `EndlessConfig::fold_cfg`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoldCfg {
    /// The view budget in bytes (D5).
    pub view_bytes: u32,
    /// Fold band half-width as a fraction of `view_bytes`.
    pub slack: f32,
    /// 137G G2: bytes of unsummarized lines shown, newest first; older ones fold into one line per run.
    pub raw_bytes: u32,
}

impl FoldCfg {
    /// Fold only above this (D5).
    pub fn high(&self) -> usize {
        (self.view_bytes as f64 * (1.0 + f64::from(self.slack))) as usize
    }
    /// … and then down to this.
    pub fn low(&self) -> usize {
        (self.view_bytes as f64 * (1.0 - f64::from(self.slack))) as usize
    }
}

/// A view entry: node `(l, i)`, built or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Part {
    /// Level.
    pub l: u8,
    /// Index within the level.
    pub i: u64,
}

impl Part {
    /// `(id, n)` of the part.
    pub fn id_n(self) -> (u64, u64) {
        id_n(self.l, self.i)
    }
    /// First unit covered.
    pub fn start(self) -> u64 {
        self.id_n().0
    }
    /// One past the last unit covered.
    pub fn end(self) -> u64 {
        let (id, n) = self.id_n();
        id + n
    }
}

/// D10: an unbuilt level-0 part whose unit is at most this renders verbatim.
pub const VERBATIM_MAX: usize = 2_048;

/// The text a part renders as: the node's line, or D10 for an unbuilt part.
/// `unit` resolves a level-0 part's unit (for the verbatim case).
pub fn part_text(tree: &Tree, part: Part, units: &[Unit]) -> (String, bool) {
    if let Some(n) = tree.get(part.l, part.i) {
        return (n.text.clone(), true);
    }
    if part.l == 0
        && let Some(u) = units.get(part.i as usize)
    {
        let line = u.tagged();
        if line.len() <= VERBATIM_MAX {
            return (line, false);
        }
    }
    (PLACEHOLDER.to_owned(), false)
}

/// D10 placeholder for an unbuilt part (without the `id+n|` prefix).
pub const PLACEHOLDER: &str = "(not summarized yet: zoom it)";

/// Bytes of [`part_text`] without building the string.
fn part_len(tree: &Tree, part: Part, units: &[Unit]) -> usize {
    if let Some(n) = tree.get(part.l, part.i) {
        return n.size as usize;
    }
    if part.l == 0
        && let Some(u) = units.get(part.i as usize)
    {
        let len = u.size as usize;
        if len <= VERBATIM_MAX {
            return len;
        }
    }
    PLACEHOLDER.len()
}

/// R1-D4: the view's first line when `base > 0` (without the `0+base|` prefix).
pub const EARLIER: &str = "(earlier history, not summarized; recall searches it)";

/// 137G G2: the line an older run of `n` unsummarized units folds into
/// (without the `id+n|` prefix).
pub fn folded_text(n: u64) -> String {
    format!("({n} lines not yet summarized — recall searches them)")
}

/// 137G G2 hard cap: the line the oldest `n` units fold into when the view
/// cannot fit otherwise (merges not built yet).
pub fn prefix_text(n: u64) -> String {
    format!("({n} older lines folded to fit the view — zoom or recall reach them)")
}

/// The view: parts tiling `[base, T)` with their rendered byte sizes.
#[derive(Clone, Debug, PartialEq)]
pub struct Fold {
    /// The parts, oldest first; they tile `[base, T)`.
    pub parts: Vec<Part>,
    sizes: Vec<usize>,
    /// Per part: not built (a level-0 unit rendered per D10).
    open: Vec<bool>,
    /// First unit the tree covers (R1-D4 backfill base); 0 = everything.
    pub base: u64,
    /// Units so far.
    pub t: u64,
    /// Fold parameters.
    pub cfg: FoldCfg,
}

impl Fold {
    /// An empty view over `[base, base)`.
    pub fn new(base: u64, cfg: FoldCfg) -> Self {
        Self {
            parts: Vec::new(),
            sizes: Vec::new(),
            open: Vec::new(),
            base,
            t: base,
            cfg,
        }
    }

    /// The pure function of `(tree, base, T)`: replay `append(i)` for every
    /// unit from `base` against `tree`. The live view IS this (recomputed when
    /// the tree or `T` changes), so reload and live are the same function by
    /// construction.
    pub fn rebuild(tree: &Tree, base: u64, t: u64, cfg: FoldCfg, units: &[Unit]) -> Self {
        let mut f = Self::new(base, cfg);
        for i in base..t {
            f.append(tree, i, units);
        }
        f
    }

    /// `on unit i appended: view.push(part(0, i)); fit()`.
    pub fn append(&mut self, tree: &Tree, i: u64, units: &[Unit]) {
        debug_assert_eq!(i, self.t);
        let part = Part { l: 0, i };
        self.parts.push(part);
        self.sizes.push(part_len(tree, part, units));
        self.open.push(!tree.built(0, i));
        self.t = i + 1;
        self.fit(tree, units);
    }

    /// G2: parts before this index that are not built fold into run lines;
    /// the unbuilt parts from it on (newest) fit `raw_bytes`.
    fn raw_cut(&self) -> usize {
        let budget = self.cfg.raw_bytes as usize;
        let mut used = 0;
        for k in (0..self.parts.len()).rev() {
            if self.open[k] {
                used += self.sizes[k];
                if used > budget {
                    return k + 1;
                }
            }
        }
        0
    }

    /// G2: runs of unbuilt parts older than the raw window (index ranges).
    fn raw_runs(&self) -> Vec<std::ops::Range<usize>> {
        let cut = self.raw_cut();
        let mut out = Vec::new();
        let mut k = 0;
        while k < cut {
            if self.open[k] {
                let s = k;
                while k < cut && self.open[k] {
                    k += 1;
                }
                out.push(s..k);
            } else {
                k += 1;
            }
        }
        out
    }

    /// Units a run of parts covers.
    fn span(&self, r: &std::ops::Range<usize>) -> u64 {
        self.parts[r.end - 1].end() - self.parts[r.start].start()
    }

    /// The view's folded runs, oldest first, and whether the first is the
    /// hard-cap prefix: the G2 raw runs, then — while the view is still over
    /// `high() + raw_bytes` (a tree whose merges are not built cannot fit) —
    /// the oldest parts, built or not, folded into one prefix run.
    pub fn folded_runs(&self) -> (Vec<std::ops::Range<usize>>, bool) {
        let raw = self.raw_runs();
        let mut rest = self.size_with(&raw, false);
        let cap = self.cfg.high() + self.cfg.raw_bytes as usize;
        if rest <= cap || self.parts.is_empty() {
            return (raw, false);
        }
        let (mut end, mut ri) = (0, 0);
        while end < self.parts.len() {
            if ri < raw.len() && raw[ri].start == end {
                rest -= folded_text(self.span(&raw[ri])).len();
                end = raw[ri].end;
                ri += 1;
            } else {
                rest -= self.sizes[end];
                end += 1;
            }
            if rest + prefix_text(self.span(&(0..end))).len() <= cap {
                break;
            }
        }
        let mut runs = vec![0..end];
        runs.extend(raw.into_iter().skip(ri));
        (runs, true)
    }

    /// G2: unsummarized units the view shows folded (`Backlog.folded`).
    pub fn folded(&self) -> u32 {
        let (runs, _) = self.folded_runs();
        runs.iter()
            .flat_map(|r| r.clone())
            .filter(|k| self.open[*k])
            .count() as u32
    }

    /// Σ bytes of the lines when `runs` render as one line each (`prefix`:
    /// the first is the hard-cap prefix).
    fn size_with(&self, runs: &[std::ops::Range<usize>], prefix: bool) -> usize {
        let mut size = self.sizes.iter().sum::<usize>();
        for (k, r) in runs.iter().enumerate() {
            size -= self.sizes[r.clone()].iter().sum::<usize>();
            size += if k == 0 && prefix {
                prefix_text(self.span(r)).len()
            } else {
                folded_text(self.span(r)).len()
            };
        }
        size + if self.base > 0 { EARLIER.len() } else { 0 }
    }

    /// What `fit()` folds against: every line, the G2 raw runs as one line
    /// each (merging cannot shrink the unbuilt backlog).
    fn fit_size(&self) -> usize {
        self.size_with(&self.raw_runs(), false)
    }

    /// Σ bytes of every rendered line (D10 placeholders counted; G2 folded
    /// runs as their one line each): at most `high() + raw_bytes` plus one line.
    pub fn size(&self) -> usize {
        let (runs, prefix) = self.folded_runs();
        self.size_with(&runs, prefix)
    }

    /// §7.1 `fit()`. Returns how many pairs were merged.
    pub fn fit(&mut self, tree: &Tree, units: &[Unit]) -> usize {
        let mut size = self.fit_size();
        if size <= self.cfg.high() {
            return 0;
        }
        let mut merged = 0;
        while size > self.cfg.low() {
            let Some(k) = self.most_due(tree) else { break };
            let a = self.parts[k];
            let parent = Part {
                l: a.l + 1,
                i: a.i / 2,
            };
            let len = part_len(tree, parent, units);
            size = size - self.sizes[k] - self.sizes[k + 1] + len;
            self.parts.splice(k..k + 2, [parent]);
            self.sizes.splice(k..k + 2, [len]);
            self.open
                .splice(k..k + 2, [!tree.built(parent.l, parent.i)]);
            merged += 1;
        }
        merged
    }

    /// Index of the left part of the mergeable pair with the largest `due`.
    fn most_due(&self, tree: &Tree) -> Option<usize> {
        let mut best: Option<(f64, usize)> = None;
        for (k, w) in self.parts.windows(2).enumerate() {
            let (a, b) = (w[0], w[1]);
            if a.l != b.l || a.i % 2 != 0 || b.i != a.i + 1 || !tree.built(a.l + 1, a.i / 2) {
                continue;
            }
            let due = (self.t - a.start()) as f64 / f64::from(1u32 << (a.l as u32 + 2));
            if best.is_none_or(|(d, _)| due > d) {
                best = Some((due, k));
            }
        }
        best.map(|(_, k)| k)
    }

    /// Parts not yet built (what `settle` waits for).
    pub fn unbuilt(&self, tree: &Tree) -> u32 {
        self.parts.iter().filter(|p| !tree.built(p.l, p.i)).count() as u32
    }

    /// Mergeable now or later: parent exists over `[0, T)`.
    pub fn parent_exists(&self, part: Part) -> bool {
        exists(part.l + 1, part.i / 2, self.t)
    }
}

/// One cache-marked text block of the rendered view (§7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewBlock {
    /// The block's text.
    pub text: String,
    /// Place a cache mark at the end of this block.
    pub mark: bool,
}

/// What a turn gets from `render_for_turn`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewRender {
    /// The cache-marked blocks, in order.
    pub blocks: Vec<ViewBlock>,
    /// Every part is a built line.
    pub settled: bool,
    /// Parts rendered per D10.
    pub unbuilt: u32,
    /// 137G G2: unsummarized units folded into run lines (not shown verbatim).
    pub folded: u32,
}

impl ViewRender {
    /// The whole `<chat>…</chat>` text (the frozen compactor context, D6).
    pub fn text(&self) -> String {
        self.blocks.iter().map(|b| b.text.as_str()).collect()
    }
}

/// `id+n|text`, newlines → spaces.
pub fn line(part: Part, text: &str) -> String {
    let (id, n) = part.id_n();
    format!("{id}+{n}|{}", text.replace('\n', " "))
}

/// One rendered line of the view: a part, or a G2 folded run `(id, n)`.
enum Entry {
    Part(Part),
    /// `(id, n, text)` of a folded run.
    Folded(u64, u64, String),
}

impl Entry {
    fn level(&self) -> u8 {
        match self {
            Self::Part(p) => p.l,
            Self::Folded(..) => 0,
        }
    }
}

/// The view's lines in order: parts, with every G2 folded run as one entry.
fn entries(fold: &Fold) -> Vec<Entry> {
    let (runs, prefix) = fold.folded_runs();
    let mut out = Vec::with_capacity(fold.parts.len());
    let mut runs = runs.into_iter().enumerate().peekable();
    let mut k = 0;
    while k < fold.parts.len() {
        if let Some((j, r)) = runs.next_if(|(_, r)| r.start == k) {
            let n = fold.span(&r);
            let text = if j == 0 && prefix {
                prefix_text(n)
            } else {
                folded_text(n)
            };
            out.push(Entry::Folded(fold.parts[k].start(), n, text));
            k = r.end;
        } else {
            out.push(Entry::Part(fold.parts[k]));
            k += 1;
        }
    }
    out
}

/// The view as lines for the panel.
pub fn render_lines(tree: &Tree, fold: &Fold, units: &[Unit]) -> Vec<ViewLine> {
    let head = (fold.base > 0).then(|| ViewLine {
        id: 0,
        n: fold.base,
        level: 0,
        text: EARLIER.to_owned(),
        built: false,
        at: None,
        mechanical: false,
    });
    head.into_iter()
        .chain(entries(fold).into_iter().map(|e| match e {
            Entry::Folded(id, n, text) => ViewLine {
                id,
                n,
                level: 0,
                text,
                built: false,
                at: None,
                mechanical: false,
            },
            Entry::Part(p) => {
                let (text, built) = part_text(tree, p, units);
                let (id, n) = p.id_n();
                ViewLine {
                    id,
                    n,
                    level: p.l,
                    text: text.replace('\n', " "),
                    built,
                    at: (n == 1)
                        .then(|| units.get(id as usize).and_then(|u| u.at))
                        .flatten(),
                    mechanical: tree.get(p.l, p.i).is_some_and(|n| n.mechanical),
                }
            }
        }))
        .collect()
}

/// §7.2: `<chat>` … `</chat>`, cut into at most four blocks at the three
/// deepest band boundaries (a line whose `n` is smaller than the previous
/// line's, taken from the oldest end) plus the end; every block marked.
pub fn render(tree: &Tree, fold: &Fold, units: &[Unit]) -> ViewRender {
    let entries = entries(fold);
    let lines: Vec<String> = entries
        .iter()
        .map(|e| match e {
            Entry::Part(p) => line(*p, &part_text(tree, *p, units).0),
            Entry::Folded(id, n, text) => format!("{id}+{n}|{text}"),
        })
        .collect();
    let mut cuts: Vec<usize> = entries
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[1].level() < w[0].level())
        .map(|(k, _)| k + 1)
        .take(3)
        .collect();
    cuts.push(lines.len());
    let mut blocks = Vec::with_capacity(cuts.len());
    let mut from = 0;
    for (b, to) in cuts.iter().enumerate() {
        let mut text = String::new();
        if b == 0 {
            text.push_str("<chat>\n");
            if fold.base > 0 {
                text.push_str(&format!("0+{}|{EARLIER}\n", fold.base));
            }
        }
        for l in &lines[from..*to] {
            text.push_str(l);
            text.push('\n');
        }
        if b + 1 == cuts.len() {
            text.push_str("</chat>");
        }
        blocks.push(ViewBlock { text, mark: true });
        from = *to;
    }
    ViewRender {
        blocks,
        settled: fold.unbuilt(tree) == 0,
        unbuilt: fold.unbuilt(tree),
        folded: fold.folded(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endless::tree::{Kind, Node};

    const TREE: &str = include_str!("../../tests/fixtures/endless/B.haiku.tree.jsonl");
    const MAIN: &str = include_str!("../../tests/fixtures/endless/B.main.jsonl");
    const CFG: FoldCfg = FoldCfg {
        view_bytes: 32_000,
        slack: 0.125,
        raw_bytes: u32::MAX,
    };

    /// 137G G2 hard cap: 5,000 built level-0 lines whose merges are not
    /// built yet (a mechanical fold, settling paused) still render within
    /// `high() + raw_bytes`: the oldest fold into one prefix line.
    #[test]
    fn built_lines_without_merges_fold_to_the_cap() {
        let cfg = FoldCfg {
            view_bytes: 128_000,
            slack: 0.125,
            raw_bytes: 32_000,
        };
        let mut tree = Tree::default();
        let units: Vec<Unit> = (0..5_000u64)
            .map(|i| {
                tree.insert(Node {
                    l: 0,
                    i,
                    text: format!("user: excerpt {i} {}", "y".repeat(460)),
                    size: 480,
                    free: false,
                    model: None,
                    tries: 0,
                    mechanical: true,
                });
                Unit {
                    i,
                    seq: i as i64,
                    part: 0,
                    kind: Kind::User,
                    text: "x".repeat(900),
                    size: 906,
                    at: None,
                }
            })
            .collect();
        let f = Fold::rebuild(&tree, 0, 5_000, cfg, &units);
        let cap = cfg.high() + cfg.raw_bytes as usize;
        assert!(f.size() <= cap, "{} > {cap}", f.size());
        let lines = render_lines(&tree, &f, &units);
        assert!(
            lines[0].text.contains("older lines folded to fit the view"),
            "{}",
            lines[0].text
        );
        assert_eq!(lines[0].id, 0);
        assert_eq!(lines.last().unwrap().id, 4_999, "the newest stay");
        let text = render(&tree, &f, &units).text();
        assert!(text.len() <= cap + 16 + lines.len() * 12, "{}", text.len());
        assert_eq!(f.folded(), 0, "nothing unsummarized is folded");
    }

    /// 137G G2: 5,000 unbuilt units render within view + raw budget: the
    /// newest fit `raw_bytes` verbatim, the rest fold into one run line that
    /// counts them, `Fold::size` and the render agree, and `folded` is right.
    #[test]
    fn unbuilt_backlog_folds_beyond_the_raw_budget() {
        let units: Vec<Unit> = (0..5_000u64)
            .map(|i| {
                let text = format!("message {i}: {}", "x".repeat(600));
                Unit {
                    i,
                    seq: i as i64,
                    part: 0,
                    kind: Kind::User,
                    size: (6 + text.len()) as u32,
                    text,
                    at: None,
                }
            })
            .collect();
        let cfg = FoldCfg {
            view_bytes: 128_000,
            slack: 0.125,
            raw_bytes: 32_000,
        };
        let tree = Tree::default();
        let f = Fold::rebuild(&tree, 0, 5_000, cfg, &units);
        let r = render(&tree, &f, &units);
        let text = r.text();
        let shown = 32_000 / units[4_999].size as usize;
        assert_eq!(f.folded(), 5_000 - shown as u32);
        assert_eq!(r.folded, f.folded());
        assert!(
            text.contains(&format!(
                "0+{}|({} lines not yet summarized",
                5_000 - shown,
                5_000 - shown
            )),
            "{}",
            &text[..200]
        );
        assert!(text.contains("user: message 4999:") && !text.contains("user: message 0:"));
        // Fixed parts: `<chat>` tags and an `id+n|` prefix per line.
        let fixed = 16 + (shown + 1) * 12;
        assert!(
            text.len() <= f.size() + fixed,
            "{} vs {}",
            text.len(),
            f.size()
        );
        assert!(text.len() <= 32_000 + 128 + fixed, "{}", text.len());
        let lines = render_lines(&tree, &f, &units);
        assert_eq!(lines.len(), shown + 1);
        assert_eq!(
            (lines[0].id, lines[0].n, lines[0].built),
            (0, (5_000 - shown) as u64, false)
        );
        // A raw budget large enough shows everything verbatim (unbuilt parts are never merged).
        let all = Fold::rebuild(&tree, 0, 50, cfg, &units);
        assert_eq!(all.folded(), 0);
    }

    /// The lab fixture: nodes in build order, units with kind/text/size.
    fn fixture() -> (Vec<Node>, Vec<Unit>) {
        let nodes = TREE
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                let text = v["text"].as_str().unwrap().to_owned();
                Node {
                    l: v["l"].as_u64().unwrap() as u8,
                    i: v["i"].as_u64().unwrap(),
                    size: text.len() as u32,
                    text,
                    free: v["free"].as_bool().unwrap_or(false),
                    model: None,
                    tries: 1,
                    mechanical: false,
                }
            })
            .collect();
        let units = MAIN
            .lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                let kind: Kind = v["kind"].as_str().unwrap().parse().unwrap();
                let text = v["text"].as_str().unwrap().to_owned();
                Unit {
                    i: v["i"].as_u64().unwrap(),
                    seq: v["seq"].as_i64().unwrap_or(0),
                    part: 0,
                    kind,
                    size: (kind.as_str().len() + 2 + text.len()) as u32,
                    text,
                    at: None,
                }
            })
            .collect();
        (nodes, units)
    }

    fn tiles(f: &Fold) {
        let mut at = f.base;
        for p in &f.parts {
            assert_eq!(p.start(), at, "parts tile [0, T) without gaps or overlap");
            at = p.end();
        }
        assert_eq!(at, f.t);
    }

    fn mergeable_pair(f: &Fold, tree: &Tree) -> bool {
        f.parts.windows(2).any(|w| {
            w[0].l == w[1].l
                && w[0].i % 2 == 0
                && w[1].i == w[0].i + 1
                && tree.built(w[0].l + 1, w[0].i / 2)
        })
    }

    /// §7.1 invariants on the fixture at VIEW 32,000: tiling, no split, the
    /// slack band, and purity (re-fold == fold; append path == rebuild).
    #[test]
    fn fixture_fold_invariants_and_refold_equality() {
        let (nodes, units) = fixture();
        let t = units.len() as u64;
        let tree = Tree::from_nodes(nodes);
        let f = Fold::rebuild(&tree, 0, t, CFG, &units);
        tiles(&f);
        for p in &f.parts {
            assert!(exists(p.l, p.i, t), "a part is a whole node over [0, T)");
        }
        let size = f.size();
        assert!(
            size <= CFG.high(),
            "size {size} within the upper band {}",
            CFG.high()
        );
        assert!(
            size <= CFG.low() || !mergeable_pair(&f, &tree),
            "after fit: under the lower band or nothing left to merge"
        );
        assert!(
            f.parts.len() > 20 && f.parts.len() < 200,
            "{} parts",
            f.parts.len()
        );
        assert!(f.parts.iter().any(|p| p.l >= 4), "deep bands exist");

        // Purity: the same function twice, and incremental append == rebuild.
        assert_eq!(Fold::rebuild(&tree, 0, t, CFG, &units), f);
        let mut inc = Fold::new(0, CFG);
        for i in 0..t {
            inc.append(&tree, i, &units);
        }
        assert_eq!(inc, f);

        // Re-fold at a smaller T is a prefix-consistent fold too.
        let half = Fold::rebuild(&tree, 0, t / 2, CFG, &units);
        tiles(&half);

        // R1-D4: a base — parts tile [base, T) and the view opens with the raw-history line.
        let based = Fold::rebuild(&tree, 300, t, CFG, &units);
        tiles(&based);
        assert!(based.parts.iter().all(|p| p.start() >= 300));
        let r = render(&tree, &based, &units);
        assert!(
            r.blocks[0]
                .text
                .starts_with(&format!("<chat>\n0+300|{EARLIER}\n300+")),
            "{}",
            r.blocks[0].text
        );
        assert_eq!(render_lines(&tree, &based, &units)[0].n, 300);
    }

    /// D5 turn-to-turn stability on the fixture: the identical line prefix
    /// between consecutive T is high (reported, bounded loosely).
    #[test]
    fn fixture_slack_fold_is_stable_turn_to_turn() {
        let (nodes, units) = fixture();
        let t = units.len() as u64;
        let tree = Tree::from_nodes(nodes);
        let mut prev = Fold::rebuild(&tree, 0, 200, CFG, &units);
        let (mut same, mut turns) = (0usize, 0usize);
        for n in 201..=t {
            let f = Fold::rebuild(&tree, 0, n, CFG, &units);
            let common = prev
                .parts
                .iter()
                .zip(f.parts.iter())
                .take_while(|(a, b)| a == b)
                .count();
            same += common;
            turns += prev.parts.len();
            prev = f;
        }
        let share = same as f64 / turns as f64;
        assert!(share > 0.7, "identical part prefix share {share:.2}");
    }

    /// §7.2 render: ≤ 4 blocks, cut at the deepest band boundaries, `<chat>`
    /// first and `</chat>` last, every block marked; lines are `id+n|text`.
    #[test]
    fn render_cuts_at_band_boundaries_and_marks() {
        let (nodes, units) = fixture();
        let t = units.len() as u64;
        let tree = Tree::from_nodes(nodes);
        let f = Fold::rebuild(&tree, 0, t, CFG, &units);
        let r = render(&tree, &f, &units);
        assert!(r.blocks.len() <= 4 && r.blocks.len() >= 2);
        assert!(r.blocks[0].text.starts_with("<chat>\n"));
        assert!(r.blocks.last().unwrap().text.ends_with("</chat>"));
        assert!(r.blocks.iter().all(|b| b.mark));
        let text = r.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), f.parts.len() + 2);
        let (id, n) = f.parts[0].id_n();
        assert!(lines[1].starts_with(&format!("{id}+{n}|")));
        // Every block boundary (except the end) sits where n drops.
        let mut at = 0;
        for b in &r.blocks[..r.blocks.len() - 1] {
            at += b.text.lines().count() - usize::from(b.text.starts_with("<chat>"));
            assert!(
                f.parts[at].l < f.parts[at - 1].l,
                "cut {at} at a band boundary"
            );
        }
        assert!(r.settled && r.unbuilt == 0);
    }

    /// D10: an unbuilt level-0 part renders verbatim when ≤ 2,048 bytes,
    /// else as the placeholder; merged parts never render unbuilt.
    #[test]
    fn unbuilt_parts_render_verbatim_or_placeholder() {
        let unit = |i: u64, text: &str| Unit {
            i,
            seq: i as i64,
            part: 0,
            kind: Kind::User,
            size: (6 + text.len()) as u32,
            text: text.to_owned(),
            at: None,
        };
        let units = vec![unit(0, "short"), unit(1, &"x".repeat(3000))];
        let tree = Tree::default();
        let f = Fold::rebuild(&tree, 0, 2, CFG, &units);
        let r = render(&tree, &f, &units);
        assert_eq!(r.unbuilt, 2);
        assert!(!r.settled);
        assert_eq!(r.blocks.len(), 1);
        assert_eq!(
            r.text(),
            format!("<chat>\n0+1|user: short\n1+1|{PLACEHOLDER}\n</chat>")
        );
        let lines = render_lines(&tree, &f, &units);
        assert_eq!((lines[0].built, lines[1].built), (false, false));
        assert_eq!(lines[0].text, "user: short");
    }
}
