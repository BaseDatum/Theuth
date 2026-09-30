//! Public types of the roster (67 §6). Everything that crosses the RPC
//! surface (`agents_list`, `agent_*`) is exported to TypeScript.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};

/// An agent name: `[a-z0-9-]{1,32}`. `theuth` is reserved for the built-in
/// default agent and is never a roster entry (67 §6.1).
#[derive(
    Clone,
    Debug,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Hash,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
    kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, type = "string"))]
#[serde(try_from = "String", into = "String")]
pub struct AgentName(String);

impl AgentName {
    /// The reserved name of the built-in default agent.
    pub const DEFAULT: &'static str = "theuth";

    /// The built-in default agent's name.
    pub fn theuth() -> Self {
        Self(Self::DEFAULT.to_owned())
    }

    /// `true` for the built-in default agent.
    pub fn is_default(&self) -> bool {
        self.0 == Self::DEFAULT
    }

    /// Validate `[a-z0-9-]{1,32}`.
    pub fn new(s: &str) -> Result<Self, String> {
        let ok = (1..=32).contains(&s.len())
            && s.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if ok {
            Ok(Self(s.to_owned()))
        } else {
            Err(format!("agent name {s:?}: expected [a-z0-9-]{{1,32}}"))
        }
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// `agent:<name>`, the peer/owner form.
    pub fn peer(&self) -> String {
        format!("agent:{}", self.0)
    }
}

impl fmt::Display for AgentName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl FromStr for AgentName {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl TryFrom<String> for AgentName {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::new(&s)
    }
}

impl From<AgentName> for String {
    fn from(n: AgentName) -> Self {
        n.0
    }
}

/// `budget:` in the file: `none` (default, infinite) or `<amount> <CUR>/month` (67 §6.4).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Budget {
    /// Never checked.
    #[default]
    None,
    /// Warn at 80 %, pause at 100 % of `amount` per calendar month.
    Monthly {
        /// Cap.
        amount: f64,
        /// ISO code as written (`USD`).
        currency: String,
    },
}

impl fmt::Display for Budget {
    /// The file form: `none` | `40 USD/month`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::None => f.write_str("none"),
            Self::Monthly { amount, currency } => write!(f, "{amount} {currency}/month"),
        }
    }
}

impl FromStr for Budget {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        if s.eq_ignore_ascii_case("none") || s.is_empty() {
            return Ok(Self::None);
        }
        let bad = || format!("budget: expected `none` or `<amount> <CUR>/month`, got {s:?}");
        let (amount, rest) = s.split_once(char::is_whitespace).ok_or_else(bad)?;
        let amount: f64 = amount.parse().map_err(|_| bad())?;
        let currency = rest
            .trim()
            .strip_suffix("/month")
            .map(str::trim)
            .filter(|c| !c.is_empty() && c.chars().all(|ch| ch.is_ascii_alphabetic()))
            .ok_or_else(bad)?;
        if !(amount.is_finite() && amount >= 0.0) {
            return Err(bad());
        }
        Ok(Self::Monthly {
            amount,
            currency: currency.to_ascii_uppercase(),
        })
    }
}

/// `runs_in:` front matter (79 §2.8): `any` (default) | `vm` (the agent's own
/// VM, `vm-<agent>`) | `vm:<name>` (a named, shared VM). Wire form is the same
/// string.
#[derive(
    Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(into = "String", try_from = "String")]
pub enum RunsIn {
    /// No constraint; placement policy decides.
    #[default]
    Any,
    /// The agent's own VM, created from `sandbox.vm.defaults` on first need.
    OwnVm,
    /// A named VM record.
    Vm(String),
}

impl RunsIn {
    /// `true` for the default (omitted when rendering).
    pub fn is_any(&self) -> bool {
        *self == Self::Any
    }
}

impl fmt::Display for RunsIn {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Any => f.write_str("any"),
            Self::OwnVm => f.write_str("vm"),
            Self::Vm(n) => write!(f, "vm:{n}"),
        }
    }
}

