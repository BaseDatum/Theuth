//! 137 §5.3–§5.4, §6: one compactor call per node — the COMPACT prompt as
//! the system block, the frozen `<chat>` context as a cached user block, the
//! step (earlier lines, ruler, instruction + input) as an uncached one; size
//! and guard failures retry in the same conversation; the best candidate wins.

use std::time::{Duration, Instant};

use chrono::Utc;
use futures::StreamExt;
use futures::future::BoxFuture;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent, ReasoningEffort, Usage,
};
use models::Resolved;
use runtime::SessionId;

use super::guards;
use super::tree::Kind;
use crate::backend::Backend;
use crate::cost::{CostObservation, Money};
use crate::frame::Transport;

/// §5.4, verbatim; `{agent}` substituted by [`prompt`].
pub const COMPACT: &str = include_str!("compact_prompt.txt");

/// The COMPACT prompt for `agent` (constant bytes per session).
pub fn prompt(agent: &str) -> String {
    COMPACT.replace("{agent}", agent)
}

/// 201 §7.2 (P3): the `small` profile's system prompt (≤ 700 bytes, no ruler).
pub const SMALL: &str = include_str!("small_prompt.txt");
/// 201 §7.2: the `small` profile's reply cap in tokens (it also stops at the first newline).
pub const SMALL_REPLY_TOKENS: u32 = 160;
/// 201 §7.2: the `small` profile's context (nearby lines + the user's last message) in bytes at most.
pub const SMALL_CTX_BYTES: usize = 2_000;
/// 201 §7.2: a `small` chunk is never above this many bytes.
pub const SMALL_CHUNK_MAX: usize = 4_096;

/// 201 §7.2: the `small` profile's chunk for an oversized unit:
/// `(window − prompt − reply) × 0.5`, never above [`SMALL_CHUNK_MAX`]
/// (the prompt counted as the system prompt plus the context budget; at
/// least 512 bytes so a tiny window still makes progress).
pub fn small_chunk_bytes(window_bytes: usize) -> usize {
    let fixed = SMALL.len() + SMALL_CTX_BYTES + SMALL_REPLY_TOKENS as usize * 4;
    (window_bytes.saturating_sub(fixed) / 2).clamp(512, SMALL_CHUNK_MAX)
}

/// 201 §7.2: the `small` profile's user block for `step`: one short
/// imperative and the input; no earlier lines, no ruler.
pub fn small_render(step: &Step, node_bytes: u32) -> String {
    // R2-10: a word count a 1–4 B model can follow (about 6 bytes a word).
    let words = (node_bytes / 6).max(8);
    match step {
        Step::Unit { kind, text, .. } => format!(
            "Write one line about this message: one plain sentence of at most {words} words.\n{}: {text}",
            kind.as_str()
        ),
        Step::Merge { a, b, .. } => format!(
            "Join these two lines into one plain sentence of at most {words} words; keep names, paths and numbers; user items first.\n{a}\n{b}"
        ),
        Step::Batch { .. } => format!(
            "Write one line about these messages: one plain sentence of at most {words} words.\n{}",
            step.input()
        ),
    }
}

/// Reply ceiling without thinking (§5.3).
pub const MAX_TOKENS: u32 = 600;
/// 137F E6: a local stream's between-chunk idle bound (its header bound is the attempt's).
const LOCAL_IDLE: Duration = Duration::from_secs(120);

/// A timeout (ours or genai's stream header/read bound), not a provider error (R1-13).
fn is_timeout(e: &str) -> bool {
    e.contains("timed out")
}

/// Provider / adapter names of local model runtimes (E6).
const LOCAL_RUNTIMES: [&str; 11] = [
    "ollama",
    "lmstudio",
    "lm-studio",
    "llama.cpp",
    "llamacpp",
    "llama-cpp",
    "vllm",
    "localai",
    "koboldcpp",
    "mlx",
    "omlx",
];

/// A local runtime's provider / adapter name ([`LOCAL_RUNTIMES`]).
pub fn local_name(n: &str) -> bool {
    LOCAL_RUNTIMES.iter().any(|r| n.eq_ignore_ascii_case(r))
}

/// 137F E6: a local compactor — a local runtime provider; a loopback /
/// private (RFC 1918, CGNAT 100.64/10, link-local, ULA) / `*.local` /
/// `*.internal` / `*.lan` / `*.home.arpa` / `*.ts.net` / single-label
/// base-URL host; or a catalog entry without a price. An id
/// the catalog does not know is cloud unless its provider or host says
/// local (custom gateways, Azure deployments). The one locality rule
/// (attempt bound, E8 prompt cap, E9 queue).
pub fn is_local(model: &Resolved, base_url: Option<&str>) -> bool {
    let names = [
        model.provider.as_deref().unwrap_or(""),
        crate::loop_::ns(&model.genai),
    ];
    if names
        .iter()
        .any(|n| LOCAL_RUNTIMES.iter().any(|r| n.eq_ignore_ascii_case(r)))
    {
        return true;
    }
    if base_url.is_some_and(crate::local::private_url) {
        return true;
    }
    model.model.as_ref().is_some_and(|m| m.cost.is_none())
}

/// The E9 queue key of a local compactor (`None` = cloud): its base URL,
/// else its provider, else its genai namespace, else its id.
pub fn local_endpoint(model: &Resolved, base_url: Option<&str>) -> Option<String> {
    is_local(model, base_url).then(|| {
        base_url
            .map(str::to_owned)
            .or_else(|| model.provider.clone())
            .unwrap_or_else(|| match crate::loop_::ns(&model.genai) {
                "" => model.id.clone(),
                ns => ns.to_owned(),
            })
    })
}

/// R1 Q1: a compactor never thinks — thinking spends the reply cap and
/// leaves no text (Haiku 5.5 and Fable 5.1 think adaptively when no config
/// is sent). R2-7: on the Anthropic transport
/// [`ModelCompactor::request_lines`] sends [`anthropic_off`] whatever this
/// returns. Bedrock's Anthropic models get [`bedrock_off`]. For the other
/// transports — a `claude-*` id on an OpenAI-compatible gateway included —
/// this is the setting: `Zero` for an Ollama model (`think: false`), else
/// the model's lowest listed effort (`none` → `minimal` → `low`), else no
/// reasoning setting.
pub fn thinking_off(model: &Resolved) -> Option<ReasoningEffort> {
    let ns = crate::loop_::ns(&model.genai);
    let slug = model
        .genai
        .split_once("::")
        .map_or(model.genai.as_str(), |(_, s)| s);
    if ns == "ollama" {
        return Some(ReasoningEffort::Zero);
    }
    if ns.starts_with("bedrock") && slug.split('.').any(|seg| seg == "anthropic") {
        return Some(bedrock_off(slug));
    }
    let values: Vec<&str> = model
        .model
        .iter()
        .flat_map(|m| &m.reasoning_options)
        .filter_map(|o| match o {
            models::ReasoningOption::Effort { values } => Some(values.iter().map(String::as_str)),
            _ => None,
        })
        .flatten()
        .collect();
    [
        ("none", ReasoningEffort::Zero),
        ("minimal", ReasoningEffort::Minimal),
        ("low", ReasoningEffort::Low),
    ]
    .into_iter()
    .find(|(name, _)| values.contains(name))
    .map(|(_, e)| e)
}

/// R2-8: the least thinking genai's Anthropic mapping (`ant_reasoning.rs`
/// at d2fd71e) can ask of `slug`. `Zero` sends `thinking: {type:
/// "disabled"}` where genai marks thinking on by default (Haiku 5+, Sonnet
/// 5, Opus 5) and nothing elsewhere. Fable and Mythos are marked off by
/// default, so `Zero` sends nothing — and they then think adaptively (3 of 4
/// probe replies empty at 600 tokens) — while `disabled` is a 400 ("use
/// thinking.type.adaptive and output_config.effort"). For them `Low`, which
/// genai sends as `thinking: {type: "adaptive"}` + `output_config.effort:
/// "low"` (probe: 4 of 4 text replies, 0 thinking tokens).
pub fn anthropic_off(slug: &str) -> ReasoningEffort {
    // genai's own family test for these two (`legacy_is_fable_or_mythos`).
    if slug.contains("fable") || slug.contains("mythos") {
        ReasoningEffort::Low
    } else {
        ReasoningEffort::Zero
    }
}

/// R3-1: Bedrock rejects `thinking: disabled`, so genai's Converse adapter
/// returns before `insert_anthropic_reasoning` on `Zero` and sends nothing
/// (`bedrock/converse.rs` `publisher_additional_fields`). That leaves every
/// model that thinks unasked — genai's thinking-on-by-default set (Haiku 5+,
/// Sonnet 5, Opus 5) and the adaptive-only Fable and Mythos — thinking, so
/// they get `Low` (adaptive thinking at `output_config.effort: "low"`).
/// Older models (4.x and before) think only when asked: `Zero`.
pub fn bedrock_off(slug: &str) -> ReasoningEffort {
    let thinks_unasked = slug.split_once("claude-").is_some_and(|(_, rest)| {
        let mut seg = rest.split('-');
        let family = seg.next().unwrap_or_default();
        let major = seg.next().and_then(|m| m.parse::<u32>().ok());
        matches!(family, "fable" | "mythos")
            || matches!(family, "haiku" | "sonnet" | "opus") && major.is_some_and(|m| m >= 5)
    });
    if thinks_unasked {
        ReasoningEffort::Low
    } else {
        ReasoningEffort::Zero
    }
}

/// The step of one call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    /// `node(0, i)`: compress one unit.
    Unit {
        /// The unit's kind.
        kind: Kind,
        /// The unit's text (newlines kept).
        text: String,
        /// Lines already written for earlier units of this stretch (text only).
        earlier: Vec<String>,
        /// 137G R1 Q3: a firmer byte target than `node_bytes` for the
        /// instruction (a unit retried alone after its batch line tripped).
        aim: Option<u32>,
    },
    /// `node(l, i)`: merge two child lines.
    Merge {
        /// Left child line.
        a: String,
        /// Right child line.
        b: String,
        /// Lines already written for earlier units of this stretch (text only).
        earlier: Vec<String>,
    },
    /// 137G G6: `node(0, i)` for several units in one call — one numbered
    /// reply line per unit, each checked by the guards on its own.
    Batch {
        /// `(kind, text)` per unit, in unit order.
        units: Vec<(Kind, String)>,
        /// Lines already written for earlier units of this stretch (text only).
        earlier: Vec<String>,
        /// R2-5: a drafting (non-meeting) session's batch; a meeting's
        /// keeps the pre-201 failure path.
        drafting: bool,
    },
}

/// 137G G4: a unit step over this many bytes is summarised map-reduce.
pub const MAP_REDUCE_OVER: usize = 24_000;
/// 137G G4: one map chunk's bytes at most.
pub const CHUNK_BYTES: usize = 16_000;

impl Step {
    /// What G2/G3 compare against: the unit as `kind: text`, or the two child
    /// lines (a batch: every unit, one per line).
    pub fn input(&self) -> String {
        match self {
            Self::Unit { kind, text, .. } => format!("{}: {text}", kind.as_str()),
            Self::Merge { a, b, .. } => format!("{a}\n{b}"),
            Self::Batch { units, .. } => units
                .iter()
                .map(|(k, t)| format!("{}: {t}", k.as_str()))
                .collect::<Vec<_>>()
                .join("\n"),
        }
    }

    /// Block 2 of the first user turn (§5.3).
    pub fn render(&self, node_bytes: u32) -> String {
        let earlier = match self {
            Self::Unit { earlier, .. }
            | Self::Merge { earlier, .. }
            | Self::Batch { earlier, .. } => earlier,
        };
        let mut s = String::from(
            "Lines already written for earlier messages of this stretch (text only, newest last):\n",
        );
        if earlier.is_empty() {
            s.push_str("none\n");
        } else {
            for l in earlier {
                s.push_str(&l.replace('\n', " "));
                s.push('\n');
            }
        }
        s.push_str(&format!(
            "\nFor scale, this ruler is exactly {node_bytes} bytes and is NOT part of the chat:\n{}\n\n",
            guards::ruler(node_bytes)
        ));
        match self {
            Self::Unit {
                kind, text, aim, ..
            } => s.push_str(&format!(
                "Compress this message into one line, in at most {} bytes:\n{}: {text}",
                aim.unwrap_or(node_bytes).min(node_bytes),
                kind.as_str()
            )),
            Self::Merge { a, b, .. } => s.push_str(&format!(
                "Merge these two lines into one, in at most {node_bytes} bytes:\n{a}\n{b}"
            )),
            Self::Batch { units, .. } => {
                let n = units.len();
                s.push_str(&format!(
                    "Compress each of these {n} messages into one line each, in at most {node_bytes} bytes per line. \
                     Reply with exactly {n} lines, numbered in order as `1| …`, `2| …`, one per message, nothing else:"
                ));
                for (k, (kind, text)) in units.iter().enumerate() {
                    s.push_str(&format!("\n\n[{}] {}: {text}", k + 1, kind.as_str()));
                }
            }
        }
        s
    }
}

