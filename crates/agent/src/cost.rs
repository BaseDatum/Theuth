//! Model usage and cost types (design doc 51 §6): integer-nanos money, native
//! attempt identity/provenance/coverage, remote cumulative meters, and the
//! owner summary `session_cost` answers with. Ledger rows live in
//! [`crate::store`] (`store/accounting.rs`); nothing here prices anything.

use std::fmt;
use std::str::FromStr;

use runtime::SessionId;
use serde::{Deserialize, Serialize};

/// One unit of currency in nanos (`1 USD = 1_000_000_000 nanos`).
pub const NANOS_PER_UNIT: u64 = 1_000_000_000;

/// Serialize `u64` nanos as a decimal string (JavaScript's 53-bit limit).
mod nanos_string {
    use serde::{Deserialize as _, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(n: &u64, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(n)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u64, D::Error> {
        let s = std::borrow::Cow::<str>::deserialize(d)?;
        s.parse().map_err(serde::de::Error::custom)
    }
}

/// A nonnegative amount of one ISO 4217 currency, in integer nanos of the
/// unit. Never converted between currencies.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Money {
    /// ISO 4217 code, three ASCII uppercase letters (`USD`).
    pub currency: String,
    /// Integer nanos of one currency unit; a decimal string over JSON.
    #[serde(with = "nanos_string")]
    #[schemars(with = "String")]
    #[cfg_attr(feature = "ts", ts(type = "string"))]
    pub nanos: u64,
}

/// Checked sum overflowed `u64` nanos.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Overflow;

impl fmt::Display for Overflow {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("money overflow")
    }
}

impl std::error::Error for Overflow {}

/// ISO 4217 shape: exactly three ASCII uppercase letters.
pub fn valid_currency(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase())
}

impl Money {
    /// Validate a provider/remote `f64` amount: finite, nonnegative, rounded
    /// once to the nearest nano, within `i64`; `currency` must be ISO 4217
    /// shaped. The `Err` is content-free (safe to log).
    pub fn from_f64(amount: f64, currency: &str) -> Result<Self, String> {
        if !valid_currency(currency) {
            return Err("invalid currency".into());
        }
        if !amount.is_finite() || amount < 0.0 {
            return Err("invalid amount".into());
        }
        let nanos = (amount * NANOS_PER_UNIT as f64).round();
        // `i64::MAX as f64` rounds up to 2^63; `>=` keeps every accepted value inside i64.
        if nanos >= i64::MAX as f64 {
            return Err("amount out of range".into());
        }
        Ok(Self {
            currency: currency.to_owned(),
            nanos: nanos as u64,
        })
    }

    /// Same-currency checked sum; `None` on a currency mismatch or overflow.
    pub fn checked_add(&self, other: &Self) -> Option<Self> {
        if self.currency != other.currency {
            return None;
        }
        Some(Self {
            currency: self.currency.clone(),
            nanos: self.nanos.checked_add(other.nanos)?,
        })
    }
}

/// Add `m` to the per-currency `bucket`, kept sorted by currency. On overflow
/// the currency's entry is removed (the caller reports it unavailable) and
/// [`Overflow`] is returned; the vec stays sorted.
pub fn add_by_currency(bucket: &mut Vec<Money>, m: &Money) -> Result<(), Overflow> {
    match bucket.binary_search_by(|e| e.currency.as_str().cmp(&m.currency)) {
        Ok(i) => match bucket[i].nanos.checked_add(m.nanos) {
            Some(n) => {
                bucket[i].nanos = n;
                Ok(())
            }
            None => {
                bucket.remove(i);
                Err(Overflow)
            }
        },
        Err(i) => {
            bucket.insert(i, m.clone());
            Ok(())
        }
    }
}

