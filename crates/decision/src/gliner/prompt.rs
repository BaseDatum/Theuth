//! GLiNER2.5-Decide's input, rendered as the Python `gliner2` package (2.0.0,
//! `gliner2/processor.py`) does for one classification task:
//!
//! `( [P] <task> ( [L] label₀ [L] label₁ … ) ) [SEP_TEXT] w₀ w₁ … .`
//!
//! Every piece — each bracket, marker, the task string, each label, each word of the text — is
//! tokenised on its own (`SchemaTransformer._format_input_with_mapping`) and the subwords are
//! concatenated; there is no `[CLS]`/`[SEP]`. The text gets a terminal `.` unless it already
//! ends in `.!?` (`_collate_batch`), is split into words by `WhitespaceTokenSplitter` and
//! lower-cased per word; the task and the labels keep their case. One logit per `[L]` marker
//! (`_extract_classification_result`: `classifier(embs[1:])`), softmax over them for a
//! single-label task. gliner2 never truncates subwords; the 512 here is DeBERTa-v3's
//! `max_position_embeddings`, applied like laya's: the state is right-truncated first.
//!
//! Mapping of [`Question`]: `Choice` → task = `instructions`, labels = candidate `text` (the
//! id only when `text` is empty) — the `none` control is a candidate the Engine appended
//! (`engine::with_none`, text "none of these") before the question got here; `Bool` → task =
//! `statement`, labels `["yes", "no"]`, P(true) = p(yes); `Score` → task = `instructions`,
//! labels = the level criteria in order. `state` is the text. One question per row: gliner2
//! joins several *tasks over the same text* with `[SEP_STRUCT]`, never several texts.

use std::sync::LazyLock;

use regex::Regex;
use tokenizers::Tokenizer;

use crate::question::Question;

/// `[PAD]`.
pub const PAD: i64 = 0;
/// `[SEP_STRUCT]`: between tasks over one text (unused by single-question rows).
pub const SEP_STRUCT: i64 = 128001;
/// `[SEP_TEXT]`: schema | text.
pub const SEP_TEXT: i64 = 128002;
/// `[P]`: task prompt marker.
pub const P: i64 = 128003;
/// `[C]`: json-structure child field (extraction only).
pub const C: i64 = 128004;
/// `[E]`: entity type (extraction only).
pub const E: i64 = 128005;
/// `[R]`: relation type (extraction only).
pub const R: i64 = 128006;
/// `[L]`: one per classification label; the head scores its hidden state.
pub const L: i64 = 128007;
/// `[EXAMPLE]`: few-shot input (unused here).
pub const EXAMPLE: i64 = 128008;
/// `[OUTPUT]`: few-shot output (unused here).
pub const OUTPUT: i64 = 128009;
/// `[DESCRIPTION]`: `label: description` in the prompt (unused here).
pub const DESCRIPTION: i64 = 128010;

/// The ten marker tokens `SchemaTransformer.SPECIAL_TOKENS` adds — already in
/// GLiNER2.5-Decide's `tokenizer.json` at these ids — plus `[PAD]` for batching.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlinerSpecial {
    /// `[PAD]`.
    pub pad: i64,
    /// `[SEP_STRUCT]`.
    pub sep_struct: i64,
    /// `[SEP_TEXT]`.
    pub sep_text: i64,
    /// `[P]`.
    pub p: i64,
    /// `[C]`.
    pub c: i64,
    /// `[E]`.
    pub e: i64,
    /// `[R]`.
    pub r: i64,
    /// `[L]`.
    pub l: i64,
    /// `[EXAMPLE]`.
    pub example: i64,
    /// `[OUTPUT]`.
    pub output: i64,
    /// `[DESCRIPTION]`.
    pub description: i64,
}

impl GlinerSpecial {
    /// `fastino/GLiNER2.5-Decide` @ 5a7adf72 (DeBERTa-v3 vocab 0..=127999, `[MASK]` 128000).
    pub const DECIDE: GlinerSpecial = GlinerSpecial {
        pad: PAD,
        sep_struct: SEP_STRUCT,
        sep_text: SEP_TEXT,
        p: P,
        c: C,
        e: E,
        r: R,
        l: L,
        example: EXAMPLE,
        output: OUTPUT,
        description: DESCRIPTION,
    };

