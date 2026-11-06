//! 137G G1: one request-size policy for every model call. Requests are
//! sized in bytes and estimated at a per-model bytes-per-token ratio
//! (conservative 2.2 until real usage teaches it); [`budget`] says what a
//! caller should size to (`target`) and what may never be sent (`ceiling`);
//! [`preflight`] in `Backend::stream` refuses locally anything over the
//! ceiling, in the provider's own "prompt is too long" shape so every
//! caller's overflow path (and `compact::overflow_numbers`) reads it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use genai::chat::{ChatRequest, ContentPart};
use models::Resolved;
use parking_lot::Mutex;

/// Bytes per token before a model's usage is measured: dense JSON, code and
/// CJK run 2.0–2.7, so 2.2 over-counts prose rather than under-counting code.
/// What callers SIZE to.
pub const DEFAULT_BPT: f64 = 2.2;
/// R1-1: what the preflight REFUSES at while a model is unmeasured — the
/// optimistic end, so a request that may fit is sent once and teaches the
/// ratio (a local refusal teaches nothing).
pub const UNLEARNED_REFUSE_BPT: f64 = 4.0;
/// The learner's clamp.
const BPT_RANGE: (f64, f64) = (1.8, 4.0);
/// EWMA weight of one new observation.
const ALPHA: f64 = 0.3;
/// The window assumed when the catalog does not know the model.
pub const UNKNOWN_WINDOW: u64 = 200_000;
/// Ceiling margin under `window − reply`.
const MARGIN: f64 = 0.05;
/// R1-1: the file under the data dir that keeps the learned ratios.
pub const RATIOS_FILE: &str = "token_ratios.json";
/// A learned ratio is written back when it moved by more than this.
const SAVE_DELTA: f64 = 0.02;

/// Learned bytes/token per model id.
static BPT: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(Default::default);
/// Learned prompt tokens per second per local model id (G1 latency limb).
static EVAL: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(Default::default);
/// Where [`learn`] persists the ratios (`init`); `None` = memory only. Held
/// while a save writes, so saves are serial and the last one is the newest.
static RATIOS: LazyLock<Mutex<Option<PathBuf>>> = LazyLock::new(Default::default);

/// Engine start: load the ratios learned before the restart from
/// `<data_dir>/token_ratios.json` and keep writing them there.
pub fn init(data_dir: &Path) {
    let path = data_dir.join(RATIOS_FILE);
    let loaded = load_from(&path);
    {
        let mut map = BPT.lock();
        for (model, bpt) in loaded {
            map.entry(model).or_insert(bpt);
        }
    }
    *RATIOS.lock() = Some(path);
}

/// The ratios a file holds (clamped); empty when missing or unreadable.
fn load_from(path: &Path) -> HashMap<String, f64> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<HashMap<String, f64>>(&s).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|(_, v)| v.is_finite())
        .map(|(k, v)| (k, v.clamp(BPT_RANGE.0, BPT_RANGE.1)))
        .collect()
}

/// Write `map` to `path` (best-effort, via a temp file so a crash never leaves half a file).
fn save_to(path: &Path, map: &HashMap<String, f64>) {
    let Ok(json) = serde_json::to_string_pretty(map) else {
        return;
    };
    let tmp = path.with_extension("json.tmp");
    if let Err(e) = std::fs::write(&tmp, json).and_then(|()| std::fs::rename(&tmp, path)) {
        tracing::debug!(path = %path.display(), error = %e, "token ratios not saved");
    }
}

/// The model's bytes per token: learned, else [`DEFAULT_BPT`].
pub fn bytes_per_token(model: &str) -> f64 {
    BPT.lock().get(model).copied().unwrap_or(DEFAULT_BPT)
}

/// The model has a learned ratio.
pub fn learned(model: &str) -> bool {
    BPT.lock().contains_key(model)
}