/// Unit-variant enum with fixed `snake_case` strings for SQLite and JSON.
macro_rules! str_enum {
    ($(#[$m:meta])* $name:ident { $($(#[$vm:meta])* $v:ident = $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
        #[serde(rename_all = "snake_case")]
        pub enum $name { $($(#[$vm])* $v),+ }

        impl $name {
            /// The stored string form.
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$v => $s),+ }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(self.as_str())
            }
        }

        impl FromStr for $name {
            type Err = String;
            fn from_str(s: &str) -> Result<Self, String> {
                match s {
                    $($s => Ok(Self::$v),)+
                    _ => Err(format!("unknown {}: {s:?}", stringify!($name))),
                }
            }
        }
    };
}

str_enum! {
    /// Where a monetary observation came from (§6.2).
    Provenance {
        /// An explicit provider/upstream value.
        ProviderReported = "provider_reported",
        /// Derived by the adapter from tokens and a rate table.
        AdapterEstimated = "adapter_estimated",
        /// A remote wrapper's cumulative telemetry, not an invoice.
        OpaqueRemoteReported = "opaque_remote_reported",
    }
}

str_enum! {
    /// What a native attempt was for (§6.1).
    CallKind {
        /// A tool round of a user turn.
        Round = "round",
        /// Automatic or manual compaction.
        Compaction = "compaction",
        /// Explicit durable harvest.
        Harvest = "harvest",
        /// Title generation.
        Title = "title",
        /// Vision-fallback question or inspection call (52 §5).
        Vision = "vision",
    }
}

str_enum! {
    /// Lifecycle of one native attempt (§6.1).
    CallStatus {
        /// Reserved, transport not yet invoked.
        Pending = "pending",
        /// Transport invoked, no settlement yet.
        Dispatched = "dispatched",
        /// Finished with a reply.
        Settled = "settled",
        /// Finished with an error.
        Failed = "failed",
        /// Cancelled locally after dispatch.
        Cancelled = "cancelled",
        /// Found open at startup; outcome unknown.
        Unresolved = "unresolved",
        /// Conclusively never sent; excluded from every total.
        NotDispatched = "not_dispatched",
    }
}

impl CallStatus {
    /// A settlement outcome (`settle_call` accepts only these).
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Settled | Self::Failed | Self::Cancelled)
    }
}

str_enum! {
    /// How completely a figure covers what it claims to (§6.2, §6.3).
    Coverage {
        /// Every observation accounted for.
        Complete = "complete",
        /// Gaps exist (missing observations, imported history).
        Unknown = "unknown",
        /// Contradictory observations (a lower cumulative total).
        Ambiguous = "ambiguous",
    }
}

str_enum! {
    /// Whose spend a bucket describes (§6.3).
    CostScope {
        /// This owner's own provider calls.
        Native = "native",
        /// A remote wrapper's inclusive charge; hidden descendants are inside it.
        OpaqueRemoteInclusive = "opaque_remote_inclusive",
    }
}

/// Primary and idempotency identity of one native attempt.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct CallId(pub String);

impl fmt::Display for CallId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What settlement observed for one attempt. `returned` and `estimated` may
/// coexist; aggregation uses `returned` when present, else `estimated`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostObservation {
    /// Provider-reported money.
    pub returned: Option<Money>,
    /// Adapter-estimated money.
    pub estimated: Option<Money>,
    /// Billed input tokens.
    pub input_tokens: Option<u32>,
    /// Completion tokens.
    pub output_tokens: Option<u32>,
    /// Prompt tokens served from cache.
    pub cache_read_tokens: Option<u32>,
    /// Prompt tokens written to cache.
    pub cache_write_tokens: Option<u32>,
    /// Current context size after the call.
    pub context_tokens: Option<u32>,
    /// Provider namespace.
    pub provider: Option<String>,
    /// Model as requested.
    pub requested_model: Option<String>,
    /// Model the provider reports having served.
    pub served_model: Option<String>,
    /// Provider request id (provenance, never identity).
    pub provider_request_id: Option<String>,
    /// Content-free reason when no money was observed.
    pub unknown_reason: Option<String>,
}

/// Token sums over a bucket's calls.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct TokenTotals {
    /// Input tokens.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub input: u64,
    /// Output tokens.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub output: u64,
    /// Cache-read tokens.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cache_read: u64,
    /// Cache-write tokens.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub cache_write: u64,
    /// Settled calls that reported no tokens at all.
    pub unknown_calls: u32,
}

/// Aggregated spend (§6.2). Monetary partition per call: returned bucket when
/// a returned value exists, else the estimate bucket, else `unavailable`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CostBucket {
    /// Provider-reported (and remote-meter attributed) money, one entry per currency, sorted.
    pub returned: Vec<Money>,
    /// Adapter estimates for calls without a returned value, per currency, sorted.
    pub estimated_for_unavailable: Vec<Money>,
    /// Attempts that were reserved and not conclusively `not_dispatched`.
    pub calls: u32,
    /// Attempts that reached transport.
    pub dispatched: u32,
    /// Settled attempts with no money observation (overflowed sums included).
    pub unavailable: u32,
    /// Attempts left open by a crash; outcome unknown.
    pub unresolved: u32,
    /// Token sums.
    pub tokens: TokenTotals,
}