impl FromStr for RunsIn {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.trim() {
            "any" => Ok(Self::Any),
            "vm" => Ok(Self::OwnVm),
            v => match v.strip_prefix("vm:").map(str::trim) {
                Some(n) if !n.is_empty() => Ok(Self::Vm(n.to_owned())),
                _ => Err(format!("runs_in: expected any|vm|vm:<name>, got {s:?}")),
            },
        }
    }
}

impl From<RunsIn> for String {
    fn from(r: RunsIn) -> Self {
        r.to_string()
    }
}

impl TryFrom<String> for RunsIn {
    type Error = String;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.parse()
    }
}

/// `agents/<name>.md` parsed (67 §6.1): the front matter plus the charter body.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AgentFile {
    /// File stem.
    pub name: AgentName,
    /// Display title; defaults to the name.
    pub title: String,
    /// Manager: an agent name, or `theuth` (the default).
    pub reports_to: String,
    /// Model for every session it opens.
    pub model: Option<String>,
    /// Effort for every session it opens.
    pub effort: Option<String>,
    /// Risk ceiling: `read_only` | `mutating` | `destructive` (67 §6.3).
    pub tools: Option<String>,
    /// MCP servers (or `server.tool`) it may see (67 §6.3): `None` = every
    /// server, `Some([])` = none, `Some(list)` = only those.
    pub mcp: Option<Vec<String>>,
    /// Monthly cap.
    pub budget: Budget,
    /// "When I am relevant", read by other agents when handing off.
    pub capabilities: Option<String>,
    /// Receives no wakes while true.
    pub paused: bool,
    /// `agent:<name>` when this file came from `agent_propose` and is not yet approved (67 §6.5).
    pub proposed_by: Option<String>,
    /// Placement pin (79 decision 14): where every work session of this agent runs.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(type = "\"any\" | \"vm\" | `vm:${string}`"))]
    pub runs_in: RunsIn,
    /// The charter (markdown body).
    pub body: String,
}

/// What the Agents view shows per agent (67 §12).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    /// Desk open or closed, no wake in flight.
    Idle,
    /// A wake is being handled.
    Working,
    /// Every open task is blocked.
    Blocked,
    /// `paused: true` (by hand or by budget).
    Paused,
    /// Proposed, awaiting approval.
    Proposed,
    /// Its desk's engine is unreachable.
    Unreachable,
    /// The file does not parse; `AgentSummary::error` says why. Takes no wakes.
    Invalid,
}

/// One row of `agents_list`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct AgentSummary {
    /// The file.
    pub file: AgentFile,
    /// Status.
    pub status: AgentStatus,
    /// Desk session id when one is open.
    pub desk: Option<String>,
    /// Tasks it owns in `todo` / `doing` / `blocked` / `review`.
    pub open_tasks: u32,
    /// Spent this calendar month, in the budget's currency (0 when `budget: none`).
    pub spent_month: f64,
    /// Why the file is `Invalid`; `None` otherwise.
    #[serde(default)]
    #[cfg_attr(feature = "ts", ts(optional))]
    pub error: Option<String>,
}

/// One desk episode (67 §6.2).
#[derive(
    Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Episode {
    /// uuidv7.
    pub id: String,
    /// Agent name.
    pub agent: String,
    /// The desk session.
    pub session: String,
    /// ms since epoch.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub opened_ms: u64,
    /// ms since epoch; `None` while open.
    #[cfg_attr(feature = "ts", ts(type = "number | null"))]
    pub closed_ms: Option<u64>,
    /// `idle` | `context` | `thread_end` | `lease` | `shutdown`.
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn budget_forms() {
        assert_eq!("none".parse::<Budget>(), Ok(Budget::None));
        assert_eq!(
            "40 USD/month".parse::<Budget>(),
            Ok(Budget::Monthly {
                amount: 40.0,
                currency: "USD".into()
            })
        );
        assert!(
            "12.5 eur / month".parse::<Budget>().is_err(),
            "space before the slash is not the documented form"
        );
        assert!("40/month".parse::<Budget>().is_err());
        assert!("-1 USD/month".parse::<Budget>().is_err());
        assert_eq!(
            Budget::Monthly {
                amount: 40.0,
                currency: "USD".into()
            }
            .to_string(),
            "40 USD/month"
        );
    }
}