/// Fold one observation into the model's ratio — `bytes` of text sent,
/// `flat` image/PDF tokens of the request ([`request_size`]), `tokens` the
/// provider counted for all of it (R2-4: the media share is taken out, so a
/// media-heavy request does not pull the ratio down): EWMA, clamped to
/// 1.8..=4.0; persisted (R1-1) when it moved by more than [`SAVE_DELTA`].
pub fn learn(model: &str, bytes: usize, flat: u64, tokens: u64) {
    let tokens = tokens.saturating_sub(flat);
    if bytes < 256 || tokens == 0 {
        return;
    }
    let seen = (bytes as f64 / tokens as f64).clamp(BPT_RANGE.0, BPT_RANGE.1);
    let moved = {
        let mut map = BPT.lock();
        let before = map.get(model).copied();
        let v = map.entry(model.to_owned()).or_insert(seen);
        *v = (*v * (1.0 - ALPHA) + seen * ALPHA).clamp(BPT_RANGE.0, BPT_RANGE.1);
        before.is_none_or(|b| (b - *v).abs() > SAVE_DELTA)
    };
    // R2-4: the snapshot is taken under the ratio lock, the file written after it.
    if moved {
        let path = RATIOS.lock();
        if let Some(path) = path.as_ref() {
            let snapshot = BPT.lock().clone();
            save_to(path, &snapshot);
        }
    }
}

/// Test seam (R1-10): forget what the process learned about `model`.
#[cfg(test)]
pub(crate) fn forget_for_tests(model: &str) {
    BPT.lock().remove(model);
    EVAL.lock().remove(model);
}

/// A completed local call: `tokens` of prompt evaluated in `secs`.
pub fn learn_eval(model: &str, tokens: u64, secs: f64) {
    if tokens < 64 || secs <= 0.0 {
        return;
    }
    let rate = tokens as f64 / secs;
    let mut map = EVAL.lock();
    let v = map.entry(model.to_owned()).or_insert(rate);
    *v = *v * (1.0 - ALPHA) + rate * ALPHA;
}

/// Measured prompt-eval tokens/s of a local model; `None` until measured.
pub fn eval_rate(model: &str) -> Option<f64> {
    EVAL.lock().get(model).copied()
}

/// What the estimate counts of a request: text bytes (system, messages,
/// tool definitions) and image/PDF tokens (charged flat, as `compact::est`).
pub fn request_size(req: &ChatRequest) -> (usize, u64) {
    let mut bytes = req.system.as_ref().map_or(0, String::len);
    let mut flat = 0u64;
    for m in &req.messages {
        for p in m.content.iter() {
            match p {
                ContentPart::Text(t) => bytes += t.len(),
                ContentPart::ToolResponse(r) => {
                    bytes += r.content.len();
                    flat += r.images.len() as u64 * u64::from(crate::compact::IMAGE_TOKENS);
                }
                ContentPart::Binary(_) => flat += u64::from(crate::compact::est_part(p)),
                other => bytes += serde_json::to_string(other).map_or(0, |s| s.len()),
            }
        }
    }
    if let Some(tools) = &req.tools {
        bytes += serde_json::to_string(tools).map_or(0, |s| s.len());
    }
    (bytes, flat)
}

/// Estimated prompt tokens of `req` for `model`.
pub fn estimate(model: &str, req: &ChatRequest) -> u64 {
    let (bytes, flat) = request_size(req);
    tokens_of(model, bytes) + flat
}

/// `bytes` as tokens at the model's ratio (rounded up).
pub fn tokens_of(model: &str, bytes: usize) -> u64 {
    (bytes as f64 / bytes_per_token(model)).ceil() as u64
}

/// `tokens` as bytes at the model's ratio (rounded down).
pub fn bytes_of(model: &str, tokens: u64) -> usize {
    (tokens as f64 * bytes_per_token(model)) as usize
}

/// The context window in tokens: the catalog's, else (177 E7) the assumed
/// local window for a local runtime's id, else [`UNKNOWN_WINDOW`]. A local
/// server's catalog entry already carries the assumed window
/// (`models::Catalog::load_with`).
pub fn window(model: &Resolved) -> u64 {
    model
        .model
        .as_ref()
        .map(|m| u64::from(m.limit.context))
        .filter(|w| *w > 0)
        .unwrap_or_else(|| {
            if crate::local::runtime_named(model) {
                u64::from(crate::local::ASSUMED_CONTEXT)
            } else {
                UNKNOWN_WINDOW
            }
        })
}