/// 137G G6: the batch reply's line for each of `n` units (`k| text`, also
/// `k.` `k:` `k)`); `None` for a number the reply lacks. R1-9: the last line
/// of a repeated number wins; an unnumbered line continues the numbered
/// line before it (a wrapped reply) — R2-5: up to a blank line.
pub fn batch_lines(reply: &str, n: usize) -> Vec<Option<String>> {
    let mut out: Vec<Option<String>> = vec![None; n];
    let mut last: Option<usize> = None;
    for line in reply.lines() {
        let l = line.trim();
        if l.is_empty() {
            last = None;
            continue;
        }
        let digits = l.bytes().take_while(u8::is_ascii_digit).count();
        let numbered = l[..digits].parse::<usize>().ok().and_then(|k| {
            let rest = l[digits..].trim_start();
            rest.strip_prefix(['|', '.', ':', ')'])
                .map(|rest| (k, rest.trim()))
        });
        match numbered {
            Some((k, rest)) if (1..=n).contains(&k) => {
                out[k - 1] = Some(rest.to_owned());
                last = Some(k - 1);
            }
            Some(_) => last = None,
            None => {
                if let Some(slot) = last.and_then(|k| out[k].as_mut()) {
                    slot.push(' ');
                    slot.push_str(l);
                }
            }
        }
    }
    out
}

/// One model reply.
#[derive(Clone, Debug, Default)]
pub struct Reply {
    /// The unit's text (newlines kept).
    pub text: String,
    /// Provider usage, when reported.
    pub usage: Option<Usage>,
    /// Wall-clock time of the attempt.
    pub latency: Duration,
    /// 177 E3: the provider's finish reason (`stop`, `length`, …), when reported.
    pub stop: Option<String>,
    /// 177 E3: the stop reason is the reply cap (`length`, `max_tokens`, …).
    pub truncated: bool,
    /// 177 E3: the reply carried reasoning (chunks, or reasoning tokens in usage).
    pub reasoning: bool,
    /// 177 E3: the `max_tokens` the attempt asked for (0 = not set).
    pub cap: u32,
    /// 177 E2/E3: the request switched thinking off (kwargs or an effort).
    pub thinking_off: bool,
}

impl Reply {
    /// Reasoning tokens from usage, when reported.
    pub fn reasoning_tokens(&self) -> Option<u32> {
        self.usage
            .as_ref()?
            .completion_tokens_details
            .as_ref()?
            .reasoning_tokens
            .and_then(|n| u32::try_from(n).ok())
    }

    /// 177 E3: no text, and the model thought or hit the cap — the reply was
    /// spent on thinking.
    pub fn spent_on_thinking(&self) -> bool {
        self.text.trim().is_empty() && (self.reasoning || self.truncated)
    }
}

/// 177 E3: what the panel shows when a reply was spent on thinking.
pub const THINKING_HINT: &str =
    "Pick a non-thinking summary model, or switch thinking off on the server.";

/// 177 E3: the error of a reply spent on thinking (a hard failure, R2-2).
pub fn thinking_error(model: &str, r: &Reply) -> String {
    format!(
        "{model} used its whole {}-token reply on thinking (finish_reason={}). {THINKING_HINT}",
        r.cap,
        r.stop.as_deref().unwrap_or("unknown")
    )
}

/// What the retry loop needs from a model: send the conversation so far, get
/// the next assistant text. `convo` alternates user/assistant after the two
/// opening user blocks (`ctx`, `step`).
pub trait Compactor: Send + Sync {
    /// The next assistant text for `convo` under the frozen `ctx`.
    fn compress<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>>;
    /// The model (id for `endless_nodes.model`, rates for the USD estimate, provider for the ledger).
    fn model(&self) -> &Resolved;
    /// Model id for `endless_nodes.model`.
    fn model_id(&self) -> &str {
        &self.model().id
    }
    /// The catalog rates for the USD estimate, if known.
    fn rates(&self) -> Option<models::Cost> {
        self.model().model.as_ref().and_then(|m| m.cost)
    }
    /// 137F E6/E9: the queue key when this is a local compactor, `None` for cloud.
    fn local(&self) -> Option<String> {
        local_endpoint(self.model(), None)
    }
    /// The reply cap in tokens of a reply of `lines` lines (E8 prompt
    /// fitting; G6 batch).
    fn out_tokens(&self, lines: u32) -> u32 {
        batch_reply_tokens(MAX_TOKENS, lines)
    }
    /// 137G G6: [`Compactor::compress`] for a reply of `lines` lines (a
    /// batch): the real compactor raises its reply cap to match.
    fn compress_lines<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
        lines: u32,
    ) -> BoxFuture<'a, Result<Reply, String>> {
        let _ = lines;
        self.compress(ctx, convo)
    }
    /// 177 E3: [`Compactor::compress`] with thinking off forced and twice the
    /// reply cap (R2-4: within the output limit and the window) — the one
    /// retry of a local reply spent on thinking.
    fn compress_thinking_off<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        self.compress(ctx, convo)
    }
    /// 177 R2-4: the forced retry could ask for more than the attempt that
    /// thought through its cap (`false`: the thinking error at once).
    fn can_grow(&self, ctx: &str, convo: &[ChatMessage]) -> bool {
        let _ = (ctx, convo);
        true
    }
    /// 201 §7.2 (P3): one `small`-profile attempt: the [`SMALL`] system
    /// prompt and a [`SMALL_REPLY_TOKENS`] cap (R2-7: no stop sequence; the
    /// first non-empty line of the reply is taken).
    fn compress_small<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        self.compress(ctx, convo)
    }
    /// 201 §8 (P4): the catalog says this model accepts images.
    fn accepts_images(&self) -> bool {
        crate::backend::supports_images(self.model())
    }
    /// 201 §8 (P4): a one-line description of `image` ([`DESCRIBE`]); asked
    /// only of a model that accepts images. The default sends nothing.
    fn describe_image<'a>(
        &'a self,
        image: &'a genai::chat::Binary,
    ) -> BoxFuture<'a, Result<Reply, String>> {
        let _ = image;
        Box::pin(async { Err("this compactor does not describe images".to_owned()) })
    }
}

/// 201 §8 (P4): the system prompt of an image description.
pub const DESCRIBE: &str = "Describe the image in one line of at most 30 words: what it shows, and any visible text that matters. Reply with that line only.";

/// R2-11: `inner`, recording every answered attempt as it lands — so a job
/// dropped by its outer bound (or a panic) still settles the usage of the
/// attempts that were answered, which [`build`] can no longer return.
pub struct Tally<'a> {
    inner: &'a dyn Compactor,
    rows: parking_lot::Mutex<Vec<CallRow>>,
    /// 177 E9: the last answered reply.
    last: parking_lot::Mutex<Option<Reply>>,
}

impl<'a> Tally<'a> {
    /// `inner` with no attempt recorded yet.
    pub fn new(inner: &'a dyn Compactor) -> Self {
        Self {
            inner,
            rows: parking_lot::Mutex::new(Vec::new()),
            last: parking_lot::Mutex::new(None),
        }
    }

    /// The answered attempts so far (guard unknown: the build never judged them).
    pub fn rows(&self) -> Vec<CallRow> {
        self.rows.lock().clone()
    }

    /// 177 E9: the last answered reply, if any.
    pub fn last(&self) -> Option<Reply> {
        self.last.lock().clone()
    }

    fn record(&self, reply: &Result<Reply, String>) {
        if let Ok(r) = reply {
            *self.last.lock() = Some(r.clone());
            self.rows.lock().push(CallRow {
                line: None,
                bytes: Some(r.text.trim().len() as u32),
                // R3-3: the build never judged it (dropped or panicked).
                guard: Some("unknown"),
                usage: r.usage.clone(),
                usd_estimate: r
                    .usage
                    .as_ref()
                    .and_then(|u| self.inner.rates().map(|c| estimate_usd(u, &c))),
                latency_ms: r.latency.as_millis() as u64,
            });
        }
    }
}

impl Compactor for Tally<'_> {
    fn compress<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(async move {
            let r = self.inner.compress(ctx, convo).await;
            self.record(&r);
            r
        })
    }
    fn model(&self) -> &Resolved {
        self.inner.model()
    }
    fn model_id(&self) -> &str {
        self.inner.model_id()
    }
    fn rates(&self) -> Option<models::Cost> {
        self.inner.rates()
    }
    fn local(&self) -> Option<String> {
        self.inner.local()
    }
    fn out_tokens(&self, lines: u32) -> u32 {
        self.inner.out_tokens(lines)
    }
    fn compress_lines<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
        lines: u32,
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(async move {
            let r = self.inner.compress_lines(ctx, convo, lines).await;
            self.record(&r);
            r
        })
    }
    fn compress_thinking_off<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(async move {
            let r = self.inner.compress_thinking_off(ctx, convo).await;
            self.record(&r);
            r
        })
    }
    fn compress_small<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(async move {
            let r = self.inner.compress_small(ctx, convo).await;
            self.record(&r);
            r
        })
    }
    fn can_grow(&self, ctx: &str, convo: &[ChatMessage]) -> bool {
        self.inner.can_grow(ctx, convo)
    }
}

/// 137G G6: the reply cap of a batch of `lines` (the single cap per line,
/// at most 16k tokens).
pub fn batch_reply_tokens(single: u32, lines: u32) -> u32 {
    (single + MAX_TOKENS * lines.saturating_sub(1)).min(16_000)
}

/// One attempt's ledger row (`endless_calls`). The store numbers it
/// (`try`): the attempt's 0-based ordinal on its node across the session.
#[derive(Clone, Debug, PartialEq)]
pub struct CallRow {
    /// 137G G6, a batch only: the 0-based line (unit) of the batch this row
    /// judges; `None` = the node the call was made for.
    pub line: Option<u32>,
    /// Reply bytes before any cut. `None` on a batch line after the first:
    /// that row shares the call of line 0's row, which alone carries its
    /// bytes, usage, cost and latency (a call is counted and charged once).
    pub bytes: Option<u32>,
    /// The guard that refused it (a batch row: its own line); `None` = accepted.
    pub guard: Option<&'static str>,
    /// Provider usage, when reported.
    pub usage: Option<Usage>,
    /// Catalog-rate estimate in USD.
    pub usd_estimate: Option<f64>,
    /// Wall-clock milliseconds.
    pub latency_ms: u64,
}

/// The result of [`build`].
#[derive(Clone, Debug, PartialEq)]
pub struct Built {
    /// The unit's text (newlines kept); empty for a batch.
    pub text: String,
    /// Attempts used; negative when no candidate passed (§6).
    pub tries: i32,
    /// Every attempt, in order.
    pub calls: Vec<CallRow>,
    /// 137G G6, a batch only: each unit's guard-checked line, or the guard
    /// its line tripped (that unit is retried alone).
    pub lines: Vec<Result<String, &'static str>>,
}

impl Built {
    /// The attempts' usage summed into one (for the node's `model_calls` row);
    /// `None` when no attempt reported usage.
    pub fn usage(&self) -> Option<Usage> {
        sum_usage(&self.calls)
    }
}

/// `calls`' usage summed into one; `None` when no attempt reported usage.
pub fn sum_usage(calls: &[CallRow]) -> Option<Usage> {
    let mut out: Option<Usage> = None;
    for u in calls.iter().filter_map(|c| c.usage.as_ref()) {
        let o = out.get_or_insert_with(Usage::default);
        let add = |a: &mut Option<i32>, b: Option<i32>| {
            if let Some(b) = b {
                *a = Some(a.unwrap_or(0).saturating_add(b));
            }
        };
        add(&mut o.prompt_tokens, u.prompt_tokens);
        add(&mut o.completion_tokens, u.completion_tokens);
        if let Some(d) = &u.prompt_tokens_details {
            let od = o.prompt_tokens_details.get_or_insert_with(Default::default);
            add(&mut od.cached_tokens, d.cached_tokens);
            add(&mut od.cache_creation_tokens, d.cache_creation_tokens);
        }
    }
    out
}

