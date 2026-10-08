//! 137 §7: the slack fold (pure) and the view renderer. The fold is a pure
//! function of `(tree, T, cfg)`: [`Fold::rebuild`] at load reproduces the live
//! view that [`Fold::append`] + [`Fold::fit`] maintained.

use super::tree::{Tree, exists, id_n};
use super::{ViewLine, tree::Unit};

/// Fold parameters (clamped by `EndlessConfig::fold_cfg`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FoldCfg {
    pub view_bytes: u32,
    pub slack: f32,
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
    pub l: u8,
    pub i: u64,
}

impl Part {
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

/// The view: parts tiling `[0, T)` with their rendered byte sizes.
#[derive(Clone, Debug, PartialEq)]
pub struct Fold {
    pub parts: Vec<Part>,
    sizes: Vec<usize>,
    pub t: u64,
    pub cfg: FoldCfg,
}

impl Fold {
    pub fn new(cfg: FoldCfg) -> Self {
        Self {
            parts: Vec::new(),
            sizes: Vec::new(),
            t: 0,
            cfg,
        }
    }

    /// The pure function of `(tree, T)`: replay `append(i)` for every unit
    /// against `tree`. The live view IS this (recomputed when the tree or `T`
    /// changes), so reload and live are the same function by construction.
    pub fn rebuild(tree: &Tree, t: u64, cfg: FoldCfg, units: &[Unit]) -> Self {
        let mut f = Self::new(cfg);
        for i in 0..t {
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
        self.t = i + 1;
        self.fit(tree, units);
    }

    /// Σ bytes of every part's rendered line (D10 placeholders counted).
    pub fn size(&self) -> usize {
        self.sizes.iter().sum()
    }

    /// §7.1 `fit()`. Returns how many pairs were merged.
    pub fn fit(&mut self, tree: &Tree, units: &[Unit]) -> usize {
        let mut size = self.size();
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
        self.parts
            .iter()
            .filter(|p| !tree.built(p.l, p.i))
            .count() as u32
    }

    /// Mergeable now or later: parent exists over `[0, T)`.
    pub fn parent_exists(&self, part: Part) -> bool {
        exists(part.l + 1, part.i / 2, self.t)
    }
}

/// One cache-marked text block of the rendered view (§7.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewBlock {
    pub text: String,
    /// Place a cache mark at the end of this block.
    pub mark: bool,
}

/// What a turn gets from `render_for_turn`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewRender {
    pub blocks: Vec<ViewBlock>,
    pub settled: bool,
    pub unbuilt: u32,
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

/// The view as lines for the panel.
pub fn render_lines(tree: &Tree, fold: &Fold, units: &[Unit]) -> Vec<ViewLine> {
    fold.parts
        .iter()
        .map(|p| {
            let (text, built) = part_text(tree, *p, units);
            let (id, n) = p.id_n();
            ViewLine {
                id,
                n,
                level: p.l,
                text: text.replace('\n', " "),
                built,
            }
        })
        .collect()
}

/// §7.2: `<chat>` … `</chat>`, cut into at most four blocks at the three
/// deepest band boundaries (a line whose `n` is smaller than the previous
/// line's, taken from the oldest end) plus the end; every block marked.
pub fn render(tree: &Tree, fold: &Fold, units: &[Unit]) -> ViewRender {
    let lines: Vec<String> = fold
        .parts
        .iter()
        .map(|p| line(*p, &part_text(tree, *p, units).0))
        .collect();
    let mut cuts: Vec<usize> = fold
        .parts
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[1].l < w[0].l)
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
    }
}
