//! Bundled models.dev catalog (`data/models.json`, refreshed by `just models`)
//! merged with `<data_dir>/models.json`, plus `provider/model` id resolution
//! to genai `adapter::model` ids. No network; see design doc 02 §1.
//!
//! [`onnx`] is the one process-wide ONNX Runtime initialiser (57 §6 step 1); it lives
//! here because every `ort` user (`voice`, `meeting`, `decision`) already sits above `models`.
//! [`accelerator`] is the `models.accelerator` knob those users (and the candle engine) read.

pub mod accelerator;
pub mod onnx;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// One models.dev provider and its models, keyed by model id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provider {
    /// models.dev provider id (`anthropic`, `openrouter`); a user block may omit it (its key is used).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// Display name; a user block may omit it to keep the bundled one (162 D4).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// Env vars models.dev lists for the key; informational.
    #[serde(default)]
    pub env: Vec<String>,
    /// 177 E7: a local model server's block (written by the provider listing);
    /// its unknown windows are never filled from a cloud sibling.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub local: bool,
    /// Models keyed by model id (may contain `/`, e.g. `anthropic/claude-sonnet-4.5` on openrouter).
    pub models: BTreeMap<String, Model>,
}

/// One model.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Model {
    /// Model id within its provider.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Token limits.
    pub limit: Limit,
    /// USD per million tokens; absent for free/local models.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<Cost>,
    /// Supports reasoning/thinking.
    #[serde(default)]
    pub reasoning: bool,
    /// Supports tool calls.
    #[serde(default)]
    pub tool_call: bool,
    /// Accepts image input (models.dev `modalities.input` contains `"image"`); false when unknown.
    #[serde(default)]
    pub image: bool,
    /// How reasoning can be steered (models.dev `reasoning_options`).
    #[serde(default)]
    pub reasoning_options: Vec<ReasoningOption>,
    /// Wire protocol the model speaks (85 §0.3, from models.dev `provider.npm`);
    /// absent in user files → the dispatcher falls back to [`Protocol::guess`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<Protocol>,
    /// Upstream vendor id (`anthropic/claude-opus-5-5`; models.dev `canonical_model_id`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical: Option<String>,
}

/// Wire protocol a model is served over (85 §0.2): decides which genai adapter
/// builds the request when theuth dispatches the target itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// Anthropic Messages (`…/messages`, `x-api-key`).
    Anthropic,
    /// OpenAI Responses (`…/responses`, Bearer).
    OpenaiResponses,
    /// Gemini generateContent (`…/models/{id}:…`, `x-goog-api-key`).
    Gemini,
    /// OpenAI chat completions (`…/chat/completions`, Bearer).
    OpenaiChat,
}

impl Protocol {
    /// models.dev `provider.npm` → protocol; absent or `@ai-sdk/openai-compatible` → chat.
    pub fn from_npm(npm: Option<&str>) -> Self {
        match npm {
            Some("@ai-sdk/anthropic") => Self::Anthropic,
            Some("@ai-sdk/openai") => Self::OpenaiResponses,
            Some("@ai-sdk/google") => Self::Gemini,
            _ => Self::OpenaiChat,
        }
    }

    /// Prefix fallback for a model with no catalog protocol (85 §0.3).
    pub fn guess(slug: &str) -> Self {
        if slug.starts_with("claude-") {
            Self::Anthropic
        } else if ["gpt-", "grok-", "muse-"]
            .iter()
            .any(|p| slug.starts_with(p))
        {
            Self::OpenaiResponses
        } else if slug.starts_with("gemini-") {
            Self::Gemini
        } else {
            Self::OpenaiChat
        }
    }
}

/// Zen models where the Zen docs disagree with models.dev (85 §0.3; docs win):
/// `Some(protocol)` overrides, `None` excludes the model (System One, not chat).
pub fn zen_override(id: &str) -> Option<Option<Protocol>> {
    match id {
        "qwen3.7-max" | "qwen3.7-plus" => Some(Some(Protocol::Anthropic)),
        _ if id.starts_with("jev-") => Some(None),
        _ => None,
    }
}

/// One way a model lets callers steer reasoning.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ReasoningOption {
    /// Named effort levels (`low`, `high`, ...).
    Effort {
        /// Accepted keywords, in the provider's order.
        values: Vec<String>,
    },
    /// Thinking token budget; models.dev omits `min` for some models.
    BudgetTokens {
        /// Smallest accepted budget; 0 when upstream omits it.
        #[serde(default)]
        min: u32,
        /// Largest accepted budget when bounded.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max: Option<u32>,
    },
    /// Thinking on/off.
    Toggle,
}

