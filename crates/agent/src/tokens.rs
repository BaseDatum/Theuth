//! 137G G1: one request-size policy for every model call. Requests are
//! sized in bytes and estimated at a per-model bytes-per-token ratio
//! (conservative 2.2 until real usage teaches it); [`budget`] says what a
//! caller should size to (`target`) and what may never be sent (`ceiling`);
//! [`preflight`] in `Backend::stream` refuses locally anything over the
//! ceiling, in the provider's own "prompt is too long" shape so every
//! caller's overflow path (and `compact::overflow_numbers`) reads it.

use std::collections::HashMap;
use std::sync::LazyLock;

use genai::chat::{ChatRequest, ContentPart};
use models::Resolved;
use parking_lot::Mutex;

/// Bytes per token before a model's usage is measured: dense JSON, code and
/// CJK run 2.0–2.7, so 2.2 over-counts prose rather than under-counting code.
pub const DEFAULT_BPT: f64 = 2.2;
/// The learner's clamp.
const BPT_RANGE: (f64, f64) = (1.8, 4.0);
/// EWMA weight of one new observation.
const ALPHA: f64 = 0.3;
/// The window assumed when the catalog does not know the model.
pub const UNKNOWN_WINDOW: u64 = 200_000;
/// Ceiling margin under `window − reply`.
const MARGIN: f64 = 0.05;

/// Learned bytes/token per model id.
static BPT: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(Default::default);
/// Learned prompt tokens per second per local model id (G1 latency limb).
static EVAL: LazyLock<Mutex<HashMap<String, f64>>> = LazyLock::new(Default::default);

/// The model's bytes per token: learned, else [`DEFAULT_BPT`].
pub fn bytes_per_token(model: &str) -> f64 {
    BPT.lock().get(model).copied().unwrap_or(DEFAULT_BPT)
}

/// Fold one observation (`bytes` sent, `tokens` the provider counted) into
/// the model's ratio: EWMA, clamped to 1.8..=4.0.
pub fn learn(model: &str, bytes: usize, tokens: u64) {
    if bytes < 256 || tokens == 0 {
        return;
    }
    let seen = (bytes as f64 / tokens as f64).clamp(BPT_RANGE.0, BPT_RANGE.1);
    let mut map = BPT.lock();
    let v = map.entry(model.to_owned()).or_insert(seen);
    *v = (*v * (1.0 - ALPHA) + seen * ALPHA).clamp(BPT_RANGE.0, BPT_RANGE.1);
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

/// The context window in tokens: the catalog's, else [`UNKNOWN_WINDOW`].
pub fn window(model: &Resolved) -> u64 {
    model
        .model
        .as_ref()
        .map(|m| u64::from(m.limit.context))
        .filter(|w| *w > 0)
        .unwrap_or(UNKNOWN_WINDOW)
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

/// `window − reply − 5 %`, in tokens.
pub fn ceiling(model: &Resolved, reply_tokens: u64) -> u64 {
    let room = window(model).saturating_sub(reply_tokens);
    (room as f64 * (1.0 - MARGIN)) as u64
}

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
/// estimated. `Ok(estimate)` otherwise.
pub fn preflight(model: &Resolved, req: &ChatRequest, reply_tokens: u64) -> Result<u64, String> {
    let est = estimate(&model.id, req);
    let max = ceiling(model, reply_tokens);
    if est > max {
        let known = model.model.as_ref().is_some_and(|m| m.limit.context > 0);
        return Err(format!(
            "prompt is too long: {est} tokens > {max} maximum (theuth preflight, not sent: {} estimated at {:.2} bytes/token, window {}{}, reply {reply_tokens})",
            model.id,
            bytes_per_token(&model.id),
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

    /// An unknown window is 200k for both limbs; the preflight refuses
    /// locally in the shape the overflow parser reads.
    #[test]
    fn unknown_window_preflight_refuses_locally() {
        let m = model("t/unknown", 0, None);
        let b = budget(Kind::Turn, &m, Limits::default());
        assert_eq!((b.target_tokens, b.ceiling_tokens), (100_000, 190_000));
        let big = ChatRequest::new(vec![genai::chat::ChatMessage::user("x".repeat(500_000))]);
        let e = preflight(&m, &big, 0).unwrap_err();
        let n = crate::compact::overflow_numbers(&e).expect("parsed");
        assert_eq!((n.prompt, n.window), (227_273, 190_000), "{e}");
        assert!(
            crate::compact::is_overflow(&e) && e.contains("not sent"),
            "{e}"
        );
        let small = ChatRequest::new(vec![genai::chat::ChatMessage::user("hello")]);
        assert!(preflight(&m, &small, 0).is_ok());
    }

    /// The learner converges on the observed ratio and stays clamped.
    #[test]
    fn learner_converges_and_clamps() {
        let id = "t/learn";
        assert_eq!(bytes_per_token(id), DEFAULT_BPT);
        for _ in 0..30 {
            learn(id, 350_000, 100_000);
        }
        assert!(
            (bytes_per_token(id) - 3.5).abs() < 0.01,
            "{}",
            bytes_per_token(id)
        );
        for _ in 0..30 {
            learn(id, 100_000, 100_000);
        }
        assert!(
            (bytes_per_token(id) - 1.8).abs() < 0.001,
            "clamped: {}",
            bytes_per_token(id)
        );
    }
}
