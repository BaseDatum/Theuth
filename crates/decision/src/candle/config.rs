// Vendored from laya-rs 0.1.0 `src/config.rs` (Apache-2.0, laya-rs contributors / bob-rietveld;
// https://static.crates.io/crates/laya-rs/laya-rs-0.1.0.crate). Local changes are listed in
// crates/decision/NOTICE.
#![allow(missing_docs, clippy::manual_is_multiple_of)]

//! Checkpoint configuration, validated exactly as upstream validates it.

use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;

/// A rejected `encoder/config.json` or `rl_agent_config.json` (laya-core's `Error::Config`).
#[derive(Debug)]
pub enum Error {
    Config(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let Error::Config(msg) = self;
        f.write_str(msg)
    }
}

impl std::error::Error for Error {}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub const FULL_ATTENTION: &str = "full_attention";
pub const SLIDING_ATTENTION: &str = "sliding_attention";

fn default_norm_eps() -> f64 {
    1e-5
}
fn default_activation() -> String {
    "gelu".into()
}
fn default_model_type() -> String {
    "modernbert".into()
}
fn default_local_attention() -> usize {
    128
}
fn default_global_every() -> usize {
    3
}
fn default_global_rope() -> f64 {
    160_000.0
}
fn default_local_rope() -> f64 {
    10_000.0
}
fn default_max_positions() -> usize {
    8192
}

/// `encoder/config.json`.
#[derive(Clone, Debug, Deserialize)]
pub struct EncoderConfig {
    pub vocab_size: usize,
    pub hidden_size: usize,
    pub intermediate_size: usize,
    pub num_hidden_layers: usize,
    pub num_attention_heads: usize,

    #[serde(default = "default_model_type")]
    pub model_type: String,
    #[serde(default = "default_norm_eps")]
    pub norm_eps: f64,
    #[serde(default)]
    pub norm_bias: bool,
    #[serde(default)]
    pub attention_bias: bool,
    #[serde(default)]
    pub mlp_bias: bool,
    #[serde(default = "default_activation")]
    pub hidden_activation: String,
    #[serde(default = "default_local_attention")]
    pub local_attention: usize,
    #[serde(default = "default_global_every")]
    pub global_attn_every_n_layers: usize,
    #[serde(default = "default_global_rope")]
    pub global_rope_theta: f64,
    #[serde(default = "default_local_rope")]
    pub local_rope_theta: f64,
    #[serde(default = "default_max_positions")]
    pub max_position_embeddings: usize,
    #[serde(default)]
    pub layer_types: Option<Vec<String>>,
    #[serde(default)]
    pub rope_parameters: Option<BTreeMap<String, Value>>,
}

impl EncoderConfig {
    /// Parse and validate, mirroring `EncoderConfig.from_dict`.
    pub fn from_value(value: &Value) -> Result<Self> {
        let mut config: Self = serde_json::from_value(value.clone())
            .map_err(|error| Error::Config(format!("Invalid encoder config: {error}")))?;

        if config.model_type != "modernbert" {
            return Err(Error::Config(format!(
                "Unsupported encoder: {:?}; expected modernbert",
                config.model_type
            )));
        }
        if config.hidden_activation != "gelu" {
            return Err(Error::Config(format!(
                "Unsupported encoder activation: {:?}",
                config.hidden_activation
            )));
        }
        if config.num_attention_heads == 0
            || config.hidden_size % config.num_attention_heads != 0
            || config.head_dim() % 2 != 0
        {
            return Err(Error::Config(
                "ModernBERT requires an even, integral attention head dimension".into(),
            ));
        }

        let layer_types = match config.layer_types.take() {
            Some(types) => types,
            None => (0..config.num_hidden_layers)
                .map(|index| {
                    if config.global_attn_every_n_layers == 0
                        || index % config.global_attn_every_n_layers == 0
                    {
                        FULL_ATTENTION.to_string()
                    } else {
                        SLIDING_ATTENTION.to_string()
                    }
                })
                .collect(),
        };
        if layer_types.len() != config.num_hidden_layers
            || layer_types
                .iter()
                .any(|kind| kind != FULL_ATTENTION && kind != SLIDING_ATTENTION)
        {
            return Err(Error::Config("Invalid ModernBERT layer_types".into()));
        }

        for kind in [FULL_ATTENTION, SLIDING_ATTENTION] {
            if !layer_types.iter().any(|value| value == kind) {
                continue;
            }
            let rope_type = config
                .rope_parameters
                .as_ref()
                .and_then(|params| params.get(kind))
                .and_then(|params| params.get("rope_type"))
                .and_then(Value::as_str)
                .unwrap_or("default");
            if rope_type != "default" {
                return Err(Error::Config(
                    "Only default (unscaled) ModernBERT RoPE is supported".into(),
                ));
            }
        }

        config.layer_types = Some(layer_types);
        Ok(config)
    }

    pub fn head_dim(&self) -> usize {
        self.hidden_size / self.num_attention_heads
    }