impl Model {
    /// Effort keywords the model accepts: the `effort` option's values verbatim, or
    /// `low|medium|high` for a `budget_tokens`-only model (genai maps them to budgets);
    /// `off` is prepended for every steerable model (theuth enables thinking by default
    /// on Anthropic, 70 THINKING-TRACES), so toggle-only → `off|on`.
    pub fn efforts(&self) -> Vec<String> {
        let mut out = Vec::new();
        let toggle = self
            .reasoning_options
            .iter()
            .any(|o| matches!(o, ReasoningOption::Toggle));
        if !self.reasoning_options.is_empty() {
            out.push("off".to_owned());
        }
        let effort = self.reasoning_options.iter().find_map(|o| match o {
            ReasoningOption::Effort { values } => Some(values.as_slice()),
            _ => None,
        });
        match effort {
            Some(values) => out.extend(values.iter().filter(|v| *v != "off").cloned()),
            None if self.budget().is_some() => {
                out.extend(["low", "medium", "high"].map(str::to_owned));
            }
            None if toggle => out.push("on".to_owned()),
            None => {}
        }
        out
    }

    /// `(min, max)` when the model takes a raw `budget_tokens` and has no `effort`
    /// keywords. Models with both (Claude 4.6+) are keyword-only here: genai sends
    /// the budget inside `thinking.adaptive`, which Anthropic rejects.
    // ponytail: drop the `Effort` exclusion when genai emits budgets as `thinking.enabled` on adaptive models.
    pub fn budget(&self) -> Option<(u32, Option<u32>)> {
        let mut budget = None;
        for o in &self.reasoning_options {
            match o {
                ReasoningOption::Effort { .. } => return None,
                ReasoningOption::BudgetTokens { min, max } => budget = Some((*min, *max)),
                ReasoningOption::Toggle => {}
            }
        }
        budget
    }

    /// Ok for a value in [`Self::efforts`] or an integer within [`Self::budget`];
    /// Err names the model and lists what it accepts.
    pub fn validate_effort(&self, s: &str) -> Result<(), String> {
        let efforts = self.efforts();
        if efforts.iter().any(|e| e == s) {
            return Ok(());
        }
        let budget = self.budget();
        if let (Ok(n), Some((min, max))) = (s.parse::<u32>(), budget)
            && n >= min
            && n <= max.unwrap_or(u32::MAX)
        {
            return Ok(());
        }
        let mut accepted = efforts.join("|");
        if let Some((min, max)) = budget {
            let max = max.map(|m| m.to_string()).unwrap_or_default();
            let sep = if accepted.is_empty() { "" } else { " " };
            accepted = format!("{accepted}{sep}or a token budget {min}..{max}");
        }
        if accepted.is_empty() {
            accepted.push_str("no reasoning effort");
        }
        Err(format!("{} accepts {accepted}", self.id))
    }
}

/// Token limits.
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct Limit {
    /// Context window in tokens; 0 = unknown.
    pub context: u32,
    /// Max output tokens; 0 = unknown.
    pub output: u32,
    /// 177 E5: where the limits came from; absent (bundled, older user files) = unknown.
    #[serde(default, skip_serializing_if = "LimitSource::is_unknown")]
    pub source: LimitSource,
    /// 177 E7: the window memory and summary budgets assume while `context` is
    /// unknown (a local server's model: [`ASSUMED_LOCAL_CONTEXT`]); 0 elsewhere.
    /// Never a known window — nothing is refused or auto-compacted on it — and
    /// never written to a file.
    #[serde(skip)]
    pub assumed: u32,
}

/// 177: who set a model's limits. A server-reported value never overwrites a user one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum LimitSource {
    /// The serving runtime reported it at listing time.
    Server,
    /// Typed by the user (`set_model_limits`, `/provider limits`, a hand edit).
    User,
    /// Nobody knows.
    #[default]
    Unknown,
}

impl LimitSource {
    /// The serde default (skipped when writing).
    pub fn is_unknown(&self) -> bool {
        *self == Self::Unknown
    }
}

/// USD per million tokens.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Cost {
    /// Input tokens.
    pub input: f64,
    /// Output tokens.
    pub output: f64,
    /// Cache read tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_read: Option<f64>,
    /// Cache write tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_write: Option<f64>,
}

/// Outcome of [`Catalog::resolve`].
#[derive(Clone, Debug)]
pub struct Resolved {
    /// Canonical `provider/model` when known, otherwise the input unchanged.
    pub id: String,
    /// What genai receives: `adapter::model`, or the raw input for a bare unknown id.
    pub genai: String,
    /// models.dev provider id when the input named one.
    pub provider: Option<String>,
    /// Catalog entry when the model is known.
    pub model: Option<Model>,
}

/// Providers keyed by models.dev provider id.
#[derive(Clone, Debug)]
pub struct Catalog {
    providers: BTreeMap<String, Provider>,
}

/// models.dev provider ids we bundle: the ones genai can route (`just models` keeps only
/// these) plus `opencode` (Zen), which theuth dispatches per model protocol (85 §0.2).
pub const ROUTABLE: &[&str] = &[
    "opencode",
    "google",
    "openrouter",
    "togetherai",
    "fireworks-ai",
    "moonshotai",
    "alibaba",
    "alibaba-cn",
    "amazon-bedrock",
    "opencode-go",
    "github-copilot",
    "google-vertex",
    "google-vertex-anthropic",
    "ollama-cloud",
    "anthropic",
    "openai",
    "groq",
    "xai",
    "deepseek",
    "cohere",
    "zai",
    "nebius",
    "minimax",
    "aihubmix",
];