    /// Look every marker up by string and check it has the id [`Self::DECIDE`] expects — the
    /// layout below is only valid for this vocabulary.
    pub fn from_tokenizer(tok: &Tokenizer) -> Result<GlinerSpecial, String> {
        let want = Self::DECIDE;
        for (text, id) in [
            ("[PAD]", want.pad),
            ("[SEP_STRUCT]", want.sep_struct),
            ("[SEP_TEXT]", want.sep_text),
            ("[P]", want.p),
            ("[C]", want.c),
            ("[E]", want.e),
            ("[R]", want.r),
            ("[L]", want.l),
            ("[EXAMPLE]", want.example),
            ("[OUTPUT]", want.output),
            ("[DESCRIPTION]", want.description),
        ] {
            match tok.token_to_id(text) {
                Some(got) if i64::from(got) == id => {}
                Some(got) => return Err(format!("tokenizer maps {text} to {got}, expected {id}")),
                None => return Err(format!("tokenizer lacks {text}")),
            }
        }
        Ok(want)
    }
}

/// Row budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlinerConfig {
    /// Subwords per row (DeBERTa-v3's 512).
    pub max_len: usize,
    /// A right-truncated state is allowed (a flag on the row); otherwise it is an error.
    pub cut_ok: bool,
}

impl Default for GlinerConfig {
    fn default() -> Self {
        GlinerConfig {
            max_len: 512,
            cut_ok: true,
        }
    }
}

/// One model row; flags as [`crate::laya::Sequence`]'s.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GlinerRow {
    /// Token ids, at most `max_len`, no `[CLS]`/`[SEP]`.
    pub input_ids: Vec<i64>,
    /// All ones (padding is the batch's job).
    pub attention_mask: Vec<i64>,
    /// Position of each label's `[L]`, label order.
    pub label_pos: Vec<i64>,
    /// `label_pos.len()`.
    pub label_count: usize,
    /// `state_cut || markers_lost`.
    pub truncated: bool,
    /// The state was right-truncated (only reachable with `cut_ok`).
    pub state_cut: bool,
    /// A label's `[L]` fell past `max_len`. Always a hard error: [`build_row`] returns `Err`
    /// instead, so this is `false` on every row it returns; kept so a laya-shaped consumer
    /// reads the same three flags.
    pub markers_lost: bool,
    /// A `Bool` question: the labels are `[yes, no]` (Python order), which the engine reads back
    /// as `[false, true]` — the backends reverse the two scores (`gliner::scores`).
    pub is_bool: bool,
}

/// The classification labels in logit order.
pub fn labels(q: &Question) -> Vec<String> {
    match q {
        Question::Choice { candidates, .. } => candidates
            .iter()
            .map(|c| {
                if c.text.is_empty() {
                    c.id.clone()
                } else {
                    c.text.clone()
                }
            })
            .collect(),
        Question::Bool { .. } => vec!["yes".to_owned(), "no".to_owned()],
        Question::Score { levels, .. } => levels.clone(),
    }
}

/// The task string after `[P]`, and the text after `[SEP_TEXT]`.
pub fn task_and_state(q: &Question) -> (&str, &str) {
    match q {
        Question::Choice {
            instructions,
            state,
            ..
        }
        | Question::Score {
            instructions,
            state,
            ..
        } => (instructions, state),
        Question::Bool { statement, state } => (statement, state),
    }
}

/// `WhitespaceTokenSplitter._PATTERN` (gliner2/processing/word_splitter.py:25), `re.IGNORECASE`.
// ponytail: Rust `\w` adds marks/connectors and lacks Nl/No digits vs Python's; only exotic
// scripts differ — swap in an explicit class if a golden ever disagrees.
static WORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)(?:https?://\S+|www\.\S+)|[a-z0-9._%+-]+@[a-z0-9.-]+\.[a-z]{2,}|@[a-z0-9_]+|\w+(?:[-_]\w+)*|\S",
    )
    .expect("static pattern")
});

/// The text as gliner2 feeds it: terminal `.` added (`_collate_batch`), split into words and
/// lower-cased per word (`_transform_record`, `lower=True`).
pub fn words(state: &str) -> Vec<String> {
    let text = if state.is_empty() {
        ".".to_owned()
    } else if state.ends_with(['.', '!', '?']) {
        state.to_owned()
    } else {
        format!("{state}.")
    };
    WORD.find_iter(&text)
        .map(|m| m.as_str().to_lowercase())
        .collect()
}