/// A build that failed, with the ledger rows of the attempts that were
/// answered before it (R1-4: they were billed).
#[derive(Clone, Debug, PartialEq)]
pub struct Failed {
    /// The error, one provider or transport message.
    pub error: String,
    /// Answered attempts before the failure.
    pub calls: Vec<CallRow>,
}

impl From<String> for Failed {
    fn from(error: String) -> Self {
        Self {
            error,
            calls: Vec::new(),
        }
    }
}

/// R1 Q2: the guards, except that a line whose only trip is `size` is cut
/// at a word boundary to `node_bytes` (with an ellipsis) and accepted.
pub fn accept(
    line: &str,
    input: &str,
    ruler: &str,
    node_bytes: u32,
) -> Result<String, guards::Trip> {
    match guards::check(line, input, ruler, guards::PROMPT_PHRASES, node_bytes) {
        Err(guards::Trip::Size) => {
            guards::check(line, input, ruler, guards::PROMPT_PHRASES, u32::MAX)
                .map(|l| guards::cut_words(&l, node_bytes as usize))
        }
        other => other,
    }
}

/// One node's build: a batch (G6), a long unit map-reduced (G4), else the
/// single-call retry loop ([`build_one`]).
pub async fn build(
    c: &dyn Compactor,
    ctx: &str,
    step: &Step,
    node_bytes: u32,
    tries: u8,
    attempt: Duration,
) -> Result<Built, Failed> {
    match step {
        Step::Batch {
            units, drafting, ..
        } => build_batch(c, ctx, step, units, *drafting, node_bytes, attempt).await,
        Step::Unit {
            kind,
            text,
            earlier,
            aim,
        } if text.len() > MAP_REDUCE_OVER => {
            // G4: chunks of ≤ CHUNK_BYTES summarised alone, then one reduce
            // call over their lines gives the node; every call is bounded by
            // the same ctx and the backend preflight.
            let mut lines = Vec::new();
            let mut calls = Vec::new();
            let mut tries_used = 0i32;
            let mut failed_guard = false;
            let parts = chunks(text, CHUNK_BYTES)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>();
            for chunk in parts.into_iter().map(Some).chain([None]) {
                let part = Step::Unit {
                    kind: *kind,
                    text: chunk.unwrap_or_else(|| lines.join("\n")),
                    earlier: earlier.clone(),
                    aim: *aim,
                };
                match build_one(c, ctx, &part, node_bytes, tries, attempt).await {
                    Ok(b) => {
                        tries_used += b.tries.abs();
                        failed_guard |= b.tries < 0;
                        calls.extend(b.calls);
                        lines.push(b.text);
                        // R2-11: a chunk that failed its guards fails the
                        // build; the remaining chunks are not called.
                        if failed_guard {
                            break;
                        }
                    }
                    Err(mut f) => {
                        calls.append(&mut f.calls);
                        return Err(Failed {
                            error: f.error,
                            calls,
                        });
                    }
                }
            }
            Ok(Built {
                text: lines.pop().unwrap_or_default(),
                tries: if failed_guard {
                    -tries_used
                } else {
                    tries_used
                },
                calls,
                lines: Vec::new(),
            })
        }
        _ => build_one(c, ctx, step, node_bytes, tries, attempt).await,
    }
}

/// 201 §7.2 (P3) seam: constrained output (a JSON schema or grammar) for a
/// `small` request where the server supports it. Nothing in the backend
/// plumbs a response format today, so this returns `opts` unchanged; the
/// phase that adds per-server capability detection fills it.
fn small_output_format(opts: ChatOptions) -> ChatOptions {
    opts
}

/// R2-2: at most this many chunk calls for one `small`-profile unit; a
/// larger unit keeps its draft (head and tail), never a line of its head.
pub const SMALL_MAX_CHUNKS: usize = 16;

/// 201 §7.2 (P3): a `small`-profile build — one attempt per call, no
/// correction dialogue; a unit over `chunk` bytes is summarised chunk by
/// chunk, then (R2-2) reduced in stages: the lines are joined in groups of
/// at least two that fit `chunk`, each group summarised, until one line
/// covers the whole unit. Over [`SMALL_MAX_CHUNKS`] chunks nothing is sent.
/// A guard trip (any part) is a [`Built`] with negative `tries`, which the
/// pump never stores: the draft stays and the unit is retried once later.
pub async fn build_small(
    c: &dyn Compactor,
    ctx: &str,
    step: &Step,
    node_bytes: u32,
    attempt: Duration,
    chunk: usize,
) -> Result<Built, Failed> {
    let Step::Unit { kind, text, .. } = step else {
        return small_once(c, ctx, step, node_bytes, attempt).await;
    };
    if text.len() <= chunk {
        return small_once(c, ctx, step, node_bytes, attempt).await;
    }
    let parts = chunks(text, chunk);
    if parts.len() > SMALL_MAX_CHUNKS {
        return Ok(Built {
            text: String::new(),
            tries: -1,
            calls: Vec::new(),
            lines: Vec::new(),
        });
    }
    let unit = |text: String| Step::Unit {
        kind: *kind,
        text,
        earlier: Vec::new(),
        aim: None,
    };
    let mut calls = Vec::new();
    let mut used = 0i32;
    // Each entry: the input of one call, or `None` for a line that passes
    // to the next stage as it is (an odd one out).
    let mut stage: Vec<(Option<String>, String)> = parts
        .into_iter()
        .map(|p| (Some(p.to_owned()), String::new()))
        .collect();
    loop {
        let mut lines = Vec::with_capacity(stage.len());
        for (input, kept) in stage {
            let Some(input) = input else {
                lines.push(kept);
                continue;
            };
            match small_once(c, ctx, &unit(input), node_bytes, attempt).await {
                Ok(b) if b.tries > 0 => {
                    used += b.tries;
                    calls.extend(b.calls);
                    lines.push(b.text);
                }
                Ok(mut b) => {
                    b.calls.splice(0..0, calls);
                    b.tries -= used;
                    return Ok(b);
                }
                Err(mut f) => {
                    f.calls.splice(0..0, calls);
                    return Err(f);
                }
            }
        }
        if lines.len() == 1 {
            return Ok(Built {
                text: lines.pop().unwrap_or_default(),
                tries: used,
                calls,
                lines: Vec::new(),
            });
        }
        stage = small_groups(lines, chunk);
    }
}

/// R2-2: the next reduce stage — `lines` joined greedily into groups of at
/// least two that fit `chunk` (a pair over it is cut), so every stage at
/// least halves the lines; a last lone line passes through.
fn small_groups(lines: Vec<String>, chunk: usize) -> Vec<(Option<String>, String)> {
    let mut out = Vec::new();
    let mut group: Vec<String> = Vec::new();
    let mut bytes = 0usize;
    for line in lines {
        if group.len() >= 2 && bytes + 1 + line.len() > chunk {
            let joined = group.join("\n");
            out.push((
                Some(guards::cut_utf8(&joined, chunk).to_owned()),
                String::new(),
            ));
            group.clear();
            bytes = 0;
        }
        bytes += line.len() + usize::from(!group.is_empty());
        group.push(line);
    }
    match group.len() {
        0 => {}
        1 => out.push((None, group.pop().unwrap_or_default())),
        _ => {
            let joined = group.join("\n");
            out.push((
                Some(guards::cut_utf8(&joined, chunk).to_owned()),
                String::new(),
            ));
        }
    }
    out
}

/// One `small` call: [`small_render`] as the only user turn after `ctx`,
/// the reply's first non-empty line checked by the unchanged guards.
async fn small_once(
    c: &dyn Compactor,
    ctx: &str,
    step: &Step,
    node_bytes: u32,
    attempt: Duration,
) -> Result<Built, Failed> {
    let input = step.input();
    let ruler = guards::ruler(node_bytes);
    let convo = [ChatMessage::user(small_render(step, node_bytes))];
    let reply = match bounded(c.compress_small(ctx, &convo), attempt, false).await {
        Ok(Some(r)) => r,
        Ok(None) => return Err("compactor returned nothing".to_owned().into()),
        Err(error) => {
            return Err(Failed {
                error,
                calls: Vec::new(),
            });
        }
    };
    let row = |raw: &str, guard: Option<&'static str>| CallRow {
        line: None,
        bytes: Some(raw.len() as u32),
        guard,
        usage: reply.usage.clone(),
        usd_estimate: reply
            .usage
            .as_ref()
            .and_then(|u| c.rates().map(|r| estimate_usd(u, &r))),
        latency_ms: reply.latency.as_millis() as u64,
    };
    if reply.spent_on_thinking() {
        return Err(Failed {
            error: thinking_error(c.model_id(), &reply),
            calls: vec![row("", Some(guards::Trip::Empty.as_str()))],
        });
    }
    let raw = reply
        .text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or_default()
        .to_owned();
    let check = accept(&raw, &input, &ruler, node_bytes);
    let calls = vec![row(&raw, check.as_ref().err().map(|t| t.as_str()))];
    Ok(match check {
        Ok(text) => Built {
            text,
            tries: 1,
            calls,
            lines: Vec::new(),
        },
        Err(_) => Built {
            text: guards::cut_utf8(&raw, node_bytes as usize).to_owned(),
            tries: -1,
            calls,
            lines: Vec::new(),
        },
    })
}

/// R1-3 / G11: the prompt bytes of the largest single call `step` makes,
/// without its context (a map-reduce unit: one chunk's call). R2-9:
/// `small` = the small profile's chunk: its prompt and its chunk size.
pub fn largest_call_bytes(step: &Step, node_bytes: u32, small: Option<usize>) -> usize {
    let (over, chunk, prompt) = match small {
        Some(chunk) => (chunk, chunk, SMALL.len()),
        None => (MAP_REDUCE_OVER, CHUNK_BYTES, COMPACT.len()),
    };
    let render = |s: &Step| match small {
        Some(_) => small_render(s, node_bytes).len(),
        None => s.render(node_bytes).len(),
    };
    let one = match step {
        Step::Unit {
            kind,
            text,
            earlier,
            aim,
        } if text.len() > over => render(&Step::Unit {
            kind: *kind,
            text: chunks(text, chunk)
                .first()
                .copied()
                .unwrap_or_default()
                .to_owned(),
            earlier: earlier.clone(),
            aim: *aim,
        }),
        _ => render(step),
    };
    prompt + one
}

/// `text` cut into pieces of at most `max` bytes, at a line end when one
/// falls in the second half of the piece, else at a char boundary.
pub fn chunks(text: &str, max: usize) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = text;
    while rest.len() > max {
        let head = guards::cut_utf8(rest, max);
        let at = head
            .rfind('\n')
            .filter(|p| *p >= max / 2)
            .map_or(head.len(), |p| p + 1);
        out.push(&rest[..at]);
        rest = &rest[at..];
    }
    if !rest.is_empty() {
        out.push(rest);
    }
    out
}