/// Who is asking (the quality fill differs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A session turn's round.
    Turn,
    /// An endless compactor call.
    Compactor,
    /// Any other side call (title, decide, insights …).
    Side,
}

impl Kind {
    /// Models degrade long before their maximum: the share of the window a
    /// caller of this kind sizes to.
    fn fill(self) -> f64 {
        match self {
            Self::Turn => 0.5,
            Self::Compactor | Self::Side => 0.25,
        }
    }
}

/// What one request may weigh, in prompt tokens.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Budget {
    /// What callers size to: the smallest of the quality, cost, latency and fixed-cap limbs.
    pub target_tokens: u64,
    /// Never sent above: `window − reply − 5 %`.
    pub ceiling_tokens: u64,
}

/// The inputs of [`budget`] beside the kind and the model.
#[derive(Clone, Copy, Debug, Default)]
pub struct Limits {
    /// The request's reply cap (`max_tokens`).
    pub reply_tokens: u64,
    /// Cost limb (compactor, side calls): USD one call's prompt may cost.
    pub max_usd_per_call: Option<f64>,
    /// Latency limb (local endpoints): one attempt's bound in seconds.
    pub local_attempt_secs: Option<u64>,
    /// The kind's fixed cap in bytes (`compactor_ctx_bytes`; the turn's view budget).
    pub cap_bytes: Option<usize>,
}

/// `window − reply − 5 %`, in tokens. A reply cap over half the window
/// (a frame sized for a larger window than the catalog or the 200k default
/// says) counts as half the window.
pub fn ceiling(model: &Resolved, reply_tokens: u64) -> u64 {
    let window = window(model);
    let room = window - reply_tokens.min(window / 2);
    (room as f64 * (1.0 - MARGIN)) as u64
}

/// Test seam: model ids whose requests skip the preflight (scenarios whose
/// fake provider accepts what the catalog window could not hold).
#[cfg(test)]
pub(crate) static EXEMPT_FOR_TESTS: LazyLock<Mutex<std::collections::HashSet<String>>> =
    LazyLock::new(Default::default);

/// G1 budget policy for one request of `kind` to `model`.
pub fn budget(kind: Kind, model: &Resolved, l: Limits) -> Budget {
    let ceiling = ceiling(model, l.reply_tokens);
    let mut target = (window(model) as f64 * kind.fill()) as u64;
    if kind != Kind::Turn
        && let (Some(usd), Some(price)) = (
            l.max_usd_per_call,
            model.model.as_ref().and_then(|m| m.cost).map(|c| c.input),
        )
        && price > 0.0
    {
        target = target.min((usd / (price / 1e6)) as u64);
    }
    if let Some(secs) = l.local_attempt_secs
        && let Some(rate) = eval_rate(&model.id)
    {
        target = target.min((rate * secs as f64 * 0.5) as u64);
    }
    if let Some(cap) = l.cap_bytes {
        target = target.min(tokens_of(&model.id, cap));
    }
    Budget {
        target_tokens: target.min(ceiling),
        ceiling_tokens: ceiling,
    }
}