fn encode(tok: &Tokenizer, piece: &str, into: &mut Vec<i64>) -> Result<(), String> {
    let enc = tok
        .encode(piece, false)
        .map_err(|e| format!("tokenize: {e}"))?;
    into.extend(enc.get_ids().iter().map(|&i| i64::from(i)));
    Ok(())
}

/// Render `q` into one row (`_format_input_with_mapping`, one classification task).
///
/// Errors: no labels; a label marker past `max_len` (the schema alone does not fit — that label
/// could never be scored); a cut state when `cut_ok` is false. Messages start with `truncated:`
/// for the last two.
pub fn build_row(tok: &Tokenizer, q: &Question, cfg: &GlinerConfig) -> Result<GlinerRow, String> {
    let labels = labels(q);
    if labels.is_empty() {
        return Err("no labels".into());
    }
    let (task, state) = task_and_state(q);
    let mut ids = Vec::with_capacity(cfg.max_len.min(1024));
    let mut label_pos = Vec::with_capacity(labels.len());
    encode(tok, "(", &mut ids)?;
    encode(tok, "[P]", &mut ids)?;
    encode(tok, task, &mut ids)?;
    encode(tok, "(", &mut ids)?;
    for label in &labels {
        label_pos.push(ids.len() as i64);
        encode(tok, "[L]", &mut ids)?;
        encode(tok, label, &mut ids)?;
    }
    encode(tok, ")", &mut ids)?;
    encode(tok, ")", &mut ids)?;
    encode(tok, "[SEP_TEXT]", &mut ids)?;
    if ids.len() > cfg.max_len {
        let lost = label_pos
            .iter()
            .filter(|&&p| p as usize >= cfg.max_len)
            .count();
        return Err(format!(
            "truncated: the schema is {} tokens, {lost} of {} label markers past max_len {}",
            ids.len(),
            labels.len(),
            cfg.max_len
        ));
    }
    let mut text = Vec::new();
    for w in words(state) {
        encode(tok, &w, &mut text)?;
    }
    let room = cfg.max_len - ids.len();
    let state_cut = text.len() > room;
    if state_cut && !cfg.cut_ok {
        return Err(format!(
            "truncated: the state is {} tokens, {room} fit after the {}-token schema",
            text.len(),
            ids.len()
        ));
    }
    text.truncate(room);
    ids.extend(text);
    Ok(GlinerRow {
        attention_mask: vec![1; ids.len()],
        input_ids: ids,
        label_count: label_pos.len(),
        label_pos,
        truncated: state_cut,
        state_cut,
        markers_lost: false,
        is_bool: matches!(q, Question::Bool { .. }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    use serde::Deserialize;

    use crate::question::Candidate;

    /// `scripts/tests/gliner-golden.py` output.
    #[derive(Deserialize)]
    struct Golden {
        markers: std::collections::BTreeMap<String, i64>,
        cases: Vec<Case>,
    }

    #[derive(Deserialize)]
    struct Case {
        name: String,
        question: Question,
        labels: Vec<String>,
        input_ids: Vec<i64>,
        attention_mask: Vec<i64>,
        label_pos: Vec<i64>,
    }

    fn golden() -> Golden {
        serde_json::from_str(include_str!("fixtures/golden.json")).unwrap()
    }

    /// The checkpoint's tokenizer when a copy is around; `None` prints why.
    fn real() -> Option<Tokenizer> {
        let dir = std::env::var_os("THEUTH_MODELS_DIR")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".theuth/models"))
            })?
            .join("decision-gliner/tokenizer.json");
        if !dir.is_file() {
            eprintln!(
                "SKIP: {} missing — run scripts/tests/gliner-golden.py once",
                dir.display()
            );
            return None;
        }
        Some(Tokenizer::from_file(&dir).unwrap())
    }

    fn unbounded() -> GlinerConfig {
        GlinerConfig {
            max_len: usize::MAX,
            cut_ok: false,
        }
    }

    #[test]
    fn marker_table_matches_python() {
        let g = golden();
        let sp = GlinerSpecial::DECIDE;
        let table = [
            ("[SEP_STRUCT]", sp.sep_struct),
            ("[SEP_TEXT]", sp.sep_text),
            ("[P]", sp.p),
            ("[C]", sp.c),
            ("[E]", sp.e),
            ("[R]", sp.r),
            ("[L]", sp.l),
            ("[EXAMPLE]", sp.example),
            ("[OUTPUT]", sp.output),
            ("[DESCRIPTION]", sp.description),
        ];
        assert_eq!(g.markers.len(), table.len());
        for (t, id) in table {
            assert_eq!(g.markers[t], id, "{t}");
        }
    }

    #[test]
    fn labels_match_python_mapping() {
        for c in golden().cases {
            assert_eq!(labels(&c.question), c.labels, "{}", c.name);
            assert_eq!(c.label_pos.len(), c.labels.len(), "{}", c.name);
        }
    }

    #[test]
    fn word_split_is_pythons() {
        // Checked against `WhitespaceTokenSplitter` in gliner2 2.0.0.
        assert_eq!(
            words(
                "Mail Bob@Example.COM re: e-mail_v2 or https://x.io/a?b=1 — İstanbul日本語テキスト, straße…!"
            ),
            [
                "mail",
                "bob@example.com",
                "re",
                ":",
                "e-mail_v2",
                "or",
                "https://x.io/a?b=1",
                "—",
                "i̇stanbul日本語テキスト",
                ",",
                "straße",
                "…",
                "!"
            ]
        );
        assert_eq!(words(""), ["."]);
        assert_eq!(
            words("ends with a bang!"),
            ["ends", "with", "a", "bang", "!"]
        );
        assert_eq!(words("trailing space "), ["trailing", "space", "."]);
    }

    #[test]
    fn golden_rows_byte_for_byte() {
        let Some(tok) = real() else { return };
        let sp = GlinerSpecial::from_tokenizer(&tok).unwrap();
        assert_eq!(sp, GlinerSpecial::DECIDE);
        let g = golden();
        assert!(g.cases.len() >= 12);
        let mut long = 0;
        for c in &g.cases {
            let row = build_row(&tok, &c.question, &unbounded()).unwrap();
            assert_eq!(row.input_ids, c.input_ids, "{}", c.name);
            assert_eq!(row.attention_mask, c.attention_mask, "{}", c.name);
            assert_eq!(row.label_pos, c.label_pos, "{}", c.name);
            assert_eq!(row.label_count, c.labels.len(), "{}", c.name);
            assert!(
                !row.truncated && !row.state_cut && !row.markers_lost,
                "{}",
                c.name
            );
            for &p in &row.label_pos {
                assert_eq!(row.input_ids[p as usize], L, "{}", c.name);
            }
            if c.input_ids.len() > 512 {
                long += 1;
                // Default budget: the state is cut at 512 subwords, the schema intact.
                let cut = build_row(&tok, &c.question, &GlinerConfig::default()).unwrap();
                assert_eq!(cut.input_ids, &c.input_ids[..512], "{}", c.name);
                assert_eq!(cut.label_pos, c.label_pos, "{}", c.name);
                assert!(
                    cut.state_cut && cut.truncated && !cut.markers_lost,
                    "{}",
                    c.name
                );
                let strict = GlinerConfig {
                    cut_ok: false,
                    ..GlinerConfig::default()
                };
                let err = build_row(&tok, &c.question, &strict).unwrap_err();
                assert!(err.starts_with("truncated: the state"), "{err}");
            }
        }
        assert_eq!(long, 1, "exactly one golden case exercises the cut");
        eprintln!("golden: {} cases reproduced", g.cases.len());
    }

    #[test]
    fn schema_past_budget_is_an_error() {
        let Some(tok) = real() else { return };
        let q = Question::Choice {
            instructions: "pick".into(),
            candidates: (0..40)
                .map(|i| Candidate {
                    id: format!("e{i}"),
                    text: format!("element {i}"),
                })
                .collect(),
            state: "x".into(),
        };
        let cfg = GlinerConfig {
            max_len: 64,
            cut_ok: true,
        };
        let err = build_row(&tok, &q, &cfg).unwrap_err();
        assert!(err.starts_with("truncated: the schema"), "{err}");
        let empty = Question::Score {
            instructions: "rate".into(),
            levels: vec![],
            state: "x".into(),
        };
        assert_eq!(build_row(&tok, &empty, &cfg).unwrap_err(), "no labels");
    }
}