/// 137G G6: one call for a batch; each unit's numbered line passes the same
/// guards as a single build (a size-only trip is cut, Q2), or names the
/// guard it tripped. No in-call retries: a failing unit is retried alone
/// with the single-unit prompt.
/// 201 P1 (F8): a reply spent on thinking gets one retry with thinking off
/// forced for a local compactor (as [`build_one`]); otherwise, or again,
/// the whole batch fails with [`thinking_error`], and an empty reply fails
/// it as "returned nothing": its units keep their drafts and backoff
/// instead of each going solo. R2-4: every reply is charged once, as rows
/// of the result, failed or not: one row per unit line carrying that line's
/// guard (a reply spent on thinking or empty: `empty` on every line), the
/// call's bytes, usage and cost on line 0's row only. R2-5: a meeting batch
/// (`!drafting`) keeps the pre-201 path — one call, an empty or
/// thinking-spent reply trips every line, and its units run solo at once.
async fn build_batch(
    c: &dyn Compactor,
    ctx: &str,
    step: &Step,
    units: &[(Kind, String)],
    drafting: bool,
    node_bytes: u32,
    attempt: Duration,
) -> Result<Built, Failed> {
    let convo = &[ChatMessage::user(step.render(node_bytes))];
    let lines_n = units.len() as u32;
    let send = |forced: bool| async move {
        let fut = if forced {
            c.compress_thinking_off(ctx, convo)
        } else {
            c.compress_lines(ctx, convo, lines_n)
        };
        match tokio::time::timeout(attempt, fut).await {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(e)) => Err(Failed::from(e)),
            Err(_) => Err(format!("timed out after {}s", attempt.as_secs()).into()),
        }
    };
    // One row per line; line 0's row carries the call (bytes, usage, cost,
    // latency), the others only their line's guard.
    let rows = |reply: &Reply, guards: Vec<Option<&'static str>>| -> Vec<CallRow> {
        guards
            .into_iter()
            .enumerate()
            .map(|(k, guard)| {
                let call = k == 0;
                CallRow {
                    line: Some(k as u32),
                    bytes: call.then(|| reply.text.trim().len() as u32),
                    guard,
                    usage: reply.usage.clone().filter(|_| call),
                    usd_estimate: reply
                        .usage
                        .as_ref()
                        .filter(|_| call)
                        .and_then(|u| c.rates().map(|r| estimate_usd(u, &r))),
                    latency_ms: if call {
                        reply.latency.as_millis() as u64
                    } else {
                        0
                    },
                }
            })
            .collect()
    };
    // `max(1)`: a call is charged even with no line to judge.
    let empty = || vec![Some(guards::Trip::Empty.as_str()); units.len().max(1)];
    let fail = |error: String, calls: Vec<CallRow>| Failed { error, calls };
    let mut calls = Vec::new();
    let mut reply = send(false).await?;
    if drafting && reply.spent_on_thinking() {
        calls.extend(rows(&reply, empty()));
        if c.local().is_none() || !c.can_grow(ctx, convo) {
            return Err(fail(thinking_error(c.model_id(), &reply), calls));
        }
        reply = match send(true).await {
            Ok(r) => r,
            Err(f) => return Err(fail(f.error, calls)),
        };
        if reply.spent_on_thinking() {
            calls.extend(rows(&reply, empty()));
            return Err(fail(thinking_error(c.model_id(), &reply), calls));
        }
    }
    if drafting && reply.text.trim().is_empty() {
        calls.extend(rows(&reply, empty()));
        return Err(fail("compactor returned nothing".to_owned(), calls));
    }
    let raw = reply.text.trim();
    let ruler = guards::ruler(node_bytes);
    let lines: Vec<Result<String, &'static str>> = units
        .iter()
        .zip(batch_lines(raw, units.len()))
        .map(|((kind, text), line)| match line {
            None => Err(guards::Trip::Format.as_str()),
            Some(l) => accept(
                &l,
                &format!("{}: {text}", kind.as_str()),
                &ruler,
                node_bytes,
            )
            .map_err(guards::Trip::as_str),
        })
        .collect();
    let mut judged: Vec<Option<&'static str>> =
        lines.iter().map(|l| l.as_ref().err().copied()).collect();
    if judged.is_empty() {
        judged.push(None);
    }
    calls.extend(rows(&reply, judged));
    Ok(Built {
        text: String::new(),
        tries: 1,
        calls,
        lines,
    })
}

/// One attempt bounded by `attempt`: `Ok(None)` ends the loop on the
/// candidate already held (`have`, E7); a provider/transport error, or a
/// timeout with no candidate, fails the node.
async fn bounded(
    fut: BoxFuture<'_, Result<Reply, String>>,
    attempt: Duration,
    have: bool,
) -> Result<Option<Reply>, String> {
    match tokio::time::timeout(attempt, fut).await {
        Ok(Ok(reply)) => Ok(Some(reply)),
        Ok(Err(error)) if !have || !is_timeout(&error) => Err(error),
        Err(_) if !have => Err(format!("timed out after {}s", attempt.as_secs())),
        _ => Ok(None),
    }
}

/// Run the retry loop for one node: up to `tries` attempts, each guard
/// failure answered in the same conversation (a size-only trip is cut and
/// accepted, Q2); keep the shortest passing candidate, else the shortest of
/// all. Each attempt is bounded by `attempt` (137F E6); an attempt that
/// fails or times out after a candidate exists ends the loop on that
/// candidate (E7) instead of failing the node. A failure carries the rows
/// of the attempts answered before it (R1-4).
/// 177 E3: a local reply spent on thinking gets one more attempt with
/// thinking off forced and twice the cap; a second one (or a cloud one)
/// fails the node with [`thinking_error`].
async fn build_one(
    c: &dyn Compactor,
    ctx: &str,
    step: &Step,
    node_bytes: u32,
    tries: u8,
    attempt: Duration,
) -> Result<Built, Failed> {
    let input = step.input();
    let ruler = guards::ruler(node_bytes);
    let mut convo = vec![ChatMessage::user(step.render(node_bytes))];
    let mut calls = Vec::new();
    let mut passing: Option<String> = None;
    let mut shortest: Option<String> = None;
    let mut used = 0i32;
    // 177 E3: thinking off forced (after a local reply spent on thinking).
    let mut forced = false;
    let row = |reply: &Reply, raw: &str, guard: Option<&'static str>| CallRow {
        line: None,
        bytes: Some(raw.len() as u32),
        guard,
        usage: reply.usage.clone(),
        usd_estimate: reply
            .usage
            .as_ref()
            .and_then(|u| c.rates().map(|r| estimate_usd(u, &r))),
        latency_ms: reply.latency.as_millis() as u64,
    };
    for _ in 0..tries.max(1) as u32 {
        used += 1;
        let send = |forced: bool| {
            if forced {
                c.compress_thinking_off(ctx, &convo)
            } else {
                c.compress(ctx, &convo)
            }
        };
        let mut reply = match bounded(send(forced), attempt, shortest.is_some()).await {
            Ok(Some(r)) => r,
            Ok(None) => break,
            Err(error) => return Err(Failed { error, calls }),
        };
        // 177 E3: a reply spent on thinking is no guard trip to correct: a
        // local model gets one retry with thinking off forced and twice the
        // cap; otherwise (or again) it is the node's error.
        if reply.spent_on_thinking() {
            calls.push(row(&reply, "", Some(guards::Trip::Empty.as_str())));
            if forced || c.local().is_none() || !c.can_grow(ctx, &convo) {
                return Err(Failed {
                    error: thinking_error(c.model_id(), &reply),
                    calls,
                });
            }
            forced = true;
            used += 1;
            reply = match bounded(send(true), attempt, shortest.is_some()).await {
                Ok(Some(r)) => r,
                Ok(None) => break,
                Err(error) => return Err(Failed { error, calls }),
            };
            if reply.spent_on_thinking() {
                calls.push(row(&reply, "", Some(guards::Trip::Empty.as_str())));
                return Err(Failed {
                    error: thinking_error(c.model_id(), &reply),
                    calls,
                });
            }
        }
        let raw = reply.text.trim().to_owned();
        let check = accept(&raw, &input, &ruler, node_bytes);
        calls.push(row(&reply, &raw, check.as_ref().err().map(|t| t.as_str())));
        let candidate = guards::cut_utf8(&raw, node_bytes as usize).to_owned();
        if !candidate.is_empty() && shortest.as_ref().is_none_or(|s| candidate.len() < s.len()) {
            shortest = Some(candidate.clone());
        }
        match check {
            Ok(line) => {
                if passing.as_ref().is_none_or(|p| line.len() < p.len()) {
                    passing = Some(line);
                }
                break;
            }
            Err(trip) => {
                convo.push(ChatMessage::assistant(raw.clone()));
                convo.push(ChatMessage::user(guards::correction(
                    trip, &raw, &input, node_bytes,
                )));
            }
        }
    }
    match (passing, shortest) {
        (Some(text), _) => Ok(Built {
            text,
            tries: used,
            calls,
            lines: Vec::new(),
        }),
        (None, Some(text)) => Ok(Built {
            text,
            tries: -used,
            calls,
            lines: Vec::new(),
        }),
        (None, None) => Err(Failed {
            error: "compactor returned nothing".into(),
            calls,
        }),
    }
}

/// Catalog rates (USD per million) × this call's tokens — the formula of
/// `loop_::estimate` (private there), so `endless_calls.usd_estimate` and
/// `model_calls` agree.
pub fn estimate_usd(u: &Usage, rates: &models::Cost) -> f64 {
    let obs = observe(u);
    let read = obs.cache_read_tokens.unwrap_or(0);
    let write = obs.cache_write_tokens.unwrap_or(0);
    let uncached = obs
        .input_tokens
        .unwrap_or(0)
        .saturating_sub(read)
        .saturating_sub(write);
    (f64::from(uncached) * rates.input
        + f64::from(read) * rates.cache_read.unwrap_or(rates.input)
        + f64::from(write) * rates.cache_write.unwrap_or(rates.input)
        + f64::from(obs.output_tokens.unwrap_or(0)) * rates.output)
        / 1e6
}

/// Token buckets of a genai usage (the fields `loop_::observe` reads).
pub fn observe(u: &Usage) -> CostObservation {
    let tokens = |n: Option<i32>| n.and_then(|n| u32::try_from(n).ok());
    let details = u.prompt_tokens_details.as_ref();
    CostObservation {
        input_tokens: tokens(u.prompt_tokens),
        output_tokens: tokens(u.completion_tokens),
        cache_read_tokens: tokens(details.and_then(|d| d.cached_tokens)),
        cache_write_tokens: tokens(details.and_then(|d| d.cache_creation_tokens)),
        ..Default::default()
    }
}

/// The `model_calls` observation for one attempt (settled under `CallKind::Endless`).
pub fn observation(u: Option<&Usage>, model: &Resolved) -> CostObservation {
    let mut obs = match u {
        Some(u) => observe(u),
        None => CostObservation {
            unknown_reason: Some("no usage returned".into()),
            ..Default::default()
        },
    };
    obs.requested_model = Some(model.id.clone());
    obs.provider = model.provider.clone();
    obs.context_tokens = match (obs.input_tokens, obs.output_tokens) {
        (None, None) => None,
        (i, o) => Some(i.unwrap_or(0).saturating_add(o.unwrap_or(0))),
    };
    if let (Some(u), Some(rates)) = (u, model.model.as_ref().and_then(|m| m.cost)) {
        obs.estimated = Money::from_f64(estimate_usd(u, &rates), "USD").ok();
    } else if u.is_some() {
        obs.unknown_reason = Some("no cost returned".into());
    }
    obs
}

/// The real compactor: the engine's [`Backend`] for `model` (D8), the §5.3
/// request shape, cache marks via genai's [`CacheControl`] (07 §3).
#[derive(Clone)]
pub struct ModelCompactor {
    /// The session (cache key, ledger owner).
    pub session: SessionId,
    /// The compactor model (D8).
    pub model: Resolved,
    pub(crate) backend: Backend,
    /// The COMPACT prompt with `{agent}` substituted.
    pub system: String,
    /// The reasoning setting off the Anthropic transport ([`thinking_off`]),
    /// or the setting's own pick when `chosen`.
    pub effort: Option<ReasoningEffort>,
    /// 194 D4e: `effort` is the user's pick for this compactor (Settings ›
    /// Memory, the chat's own, the agent's `memory_effort`), sent on every
    /// transport with its thinking budget; `false` = thinking off (R1 Q1).
    pub chosen: bool,
    /// 137F E6: `Some(queue key)` for a local compactor ([`local_endpoint`]).
    pub local: Option<String>,
    /// 177 E2: a local OpenAI-protocol server — thinking off by
    /// `chat_template_kwargs`, no effort keyword.
    pub quiet: bool,
    /// `agent.endless` in force (the cloud stream bounds follow `attempt_secs`, R1-12).
    pub(crate) cfg: std::sync::Arc<parking_lot::RwLock<super::EndlessConfig>>,
}

/// 177 E3: a local compactor's single-unit reply cap (local calls cost nothing).
pub const LOCAL_MAX_TOKENS: u32 = 2_048;

