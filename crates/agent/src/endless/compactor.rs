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
    /// The reply cap in tokens (E8 prompt fitting).
    fn out_tokens(&self) -> u32 {
        MAX_TOKENS
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
}

/// R2-11: `inner`, recording every answered attempt as it lands — so a job
/// dropped by its outer bound (or a panic) still settles the usage of the
/// attempts that were answered, which [`build`] can no longer return.
pub struct Tally<'a> {
    inner: &'a dyn Compactor,
    rows: parking_lot::Mutex<Vec<CallRow>>,
}

impl<'a> Tally<'a> {
    /// `inner` with no attempt recorded yet.
    pub fn new(inner: &'a dyn Compactor) -> Self {
        Self {
            inner,
            rows: parking_lot::Mutex::new(Vec::new()),
        }
    }

    /// The answered attempts so far (guard unknown: the build never judged them).
    pub fn rows(&self) -> Vec<CallRow> {
        self.rows.lock().clone()
    }

    fn record(&self, reply: &Result<Reply, String>) {
        if let Ok(r) = reply {
            let mut rows = self.rows.lock();
            let try_no = rows.len() as u32;
            rows.push(CallRow {
                try_no,
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
    fn out_tokens(&self) -> u32 {
        self.inner.out_tokens()
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
}

/// 137G G6: the reply cap of a batch of `lines` (the single cap per line,
/// at most 16k tokens).
pub fn batch_reply_tokens(single: u32, lines: u32) -> u32 {
    (single + MAX_TOKENS * lines.saturating_sub(1)).min(16_000)
}

/// One attempt's ledger row (`endless_calls`).
#[derive(Clone, Debug, PartialEq)]
pub struct CallRow {
    /// 0-based attempt number.
    pub try_no: u32,
    /// Reply bytes before any cut.
    pub bytes: Option<u32>,
    /// The guard that refused it; `None` = accepted.
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
        Step::Batch { units, .. } => build_batch(c, ctx, step, units, node_bytes, attempt).await,
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

/// R1-3 / G11: the prompt bytes of the largest single call `step` makes,
/// without its context (a map-reduce unit: one chunk's call).
pub fn largest_call_bytes(step: &Step, node_bytes: u32) -> usize {
    let one = match step {
        Step::Unit {
            kind,
            text,
            earlier,
            aim,
        } if text.len() > MAP_REDUCE_OVER => Step::Unit {
            kind: *kind,
            text: chunks(text, CHUNK_BYTES)
                .first()
                .copied()
                .unwrap_or_default()
                .to_owned(),
            earlier: earlier.clone(),
            aim: *aim,
        }
        .render(node_bytes)
        .len(),
        _ => step.render(node_bytes).len(),
    };
    COMPACT.len() + one
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
async fn build_batch(
    c: &dyn Compactor,
    ctx: &str,
    step: &Step,
    units: &[(Kind, String)],
    node_bytes: u32,
    attempt: Duration,
) -> Result<Built, Failed> {
    let convo = vec![ChatMessage::user(step.render(node_bytes))];
    let reply = match tokio::time::timeout(
        attempt,
        c.compress_lines(ctx, &convo, units.len() as u32),
    )
    .await
    {
        Ok(Ok(reply)) => reply,
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => return Err(format!("timed out after {}s", attempt.as_secs()).into()),
    };
    let raw = reply.text.trim();
    let ruler = guards::ruler(node_bytes);
    let lines = units
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
    let usd = reply
        .usage
        .as_ref()
        .and_then(|u| c.rates().map(|r| estimate_usd(u, &r)));
    Ok(Built {
        text: String::new(),
        tries: 1,
        calls: vec![CallRow {
            try_no: 0,
            bytes: Some(raw.len() as u32),
            guard: None,
            usage: reply.usage,
            usd_estimate: usd,
            latency_ms: reply.latency.as_millis() as u64,
        }],
        lines,
    })
}

/// Run the retry loop for one node: up to `tries` attempts, each guard
/// failure answered in the same conversation (a size-only trip is cut and
/// accepted, Q2); keep the shortest passing candidate, else the shortest of
/// all. Each attempt is bounded by `attempt` (137F E6); an attempt that
/// fails or times out after a candidate exists ends the loop on that
/// candidate (E7) instead of failing the node. A failure carries the rows
/// of the attempts answered before it (R1-4).
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
    for try_no in 0..tries.max(1) as u32 {
        used += 1;
        // E7 (R1-13): a slow attempt after a candidate exists keeps the
        // candidate; a provider/transport error fails the node as before.
        let reply = match tokio::time::timeout(attempt, c.compress(ctx, &convo)).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(error)) if shortest.is_none() || !is_timeout(&error) => {
                return Err(Failed { error, calls });
            }
            Err(_) if shortest.is_none() => {
                return Err(Failed {
                    error: format!("timed out after {}s", attempt.as_secs()),
                    calls,
                });
            }
            _ => break,
        };
        let raw = reply.text.trim().to_owned();
        let check = accept(&raw, &input, &ruler, node_bytes);
        let usd = reply
            .usage
            .as_ref()
            .and_then(|u| c.rates().map(|r| estimate_usd(u, &r)));
        calls.push(CallRow {
            try_no,
            bytes: Some(raw.len() as u32),
            guard: check.as_ref().err().map(|t| t.as_str()),
            usage: reply.usage.clone(),
            usd_estimate: usd,
            latency_ms: reply.latency.as_millis() as u64,
        });
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
    /// The reasoning setting off the Anthropic transport ([`thinking_off`]).
    pub effort: Option<ReasoningEffort>,
    /// 137F E6: `Some(queue key)` for a local compactor ([`local_endpoint`]).
    pub local: Option<String>,
    /// `agent.endless` in force (the cloud stream bounds follow `attempt_secs`, R1-12).
    pub(crate) cfg: std::sync::Arc<parking_lot::RwLock<super::EndlessConfig>>,
}

impl ModelCompactor {
    /// `max_tokens`: 600 plus the thinking budget the effort implies on
    /// Anthropic (genai's `ant_reasoning.rs` budgets), else 600.
    fn reply_tokens(&self) -> u32 {
        let budget = match &self.effort {
            None | Some(ReasoningEffort::Zero | ReasoningEffort::Minimal) => 0,
            Some(ReasoningEffort::Low) => 1_024,
            Some(ReasoningEffort::Medium) => 8_000,
            Some(ReasoningEffort::High | ReasoningEffort::XHigh | ReasoningEffort::Max) => 24_000,
            Some(ReasoningEffort::Budget(n)) => *n,
        };
        MAX_TOKENS + budget
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
        let anthropic = t == Transport::Anthropic;
        let mut messages = Vec::with_capacity(convo.len() + 2);
        if t == Transport::Anthropic {
            messages.push(
                ChatMessage::system(self.system.clone()).with_options(CacheControl::Ephemeral1h),
            );
            messages.push(ChatMessage::user(ctx.to_owned()).with_options(CacheControl::Ephemeral));
        } else {
            messages.push(ChatMessage::user(ctx.to_owned()));
        }
        messages.extend_from_slice(convo);
        let mut req = ChatRequest::new(messages);
        if t != Transport::Anthropic {
            req = req.with_system(self.system.clone());
        }
        let mut opts = ChatOptions::default()
            .with_capture_usage(true)
            .with_capture_content(true)
            // R3-2: no `with_capture_reasoning_content` — only text parts are read.
            .with_max_tokens(batch_reply_tokens(
                if anthropic {
                    MAX_TOKENS
                } else {
                    self.reply_tokens()
                },
                lines,
            ));
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
        // R2-7 / R2-8: thinking off by transport, not by model id.
        let effort = if anthropic {
            let slug = self
                .model
                .genai
                .split_once("::")
                .map_or(self.model.genai.as_str(), |(_, s)| s);
            Some(anthropic_off(slug))
        } else {
            self.effort.clone()
        };
        if let Some(e) = effort {
            opts = opts.with_reasoning_effort(e);
        }
        (req, opts)
    }
}

impl Compactor for ModelCompactor {
    fn compress<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(self.compress_inner(ctx, convo, 1))
    }

    fn compress_lines<'a>(
        &'a self,
        ctx: &'a str,
        convo: &'a [ChatMessage],
        lines: u32,
    ) -> BoxFuture<'a, Result<Reply, String>> {
        Box::pin(self.compress_inner(ctx, convo, lines))
    }

    fn model(&self) -> &Resolved {
        &self.model
    }

    fn local(&self) -> Option<String> {
        self.local.clone()
    }

    fn out_tokens(&self) -> u32 {
        self.reply_tokens()
    }
}

impl ModelCompactor {
    /// One attempt; [`build`] bounds it whole (auth resolution included, E6).
    async fn compress_inner(
        &self,
        ctx: &str,
        convo: &[ChatMessage],
        lines: u32,
    ) -> Result<Reply, String> {
        let transport = self.backend.transport(&self.model).await?;
        let (req, opts) = self.request_lines(transport, ctx, convo, lines);
        let started = Instant::now();
        let mut stream = self
            .backend
            .stream(&self.model, req, &opts, self.session)
            .await?;
        while let Some(ev) = stream.next().await {
            if let ChatStreamEvent::End(end) = ev? {
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
                return Ok(Reply {
                    text: end.captured_texts().map(|t| t.concat()).unwrap_or_default(),
                    usage: end.captured_usage,
                    latency,
                });
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
        let dir = crate::TestDir::new("compactor-thinking");
        let compactor = |model: Resolved| ModelCompactor {
            session: runtime::SessionId::new(),
            backend: crate::backend::Backend::for_model(
                &model,
                &genai::Client::builder().build().unwrap(),
                &crate::supervisor::Auth::stub(&dir.0),
            ),
            effort: thinking_off(&model),
            model,
            system: prompt("theuth"),
            local: None,
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
    fn request_marks_context_block_on_anthropic_only() {
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
            local: None,
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
    }
}
