//! `share:` config section (210 §8.1) and the edge choice (§5.5). The master switch and the edge
//! are never config keys (D11): they live in the plugin's own `settings` table ([`crate::store`]).

use std::time::Duration;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use share_proto::ShareEdgeTicket;

/// A share's lifetime, counted from going live (§7.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum Expiry {
    /// One hour.
    #[serde(rename = "1h")]
    H1,
    /// Eight hours (the default, D11).
    #[default]
    #[serde(rename = "8h")]
    H8,
    /// One day.
    #[serde(rename = "24h")]
    H24,
    /// One week.
    #[serde(rename = "7d")]
    D7,
}

impl Expiry {
    /// The lifetime.
    pub fn duration(self) -> Duration {
        Duration::from_secs(match self {
            Expiry::H1 => 3600,
            Expiry::H8 => 8 * 3600,
            Expiry::H24 => 24 * 3600,
            Expiry::D7 => 7 * 24 * 3600,
        })
    }

    /// "8 hours" for cards.
    pub fn label(self) -> &'static str {
        match self {
            Expiry::H1 => "1 hour",
            Expiry::H8 => "8 hours",
            Expiry::H24 => "24 hours",
            Expiry::D7 => "7 days",
        }
    }
}

/// `share:` in `config.yaml`: defaults only (§8.1). A `share.enabled` or `share.edge` key is
/// ignored with a warning ([`ShareConfig::warn_ignored`]).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ShareConfig {
    /// Default expiry of a publish.
    pub default_expiry: Expiry,
    /// Live shares per engine.
    pub max_live: u32,
    /// Interstitial on for new shares.
    pub interstitial: bool,
}

impl Default for ShareConfig {
    fn default() -> Self {
        Self {
            default_expiry: Expiry::H8,
            max_live: 5,
            interstitial: true,
        }
    }
}

impl ShareConfig {
    /// Keys of the raw `share:` section that §8.1 forbids in config or env.
    pub const IGNORED: &[&str] = &["enabled", "edge"];

    /// Warn once per forbidden key present in the raw section.
    pub fn warn_ignored(raw: &serde_json::Value) {
        for key in Self::IGNORED {
            if raw.get(key).is_some() {
                tracing::warn!(
                    "share.{key} in config.yaml or THEUTH_ env is ignored: set it in Settings › Sharing"
                );
            }
        }
    }
}

/// The shipped default edge (D15): live key first, cold standby second. Placeholders until the
/// operator fills them with `theuth-share-edge ticket` output; a placeholder never parses, so
/// [`default_edges`] is empty and publishing says "No sharing edge configured".
// TODO(210-M4): the operator pastes the two real tickets of tun.zip's live and standby keys here.
pub const DEFAULT_EDGE_TICKETS: [&str; 2] = [
    "OPERATOR-FILL-ME: theuthedge… ticket of the live key",
    "OPERATOR-FILL-ME: theuthedge… ticket of the cold standby key",
];

/// The parsed default tickets; placeholders drop out.
pub fn default_edges() -> Vec<ShareEdgeTicket> {
    DEFAULT_EDGE_TICKETS
        .iter()
        .filter_map(|t| t.parse().ok())
        .collect()
}

/// The machine's edge choice (§5.5), the `edge` key of the local `settings` table.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum EdgeChoice {
    /// The shipped default.
    #[default]
    Default,
    /// A pasted ticket.
    Ticket(Box<ShareEdgeTicket>),
    /// Sharing has no edge.
    None,
}

/// The text of "none" in `settings`.
pub const NO_SHARING_EDGE: &str = "No sharing edge configured";

impl EdgeChoice {
    /// Stored form: `default`, `none`, or the ticket string.
    pub fn to_setting(&self) -> String {
        match self {
            EdgeChoice::Default => "default".into(),
            EdgeChoice::None => "none".into(),
            EdgeChoice::Ticket(t) => t.to_string(),
        }
    }

    /// Parse the stored form or a pasted value.
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        match s.trim() {
            "default" | "" => Ok(EdgeChoice::Default),
            "none" => Ok(EdgeChoice::None),
            t => Ok(EdgeChoice::Ticket(Box::new(t.parse().map_err(|e| {
                anyhow::anyhow!("not a share edge ticket: {e}")
            })?))),
        }
    }

    /// Tickets to dial, preferred first.
    pub fn tickets(&self) -> Vec<ShareEdgeTicket> {
        match self {
            EdgeChoice::Default => default_edges(),
            EdgeChoice::Ticket(t) => vec![(**t).clone()],
            EdgeChoice::None => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_never_parse_and_choice_roundtrips() {
        assert!(default_edges().is_empty());
        assert_eq!(EdgeChoice::parse("none").unwrap(), EdgeChoice::None);
        assert_eq!(
            EdgeChoice::parse("default").unwrap().to_setting(),
            "default"
        );
        assert!(EdgeChoice::parse("theuthedgeNOPE").is_err());
        let e: Expiry = serde_json::from_str("\"24h\"").unwrap();
        assert_eq!(e.duration().as_secs(), 86_400);
    }
}