impl ModelCompactor {
    /// The thinking budget the effort implies on Anthropic (genai's
    /// `ant_reasoning.rs` budgets), else 0.
    fn budget(&self) -> u32 {
        match &self.effort {
            None | Some(ReasoningEffort::Zero | ReasoningEffort::Minimal) => 0,
            Some(ReasoningEffort::Low) => 1_024,
            Some(ReasoningEffort::Medium) => 8_000,
            Some(ReasoningEffort::High | ReasoningEffort::XHigh | ReasoningEffort::Max) => 24_000,
            Some(ReasoningEffort::Budget(n)) => *n,
        }
    }

    /// A single unit's reply cap, without thinking: 600 (177 E3: a local
    /// compactor 2048, within the model's own output limit).
    fn reply_tokens(&self) -> u32 {
        match (
            &self.local,
            self.model.model.as_ref().map(|m| m.limit.output),
        ) {
            (Some(_), Some(out)) if out > 0 => LOCAL_MAX_TOKENS.min(out),
            (Some(_), _) => LOCAL_MAX_TOKENS,
            (None, _) => MAX_TOKENS,
        }
    }

    /// 194 R1: the most `max_tokens` one request can carry — the model's
    /// output limit and half its window (what [`crate::tokens::ceiling`]
    /// keeps for a reply).
    fn carry(&self) -> u32 {
        let half = u32::try_from(crate::tokens::memory_window(&self.model) / 2).unwrap_or(u32::MAX);
        self.model
            .model
            .as_ref()
            .map(|m| m.limit.output)
            .filter(|&o| o > 0)
            .map_or(half, |o| o.min(half))
    }

    /// `max_tokens` for a reply of `lines` lines: the reply ([`Self::reply_tokens`]
    /// for one unit, 600 a line for a batch, at most 16k) and beside it (194
    /// R1) the effort's thinking budget — none under `anthropic_off` —
    /// within [`Self::carry`]. [`Self::with_pick`] keeps a numeric budget a
    /// line under the carry, so the cap stays above it.
    fn cap(&self, anthropic_off: bool, lines: u32) -> u32 {
        let reply = if lines <= 1 && !anthropic_off {
            self.reply_tokens()
        } else {
            batch_reply_tokens(MAX_TOKENS, lines)
        };
        let thinking = if anthropic_off { 0 } else { self.budget() };
        reply.saturating_add(thinking).min(self.carry())
    }

    /// 194 D4e: the user's `pick` for this compactor (Settings › Memory, the
    /// chat's own, the agent's `memory_effort`). A positive level rides every
    /// transport as picked (`chosen`); a numeric budget a request cannot
    /// carry beside a one-line reply is clamped to what it can (R1, warned).
    /// None or an explicit Off keeps today's thinking off by transport (R1
    /// Q1): [`anthropic_off`] (Fable: adaptive at `low`), the local kwargs —
    /// never `Zero` on the wire, which genai's omlx adapter sends as
    /// `enable_thinking: true` and Fable answers empty.
    pub(crate) fn with_pick(mut self, pick: Option<ReasoningEffort>) -> Self {
        let Some(e) =
            pick.filter(|e| !matches!(e, ReasoningEffort::Zero | ReasoningEffort::Budget(0)))
        else {
            return self;
        };
        let room = self.carry().saturating_sub(MAX_TOKENS);
        // ponytail: a named level is sent as picked; its genai budget (24k at
        // High, Anthropic budget thinking only) needs an output limit above it.
        self.effort = Some(match e {
            ReasoningEffort::Budget(n) if n > room => {
                tracing::warn!(model = %self.model.id, budget = n, room, "endless compactor: thinking budget clamped to what a request carries");
                ReasoningEffort::Budget(room)
            }
            e => e,
        });
        self.chosen = true;
        self
    }

    /// `[system][user ctx (marked)][user step][assistant][user correction]…`.
    pub fn request(
        &self,
        t: Transport,
        ctx: &str,
        convo: &[ChatMessage],
    ) -> (ChatRequest, ChatOptions) {
        self.request_lines(t, ctx, convo, 1)
    }

    /// [`ModelCompactor::request`] for a reply of `lines` lines (G6 batch).
    pub fn request_lines(
        &self,
        t: Transport,
        ctx: &str,
        convo: &[ChatMessage],
        lines: u32,
    ) -> (ChatRequest, ChatOptions) {
        self.request_opts(t, ctx, convo, lines, false)
    }

    /// [`ModelCompactor::request_lines`]; `forced` (177 E3, the retry of a
    /// reply spent on thinking) doubles the cap. Thinking is already off as far
    /// as the transport allows: kwargs on a local OpenAI-protocol server,
    /// `think: false` on genai's Ollama adapter, nothing elsewhere (a priceless
    /// cloud gateway counts as local for 137F E6 but must not get the kwargs).
    pub fn request_opts(
        &self,
        t: Transport,
        ctx: &str,
        convo: &[ChatMessage],
        lines: u32,
        forced: bool,
    ) -> (ChatRequest, ChatOptions) {
        self.request_with(&self.system, t, ctx, convo, lines, forced)
    }

    /// 201 §7.2 (P3): a `small`-profile request: the [`SMALL`] system
    /// prompt, a [`SMALL_REPLY_TOKENS`] cap (plus a chosen thinking budget,
    /// within [`Self::carry`]); R2-7: no stop sequence (a reply opening
    /// with a newline would be empty; some providers refuse it). Thinking off
    /// and the transport options as [`Self::request_opts`].
    pub fn request_small(
        &self,
        t: Transport,
        ctx: &str,
        convo: &[ChatMessage],
    ) -> (ChatRequest, ChatOptions) {
        let (req, opts) = self.request_with(SMALL, t, ctx, convo, 1, false);
        let thinking = if self.chosen { self.budget() } else { 0 };
        let cap = SMALL_REPLY_TOKENS
            .saturating_add(thinking)
            .min(self.carry());
        let opts = small_output_format(opts.with_max_tokens(cap));
        (req, opts)
    }

    fn request_with(
        &self,
        system: &str,
        t: Transport,
        ctx: &str,
        convo: &[ChatMessage],
        lines: u32,
        forced: bool,
    ) -> (ChatRequest, ChatOptions) {
        let anthropic = t == Transport::Anthropic;
        let mut messages = Vec::with_capacity(convo.len() + 2);
        if t == Transport::Anthropic {
            messages.push(
                ChatMessage::system(system.to_owned()).with_options(CacheControl::Ephemeral1h),
            );
            messages.push(ChatMessage::user(ctx.to_owned()).with_options(CacheControl::Ephemeral));
        } else {
            messages.push(ChatMessage::user(ctx.to_owned()));
        }
        messages.extend_from_slice(convo);
        messages
            .iter_mut()
            .for_each(crate::diff_output::strip_message);
        let mut req = ChatRequest::new(messages);
        if t != Transport::Anthropic {
            req = req.with_system(system.to_owned());
        }
        let ant_off = anthropic && !self.chosen;
        let cap = self.cap(ant_off, lines);
        let max = if forced {
            self.forced_cap(&req, cap)
        } else {
            cap
        };
        let mut opts = ChatOptions::default()
            .with_capture_usage(true)
            .with_capture_content(true)
            // R3-2: no `with_capture_reasoning_content` — only text parts are read.
            .with_max_tokens(max);
        // E6: idle bounds; a local stream's first token may take the whole
        // attempt (prompt eval on slow hardware), which `build` bounds. A
        // cloud stream's first byte and every silence between chunks may
        // take the attempt bound (R1-12, R2-4: a silent reasoning model).
        opts = if self.local.is_some() {
            opts.with_stream_read_timeout(LOCAL_IDLE)
        } else {
            let attempt =
                Duration::from_secs(u64::from(self.cfg.read().attempt_secs.clamp(10, 3_600)));
            opts.with_stream_header_timeout(attempt)
                .with_stream_read_timeout(attempt)
        };
        if t == Transport::OpenAiLike {
            opts = opts
                .with_prompt_cache_key(format!("endless:{}", self.session))
                .with_cache_control(CacheControl::Ephemeral24h);
        }
        // 177 E2: a local OpenAI-protocol server is told by its chat template,
        // and never gets an effort keyword (genai's omlx adapter would turn
        // it into `enable_thinking: true`); genai's own Ollama adapter keeps
        // its `think: false`.
        let kwargs = !anthropic && self.quiet && !self.chosen;
        if kwargs {
            opts = crate::backend::thinking_off(opts);
        }
        // R2-7 / R2-8: thinking off by transport, not by model id.
        let effort = if ant_off {
            let slug = self
                .model
                .genai
                .split_once("::")
                .map_or(self.model.genai.as_str(), |(_, s)| s);
            Some(anthropic_off(slug))
        } else if kwargs && crate::loop_::ns(&self.model.genai) != "ollama" {
            None
        } else {
            self.effort.clone()
        };
        if let Some(e) = effort {
            opts = opts.with_reasoning_effort(e);
        }
        (req, opts)
    }
}

impl ModelCompactor {
    /// 177 R2-4: the forced retry's cap — twice `cap`, within the model's
    /// output limit and what the window the prompt was fitted for leaves
    /// beside `req` (`tokens::memory_window`, 5 % margin).
    fn forced_cap(&self, req: &ChatRequest, cap: u32) -> u32 {
        let mut forced = cap.saturating_mul(2);
        if let Some(out) = self
            .model
            .model
            .as_ref()
            .map(|m| m.limit.output)
            .filter(|&o| o > 0)
        {
            forced = forced.min(out);
        }
        let (bytes, flat) = crate::tokens::request_size(req);
        let prompt = crate::tokens::tokens_of(&self.model.id, bytes) + flat;
        let window = crate::tokens::memory_window(&self.model);
        let room = ((window as f64) * 0.95) as u64;
        let room = u32::try_from(room.saturating_sub(prompt)).unwrap_or(u32::MAX);
        forced.min(room)
    }
}

impl Compactor for ModelCompactor {
    fn compress<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(self.compress_inner(ctx, convo, 1, false, false))
    }

    fn compress_lines<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
        lines: u32,
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(self.compress_inner(ctx, convo, lines, false, false))
    }

    fn compress_thinking_off<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(self.compress_inner(ctx, convo, 1, true, false))
    }

    fn compress_small<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(self.compress_inner(ctx, convo, 1, false, true))
    }

    fn describe_image<'a>(
        &'a self,
        image: &'a genai::chat::Binary,
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(self.describe_inner(image))
    }

    fn model(&self) -> &Resolved {
        &self.model
    }

    fn local(&self) -> Option<String> {
        self.local.clone()
    }

    fn out_tokens(&self, lines: u32) -> u32 {
        self.cap(false, lines)
    }

    fn can_grow(&self, ctx: &str, convo: &[ChatMessage]) -> bool {
        let (_, opts) = self.request_opts(Transport::OpenAiLike, ctx, convo, 1, true);
        opts.max_tokens.unwrap_or(0) > self.cap(false, 1)
    }
}

impl ModelCompactor {
    /// One attempt; [`build`] bounds it whole (auth resolution included, E6).
    async fn compress_inner(
        &self,
        ctx: &str,
        convo: &[ChatMessage],
        lines: u32,
        forced: bool,
        small: bool,
    ) -> Result<Reply, String> {
        let transport = self.backend.transport(&self.model).await?;
        let (req, opts) = if small {
            self.request_small(transport, ctx, convo)
        } else {
            self.request_opts(transport, ctx, convo, lines, forced)
        };
        self.send(req, opts).await
    }

    /// 201 §8 (P4): one [`DESCRIBE`] request carrying `image`, capped like a
    /// small-profile reply.
    async fn describe_inner(&self, image: &genai::chat::Binary) -> Result<Reply, String> {
        let transport = self.backend.transport(&self.model).await?;
        use genai::chat::{ContentPart, MessageContent};
        let convo = [ChatMessage::user(MessageContent::from_parts(vec![
            ContentPart::Text("Describe this image in one line.".to_owned()),
            ContentPart::Binary(image.clone()),
        ]))];
        let ctx = "An image from the conversation follows.";
        let (req, opts) = self.request_with(DESCRIBE, transport, ctx, &convo, 1, false);
        let opts = opts.with_max_tokens(SMALL_REPLY_TOKENS.min(self.carry()));
        self.send(req, opts).await
    }

