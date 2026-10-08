//! 137 §5.3–§5.4, §6: one compactor call per node — the COMPACT prompt as
//! the system block, the frozen `<chat>` context as a cached user block, the
//! step (earlier lines, ruler, instruction + input) as an uncached one; size
//! and guard failures retry in the same conversation; the best candidate wins.

use std::time::{Duration, Instant};

use chrono::Utc;
use futures::StreamExt;
use futures::future::BoxFuture;
use genai::chat::{
    CacheControl, ChatMessage, ChatOptions, ChatRequest, ChatStreamEvent, ReasoningEffort,
    StreamEnd, Usage,
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
/// One attempt's wall-clock ceiling.
pub const ATTEMPT_TIMEOUT: Duration = Duration::from_secs(120);

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
}

impl Step {
    /// What G2/G3 compare against: the unit as `kind: text`, or the two child lines.
    pub fn input(&self) -> String {
        match self {
            Self::Unit { kind, text, .. } => format!("{}: {text}", kind.as_str()),
            Self::Merge { a, b, .. } => format!("{a}\n{b}"),
        }
    }

    /// Block 2 of the first user turn (§5.3).
    pub fn render(&self, node_bytes: u32) -> String {
        let earlier = match self {
            Self::Unit { earlier, .. } | Self::Merge { earlier, .. } => earlier,
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
            Self::Unit { kind, text, .. } => s.push_str(&format!(
                "Compress this message into one line, in at most {node_bytes} bytes:\n{}: {text}",
                kind.as_str()
            )),
            Self::Merge { a, b, .. } => s.push_str(&format!(
                "Merge these two lines into one, in at most {node_bytes} bytes:\n{a}\n{b}"
            )),
        }
        s
    }
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
    /// The unit's text (newlines kept).
    pub text: String,
    /// Attempts used; negative when no candidate passed (§6).
    pub tries: i32,
    /// Every attempt, in order.
    pub calls: Vec<CallRow>,
}

impl Built {
    /// The attempts' usage summed into one (for the node's `model_calls` row);
    /// `None` when no attempt reported usage.
    pub fn usage(&self) -> Option<Usage> {
        let mut out: Option<Usage> = None;
        for u in self.calls.iter().filter_map(|c| c.usage.as_ref()) {
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
}

/// Run the retry loop for one node: up to `tries` attempts, each guard or
/// size failure answered in the same conversation; keep the shortest passing
/// candidate, else the shortest of all.
pub async fn build(
    c: &dyn Compactor,
    ctx: &str,
    step: &Step,
    node_bytes: u32,
    tries: u8,
) -> Result<Built, String> {
    let input = step.input();
    let ruler = guards::ruler(node_bytes);
    let mut convo = vec![ChatMessage::user(step.render(node_bytes))];
    let mut calls = Vec::new();
    let mut passing: Option<String> = None;
    let mut shortest: Option<String> = None;
    let mut used = 0i32;
    for try_no in 0..tries.max(1) as u32 {
        used += 1;
        let reply = c.compress(ctx, &convo).await?;
        let raw = reply.text.trim().to_owned();
        let check = guards::check(&raw, &input, &ruler, guards::PROMPT_PHRASES, node_bytes);
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
        }),
        (None, Some(text)) => Ok(Built {
            text,
            tries: -used,
            calls,
        }),
        (None, None) => Err("compactor returned nothing".into()),
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
    /// The session's effort when the compactor is the session model (D8).
    pub effort: Option<ReasoningEffort>,
}

impl ModelCompactor {
    /// `max_tokens`: 600 plus the thinking budget the effort implies on
    /// Anthropic (genai's `ant_reasoning.rs` budgets), else 600.
    fn max_tokens(&self) -> u32 {
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
            .with_capture_reasoning_content(true)
            .with_max_tokens(self.max_tokens());
        if t == Transport::OpenAiLike {
            opts = opts
                .with_prompt_cache_key(format!("endless:{}", self.session))
                .with_cache_control(CacheControl::Ephemeral24h);
        }
        if let Some(e) = &self.effort {
            opts = opts.with_reasoning_effort(e.clone());
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
        Box::pin(self.compress_inner(ctx, convo))
    }

    fn model(&self) -> &Resolved {
        &self.model
    }
}

impl ModelCompactor {
    async fn compress_inner(&self, ctx: &str, convo: &[ChatMessage]) -> Result<Reply, String> {
        let transport = self.backend.transport(&self.model).await?;
        let (req, opts) = self.request(transport, ctx, convo);
        let started = Instant::now();
        let call = async {
            let mut stream = self
                .backend
                .stream(&self.model, req, &opts, self.session)
                .await?;
            while let Some(ev) = stream.next().await {
                if let ChatStreamEvent::End(end) = ev? {
                    return Ok(end);
                }
            }
            Err::<StreamEnd, String>("stream ended without End event".into())
        };
        let end = tokio::time::timeout(ATTEMPT_TIMEOUT, call)
            .await
            .map_err(|_| "timed out".to_owned())??;
        Ok(Reply {
            text: end.captured_texts().map(|t| t.concat()).unwrap_or_default(),
            usage: end.captured_usage,
            latency: started.elapsed(),
        })
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
                Ok(Reply {
                    text: text?,
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
        }
    }

    /// §14: size enforcement — overshoot, then comply in the same conversation.
    #[tokio::test]
    async fn size_retry_in_same_conversation() {
        let long = format!(
            "user: rename the config loader to load_settings {}",
            "x".repeat(600)
        );
        let fake = Fake::new(&[
            &long,
            "user: rename the config loader to load_settings, keep old alias",
        ]);
        let built = build(&fake, "<chat>\n</chat>", &unit_step(), 512, 5)
            .await
            .unwrap();
        assert_eq!(built.tries, 2);
        assert_eq!(
            built.text,
            "user: rename the config loader to load_settings, keep old alias"
        );
        assert_eq!(built.calls.len(), 2);
        assert_eq!(built.calls[0].guard, Some("size"));
        assert_eq!(built.calls[1].guard, None);
        let sent = fake.sent.lock();
        assert_eq!(sent[1].len(), 3, "step, assistant overshoot, correction");
        let correction = sent[1][2].content.iter().next().and_then(|p| match p {
            genai::chat::ContentPart::Text(t) => Some(t.clone()),
            _ => None,
        });
        assert!(correction.unwrap().contains("the limit is 512"));
    }

    /// Tries exhausted with no passing candidate: the shortest wins, flagged negative.
    #[tokio::test]
    async fn exhausted_tries_take_shortest_and_flag() {
        let fake = Fake::new(&[
            "user: deploy the website to production tonight",
            "user: deploy the website",
        ]);
        let built = build(&fake, "", &unit_step(), 512, 2).await.unwrap();
        assert_eq!(built.tries, -2);
        assert_eq!(built.text, "user: deploy the website");
        assert!(built.calls.iter().all(|c| c.guard == Some("grounding")));
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
    }
}