/// 116: the models.dev provider that is a model family's own vendor, keyed by the
/// family token `tokens()` yields first (lower-case). Aggregators (openrouter,
/// togetherai, nebius, bedrock, …) list the same models with their own output caps;
/// on a name-match tie the vendor's figures win.
const VENDORS: &[(&str, &str)] = &[
    ("claude", "anthropic"),
    ("gpt", "openai"),
    ("o1", "openai"),
    ("o3", "openai"),
    ("o4", "openai"),
    ("gemini", "google"),
    ("gemma", "google"),
    ("glm", "zai"),
    ("deepseek", "deepseek"),
    ("kimi", "moonshotai"),
    ("grok", "xai"),
    ("qwen", "alibaba"),
    ("qwen2", "alibaba"),
    ("qwen3", "alibaba"),
    ("minimax", "minimax"),
    ("command", "cohere"),
];

/// Vendor provider id for a family token (case-insensitive), per [`VENDORS`].
fn vendor_of(family: &str) -> Option<&'static str> {
    VENDORS
        .iter()
        .find(|(f, _)| f.eq_ignore_ascii_case(family))
        .map(|(_, p)| *p)
}

static BUNDLED: LazyLock<BTreeMap<String, Provider>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/models.json"))
        .expect("bundled data/models.json is valid")
});

/// 177 E7: the window memory and summary budgets assume for a local model whose
/// context is unknown ([`Limit::assumed`]); never used to refuse a request.
pub const ASSUMED_LOCAL_CONTEXT: u32 = 16_384;

impl Catalog {
    /// Bundled catalog merged with `<data_dir>/models.json` (same shape; user wins per model,
    /// and user provider metadata wins). A missing user file is normal; a broken one is logged and skipped.
    /// Models with no context limit (gateway ids written by `AddProvider`) inherit limits,
    /// reasoning, vision, and reasoning options from their closest models.dev sibling by name.
    pub fn load(data_dir: &Path) -> Catalog {
        Self::load_with(data_dir, &|_| false)
    }