    async fn send(&self, req: ChatRequest, opts: ChatOptions) -> Result<Reply, String> {
        let cap = opts.max_tokens.unwrap_or(0);
        let thinking_off = opts.extra_body.is_some() || opts.reasoning_effort.is_some();
        let started = Instant::now();
        let mut stream = self
            .backend
            .stream(&self.model, req, &opts, self.session)
            .await?;
        let mut reasoning = false;
        while let Some(ev) = stream.next().await {
            match ev? {
                ChatStreamEvent::ReasoningChunk(_) => reasoning = true,
                ChatStreamEvent::End(end) => {
                    let latency = started.elapsed();
                    // G1 latency limb: a local endpoint's measured prompt-eval rate.
                    if self.local.is_some()
                        && let Some(p) = end.captured_usage.as_ref().and_then(|u| u.prompt_tokens)
                    {
                        crate::tokens::learn_eval(
                            &self.model.id,
                            u64::try_from(p).unwrap_or(0),
                            latency.as_secs_f64(),
                        );
                    }
                    let stop = end.captured_stop_reason.as_ref();
                    let mut reply = Reply {
                        text: end.captured_texts().map(|t| t.concat()).unwrap_or_default(),
                        usage: end.captured_usage.clone(),
                        latency,
                        stop: stop.map(|s| s.raw().to_owned()),
                        truncated: stop.is_some_and(|s| s.is_max_tokens()),
                        reasoning,
                        cap,
                        thinking_off,
                    };
                    reply.reasoning |= reply.reasoning_tokens().is_some_and(|n| n > 0);
                    return Ok(reply);
                }
                _ => {}
            }
        }
        Err("stream ended without End event".into())
    }
}

