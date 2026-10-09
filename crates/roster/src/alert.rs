//! Design doc 155 §7 (D6): the engine decides *whether* an attention item
//! should alert Sky; clients decide *how*. Pure per-kind classing
//! ([`class_of`], [`why_of`]) and the per-episode bookkeeping ([`Episodes`])
//! that gives every alert a stable id and applies `roster.alert_delay_ms`
//! (the item must still be there when the delay has passed). Focus
//! suppression and the per-connection finished-seen set are rpc's (§7.3,
//! §7.5): nothing here knows a client.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::attention::AttentionItem;

/// Why a client should notify (155 §7.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum AlertClass {
    /// Only Sky can move it: an approval, a question, a review, …
    NeedsYou,
    /// A session finished while unseen (done ≠ idle, per connection).
    Finished,
}

/// The alert on one [`AttentionItem`] (155 §7.1): `id` is stable for one
/// episode (`<item id>@<episode start ms>`); clients post one notification
/// per id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Alert {
    /// `<item id>@<at_ms>`.
    pub id: String,
    /// What it is.
    pub class: AlertClass,
    /// What triggered it, one short sentence.
    pub why: String,
    /// ms since epoch: when the episode began.
    #[cfg_attr(feature = "ts", ts(type = "number"))]
    pub at_ms: u64,
}

/// The §7.2 table: the class an item of this kind alerts with, or none.
#[must_use]
pub fn class_of(item: &AttentionItem) -> Option<AlertClass> {
    // ponytail: skeleton (155 C); W3 fills the table.
    let _ = item;
    None
}

/// The §7.2 table: the one-sentence `why` for an item.
#[must_use]
pub fn why_of(item: &AttentionItem) -> String {
    // ponytail: skeleton (155 C); W3 fills the table.
    let _ = item;
    String::new()
}

/// Item id → when its current episode began (155 §7.4). Lives on the
/// `Attention` actor; `retain` after every derive.
#[derive(Debug, Default)]
pub struct Episodes {
    start_ms: HashMap<String, u64>,
}

impl Episodes {
    /// The alert for `item` now: `None` for kinds that never alert and
    /// while the episode is younger than `delay_ms`.
    pub fn alert(&mut self, item: &AttentionItem, now_ms: u64, delay_ms: u64) -> Option<Alert> {
        // ponytail: skeleton (155 C); W3 fills.
        let _ = (now_ms, delay_ms);
        self.start_ms.entry(item.id.clone()).or_insert(now_ms);
        None
    }

    /// Forget episodes of items no longer in the list.
    pub fn retain<'a>(&mut self, live: impl Iterator<Item = &'a str>) {
        let live: std::collections::HashSet<&str> = live.collect();
        self.start_ms.retain(|id, _| live.contains(id.as_str()));
    }

    /// The earliest `start + delay_ms` still in the future: when to recheck.
    #[must_use]
    pub fn next_due(&self, now_ms: u64, delay_ms: u64) -> Option<u64> {
        self.start_ms
            .values()
            .map(|s| s + delay_ms)
            .filter(|due| *due > now_ms)
            .min()
    }
}