    /// [`Catalog::load`], with `local(provider)` (or a block's own `local`)
    /// marking local model servers (177 E7): their unknown windows stay 0 and
    /// carry [`Limit::assumed`] for memory budgets, never a cloud sibling's
    /// window, and they are never anyone's sibling.
    pub fn load_with(data_dir: &Path, local: &dyn Fn(&str) -> bool) -> Catalog {
        let mut providers = BUNDLED.clone();
        let path = data_dir.join("models.json");
        match std::fs::read_to_string(&path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(user) => merge(&mut providers, user),
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "ignoring unreadable models.json")
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "ignoring unreadable models.json")
            }
        }
        for (pid, p) in &mut providers {
            p.local |= local(pid);
            if p.local {
                for m in p.models.values_mut().filter(|m| m.limit.context == 0) {
                    m.limit.assumed = ASSUMED_LOCAL_CONTEXT;
                }
            }
        }
        let mut cat = Catalog { providers };
        let fills: Vec<(String, String, Model)> = cat
            .providers
            .iter()
            .filter(|(_, p)| !p.local)
            .flat_map(|(pid, p)| p.models.values().map(move |m| (pid, m)))
            .filter(|(_, m)| m.limit.context == 0)
            .filter_map(|(pid, m)| {
                cat.closest(&m.id)
                    .map(|near| (pid.clone(), m.id.clone(), near.clone()))
            })
            .collect();
        for (pid, mid, near) in fills {
            let m = &mut cat.providers.get_mut(&pid).expect("from iteration").models;
            let m = m.get_mut(&mid).expect("from iteration");
            m.limit = near.limit;
            m.reasoning = near.reasoning;
            m.image = near.image;
            m.reasoning_options = near.reasoning_options;
        }
        cat
    }

    /// genai adapter namespace for a models.dev provider id; unknown ids pass through unchanged,
    /// which is what makes `ollama/llama3` → `ollama::llama3` work with no entry.
    pub fn adapter(provider: &str) -> &str {
        match provider {
            "google" => "gemini",
            "openrouter" => "open_router",
            "togetherai" => "together",
            "fireworks-ai" => "fireworks",
            "moonshotai" => "moonshot",
            "alibaba" => "qwen_cloud",
            "alibaba-cn" => "aliyun",
            "amazon-bedrock" => "bedrock_sigv4",
            "opencode-go" => "opencode_go",
            "github-copilot" => "github_copilot",
            "google-vertex" | "google-vertex-anthropic" => "vertex",
            "ollama-cloud" => "ollama_cloud",
            other => other,
        }
    }

    /// All providers, sorted by id.
    pub fn providers(&self) -> impl Iterator<Item = &Provider> {
        self.providers.values()
    }

    /// Exact `provider/model` lookup; splits at the first `/`.
    pub fn get(&self, id: &str) -> Option<(&Provider, &Model)> {
        let (provider, model) = id.split_once('/')?;
        let provider = self.providers.get(provider)?;
        Some((provider, provider.models.get(model)?))
    }

    /// `provider/model` exact, else the catalog model whose id shares the most
    /// name tokens with the model part (`cursor/claude-4.6-sonnet-medium` →
    /// `claude-sonnet-4-6`). Effort/lane words (`low`, `xhigh`, `thinking`,
    /// `max`, ...) are ignored; the family token (`claude`, `gpt`) must agree.
    /// Ties go to the family's vendor provider (116), then the larger context,
    /// then the lower provider id. Models with no context limit are never
    /// candidates. For limits only; never for what is sent to the provider —
    /// [`Catalog::resolve`] drops a sibling's `protocol`/`canonical` for that reason.
    pub fn closest(&self, id: &str) -> Option<&Model> {
        if let Some((_, m)) = self.get(id) {
            return Some(m);
        }
        let (provider, slug) = id.rsplit_once('/').unwrap_or(("", id));
        // 177 E7: an unlisted id of a local server (a local block, or genai's
        // own local adapters) borrows no cloud window.
        if self.providers.get(provider).is_some_and(|p| p.local)
            || matches!(provider, "ollama" | "omlx")
        {
            return None;
        }
        // `cursor/cursor-grok-4.6`: the vendor prefix is not the family.
        let mut want: Vec<&str> = tokens(slug).collect();
        if want
            .first()
            .is_some_and(|t| t.eq_ignore_ascii_case(provider))
        {
            want.remove(0);
        }
        let family = *want.first()?;
        let vendor = vendor_of(family);
        self.providers
            .iter()
            // 177 E7: a local server's model (an assumed window) is nobody's sibling.
            .filter(|(_, p)| !p.local)
            .flat_map(|(pid, p)| p.models.values().map(move |m| (pid, m)))
            .filter_map(|(pid, m)| {
                let have: Vec<&str> = tokens(&m.id).collect();
                if m.limit.context == 0
                    || !have.first().is_some_and(|h| h.eq_ignore_ascii_case(family))
                {
                    return None;
                }
                let shared = want
                    .iter()
                    .filter(|t| have.iter().any(|h| h.eq_ignore_ascii_case(t)))
                    .count() as i32;
                let score =
                    2 * shared - (want.len() as i32 - shared) - (have.len() as i32 - shared);
                let is_vendor = vendor == Some(pid.as_str());
                Some((score, is_vendor, m.limit.context, std::cmp::Reverse(pid), m))
            })
            .max_by_key(|(score, vendor, ctx, pid, _)| (*score, *vendor, *ctx, *pid))
            .map(|(.., m)| m)
    }

    /// Resolve a typed or configured id. `configured` is asked with the genai namespace
    /// (`Catalog::adapter(provider)`) and answers whether genai can authenticate it.
    ///
    /// 1. `provider/model` in the catalog → that model.
    /// 2. `provider/model` unknown → pass through with the adapter mapped; limits from the
    ///    closest catalog model by name ([`Catalog::closest`]), if any.
    /// 3. Bare id → search configured providers: exactly one hit → it; several → `Err` naming
    ///    them; none → pass through to genai unchanged.
    pub fn resolve(&self, id: &str, configured: &dyn Fn(&str) -> bool) -> Result<Resolved, String> {
        if let Some((provider, model)) = id.split_once('/') {
            let exact = self.get(id).is_some();
            return Ok(Resolved {
                id: id.to_owned(),
                genai: format!("{}::{model}", Self::adapter(provider)),
                provider: Some(provider.to_owned()),
                model: self.closest(id).cloned().map(|mut m| {
                    if !exact {
                        m.protocol = None;
                        m.canonical = None;
                    }
                    m
                }),
            });
        }
        let hits: Vec<(&String, &Model)> = self
            .providers
            .iter()
            .filter(|(pid, _)| configured(Self::adapter(pid)))
            .filter_map(|(pid, p)| p.models.get(id).map(|m| (pid, m)))
            .collect();
        match hits.as_slice() {
            [(pid, m)] => Ok(Resolved {
                id: format!("{pid}/{id}"),
                genai: format!("{}::{id}", Self::adapter(pid)),
                provider: Some((*pid).clone()),
                model: Some((*m).clone()),
            }),
            [] => Ok(Resolved {
                id: id.to_owned(),
                genai: id.to_owned(),
                provider: None,
                model: None,
            }),
            _ => {
                let names: Vec<String> =
                    hits.iter().map(|(pid, _)| format!("{pid}/{id}")).collect();
                Err(format!("ambiguous model `{id}`: {}", names.join(", ")))
            }
        }
    }
}

/// User file over base: provider metadata the user block sets replaces the bundled one
/// (an empty `id` / `name` / `env` keeps it, 162 D4), models inserted/overridden by id.
fn merge(base: &mut BTreeMap<String, Provider>, user: BTreeMap<String, Provider>) {
    for (pid, mut p) in user {
        if let Some(existing) = base.remove(&pid) {
            let mut models = existing.models;
            models.append(&mut p.models);
            p.models = models;
            if p.name.is_empty() {
                p.name = existing.name;
            }
            if p.env.is_empty() {
                p.env = existing.env;
            }
        }
        if p.id.is_empty() {
            p.id = pid.clone();
        }
        base.insert(pid, p);
    }
}

