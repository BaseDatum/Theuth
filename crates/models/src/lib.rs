//! Bundled models.dev catalog (`data/models.json`, refreshed by `just models`)
//! merged with `<data_dir>/models.json`, plus `provider/model` id resolution
//! to genai `adapter::model` ids. No network; see design doc 02 §1.
//!
//! [`onnx`] is the one process-wide ONNX Runtime initialiser (57 §6 step 1); it lives
//! here because every `ort` user (`voice`, `meeting`, `decision`) already sits above `models`.

pub mod onnx;

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// One models.dev provider and its models, keyed by model id.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Provider {
    /// models.dev provider id (`anthropic`, `openrouter`).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Env vars models.dev lists for the key; informational.
    #[serde(default)]
    pub env: Vec<String>,
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
    /// `off` is prepended when the model has a `toggle`, so toggle-only → `off|on`.
    pub fn efforts(&self) -> Vec<String> {
        let mut out = Vec::new();
        let toggle = self
            .reasoning_options
            .iter()
            .any(|o| matches!(o, ReasoningOption::Toggle));
        if toggle {
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
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
pub struct Limit {
    /// Context window in tokens.
    pub context: u32,
    /// Max output tokens.
    pub output: u32,
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

/// models.dev provider ids we bundle: the ones genai can route (`just models` keeps only these).
pub const ROUTABLE: &[&str] = &[
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

static BUNDLED: LazyLock<BTreeMap<String, Provider>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/models.json"))
        .expect("bundled data/models.json is valid")
});

impl Catalog {
    /// Bundled catalog merged with `<data_dir>/models.json` (same shape; user wins per model,
    /// and user provider metadata wins). A missing user file is normal; a broken one is logged and skipped.
    /// Models with no context limit (gateway ids written by `AddProvider`) inherit limits,
    /// reasoning, vision, and reasoning options from their closest models.dev sibling by name.
    pub fn load(data_dir: &Path) -> Catalog {
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
        let mut cat = Catalog { providers };
        let fills: Vec<(String, String, Model)> = cat
            .providers
            .iter()
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
    /// Ties go to the larger context, then the lower provider id. Models with
    /// no context limit are never candidates. For limits only; never for what
    /// is sent to the provider.
    pub fn closest(&self, id: &str) -> Option<&Model> {
        if let Some((_, m)) = self.get(id) {
            return Some(m);
        }
        let (provider, slug) = id.rsplit_once('/').unwrap_or(("", id));
        // `cursor/cursor-grok-4.6`: the vendor prefix is not the family.
        let mut want: Vec<&str> = tokens(slug).collect();
        if want
            .first()
            .is_some_and(|t| t.eq_ignore_ascii_case(provider))
        {
            want.remove(0);
        }
        let family = *want.first()?;
        self.providers
            .iter()
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
                Some((score, m.limit.context, std::cmp::Reverse(pid), m))
            })
            .max_by_key(|(score, ctx, pid, _)| (*score, *ctx, *pid))
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
            return Ok(Resolved {
                id: id.to_owned(),
                genai: format!("{}::{model}", Self::adapter(provider)),
                provider: Some(provider.to_owned()),
                model: self.closest(id).cloned(),
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

/// User file over base: provider metadata replaced, models inserted/overridden by id.
fn merge(base: &mut BTreeMap<String, Provider>, user: BTreeMap<String, Provider>) {
    for (pid, mut p) in user {
        if let Some(existing) = base.remove(&pid) {
            let mut models = existing.models;
            models.append(&mut p.models);
            p.models = models;
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
            limit: Limit { context, output: 1 },
            cost: None,
            reasoning: false,
            tool_call: false,
            image: false,
            reasoning_options: vec![],
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
        assert_eq!(m.efforts(), ["low", "high", "max"]);
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
        assert_eq!(m.efforts(), ["low", "medium", "high"]);
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
}