/// G1 preflight: `Err` (nothing sent) when `req`'s estimate is over the
/// model's ceiling — "prompt is too long: N tokens > M maximum", the
/// Anthropic shape `compact::overflow_numbers` reads, plus how it was
/// estimated. `Ok(estimate)` otherwise. R1-1: a model with no learned ratio
/// is judged at [`UNLEARNED_REFUSE_BPT`] (the optimistic end); sizing still
/// uses [`DEFAULT_BPT`].
pub fn preflight(model: &Resolved, req: &ChatRequest, reply_tokens: u64) -> Result<u64, String> {
    let bpt = BPT
        .lock()
        .get(&model.id)
        .copied()
        .unwrap_or(UNLEARNED_REFUSE_BPT);
    let (bytes, flat) = request_size(req);
    let est = (bytes as f64 / bpt).ceil() as u64 + flat;
    let max = ceiling(model, reply_tokens);
    #[cfg(test)]
    if EXEMPT_FOR_TESTS.lock().contains(&model.id) {
        return Ok(est);
    }
    if est > max {
        let known = model.model.as_ref().is_some_and(|m| {
            m.limit.context > 0
                && !(m.limit.source == models::LimitSource::Unknown
                    && m.limit.context == crate::local::ASSUMED_CONTEXT)
        });
        // 177 E7: a local server's window is a guess until someone sets it.
        let fix = if !known && window(model) == u64::from(crate::local::ASSUMED_CONTEXT) {
            match model.id.split_once('/') {
                Some((p, m)) => format!(
                    "the local window is assumed; set the real one under Local servers or `/provider limits {p} {m} <context>`"
                ),
                None => "the local window is assumed; set the real one under Local servers".into(),
            }
        } else {
            "a stale catalog window is corrected in the data dir's models.json".into()
        };
        return Err(format!(
            "prompt is too long: {est} tokens > {max} maximum (theuth preflight, not sent: {} estimated at {bpt:.2} bytes/token, window {}{}, reply {reply_tokens}; {fix})",
            model.id,
            window(model),
            if known { "" } else { " assumed" },
        ));
    }
    Ok(est)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, context: u32, input_usd: Option<f64>) -> Resolved {
        let m: Option<models::Model> = (context > 0).then(|| {
            let mut v = serde_json::json!({
                "id": id, "name": id, "limit": {"context": context, "output": 8_000}
            });
            if let Some(p) = input_usd {
                v["cost"] = serde_json::json!({"input": p, "output": p * 5.0});
            }
            serde_json::from_value(v).unwrap()
        });
        Resolved {
            id: id.to_owned(),
            genai: id.to_owned(),
            provider: None,
            model: m,
        }
    }

    /// The quality limb: a turn fills half a 1M window, a compactor a
    /// quarter; the ceiling is window − reply − 5 %.
    #[test]
    fn million_window_quality_and_ceiling() {
        let m = model("t/1m", 1_000_000, Some(1.0));
        let l = Limits {
            reply_tokens: 8_600,
            ..Default::default()
        };
        assert_eq!(
            budget(Kind::Turn, &m, l),
            Budget {
                target_tokens: 500_000,
                ceiling_tokens: 941_830
            }
        );
        assert_eq!(budget(Kind::Compactor, &m, l).target_tokens, 250_000);
    }

    /// The cost limb: $0.10 a call at $5/M input is 20k tokens, whatever
    /// the window; a turn has no cost limb.
    #[test]
    fn expensive_model_cost_limb() {
        let m = model("t/pricey", 1_000_000, Some(5.0));
        let l = Limits {
            reply_tokens: 600,
            max_usd_per_call: Some(0.10),
            ..Default::default()
        };
        assert_eq!(budget(Kind::Compactor, &m, l).target_tokens, 20_000);
        assert_eq!(budget(Kind::Side, &m, l).target_tokens, 20_000);
        assert_eq!(budget(Kind::Turn, &m, l).target_tokens, 500_000);
    }

    /// The latency limb on a 32k local window: unknown until measured
    /// (the fixed cap rules), then rate × attempt × 0.5.
    #[test]
    fn local_32k_latency_limb_learns() {
        let id = "t/local-32k";
        let m = model(id, 32_000, None);
        let l = Limits {
            reply_tokens: 600,
            local_attempt_secs: Some(60),
            cap_bytes: Some(24_000),
            ..Default::default()
        };
        let cap = tokens_of(id, 24_000);
        assert_eq!(budget(Kind::Compactor, &m, l).target_tokens, cap.min(8_000));
        learn_eval(id, 2_000, 20.0); // 100 tok/s → 3,000 tokens per 60 s attempt
        assert_eq!(budget(Kind::Compactor, &m, l).target_tokens, 3_000);
        assert!(budget(Kind::Compactor, &m, l).ceiling_tokens < 32_000);
    }

    /// An unknown window is 200k for both limbs; a learned model's preflight
    /// refuses locally in the shape the overflow parser reads.
    #[test]
    fn unknown_window_preflight_refuses_locally() {
        let m = model("t/unknown", 0, None);
        let b = budget(Kind::Turn, &m, Limits::default());
        assert_eq!((b.target_tokens, b.ceiling_tokens), (100_000, 190_000));
        learn("t/unknown", 2_200, 0, 1_000);
        let big = ChatRequest::new(vec![genai::chat::ChatMessage::user("x".repeat(500_000))]);
        let e = preflight(&m, &big, 0).unwrap_err();
        let n = crate::compact::overflow_numbers(&e).expect("parsed");
        assert_eq!((n.prompt, n.window), (227_273, 190_000), "{e}");
        assert!(
            crate::compact::is_overflow(&e) && e.contains("not sent") && e.contains("models.json"),
            "{e}"
        );
        let small = ChatRequest::new(vec![genai::chat::ChatMessage::user("hello")]);
        assert!(preflight(&m, &small, 0).is_ok());
    }

    /// R1-1: an unlearned 200k/64k model is judged at 4 B/token — a 300 KB
    /// request (≈ 136k tokens at 2.2, over the 129k ceiling) is sent; once
    /// the model is measured at 2.2 it is refused. Sizing stays at 2.2.
    #[test]
    fn unlearned_model_is_refused_only_on_the_optimistic_bound() {
        let id = "t/unlearned-200k";
        forget_for_tests(id);
        let m = {
            let mut m = model(id, 200_000, None);
            m.model.as_mut().unwrap().limit.output = 64_000;
            m
        };
        let req = ChatRequest::new(vec![genai::chat::ChatMessage::user("x".repeat(300_000))]);
        assert_eq!(preflight(&m, &req, 64_000), Ok(75_000));
        assert_eq!(bytes_of(id, 1_000), 2_200, "sizing is conservative");
        learn(id, 2_200, 0, 1_000);
        assert!(preflight(&m, &req, 64_000).is_err());
    }

    /// R1-1: learned ratios survive a restart through `token_ratios.json`.
    #[test]
    fn learned_ratios_round_trip_through_the_file() {
        let dir = crate::TestDir::new("token-ratios");
        let path = dir.0.join(RATIOS_FILE);
        let map = HashMap::from([("p/a".to_owned(), 3.1), ("p/b".to_owned(), 9.0)]);
        save_to(&path, &map);
        let back = load_from(&path);
        assert_eq!(back["p/a"], 3.1);
        assert_eq!(back["p/b"], 4.0, "clamped on load");
        assert!(load_from(&dir.0.join("missing.json")).is_empty());
    }

    /// The learner converges on the observed ratio and stays clamped.
    #[test]
    fn learner_converges_and_clamps() {
        let id = "t/learn";
        assert_eq!(bytes_per_token(id), DEFAULT_BPT);
        for _ in 0..30 {
            learn(id, 350_000, 0, 100_000);
        }
        assert!(
            (bytes_per_token(id) - 3.5).abs() < 0.01,
            "{}",
            bytes_per_token(id)
        );
        for _ in 0..30 {
            learn(id, 100_000, 0, 100_000);
        }
        assert!(
            (bytes_per_token(id) - 1.8).abs() < 0.001,
            "clamped: {}",
            bytes_per_token(id)
        );
    }

    /// R2-4: a request's image/PDF tokens (counted flat) are no text — the
    /// first sample of a media-heavy request learns the text's own ratio.
    #[test]
    fn media_tokens_are_taken_out_before_learning() {
        let id = "t/media";
        forget_for_tests(id);
        // 6,000 B of text (3,000 tokens) + one 1,600-token image.
        learn(id, 6_000, 1_600, 4_600);
        assert!(
            (bytes_per_token(id) - 2.0).abs() < 1e-9,
            "{}",
            bytes_per_token(id)
        );
        // All media: nothing to learn.
        learn("t/media-only", 300, 1_600, 1_600);
        assert!(!learned("t/media-only"));
    }
}