/// Name tokens of a model id, without effort/lane/date words.
fn tokens(id: &str) -> impl Iterator<Item = &str> {
    const NOISE: &[&str] = &[
        "none", "minimal", "low", "medium", "high", "xhigh", "max", "thinking", "fast", "preview",
        "latest",
    ];
    id.rsplit('/')
        .next()
        .unwrap_or(id)
        .split(['-', '.', '_'])
        .filter(|t| {
            let date = t.len() >= 8 && t.bytes().all(|b| b.is_ascii_digit());
            !(t.is_empty() || NOISE.contains(t) || date)
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(id: &str, context: u32) -> Model {
        Model {
            id: id.into(),
            name: id.into(),
            limit: Limit {
                context,
                output: 1,
                ..Limit::default()
            },
            cost: None,
            reasoning: false,
            tool_call: false,
            image: false,
            reasoning_options: vec![],
            protocol: None,
            canonical: None,
        }
    }

    fn provider(id: &str, models: &[(&str, u32)]) -> (String, Provider) {
        let models = models
            .iter()
            .map(|(m, c)| ((*m).to_owned(), model(m, *c)))
            .collect();
        (
            id.into(),
            Provider {
                id: id.into(),
                name: id.into(),
                env: vec![],
                local: false,
                models,
            },
        )
    }

    fn fixture() -> Catalog {
        Catalog {
            providers: BTreeMap::from([
                provider("anthropic", &[("claude-sonnet-4-5", 200_000)]),
                provider("openrouter", &[("anthropic/claude-sonnet-4-5", 1_000_000)]),
                provider("bedrock", &[("claude-sonnet-4-5", 200_000)]),
            ]),
        }
    }

    #[test]
    fn exact_id_with_slash_in_model() {
        let r = fixture()
            .resolve("openrouter/anthropic/claude-sonnet-4-5", &|_| false)
            .unwrap();
        assert_eq!(r.genai, "open_router::anthropic/claude-sonnet-4-5");
        assert_eq!(r.provider.as_deref(), Some("openrouter"));
        assert_eq!(r.model.unwrap().limit.context, 1_000_000);
    }

    #[test]
    fn bare_id_unique_ambiguous_unknown() {
        let c = fixture();
        let r = c
            .resolve("claude-sonnet-4-5", &|ns| ns == "anthropic")
            .unwrap();
        assert_eq!(
            (r.id.as_str(), r.genai.as_str()),
            (
                "anthropic/claude-sonnet-4-5",
                "anthropic::claude-sonnet-4-5"
            )
        );
        assert!(r.model.is_some());

        let e = c.resolve("claude-sonnet-4-5", &|_| true).unwrap_err();
        assert_eq!(
            e,
            "ambiguous model `claude-sonnet-4-5`: anthropic/claude-sonnet-4-5, bedrock/claude-sonnet-4-5"
        );

        let r = c.resolve("gpt-x", &|_| true).unwrap();
        assert_eq!((r.id.as_str(), r.genai.as_str()), ("gpt-x", "gpt-x"));
        assert!(r.provider.is_none() && r.model.is_none());
    }

    #[test]
    fn user_file_overrides_per_model() {
        let dir = std::env::temp_dir().join(format!("models-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("models.json"),
            r#"{"anthropic":{"id":"anthropic","name":"Mine","models":{
                "claude-sonnet-4-5":{"id":"claude-sonnet-4-5","name":"x","limit":{"context":7,"output":1}},
                "local":{"id":"local","name":"local","limit":{"context":8,"output":1}}}}}"#,
        )
        .unwrap();
        let c = Catalog::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let (p, m) = c.get("anthropic/claude-sonnet-4-5").unwrap();
        assert_eq!((p.name.as_str(), m.limit.context), ("Mine", 7));
        assert_eq!(c.get("anthropic/local").unwrap().1.limit.context, 8);
        assert!(
            c.get("anthropic/claude-haiku-4-5").is_some(),
            "bundled siblings survive"
        );
        assert!(
            c.get("openai/gpt-5").is_some(),
            "other bundled providers survive"
        );
    }

    #[test]
    fn gateway_models_inherit_from_models_dev_sibling() {
        let dir = std::env::temp_dir().join(format!("models-gw-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("models.json"),
            r#"{"rwx":{"id":"rwx","name":"rwx","models":{
                "zai-org/GLM-5.3-Flash":{"id":"zai-org/GLM-5.3-Flash","name":"zai-org/GLM-5.3-Flash","limit":{"context":0,"output":0}},
                "Qwen/Qwen3.8-Flash-Next":{"id":"Qwen/Qwen3.8-Flash-Next","name":"Qwen/Qwen3.8-Flash-Next","limit":{"context":0,"output":0}},
                "nonesuch-9":{"id":"nonesuch-9","name":"nonesuch-9","limit":{"context":0,"output":0}}}}}"#,
        )
        .unwrap();
        let c = Catalog::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let glm = c.get("rwx/zai-org/GLM-5.3-Flash").unwrap().1;
        let sib = c.closest("glm-5.3-flash").unwrap();
        assert!(sib.id.ends_with("glm-5.3-flash"), "{}", sib.id);
        assert_eq!(glm.limit.context, sib.limit.context);
        assert_eq!((glm.reasoning, glm.image), (sib.reasoning, sib.image));
        assert!(glm.limit.context > 0);
        let qwen = c.get("rwx/Qwen/Qwen3.8-Flash-Next").unwrap().1;
        assert!(qwen.limit.context > 0, "qwen3.8 family sibling found");
        assert_eq!(
            qwen.name, "Qwen/Qwen3.8-Flash-Next",
            "user-facing name kept"
        );
        assert_eq!(c.get("rwx/nonesuch-9").unwrap().1.limit.context, 0);
    }

    /// 162 D4: a user block for a built-in that holds only `models` (what a
    /// built-in refresh writes) adds its ids and keeps the bundled provider
    /// metadata, bundled models, and their protocol untouched.
    #[test]
    fn models_only_user_block_keeps_bundled_provider_metadata() {
        let bundled = BUNDLED["opencode"].clone();
        assert!(!bundled.name.is_empty() && !bundled.env.is_empty());
        let (mid, m) = bundled
            .models
            .iter()
            .find(|(_, m)| m.protocol.is_some())
            .expect("a zen model with a protocol");
        let dir = std::env::temp_dir().join(format!("models-bi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("models.json"),
            r#"{"opencode":{"models":{
                "zz-new":{"id":"zz-new","name":"zz-new","limit":{"context":0,"output":0}}}}}"#,
        )
        .unwrap();
        let c = Catalog::load(&dir);
        std::fs::remove_dir_all(&dir).unwrap();
        let p = c.providers().find(|p| p.id == "opencode").unwrap();
        assert_eq!((&p.name, &p.env), (&bundled.name, &bundled.env));
        assert_eq!(p.models.len(), bundled.models.len() + 1);
        assert!(c.get("opencode/zz-new").is_some());
        let kept = c.get(&format!("opencode/{mid}")).unwrap().1;
        assert_eq!(
            (kept.protocol, kept.limit.context),
            (m.protocol, m.limit.context)
        );
    }

    #[test]
    fn closest_matches_cursor_slugs_against_bundled_catalog() {
        let c = Catalog {
            providers: BUNDLED.clone(),
        };
        // Vertex/openrouter spell ids `vendor/model`; `closest` is for limits, so
        // the slug is what matters.
        let hit = |id: &str| {
            c.closest(id)
                .map(|m| m.id.rsplit('/').next().unwrap_or(&m.id))
                .unwrap_or("-")
                .to_owned()
        };
        assert_eq!(
            hit("cursor/claude-4.6-sonnet-medium-thinking"),
            "claude-sonnet-4-6"
        );
        assert_eq!(hit("cursor/cursor-grok-4.6-xhigh"), "grok-4.6");
        assert_eq!(
            hit("cursor/claude-opus-4-7-thinking-xhigh"),
            "claude-opus-4-7"
        );
        assert_eq!(hit("cursor/claude-sonnet-5-max"), "claude-sonnet-5");
        assert_eq!(hit("cursor/gpt-5.4-mini-xhigh"), "gpt-5.4-mini");
        assert_eq!(hit("cursor/gpt-5.1-high"), "gpt-5.1");
        assert!(hit("cursor/gemini-3.1-pro").starts_with("gemini-3.1-pro"));
        assert_eq!(hit("cursor/composer-2.5"), "-", "unknown family: no guess");
        // Exact ids still win over the matcher.
        assert_eq!(
            hit("anthropic/claude-sonnet-4-5-20250929"),
            "claude-sonnet-4-5-20250929"
        );
        // Vision flag survives the trim (design doc 13 §3).
        assert!(c.providers["anthropic"].models["claude-opus-4-6"].image);
    }

    // 116: on a name-match tie the family's vendor provider wins over aggregators.
    #[test]
    fn closest_prefers_vendor_on_tie_in_bundled_catalog() {
        let c = Catalog {
            providers: BUNDLED.clone(),
        };
        let pid_of = |hit: &Model| {
            c.providers
                .iter()
                .find(|(_, p)| p.models.values().any(|m| std::ptr::eq(m, hit)))
                .map(|(pid, _)| pid.as_str())
                .unwrap()
        };
        let glm = c.closest("litellm/zai-org/GLM-5.3-Flash").unwrap();
        assert_eq!((pid_of(glm), glm.id.as_str()), ("zai", "glm-5.3-flash"));
        assert_eq!((glm.limit.context, glm.limit.output), (1_000_000, 131_072));
        assert_ne!(glm.limit.output, 943_717, "openrouter's cap must not win");
        let claude = c.closest("gateway/claude-sonnet-4-5").unwrap();
        assert_eq!(pid_of(claude), "anthropic", "{}", claude.id);
        let gpt = c.closest("gateway/gpt-5").unwrap();
        assert_eq!(pid_of(gpt), "openai", "{}", gpt.id);
        let r = c
            .resolve("litellm/zai-org/GLM-5.3-Flash", &|_| true)
            .unwrap();
        let m = r.model.unwrap();
        assert_eq!(m.limit.output, 131_072);
        assert!(m.protocol.is_none() && m.canonical.is_none());
    }

    // 116: score dominates the vendor bit; families without a vendor row keep
    // the larger-context-then-lower-pid rule.
    #[test]
    fn closest_vendor_bit_is_below_score_and_absent_without_row() {
        let c = Catalog {
            providers: BTreeMap::from([
                provider("anthropic", &[("claude-sonnet-4", 200_000)]),
                provider("openrouter", &[("anthropic/claude-sonnet-4-5", 1_000_000)]),
                provider("meta", &[("llama-4", 100_000)]),
                provider("groq", &[("llama-4", 100_000)]),
                provider("nebius", &[("meta/llama-4", 200_000)]),
            ]),
        };
        let hit = |id: &str| c.closest(id).unwrap();
        assert_eq!(hit("gw/claude-sonnet-4-5").limit.context, 1_000_000);
        assert_eq!(hit("gw/llama-4").limit.context, 200_000, "larger context");
        let c2 = Catalog {
            providers: BTreeMap::from([
                provider("meta", &[("llama-4", 100_000)]),
                provider("groq", &[("llama-4", 100_000)]),
            ]),
        };
        assert!(std::ptr::eq(
            c2.closest("gw/llama-4").unwrap(),
            c2.get("groq/llama-4").unwrap().1
        ));
        assert_eq!(vendor_of("GLM"), Some("zai"));
        assert_eq!(vendor_of("llama"), None);
    }

    // 116: every VENDORS row names a bundled provider with a model of that family,
    // so a `just models` refresh that drops one fails here.
    #[test]
    fn vendors_table_matches_bundled_catalog() {
        for (family, pid) in VENDORS {
            let p = BUNDLED
                .get(*pid)
                .unwrap_or_else(|| panic!("{pid} not bundled"));
            assert!(
                p.models.values().any(|m| tokens(&m.id)
                    .next()
                    .is_some_and(|t| t.eq_ignore_ascii_case(family))),
                "{pid} has no {family} model"
            );
        }
    }

    fn with_options(opts: Vec<ReasoningOption>) -> Model {
        Model {
            reasoning_options: opts,
            ..model("m", 1)
        }
    }

    #[test]
    fn efforts_verbatim_for_effort_option() {
        let m = with_options(vec![
            ReasoningOption::Effort {
                values: vec!["low".into(), "high".into(), "max".into()],
            },
            ReasoningOption::BudgetTokens {
                min: 1024,
                max: None,
            },
        ]);
        assert_eq!(m.efforts(), ["off", "low", "high", "max"]);
        // Anthropic rejects a budget inside `thinking.adaptive`; keyword-only.
        assert_eq!(m.budget(), None);
        assert!(m.validate_effort("4096").is_err());
    }

    #[test]
    fn efforts_off_on_for_toggle_only() {
        let m = with_options(vec![ReasoningOption::Toggle]);
        assert_eq!(m.efforts(), ["off", "on"]);
        assert!(model("plain", 1).efforts().is_empty());
    }

    #[test]
    fn efforts_low_medium_high_for_budget_only() {
        let m = with_options(vec![ReasoningOption::BudgetTokens {
            min: 1024,
            max: Some(81920),
        }]);
        assert_eq!(m.efforts(), ["off", "low", "medium", "high"]);
        assert_eq!(m.budget(), Some((1024, Some(81920))));
    }

    #[test]
    fn validate_effort_checks_keywords_and_budget_range() {
        let m = with_options(vec![
            ReasoningOption::Toggle,
            ReasoningOption::BudgetTokens {
                min: 1024,
                max: Some(81920),
            },
        ]);
        assert_eq!(m.validate_effort("off"), Ok(()));
        assert_eq!(m.validate_effort("high"), Ok(()));
        assert_eq!(m.validate_effort("4096"), Ok(()));
        let err = m.validate_effort("100").unwrap_err();
        assert!(err.starts_with("m accepts off|low|medium|high"), "{err}");
        assert!(err.contains("1024..81920"), "{err}");
        assert!(m.validate_effort("xhigh").is_err());
        assert_eq!(
            model("plain", 1).validate_effort("high").unwrap_err(),
            "plain accepts no reasoning effort"
        );
    }

    // 85 §0.3: npm mapping, prefix guess, Zen doc overrides.
    #[test]
    fn protocol_from_npm_guess_and_zen_overrides() {
        assert_eq!(
            Protocol::from_npm(Some("@ai-sdk/anthropic")),
            Protocol::Anthropic
        );
        assert_eq!(
            Protocol::from_npm(Some("@ai-sdk/openai")),
            Protocol::OpenaiResponses
        );
        assert_eq!(Protocol::from_npm(Some("@ai-sdk/google")), Protocol::Gemini);
        assert_eq!(
            Protocol::from_npm(Some("@ai-sdk/openai-compatible")),
            Protocol::OpenaiChat
        );
        assert_eq!(Protocol::from_npm(None), Protocol::OpenaiChat);
        for (slug, want) in [
            ("claude-opus-5-5", Protocol::Anthropic),
            ("gpt-5.6-sol", Protocol::OpenaiResponses),
            ("grok-5", Protocol::OpenaiResponses),
            ("muse-1", Protocol::OpenaiResponses),
            ("gemini-3.5-flash", Protocol::Gemini),
            ("deepseek-v4", Protocol::OpenaiChat),
        ] {
            assert_eq!(Protocol::guess(slug), want, "{slug}");
        }
        assert_eq!(zen_override("qwen3.7-max"), Some(Some(Protocol::Anthropic)));
        assert_eq!(
            zen_override("qwen3.7-plus"),
            Some(Some(Protocol::Anthropic))
        );
        assert_eq!(zen_override("jev-1.13"), Some(None));
        assert_eq!(zen_override("jev-1.13-free"), Some(None));
        assert_eq!(zen_override("claude-opus-5-5"), None);
        assert_eq!(
            serde_json::to_string(&Protocol::OpenaiResponses).unwrap(),
            "\"openai_responses\""
        );
    }

    // 85 §0.3: the bundled Zen catalog carries protocols; a sibling-derived
    // model (gateway id) never leaks its protocol/canonical into `Resolved`.
    #[test]
    fn bundled_zen_protocols_and_sibling_strip() {
        let cat = Catalog {
            providers: BUNDLED.clone(),
        };
        let p = |id: &str| cat.get(id).unwrap().1.protocol;
        assert_eq!(p("opencode/claude-opus-5-5"), Some(Protocol::Anthropic));
        assert_eq!(p("opencode/gpt-5.6-sol"), Some(Protocol::OpenaiResponses));
        assert_eq!(p("opencode/gemini-3.5-flash"), Some(Protocol::Gemini));
        assert_eq!(p("opencode/qwen3.8-flash"), Some(Protocol::Anthropic));
        assert!(cat.get("opencode/jev-1.13").is_none());
        assert_eq!(
            cat.get("openrouter/anthropic/claude-opus-5.5")
                .unwrap()
                .1
                .canonical
                .as_deref(),
            Some("anthropic/claude-opus-5-5")
        );
        let exact = cat.resolve("opencode/claude-opus-5-5", &|_| true).unwrap();
        assert_eq!(exact.model.unwrap().protocol, Some(Protocol::Anthropic));
        let sibling = cat.resolve("rwx/claude-opus-5-5", &|_| true).unwrap();
        let m = sibling.model.unwrap();
        assert!(m.limit.context > 0 && m.protocol.is_none() && m.canonical.is_none());
    }

    /// 177 E7: a local server's unknown window stays unknown (0) and carries
    /// the assumed 16k for memory budgets only; it is never a cloud sibling's,
    /// and its models are nobody's sibling; the same id under a cloud custom
    /// provider still borrows the sibling. Files without `local`/`source` load as before.
    #[test]
    fn local_unknown_windows_are_assumed_not_borrowed() {
        let dir = std::env::temp_dir().join(format!("models-e7-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let block = |big: u32| {
            serde_json::json!({"models": {
                "claude-opus-5-5": {"id": "claude-opus-5-5", "name": "o", "limit": {"context": 0, "output": 0}},
                "zork-local-9": {"id": "zork-local-9", "name": "z", "limit": {"context": big, "output": 0}}
            }})
        };
        std::fs::write(
            dir.join("models.json"),
            serde_json::json!({"lan": block(0), "rwx": block(0), "flagged": {"local": true, "models": {}}})
                .to_string(),
        )
        .unwrap();
        let c = Catalog::load_with(&dir, &|p| p == "lan");
        let ctx = |id: &str| c.get(id).unwrap().1.limit.context;
        let assumed = |id: &str| c.get(id).unwrap().1.limit.assumed;
        assert_eq!(
            (ctx("lan/claude-opus-5-5"), assumed("lan/claude-opus-5-5")),
            (0, ASSUMED_LOCAL_CONTEXT)
        );
        assert_eq!(
            (ctx("lan/zork-local-9"), assumed("lan/zork-local-9")),
            (0, ASSUMED_LOCAL_CONTEXT)
        );
        assert_eq!(assumed("rwx/claude-opus-5-5"), 0);
        assert!(
            ctx("rwx/claude-opus-5-5") > ASSUMED_LOCAL_CONTEXT,
            "cloud keeps the sibling"
        );
        assert_eq!(
            ctx("rwx/zork-local-9"),
            0,
            "a local model is nobody's sibling"
        );
        assert!(c.providers().any(|p| p.id == "flagged" && p.local));
        assert!(
            c.resolve("lan/unlisted-claude-opus-5-5", &|_| true)
                .unwrap()
                .model
                .is_none()
        );
        assert!(
            c.resolve("ollama/claude-opus-5-5", &|_| true)
                .unwrap()
                .model
                .is_none()
        );
        assert_eq!(
            c.get("lan/zork-local-9").unwrap().1.limit.source,
            LimitSource::Unknown
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
