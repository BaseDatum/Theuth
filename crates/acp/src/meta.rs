//! What a parent theuth tells a remote theuth in `session/new` `_meta`
//! (67 decision 4, §8): the child's model, effort and tool ceiling, which
//! session on which engine opened it, and — for `agent: "<harness>@<engine>"`
//! — the node's own external harness to run instead of a native session.
//! One namespaced key, [`META_KEY`], so other ACP clients' metadata is never
//! mistaken for ours; an absent key is "no instructions" and the node
//! behaves exactly as for any ACP client.

use agent_client_protocol::schema::v1::Meta;
use serde::{Deserialize, Serialize};
use tools::Risk;

/// The `_meta` key theuth-to-theuth instructions live under.
pub const META_KEY: &str = "theuth";

/// `_meta.theuth` of `session/new`. Every field optional; `Default` is the
/// empty object, which [`SessionMeta::to_meta`] does not emit.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub struct SessionMeta {
    /// `provider/model` for the child; the node resolves it against its own catalog.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Reasoning effort keyword or token budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    /// Risk ceiling for the child and its descendants (`read_only | mutating | destructive`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tools: Option<Risk>,
    /// The parent session id on the calling engine.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The calling engine as the node knows it (its device id, 32 §4.1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_engine: Option<String>,
    /// Run this `acp/agents.yaml` entry of the node as the child instead of
    /// a native session (`agent: "<harness>@<engine>"`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harness: Option<String>,
}

impl SessionMeta {
    /// `{ "theuth": {...} }`; `None` when nothing is set.
    #[must_use]
    pub fn to_meta(&self) -> Option<Meta> {
        if *self == Self::default() {
            return None;
        }
        let value = serde_json::to_value(self).ok()?;
        let mut m = Meta::new();
        m.insert(META_KEY.to_owned(), value);
        Some(m)
    }

    /// The `theuth` entry of a request's `_meta`; `Ok(None)` when absent,
    /// `Err` (the text the caller sees) when present but malformed.
    pub fn from_meta(meta: Option<&Meta>) -> Result<Option<Self>, String> {
        let Some(v) = meta.and_then(|m| m.get(META_KEY)) else {
            return Ok(None);
        };
        serde_json::from_value(v.clone())
            .map(Some)
            .map_err(|e| format!("_meta.{META_KEY}: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let m = SessionMeta {
            model: Some("anthropic/claude-opus-4-8".into()),
            effort: Some("high".into()),
            tools: Some(Risk::ReadOnly),
            parent: Some("0193b7c0-0000-7000-8000-000000000001".into()),
            parent_engine: Some("Ab3dEf9h".into()),
            harness: None,
        };
        let meta = m.to_meta().expect("non-empty");
        let json = serde_json::to_string(&meta).unwrap();
        assert!(json.contains("\"tools\":\"read_only\""), "{json}");
        assert!(!json.contains("harness"), "{json}");
        let back: Meta = serde_json::from_str(&json).unwrap();
        assert_eq!(SessionMeta::from_meta(Some(&back)), Ok(Some(m)));
    }

    #[test]
    fn empty_is_absent() {
        assert_eq!(SessionMeta::default().to_meta(), None);
        assert_eq!(SessionMeta::from_meta(None), Ok(None));
        let other: Meta = serde_json::from_str(r#"{"zed":{"x":1}}"#).unwrap();
        assert_eq!(SessionMeta::from_meta(Some(&other)), Ok(None));
    }

    #[test]
    fn malformed_is_an_error() {
        let bad: Meta = serde_json::from_str(r#"{"theuth":{"tools":"everything"}}"#).unwrap();
        let e = SessionMeta::from_meta(Some(&bad)).unwrap_err();
        assert!(e.starts_with("_meta.theuth:"), "{e}");
    }
}