    pub fn layer_types(&self) -> &[String] {
        self.layer_types
            .as_deref()
            .expect("layer_types filled in by from_value")
    }

    /// RoPE base for an attention kind, honouring per-kind overrides.
    pub fn rope_base(&self, kind: &str) -> f64 {
        let fallback = if kind == FULL_ATTENTION {
            self.global_rope_theta
        } else {
            self.local_rope_theta
        };
        self.rope_parameters
            .as_ref()
            .and_then(|params| params.get(kind))
            .and_then(|params| params.get("rope_theta"))
            .and_then(Value::as_f64)
            .unwrap_or(fallback)
    }
}

/// `rl_agent_config.json`.
#[derive(Clone, Debug)]
pub struct AgentConfig {
    pub head_layers: usize,
    pub max_len: usize,
    pub head_max_len: usize,
    /// Number of action classes, i.e. `len(act_costs) + 1`.
    pub act_classes: usize,
    pub temperature: [f64; 3],
    pub temperature_by_options: BTreeMap<String, f64>,
}

impl AgentConfig {
    pub fn from_value(value: &Value, max_position_embeddings: usize) -> Result<Self> {
        let object: &Map<String, Value> = value
            .as_object()
            .ok_or_else(|| Error::Config("Laya config must be an object".into()))?;

        if !object.contains_key("encoder") || !object.contains_key("head_layers") {
            return Err(Error::Config(
                "Laya config must specify encoder and head_layers".into(),
            ));
        }

        let head_layers = object["head_layers"]
            .as_u64()
            .ok_or_else(|| Error::Config("head_layers must be an integer".into()))?
            as usize;
        // Upstream hardcodes 512, which caps a ModernBERT checkpoint well below
        // the 8192 positions it was trained for, and rejects an encoder shorter
        // than 512 outright. Default to what the encoder actually supports. An
        // explicit max_len still wins, so the parity fixtures are unaffected.
        let max_len = object
            .get("max_len")
            .and_then(Value::as_u64)
            .map_or(max_position_embeddings, |value| value as usize);
        let head_max_len = object
            .get("head_max_len")
            .and_then(Value::as_u64)
            .unwrap_or(192) as usize;

        if !(head_max_len > 4 && max_len > head_max_len && max_len <= max_position_embeddings) {
            return Err(Error::Config(
                "Expected 4 < head_max_len < max_len <= max_position_embeddings".into(),
            ));
        }

        let act_classes = object
            .get("act_costs")
            .and_then(Value::as_object)
            .map_or(0, Map::len)
            + 1;

        let temperature: Vec<f64> = match object.get("temperature") {
            Some(Value::Array(values)) => values.iter().filter_map(Value::as_f64).collect(),
            None => vec![1.0, 1.0, 1.0],
            _ => return Err(Error::Config("temperature must be a list".into())),
        };
        if temperature.len() != 3 {
            return Err(Error::Config(
                "Calibration temperatures must be finite and positive".into(),
            ));
        }

        let mut temperature_by_options = BTreeMap::new();
        if let Some(Value::Object(map)) = object.get("temperature_by_options") {
            for (key, value) in map {
                let value = value.as_f64().ok_or_else(|| {
                    Error::Config("Calibration temperatures must be finite and positive".into())
                })?;
                temperature_by_options.insert(key.clone(), value);
            }
        }

        if temperature
            .iter()
            .chain(temperature_by_options.values())
            .any(|value| !value.is_finite() || *value <= 0.0)
        {
            return Err(Error::Config(
                "Calibration temperatures must be finite and positive".into(),
            ));
        }

        Ok(Self {
            head_layers,
            max_len,
            head_max_len,
            act_classes,
            temperature: [temperature[0], temperature[1], temperature[2]],
            temperature_by_options,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn encoder(extra: Value) -> Value {
        let mut base = json!({
            "model_type": "modernbert",
            "vocab_size": 128,
            "hidden_size": 64,
            "intermediate_size": 96,
            "num_hidden_layers": 3,
            "num_attention_heads": 1,
            "local_attention": 16,
            "max_position_embeddings": 256
        });
        if let (Value::Object(base), Value::Object(extra)) = (&mut base, extra) {
            base.extend(extra);
        }
        base
    }

    #[test]
    fn derives_alternating_layer_types() {
        let config = EncoderConfig::from_value(&encoder(json!({}))).unwrap();
        assert_eq!(
            config.layer_types(),
            [FULL_ATTENTION, SLIDING_ATTENTION, SLIDING_ATTENTION]
        );
    }

    #[test]
    fn rope_base_falls_back_per_kind() {
        let config = EncoderConfig::from_value(&encoder(json!({}))).unwrap();
        assert_eq!(config.rope_base(FULL_ATTENTION), 160_000.0);
        assert_eq!(config.rope_base(SLIDING_ATTENTION), 10_000.0);
    }

    #[test]
    fn rope_override_is_honoured() {
        let config = EncoderConfig::from_value(&encoder(json!({
            "rope_parameters": {"full_attention": {"rope_theta": 5.0}}
        })))
        .unwrap();
        assert_eq!(config.rope_base(FULL_ATTENTION), 5.0);
    }

    #[test]
    fn rejects_unsupported_encoders() {
        assert!(EncoderConfig::from_value(&encoder(json!({"model_type": "bert"}))).is_err());
        assert!(EncoderConfig::from_value(&encoder(json!({"hidden_activation": "silu"}))).is_err());
        assert!(
            EncoderConfig::from_value(&encoder(json!({
                "rope_parameters": {"full_attention": {"rope_type": "yarn"}}
            })))
            .is_err()
        );
    }

    #[test]
    fn rejects_odd_head_dimensions() {
        assert!(EncoderConfig::from_value(&encoder(json!({"num_attention_heads": 5}))).is_err());
    }

    #[test]
    fn agent_config_validates_lengths() {
        let good = json!({"encoder": "x", "head_layers": 1, "max_len": 128, "head_max_len": 32});
        assert!(AgentConfig::from_value(&good, 256).is_ok());

        let bad = json!({"encoder": "x", "head_layers": 1, "max_len": 32, "head_max_len": 128});
        assert!(AgentConfig::from_value(&bad, 256).is_err());

        let missing = json!({"encoder": "x"});
        assert!(AgentConfig::from_value(&missing, 256).is_err());
    }

    #[test]
    fn max_len_defaults_to_encoder_capacity() {
        let config = json!({"encoder": "x", "head_layers": 1});

        let agent = AgentConfig::from_value(&config, 8192).unwrap();
        assert_eq!(agent.max_len, 8192);
        // The question budget is not a function of encoder size, so it holds.
        assert_eq!(agent.head_max_len, 192);

        // An encoder smaller than upstream's hardcoded 512 now works instead
        // of failing the max_len <= max_position_embeddings check.
        let small = AgentConfig::from_value(&config, 256).unwrap();
        assert_eq!(small.max_len, 256);

        // An explicit value still wins over the encoder capacity.
        let pinned = json!({"encoder": "x", "head_layers": 1, "max_len": 1024});
        assert_eq!(
            AgentConfig::from_value(&pinned, 8192).unwrap().max_len,
            1024
        );
    }

    #[test]
    fn agent_config_rejects_bad_temperatures() {
        let bad = json!({
            "encoder": "x", "head_layers": 1, "max_len": 128, "head_max_len": 32,
            "temperature": [1.0, 0.0, 1.0]
        });
        assert!(AgentConfig::from_value(&bad, 256).is_err());
    }

    /// The two shipped layouts parse and describe the encoders they are: the root laya
    /// (ModernBERT-large, per-kind RoPE) and cklxx/laya-browser v17s (mmBERT-base: Gemma vocab,
    /// 768 × 22, RoPE 160k on both attention kinds, laya head budget 768 of 1024).
    #[test]
    fn parses_root_and_v17s_checkpoint_configs() {
        let fixture = |name: &str| -> Value {
            let p = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/").to_owned() + name;
            serde_json::from_slice(&std::fs::read(&p).unwrap_or_else(|e| panic!("{p}: {e}")))
                .unwrap()
        };
        let root = EncoderConfig::from_value(&fixture("laya-root/encoder-config.json")).unwrap();
        assert_eq!(
            (root.vocab_size, root.hidden_size, root.num_hidden_layers),
            (50368, 1024, 28)
        );
        assert_eq!(root.rope_base(SLIDING_ATTENTION), 10_000.0);
        let agent = AgentConfig::from_value(
            &fixture("laya-root/rl_agent_config.json"),
            root.max_position_embeddings,
        )
        .unwrap();
        assert_eq!(
            (agent.max_len, agent.head_max_len, agent.head_layers),
            (512, 192, 2)
        );

        let v17s =
            EncoderConfig::from_value(&fixture("laya-browser-v17s/encoder-config.json")).unwrap();
        assert_eq!(
            (v17s.vocab_size, v17s.hidden_size, v17s.num_hidden_layers),
            (256_000, 768, 22)
        );
        assert_eq!(v17s.intermediate_size, 1152);
        assert_eq!(v17s.head_dim(), 64);
        assert_eq!(v17s.layer_types().len(), 22);
        assert_eq!(v17s.rope_base(FULL_ATTENTION), 160_000.0);
        assert_eq!(v17s.rope_base(SLIDING_ATTENTION), 160_000.0);
        let raw = fixture("laya-browser-v17s/rl_agent_config.json");
        let agent = AgentConfig::from_value(&raw, v17s.max_position_embeddings).unwrap();
        assert_eq!(
            (agent.max_len, agent.head_max_len, agent.head_layers),
            (1024, 768, 2)
        );
        let laya: crate::laya::LayaConfig = serde_json::from_value(raw).unwrap();
        assert_eq!(laya.head_budget(), 768);
        assert!((laya.temperature(crate::laya::QType::Choice, 12) - 5.025_58).abs() < 1e-4);
    }
}
