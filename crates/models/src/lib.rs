//! Bundled models.dev catalog (`data/models.json`, refreshed by `just models`)
//! merged with `<data_dir>/models.json`, plus `provider/model` id resolution
//! to genai `adapter::model` ids. No network; see design doc 02 §1.

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
    #[serde(default)]
    pub cost: Option<Cost>,
    /// Supports reasoning/thinking.
    #[serde(default)]
    pub reasoning: bool,
    /// Supports tool calls.
    #[serde(default)]
    pub tool_call: bool,
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
    #[serde(default)]
    pub cache_read: Option<f64>,
    /// Cache write tokens.
    #[serde(default)]
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

static BUNDLED: LazyLock<BTreeMap<String, Provider>> = LazyLock::new(|| {
    serde_json::from_str(include_str!("../data/models.json"))
        .expect("bundled data/models.json is valid")
});

impl Catalog {
    /// Bundled catalog merged with `<data_dir>/models.json` (same shape; user wins per model,
    /// and user provider metadata wins). A missing user file is normal; a broken one is logged and skipped.
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
        Catalog { providers }
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
            "amazon-bedrock" => "bedrock_api",
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

    /// Resolve a typed or configured id. `configured` is asked with the genai namespace
    /// (`Catalog::adapter(provider)`) and answers whether genai can authenticate it.
    ///
    /// 1. `provider/model` in the catalog → that model.
    /// 2. `provider/model` unknown → pass through with the adapter mapped, no limit known.
    /// 3. Bare id → search configured providers: exactly one hit → it; several → `Err` naming
    ///    them; none → pass through to genai unchanged.
    pub fn resolve(&self, id: &str, configured: &dyn Fn(&str) -> bool) -> Result<Resolved, String> {
        if let Some((provider, model)) = id.split_once('/') {
            return Ok(Resolved {
                id: id.to_owned(),
                genai: format!("{}::{model}", Self::adapter(provider)),
                provider: Some(provider.to_owned()),
                model: self.get(id).map(|(_, m)| m.clone()),
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
}