impl CostBucket {
    /// Add `other` into `self`; an overflowed currency is dropped from the
    /// affected vec and counted once in `unavailable`.
    pub fn absorb(&mut self, other: &Self) {
        for m in &other.returned {
            if add_by_currency(&mut self.returned, m).is_err() {
                self.unavailable = self.unavailable.saturating_add(1);
            }
        }
        for m in &other.estimated_for_unavailable {
            if add_by_currency(&mut self.estimated_for_unavailable, m).is_err() {
                self.unavailable = self.unavailable.saturating_add(1);
            }
        }
        self.calls = self.calls.saturating_add(other.calls);
        self.dispatched = self.dispatched.saturating_add(other.dispatched);
        self.unavailable = self.unavailable.saturating_add(other.unavailable);
        self.unresolved = self.unresolved.saturating_add(other.unresolved);
        let t = &mut self.tokens;
        t.input = t.input.saturating_add(other.tokens.input);
        t.output = t.output.saturating_add(other.tokens.output);
        t.cache_read = t.cache_read.saturating_add(other.tokens.cache_read);
        t.cache_write = t.cache_write.saturating_add(other.tokens.cache_write);
        t.unknown_calls = t.unknown_calls.saturating_add(other.tokens.unknown_calls);
    }
}

/// One immediate accounting child with its recursive total.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ChildCost {
    /// Child owner (its session id).
    pub session: SessionId,
    /// `sessions.name` while the conversation still exists.
    pub name: Option<String>,
    /// The conversation was deleted; the ledger is retained.
    pub deleted: bool,
    /// Recursive total of the child.
    pub total: CostBucket,
    /// Whether the child's figure is native or an inclusive remote meter.
    pub scope: CostScope,
}

/// Owner summary answered by `session_cost` (§6.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SessionCost {
    /// Owner.
    pub session: SessionId,
    /// Opaque revision; bumped by every ledger mutation touching this owner or a descendant.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub revision: u64,
    /// Whether the ledger covers the owner's whole history (`unknown` for backfilled sessions).
    pub history_coverage: Coverage,
    /// Native, or an inclusive remote meter owner.
    pub scope: CostScope,
    /// The owner's own rows.
    pub direct: CostBucket,
    /// Sum of the children's recursive totals.
    pub descendants: CostBucket,
    /// `direct + descendants`.
    pub total: CostBucket,
    /// Immediate accounting children (deleted ones flagged).
    pub children: Vec<ChildCost>,
    /// Remote meters in the subtree whose coverage is not complete.
    pub remote_meter_gaps: u32,
}

/// One cumulative remote meter observation (§6.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct MeterUpdate {
    /// Backend origin (harness name).
    pub origin: String,
    /// External session id the meter belongs to.
    pub external_session: String,
    /// Cumulative total as reported.
    pub cumulative: Money,
    /// The wrapper created this external session itself: baseline is known zero.
    pub confirmed_new_session: bool,
    /// Remote sequence/report id when provided; a repeat is ignored.
    pub report_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_f64_validates_and_rounds_once() {
        assert!(Money::from_f64(f64::NAN, "USD").is_err());
        assert!(Money::from_f64(f64::INFINITY, "USD").is_err());
        assert!(Money::from_f64(-0.01, "USD").is_err());
        assert!(Money::from_f64(1e10, "USD").is_err());
        assert!(Money::from_f64(1.0, "usd").is_err());
        assert!(Money::from_f64(1.0, "USDD").is_err());
        assert_eq!(Money::from_f64(0.1, "USD").unwrap().nanos, 100_000_000);
        assert_eq!(Money::from_f64(0.0000000005, "USD").unwrap().nanos, 1);
        assert_eq!(Money::from_f64(0.0000000004, "USD").unwrap().nanos, 0);
        assert_eq!(Money::from_f64(0.0, "EUR").unwrap().nanos, 0);
        let m = Money::from_f64(0.123, "USD").unwrap();
        assert_eq!(
            serde_json::to_string(&m).unwrap(),
            r#"{"currency":"USD","nanos":"123000000"}"#
        );
        assert_eq!(serde_json::from_str::<Money>(r#"{"currency":"USD","nanos":"5"}"#).unwrap().nanos, 5);
    }

    #[test]
    fn add_by_currency_sorts_and_drops_overflow() {
        let mut v = Vec::new();
        let usd = Money::from_f64(1.0, "USD").unwrap();
        let eur = Money::from_f64(2.0, "EUR").unwrap();
        add_by_currency(&mut v, &usd).unwrap();
        add_by_currency(&mut v, &eur).unwrap();
        add_by_currency(&mut v, &usd).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].currency.as_str(), v[0].nanos), ("EUR", 2 * NANOS_PER_UNIT));
        assert_eq!((v[1].currency.as_str(), v[1].nanos), ("USD", 2 * NANOS_PER_UNIT));
        let max = Money { currency: "USD".into(), nanos: u64::MAX };
        assert_eq!(add_by_currency(&mut v, &max), Err(Overflow));
        assert_eq!(v.len(), 1);
        assert_eq!(v[0].currency, "EUR");
        assert_eq!("not_dispatched".parse::<CallStatus>(), Ok(CallStatus::NotDispatched));
        assert_eq!(CostScope::OpaqueRemoteInclusive.as_str(), "opaque_remote_inclusive");
    }
}