/// `created_at` for ledger rows.
pub fn now() -> String {
    Utc::now().to_rfc3339()
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;

    /// A compactor that answers from a script, recording what it was sent.
    struct Fake {
        replies: Mutex<Vec<String>>,
        sent: Mutex<Vec<Vec<ChatMessage>>>,
        model: Resolved,
    }

    impl Fake {
        fn new(replies: &[&str]) -> Self {
            Self {
                replies: Mutex::new(replies.iter().rev().map(|s| (*s).to_owned()).collect()),
                sent: Mutex::new(Vec::new()),
                model: super::super::pump::resolved_for_tests("fake"),
            }
        }
    }

    /// Script entries: a reply, `!err:<msg>` (the call fails) or `!hang` (never answers).
    impl Compactor for Fake {
        fn compress<'a>(
            &'a self,
            _ctx: &'a str,
            convo: &'a [ChatMessage],
        ) -> BoxFuture<'a, Result<Reply, String>> {
            self.sent.lock().push(convo.to_vec());
            let text = self
                .replies
                .lock()
                .pop()
                .ok_or_else(|| "script exhausted".to_owned());
            Box::pin(async move {
                let text = text?;
                if text == "!hang" {
                    futures::future::pending::<()>().await;
                }
                if let Some(e) = text.strip_prefix("!err:") {
                    return Err(e.to_owned());
                }
                Ok(Reply {
                    text,
                    usage: None,
                    latency: Duration::from_millis(1),
                    ..Default::default()
                })
            })
        }
        fn model(&self) -> &Resolved {
            &self.model
        }
    }

    fn unit_step() -> Step {
        Step::Unit {
            kind: Kind::User,
            text: "please rename the config loader to load_settings and keep the old alias".into(),
            earlier: vec![],
            aim: None,
        }
    }

    /// R1 Q2: a reply whose only trip is `size` is cut at a word boundary
    /// with an ellipsis and accepted — one call; any other trip still
    /// answers in the same conversation.
    #[tokio::test]
    async fn size_only_overshoot_is_cut_and_other_trips_retry() {
        let long = format!(
            "user: rename the config loader to load_settings {}",
            "and keep the old alias ".repeat(40)
        );
        let fake = Fake::new(&[&long]);
        let built = build(&fake, "<chat>\n</chat>", &unit_step(), 512, 5, SECS)
            .await
            .unwrap();
        assert_eq!((built.tries, built.calls.len()), (1, 1));
        assert_eq!(built.calls[0].guard, None);
        assert!(
            built.text.len() <= 512 && built.text.ends_with("alias…"),
            "{}",
            built.text
        );

        let ungrounded = format!("user: {}", "deploy the website tonight ".repeat(30));
        let fake = Fake::new(&[
            &ungrounded,
            "user: rename the config loader to load_settings, keep old alias",
        ]);
        let built = build(&fake, "", &unit_step(), 512, 5, SECS).await.unwrap();
        assert_eq!(built.tries, 2);
        assert_eq!(built.calls[0].guard, Some("grounding"));
        assert_eq!(
            fake.sent.lock()[1].len(),
            3,
            "step, assistant reply, correction"
        );
    }

    /// R1 Q3: a unit retried alone aims at its firmer target.
    #[test]
    fn solo_unit_prompt_carries_the_firm_target() {
        let solo = Step::Unit {
            kind: Kind::User,
            text: "please rename the config loader to load_settings and keep the old alias".into(),
            earlier: vec![],
            aim: Some(409),
        };
        assert!(
            solo.render(512)
                .contains("Compress this message into one line, in at most 409 bytes:")
        );
        assert!(unit_step().render(512).contains("in at most 512 bytes:"));
    }

    /// R1-9 / R2-5: the last line of a repeated number wins; an unnumbered
    /// line continues the numbered line before it, until a blank line (a
    /// closing remark after the list is no unit's text); out-of-range
    /// numbers are dropped.
    #[test]
    fn batch_lines_take_the_last_duplicate_and_join_wraps() {
        let got = batch_lines(
            "intro\n1| first\n2| second\n  wrapped tail\n1| first again\n3| third\n\nAll lines kept under the limit.\n9| none",
            3,
        );
        assert_eq!(
            got,
            [
                Some("first again".to_owned()),
                Some("second wrapped tail".to_owned()),
                Some("third".to_owned()),
            ]
        );
    }

    /// R1 Q1 / R2-7: every compactor call runs with thinking off, decided by
    /// the transport: the Anthropic one sends `Zero` (genai: `thinking:
    /// {type: "disabled"}`) with the 600-token cap; a `claude-*` id behind an
    /// OpenAI-compatible gateway gets the catalog's lowest listed effort, or
    /// no setting — never `reasoning_effort: "none"` it did not list.
    #[test]
    fn compactor_requests_run_with_thinking_off() {
        let r = |genai: &str, efforts: Option<&[&str]>| Resolved {
            id: "p/m".into(),
            genai: genai.into(),
            provider: None,
            model: efforts.map(|v| {
                serde_json::from_value(serde_json::json!({
                    "id": "m", "name": "m", "limit": {"context": 200000, "output": 8192},
                    "reasoning": true,
                    "reasoning_options": [{"type": "effort", "values": v}]
                }))
                .unwrap()
            }),
        };
        assert!(matches!(
            thinking_off(&r("openai::gpt-5", Some(&["minimal", "low", "medium"]))),
            Some(ReasoningEffort::Minimal)
        ));
        assert!(matches!(
            thinking_off(&r("openai::gpt-5.1", Some(&["none", "low"]))),
            Some(ReasoningEffort::Zero)
        ));
        assert!(matches!(
            thinking_off(&r("ollama::qwen3:8b", None)),
            Some(ReasoningEffort::Zero)
        ));
        assert!(thinking_off(&r("openai::gpt-4.1", None)).is_none());
        // R2-8: on the Anthropic API Fable and Mythos reject `disabled`, so
        // they get adaptive at `low`; Haiku/Sonnet/Opus 5 take `disabled`.
        assert!(matches!(
            anthropic_off("claude-mythos-5"),
            ReasoningEffort::Low
        ));
        assert!(matches!(
            anthropic_off("claude-opus-5"),
            ReasoningEffort::Zero
        ));
        // R3-1: Bedrock has no `disabled` (Zero sends nothing), so every model
        // that thinks unasked gets `Low`; 4.x models think only when asked.
        for (id, low) in [
            ("bedrock_sigv4::anthropic.claude-haiku-5-5", true),
            ("bedrock_sigv4::us.anthropic.claude-haiku-5-5-v1:0", true),
            ("bedrock_sigv4::anthropic.claude-sonnet-5-v1:0", true),
            ("bedrock_sigv4::global.anthropic.claude-opus-5-1-v1:0", true),
            ("bedrock_sigv4::anthropic.claude-fable-5-1-v1:0", true),
            ("bedrock_sigv4::anthropic.claude-mythos-5-v1:0", true),
            (
                "bedrock_sigv4::anthropic.claude-sonnet-4-5-20250929-v1:0",
                false,
            ),
            (
                "bedrock_sigv4::anthropic.claude-haiku-4-5-20251001-v1:0",
                false,
            ),
            (
                "bedrock_sigv4::anthropic.claude-3-5-sonnet-20240620-v1:0",
                false,
            ),
        ] {
            let want = if low {
                ReasoningEffort::Low
            } else {
                ReasoningEffort::Zero
            };
            let got = thinking_off(&r(id, None));
            assert_eq!(format!("{got:?}"), format!("{:?}", Some(want)), "{id}");
        }
        let dir = crate::test_dir("compactor-thinking");
        let compactor = |model: Resolved| ModelCompactor {
            session: runtime::SessionId::new(),
            backend: crate::backend::Backend::for_model(
                &model,
                &genai::Client::builder().build().unwrap(),
                &crate::supervisor::Auth::stub(dir.path()),
            ),
            effort: thinking_off(&model),
            chosen: false,
            model,
            system: prompt("theuth"),
            local: None,
            quiet: false,
            cfg: Default::default(),
        };
        let haiku = compactor(r(
            "anthropic::claude-haiku-5-5",
            Some(&["low", "medium", "high"]),
        ));
        let (_, opts) = haiku.request(Transport::Anthropic, "<chat>\n</chat>", &[]);
        assert!(matches!(opts.reasoning_effort, Some(ReasoningEffort::Zero)));
        assert_eq!(opts.max_tokens, Some(MAX_TOKENS));
        // R2-8: Fable rejects `disabled` — its request opts carry `Low`
        // (genai: adaptive thinking + `output_config.effort: "low"`; wire
        // body pinned in `endless_compactor_turns_thinking_off_per_model`).
        let fable = compactor(r(
            "anthropic::claude-fable-5-1",
            Some(&["low", "medium", "high"]),
        ));
        let (_, opts) = fable.request(Transport::Anthropic, "<chat>\n</chat>", &[]);
        assert!(matches!(opts.reasoning_effort, Some(ReasoningEffort::Low)));
        assert_eq!(opts.max_tokens, Some(MAX_TOKENS));
        // A claude id on an OpenAI-compatible gateway: the catalog's lowest effort …
        let listed = compactor(r(
            "github_copilot::claude-sonnet-5",
            Some(&["low", "medium", "high"]),
        ));
        let (_, opts) = listed.request(Transport::OpenAiLike, "<chat>\n</chat>", &[]);
        assert!(matches!(opts.reasoning_effort, Some(ReasoningEffort::Low)));
        // … or nothing when it lists none.
        let plain = compactor(r("openai::claude-haiku-5-5", None));
        let (_, opts) = plain.request(Transport::OpenAiLike, "<chat>\n</chat>", &[]);
        assert!(opts.reasoning_effort.is_none());
        assert_eq!(opts.max_tokens, Some(MAX_TOKENS));
    }

    /// 194 D4e: a compactor effort the user picked rides every request — the
    /// Anthropic transport too (no `anthropic_off`), with its thinking budget
    /// on top of the reply cap — and a local OpenAI-protocol server gets the
    /// effort instead of the thinking-off kwargs.
    #[test]
    fn a_chosen_compactor_effort_rides_every_transport() {
        let dir = crate::test_dir("compactor-chosen");
        let model = Resolved {
            id: "anthropic/claude-haiku-5-5".into(),
            genai: "anthropic::claude-haiku-5-5".into(),
            provider: None,
            model: None,
        };
        let c = |quiet: bool| ModelCompactor {
            session: runtime::SessionId::new(),
            backend: crate::backend::Backend::for_model(
                &model,
                &genai::Client::builder().build().unwrap(),
                &crate::supervisor::Auth::stub(dir.path()),
            ),
            effort: Some(ReasoningEffort::High),
            chosen: true,
            model: model.clone(),
            system: prompt("theuth"),
            local: None,
            quiet,
            cfg: Default::default(),
        };
        let (_, opts) = c(false).request(Transport::Anthropic, "<chat>\n</chat>", &[]);
        assert!(matches!(opts.reasoning_effort, Some(ReasoningEffort::High)));
        assert_eq!(opts.max_tokens, Some(MAX_TOKENS + 24_000));
        let (_, opts) = c(false).request(Transport::OpenAiLike, "<chat>\n</chat>", &[]);
        assert!(matches!(opts.reasoning_effort, Some(ReasoningEffort::High)));
        let (_, opts) = c(true).request(Transport::OpenAiLike, "<chat>\n</chat>", &[]);
        assert!(matches!(opts.reasoning_effort, Some(ReasoningEffort::High)));
        assert!(opts.extra_body.is_none(), "no thinking-off kwargs");
    }

    /// Tries exhausted with no passing candidate: the shortest wins, flagged negative.
    #[tokio::test]
    async fn exhausted_tries_take_shortest_and_flag() {
        let fake = Fake::new(&[
            "user: deploy the website to production tonight",
            "user: deploy the website",
        ]);
        let built = build(&fake, "", &unit_step(), 512, 2, SECS).await.unwrap();
        assert_eq!(built.tries, -2);
        assert_eq!(built.text, "user: deploy the website");
        assert!(built.calls.iter().all(|c| c.guard == Some("grounding")));
    }

    const SECS: Duration = Duration::from_secs(60);

    /// 137F E7: a failing attempt after a candidate exists keeps the
    /// candidate (hard cut, tries negative) instead of failing the node.
    #[tokio::test]
    async fn failed_attempt_after_a_candidate_keeps_it() {
        let ungrounded = format!("user: {}", "deploy the website tonight ".repeat(30));
        let fake = Fake::new(&[&ungrounded, "!err:timed out"]);
        let built = build(&fake, "", &unit_step(), 512, 5, SECS).await.unwrap();
        assert_eq!(built.tries, -2);
        assert_eq!(built.text.len(), 512, "hard cut to node_bytes");
        assert_eq!(built.calls.len(), 1, "only answered attempts are rows");
        let fake = Fake::new(&["!err:timed out"]);
        assert_eq!(
            build(&fake, "", &unit_step(), 512, 5, SECS)
                .await
                .unwrap_err()
                .error,
            "timed out",
            "no candidate: the node fails"
        );
        // R1-13: a provider error (429/5xx) is no slow model: the node fails and backs off;
        // R1-4: the failure keeps the answered attempt's row (it was billed).
        let fake = Fake::new(&[&ungrounded, "!err:429: rate limited"]);
        let f = build(&fake, "", &unit_step(), 512, 5, SECS)
            .await
            .unwrap_err();
        assert_eq!((f.error.as_str(), f.calls.len()), ("429: rate limited", 1));
    }

    /// R2-11: a build dropped by an outer bound (the job timeout) cannot
    /// return its answered attempts; [`Tally`] still has them for the ledger.
    #[tokio::test(start_paused = true)]
    async fn tally_keeps_answered_attempts_of_a_dropped_build() {
        let ungrounded = format!("user: {}", "deploy the website tonight ".repeat(30));
        let fake = Fake::new(&[&ungrounded, "!hang"]);
        let tally = Tally::new(&fake);
        let dropped = tokio::time::timeout(
            Duration::from_secs(5),
            build(&tally, "", &unit_step(), 512, 5, Duration::from_secs(60)),
        )
        .await;
        assert!(dropped.is_err(), "the outer bound fired first");
        let rows = tally.rows();
        assert_eq!(rows.len(), 1, "the answered attempt");
        assert_eq!(rows[0].bytes, Some(ungrounded.trim().len() as u32));
        assert_eq!(rows[0].guard, Some("unknown"), "never judged, not accepted");
    }

    /// 137F E6: the attempt bound fires at `attempt`, not later.
    #[tokio::test(start_paused = true)]
    async fn hung_attempt_fails_at_the_attempt_bound() {
        let fake = Fake::new(&["!hang"]);
        let t0 = tokio::time::Instant::now();
        let err = build(&fake, "", &unit_step(), 512, 5, Duration::from_secs(1))
            .await
            .unwrap_err();
        assert_eq!(err.error, "timed out after 1s");
        assert_eq!(t0.elapsed(), Duration::from_secs(1));
    }

    /// 137F E6: the one locality rule — runtime names, private hosts, unpriced models.
    #[test]
    fn is_local_by_provider_host_and_price() {
        let priced: models::Model = serde_json::from_value(serde_json::json!({
            "id": "m", "name": "m", "limit": {"context": 200000, "output": 8192},
            "cost": {"input": 1.0, "output": 2.0}
        }))
        .unwrap();
        let m = |genai: &str, provider: Option<&str>, model: Option<&models::Model>| Resolved {
            id: "x/m".into(),
            genai: genai.into(),
            provider: provider.map(str::to_owned),
            model: model.cloned(),
        };
        let cloud = m("openai::gpt", Some("openai"), Some(&priced));
        assert!(!is_local(&cloud, None));
        assert!(!is_local(&cloud, Some("https://api.example.com/v1/")));
        assert!(is_local(&m("ollama::qwen3", None, Some(&priced)), None));
        assert!(is_local(&m("x", Some("LMStudio"), Some(&priced)), None));
        for url in [
            "http://127.0.0.1:1234/v1/",
            "http://localhost:8080/",
            "http://10.0.0.5/v1/",
            "http://172.20.1.1/",
            "http://192.168.1.9:11434/",
            "http://[::1]:8000/",
            "http://[fd12::1]/",
            "http://gpu-box.local:8000/v1/",
            "http://[::ffff:127.0.0.1]:8000/",
            "http://169.254.10.2/",
            "http://[fe80::1]/",
            "http://100.101.102.103:8000/",
            "http://gpu-box:8000/v1/",
            "http://gpu.corp.internal/v1/",
            "http://nas.lan:8080/",
            "http://box.home.arpa/",
            "http://gpu.tail1234.ts.net:11434/",
        ] {
            assert!(is_local(&cloud, Some(url)), "{url}");
        }
        for url in [
            "http://172.32.0.1/",
            "http://100.128.0.1/",
            "https://my-rg.openai.azure.com/",
        ] {
            assert!(!is_local(&cloud, Some(url)), "{url}");
        }
        let unpriced: models::Model = serde_json::from_value(serde_json::json!({
            "id": "m", "name": "m", "limit": {"context": 8192, "output": 1024}
        }))
        .unwrap();
        assert!(
            is_local(&m("x::y", Some("gw"), Some(&unpriced)), None),
            "a catalog entry without a price"
        );
        assert!(
            !is_local(
                &m("x::y", Some("azure"), None),
                Some("https://my-rg.openai.azure.com/")
            ),
            "an id the catalog does not know is cloud"
        );
        assert_eq!(
            local_endpoint(&cloud, Some("http://127.0.0.1:1234/v1/")).as_deref(),
            Some("http://127.0.0.1:1234/v1/")
        );
        assert_eq!(
            local_endpoint(&m("ollama::qwen3", None, Some(&priced)), None).as_deref(),
            Some("ollama")
        );
        assert_eq!(local_endpoint(&cloud, None), None);
    }

    #[test]
    fn step_renders_ruler_of_exact_bytes_and_marks() {
        let s = unit_step().render(512);
        assert!(s.contains("none\n"));
        assert!(s.contains(&guards::ruler(512)));
        assert!(s.ends_with("Compress this message into one line, in at most 512 bytes:\nuser: please rename the config loader to load_settings and keep the old alias"));
        assert!(prompt("theuth").starts_with("You write the memory of theuth, an AI agent"));
        assert!(!prompt("theuth").contains("{agent}"));
    }

    #[test]
    fn diff_carrier_request_marks_context_block_on_anthropic_only() {
        let c = ModelCompactor {
            session: SessionId::new(),
            model: Resolved {
                id: "x".into(),
                genai: "x".into(),
                provider: None,
                model: None,
            },
            backend: Backend::for_model(
                &Resolved {
                    id: "anthropic/claude".into(),
                    genai: "claude".into(),
                    provider: None,
                    model: None,
                },
                &genai::Client::builder().build().unwrap(),
                &crate::supervisor::Auth::stub(&std::env::temp_dir()),
            ),
            system: prompt("theuth"),
            effort: None,
            chosen: false,
            local: None,
            quiet: false,
            cfg: std::sync::Arc::new(parking_lot::RwLock::new(super::super::EndlessConfig {
                attempt_secs: 300,
                ..Default::default()
            })),
        };
        let convo = vec![ChatMessage::user("step")];
        let (req, _) = c.request(Transport::Anthropic, "<chat>\n</chat>", &convo);
        assert_eq!(req.messages.len(), 3);
        assert!(req.messages[0].options.is_some(), "system marked");
        assert!(req.messages[1].options.is_some(), "context marked");
        assert!(req.messages[2].options.is_none(), "step unmarked");
        let (req, opts) = c.request(Transport::OpenAiLike, "<chat>\n</chat>", &convo);
        assert_eq!(req.messages.len(), 2);
        assert!(req.system.is_some());
        assert_eq!(opts.max_tokens, Some(600));
        // R1-12 / R2-4: cloud first byte and idle = the attempt bound.
        assert_eq!(opts.stream_header_timeout, Some(Duration::from_secs(300)));
        assert_eq!(opts.stream_read_timeout, Some(Duration::from_secs(300)));
        let carried = crate::diff_output::carried("receipt");
        let convo = [ChatMessage::user(carried.clone())];
        for transport in [Transport::Anthropic, Transport::OpenAiLike] {
            let (request, _) = c.request(transport, &carried, &convo);
            for message in request
                .messages
                .iter()
                .filter(|m| m.role == genai::chat::ChatRole::User)
            {
                assert_eq!(message.content.first_text(), Some("receipt"));
            }
        }
    }

    fn talk(text: String) -> Step {
        Step::Unit {
            kind: Kind::Talk,
            text,
            earlier: vec![],
            aim: None,
        }
    }

    /// 201 §7.2 (P3): a small request (system prompt + ≤ 2 KB context +
    /// one user block) stays under a fixed bound for any unit size, carries
    /// no `<chat>` view, no ruler, and sends one unit per call.
    #[tokio::test]
    async fn small_request_is_bounded_and_has_no_chat_view() {
        assert!(SMALL.len() <= 700, "small prompt is {} bytes", SMALL.len());
        let ctx = format!("Context:\n{}", "user: fix it. ".repeat(140));
        let ctx = guards::cut_utf8(&ctx, SMALL_CTX_BYTES);
        let big = "the agent edited pump.rs and ran the tests again; ".repeat(600);
        let chunk = small_chunk_bytes(400_000);
        let n = chunks(&big, chunk).len();
        let replies: Vec<&str> = vec!["The agent edited pump.rs and ran the tests."; n + 1];
        let fake = Fake::new(&replies);
        let b = build_small(&fake, ctx, &talk(big), 240, Duration::from_secs(5), chunk)
            .await
            .unwrap();
        assert!(b.tries > 0, "tries {}", b.tries);
        let sent = fake.sent.lock();
        assert_eq!(sent.len(), n + 1, "one call per chunk plus the join");
        for convo in sent.iter() {
            assert_eq!(convo.len(), 1, "one user block per call");
            let body = serde_json::to_string(&convo[0]).unwrap();
            assert!(!body.contains("<chat>"), "no chat view");
            assert!(!body.contains("|0123456789abcdef"), "no ruler");
            let total = SMALL.len() + ctx.len() + body.len();
            assert!(total < 8_000, "small request {total} bytes");
        }
    }

    /// 201 §7.2: the small chunk follows the window, never above 4 KB.
    #[test]
    fn small_chunk_follows_the_window() {
        let fixed = SMALL.len() + SMALL_CTX_BYTES + SMALL_REPLY_TOKENS as usize * 4;
        assert_eq!(small_chunk_bytes(fixed + 4_000), 2_000);
        assert_eq!(small_chunk_bytes(fixed + 6_000), 3_000);
        assert_eq!(small_chunk_bytes(1_000_000), SMALL_CHUNK_MAX);
        assert_eq!(
            small_chunk_bytes(1_000),
            512,
            "a tiny window still progresses"
        );
    }

    /// 201 §7.2: a guard trip in small is one call and no correction: the
    /// build comes back with negative tries (the pump keeps the draft).
    #[tokio::test]
    async fn small_guard_trip_sends_no_second_message() {
        let fake = Fake::new(&["", "never sent"]);
        let b = build_small(
            &fake,
            "Context: none.",
            &unit_step(),
            240,
            Duration::from_secs(5),
            4_096,
        )
        .await
        .unwrap();
        assert!(b.tries < 0, "tries {}", b.tries);
        let sent = fake.sent.lock();
        assert_eq!(sent.len(), 1, "no retry inside the build");
        assert_eq!(sent[0].len(), 1, "no correction turn");
    }

    fn batch_step(drafting: bool) -> Step {
        Step::Batch {
            units: vec![
                (
                    Kind::Talk,
                    "the agent ran cargo test in crates/agent".into(),
                ),
                (
                    Kind::Talk,
                    "the agent edited pump.rs and the retry delay".into(),
                ),
            ],
            earlier: vec![],
            drafting,
        }
    }

    /// A local compactor whose plain replies are spent on thinking and
    /// whose thinking-off retry answers `forced_reply`.
    struct Thinker {
        model: Resolved,
        plain: std::sync::atomic::AtomicU32,
        forced: std::sync::atomic::AtomicU32,
        forced_reply: &'static str,
    }

    impl Compactor for Thinker {
        fn compress<'a>(
            &'a self,
            _ctx: &'a str,
            _convo: &'a [ChatMessage],
        ) -> BoxFuture<'a, Result<Reply, String>> {
            self.plain.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Box::pin(async {
                Ok(Reply {
                    reasoning: true,
                    ..Default::default()
                })
            })
        }
        fn compress_thinking_off<'a>(
            &'a self,
            _ctx: &'a str,
            _convo: &'a [ChatMessage],
        ) -> BoxFuture<'a, Result<Reply, String>> {
            self.forced
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let text = self.forced_reply.to_owned();
            Box::pin(async move {
                Ok(Reply {
                    text,
                    ..Default::default()
                })
            })
        }
        fn model(&self) -> &Resolved {
            &self.model
        }
        fn local(&self) -> Option<String> {
            Some("ep".into())
        }
    }

    /// 201 P1 (F8) / R2-12: a local drafting batch whose reply is spent on
    /// thinking is retried once with thinking off; the retry's lines land
    /// and both replies are call rows. A meeting batch takes no retry.
    #[tokio::test]
    async fn a_local_batch_spent_on_thinking_retries_once_with_thinking_off() {
        use std::sync::atomic::Ordering;
        let thinker = || Thinker {
            model: super::super::pump::resolved_for_tests("local/thinker"),
            plain: Default::default(),
            forced: Default::default(),
            forced_reply: "1| the agent ran cargo test in crates/agent\n2| the agent edited pump.rs and the retry delay",
        };
        let c = thinker();
        let b = build(&c, "ctx", &batch_step(true), 240, 2, SECS)
            .await
            .unwrap();
        assert_eq!(
            (
                c.plain.load(Ordering::SeqCst),
                c.forced.load(Ordering::SeqCst)
            ),
            (1, 1)
        );
        assert!(b.lines.iter().all(Result::is_ok), "{:?}", b.lines);
        // Two calls × two lines; each call counted (bytes) once.
        assert_eq!(b.calls.len(), 4, "the thinking reply is charged too");
        assert_eq!(b.calls.iter().filter(|c| c.bytes.is_some()).count(), 2);
        assert_eq!(
            b.calls.iter().map(|c| c.guard).collect::<Vec<_>>(),
            [Some("empty"), Some("empty"), None, None]
        );
        let c = thinker();
        let b = build(&c, "ctx", &batch_step(false), 240, 2, SECS)
            .await
            .unwrap();
        assert_eq!(
            c.forced.load(Ordering::SeqCst),
            0,
            "a meeting batch: no retry"
        );
        assert!(b.lines.iter().all(Result::is_err));
    }

    /// R2-9: on the small profile the over-cost check sizes one chunk's
    /// call under the small prompt, not the whole unit under the standard one.
    #[test]
    fn small_profile_sizes_the_largest_call_by_its_chunk() {
        let big = "the agent edited pump.rs and ran the tests again; ".repeat(260);
        let standard = largest_call_bytes(&talk(big.clone()), 240, None);
        let small = largest_call_bytes(&talk(big.clone()), 240, Some(735));
        assert!(standard > big.len(), "{standard}");
        assert!(small <= SMALL.len() + 735 + 300, "{small}");
    }

    /// R2-11: a standard map-reduce stops at the first chunk whose line
    /// fails its guards: no later chunk and no reduce call is made.
    #[tokio::test]
    async fn map_reduce_stops_at_the_first_tripped_chunk() {
        let big = "the agent edited pump.rs and ran the tests again; ".repeat(900);
        assert!(chunks(&big, CHUNK_BYTES).len() >= 3);
        let fake = Fake::new(&[
            "zebra crossing painted blue",
            "the agent edited pump.rs",
            "the agent edited pump.rs",
            "the agent edited pump.rs",
        ]);
        let b = build(&fake, "", &talk(big), 512, 1, SECS).await.unwrap();
        assert!(b.tries < 0, "tries {}", b.tries);
        assert_eq!(fake.sent.lock().len(), 1, "only the tripped chunk was sent");
    }

    /// R2-4: an empty batch reply in a drafting session fails the batch
    /// with its call row (charged). R2-5: a meeting's empty batch reply is
    /// the pre-201 outcome — one call, every line tripped (its units run
    /// solo), no failure.
    #[tokio::test]
    async fn empty_batch_reply_is_charged_and_a_meeting_runs_solo() {
        let fake = Fake::new(&["", "never sent"]);
        let f = build(
            &fake,
            "ctx",
            &batch_step(true),
            240,
            2,
            Duration::from_secs(5),
        )
        .await
        .unwrap_err();
        assert!(f.error.contains("returned nothing"), "{}", f.error);
        assert_eq!(
            f.calls
                .iter()
                .map(|c| (c.line, c.guard))
                .collect::<Vec<_>>(),
            [(Some(0), Some("empty")), (Some(1), Some("empty"))],
            "the empty reply is a call row per line"
        );
        let fake = Fake::new(&["", "never sent"]);
        let b = build(
            &fake,
            "ctx",
            &batch_step(false),
            240,
            2,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
        assert_eq!(b.lines.len(), 2);
        assert!(b.lines.iter().all(Result::is_err), "every line trips");
        assert_eq!(b.calls.len(), 2, "one row per line");
        assert!(b.calls.iter().all(|c| c.guard == Some("format")));
        assert_eq!(fake.sent.lock().len(), 1, "no retry");
    }

    /// A compactor answering one batch reply with a usage and a price.
    struct Priced {
        model: Resolved,
        reply: &'static str,
    }

    impl Compactor for Priced {
        fn compress<'a>(
            &'a self,
            _ctx: &'a str,
            _convo: &'a [ChatMessage],
        ) -> BoxFuture<'a, Result<Reply, String>> {
            let text = self.reply.to_owned();
            Box::pin(async move {
                Ok(Reply {
                    text,
                    usage: serde_json::from_str(r#"{"prompt_tokens":1000,"completion_tokens":90}"#)
                        .ok(),
                    latency: Duration::from_millis(40),
                    ..Default::default()
                })
            })
        }
        fn model(&self) -> &Resolved {
            &self.model
        }
        fn rates(&self) -> Option<models::Cost> {
            Some(models::Cost {
                input: 3.0,
                output: 15.0,
                cache_read: None,
                cache_write: None,
            })
        }
    }

    /// G6: a batch reply records one row per unit with that unit's own
    /// guard, landing on the unit's `i`; the call's usage and cost are
    /// charged once (the stats equal the single call's), and `try` counts
    /// on per node across later puts (a solo retry).
    #[tokio::test]
    async fn endless_batch_rows_carry_each_lines_guard_and_charge_once() {
        let step = Step::Batch {
            units: vec![
                (
                    Kind::Talk,
                    "the agent ran cargo test in crates/agent".into(),
                ),
                (
                    Kind::Talk,
                    "the agent edited pump.rs and the retry delay".into(),
                ),
                (
                    Kind::Talk,
                    "the agent opened compactor.rs and read build_batch".into(),
                ),
            ],
            earlier: vec![],
            drafting: true,
        };
        let c = Priced {
            model: super::super::pump::resolved_for_tests("priced"),
            reply: "1| the agent ran cargo test in crates/agent\n\
                    2| deploy the marketing website tonight with a new banner\n\
                    3| the agent opened compactor.rs and read build_batch",
        };
        let b = build(&c, "ctx", &step, 240, 2, SECS).await.unwrap();
        assert_eq!(
            b.lines
                .iter()
                .map(|l| l.as_ref().err().copied())
                .collect::<Vec<_>>(),
            [None, Some("grounding"), None]
        );
        let usage: Usage =
            serde_json::from_str(r#"{"prompt_tokens":1000,"completion_tokens":90}"#).unwrap();
        let single = estimate_usd(&usage, &c.rates().unwrap());
        assert_eq!(b.usage(), Some(usage));

        let dir = crate::test_dir("endless-batch-rows");
        let store = crate::store::Store::open(&dir.path().join("sessions.sqlite"), None)
            .await
            .unwrap();
        let id = runtime::SessionId::new();
        store
            .create_session(id, "m", None, None, None)
            .await
            .unwrap();
        store
            .endless_calls_put(id, 0, 7, &[7, 8, 9], &b.calls)
            .await
            .unwrap();
        // Unit 8 retried alone, one attempt.
        let solo = CallRow {
            line: None,
            bytes: Some(40),
            guard: Some("grounding"),
            usage: None,
            usd_estimate: None,
            latency_ms: 5,
        };
        store
            .endless_calls_put(id, 0, 8, &[], std::slice::from_ref(&solo))
            .await
            .unwrap();
        let rows: Vec<(i64, i64, Option<String>, Option<f64>)> = sqlx::query_as(
            "SELECT i, try, guard, usd_estimate FROM endless_calls WHERE session_id = ? \
             ORDER BY i, try",
        )
        .bind(id.to_string())
        .fetch_all(store.pool())
        .await
        .unwrap();
        let shape: Vec<(i64, i64, Option<&str>)> = rows
            .iter()
            .map(|(i, t, g, _)| (*i, *t, g.as_deref()))
            .collect();
        assert_eq!(
            shape,
            [
                (7, 0, None),
                (8, 0, Some("grounding")),
                (8, 1, Some("grounding")),
                (9, 0, None)
            ]
        );
        assert_eq!(rows.iter().filter(|r| r.3.is_some()).count(), 1);
        let stats = store.endless_stats(id).await.unwrap();
        assert_eq!(stats.calls, 2, "the batch call once, the solo call once");
        assert_eq!(stats.guard_trips.grounding, 2);
        assert!(
            (stats.usd_estimate - single).abs() < 1e-12,
            "{}",
            stats.usd_estimate
        );
        assert_eq!((stats.input_tokens, stats.output_tokens), (1000, 90));
    }

    /// R2-2: each reduce stage at least halves the lines and every group
    /// fits the chunk; a unit over the chunk cap sends nothing (its draft,
    /// head and tail, stays).
    #[tokio::test]
    async fn small_reduce_halves_by_stage_and_caps_the_chunks() {
        let lines: Vec<String> = (0..10)
            .map(|k| format!("{k} {}", "x".repeat(200)))
            .collect();
        let g = small_groups(lines, 512);
        assert_eq!(g.len(), 5, "{g:?}");
        assert!(
            g.iter()
                .all(|(s, _)| s.as_ref().is_some_and(|s| s.len() <= 512))
        );
        let g = small_groups(vec!["a".into(), "b".into(), "c".into()], 3);
        assert_eq!(g.len(), 2);
        assert!(g[1].0.is_none() && g[1].1 == "c", "a lone last line passes");

        let big = "the agent edited pump.rs and ran the tests again; ".repeat(400);
        assert!(chunks(&big, 512).len() > SMALL_MAX_CHUNKS);
        let fake = Fake::new(&["never sent"]);
        let b = build_small(&fake, "ctx", &talk(big), 240, Duration::from_secs(5), 512)
            .await
            .unwrap();
        assert!(b.tries < 0 && b.calls.is_empty());
        assert!(fake.sent.lock().is_empty(), "no call over the cap");
    }
}
