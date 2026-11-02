//! 160: the full-history cost cube and the calls ledger. The whole ledger
//! (native `model_calls`, read-time repriced rows, placed remote-meter
//! money and remote-turn tokens) aggregated to UTC hours × root × model ×
//! provider × kind × via × basis, held in memory by [`CostService`] and
//! answered as a full snapshot or a delta (§3); individual rows are paged
//! straight from the store (§4).

mod calls;
mod cube;
mod place;
mod service;
mod types;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use sha2::{Digest, Sha256};

pub use service::CostService;
pub use types::*;

pub(crate) use place::Piece;

/// Catalog rates per `provider/model` id, with the table's hash.
pub(crate) struct Prices {
    rates: BTreeMap<String, models::Cost>,
    /// Short hex sha256 of every rate (the cube's `price_table`).
    pub(crate) hash: String,
    /// JSON array of every priced id, for `cost_calls`' SQL basis.
    pub(crate) priced_json: String,
}

impl Prices {
    pub(crate) fn new(catalog: &models::Catalog) -> Self {
        let rates: BTreeMap<String, models::Cost> = catalog
            .providers()
            .flat_map(|p| {
                p.models
                    .values()
                    .filter_map(move |m| m.cost.map(|c| (format!("{}/{}", p.id, m.id), c)))
            })
            .collect();
        let mut h = Sha256::new();
        for (id, c) in &rates {
            h.update(format!(
                "{id}:{}:{}:{:?}:{:?};",
                c.input, c.output, c.cache_read, c.cache_write
            ));
        }
        let hash = h
            .finalize()
            .iter()
            .take(8)
            .map(|b| format!("{b:02x}"))
            .collect();
        let priced_json = serde_json::to_string(&rates.keys().collect::<Vec<_>>())
            .unwrap_or_else(|_| "[]".into());
        Self {
            rates,
            hash,
            priced_json,
        }
    }

    /// Rates of the exact catalog id.
    pub(crate) fn rate(&self, model: &str) -> Option<&models::Cost> {
        self.rates.get(model)
    }

    /// `loop_::estimate` in nanos: uncached input, cache read and write
    /// (each at its rate, else the input rate) and output.
    pub(crate) fn estimate(r: &models::Cost, input: u64, output: u64, cr: u64, cw: u64) -> u64 {
        let uncached = input.saturating_sub(cr).saturating_sub(cw);
        let usd = uncached as f64 * r.input
            + cr as f64 * r.cache_read.unwrap_or(r.input)
            + cw as f64 * r.cache_write.unwrap_or(r.input)
            + output as f64 * r.output;
        let nanos = (usd / 1e6 * crate::cost::NANOS_PER_UNIT as f64).round();
        if nanos.is_finite() && nanos > 0.0 {
            nanos as u64
        } else {
            0
        }
    }

    /// cache_read × (input rate − cache-read rate), in nanos; 0 without a cache-read rate.
    pub(crate) fn cache_saved(r: Option<&models::Cost>, cr: u64) -> u64 {
        let Some(read) = r.and_then(|r| r.cache_read.map(|c| r.input - c)) else {
            return 0;
        };
        let nanos = (cr as f64 * read / 1e6 * crate::cost::NANOS_PER_UNIT as f64).round();
        if nanos.is_finite() && nanos > 0.0 {
            nanos as u64
        } else {
            0
        }
    }
}

/// A raw model id as the cube keys it: any `:effort` suffix dropped.
pub(crate) fn model_id(raw: &str) -> &str {
    raw.split(':').next().unwrap_or(raw)
}

/// `(display name, provider)` of a raw model id: the catalog's canonical
/// vendor id when it names one, else the id itself, without the provider
/// and any `-YYYYMMDD` snapshot suffix (`claude-haiku-4-5-20251001` →
/// `claude-haiku-4-5`). No `/` → provider `unknown`.
pub(crate) fn model_name(catalog: &models::Catalog, id: &str) -> (String, String) {
    let provider = id.split_once('/').map_or("unknown", |(p, _)| p);
    let canonical = catalog.get(id).and_then(|(_, m)| m.canonical.clone());
    let full = canonical.as_deref().unwrap_or(id);
    let mut name = full.split_once('/').map_or(full, |(_, m)| m);
    if let Some((head, date)) = name.rsplit_once('-')
        && date.len() == 8
        && date.bytes().all(|b| b.is_ascii_digit())
    {
        name = head;
    }
    (name.to_owned(), provider.to_owned())
}

/// RFC 3339 → unix ms (`None` when unparsable or before 1970).
pub(crate) fn ms(at: &str) -> Option<u64> {
    chrono::DateTime::parse_from_rfc3339(at)
        .ok()
        .and_then(|t| u64::try_from(t.timestamp_millis()).ok())
}

/// UTC epoch hour of a unix ms.
pub(crate) fn hour_of(ms: u64) -> u32 {
    u32::try_from(ms / 3_600_000).unwrap_or(u32::MAX)
}

/// The text bound that compares like `ms` against chrono's `to_rfc3339`
/// (`AutoSi`: no fraction on a whole second, else 3/6/9 digits).
pub(crate) fn ms_text(ms: u64) -> String {
    use chrono::TimeZone;
    let t = chrono::Utc
        .timestamp_millis_opt(i64::try_from(ms).unwrap_or(i64::MAX))
        .single()
        .unwrap_or_default();
    if ms.is_multiple_of(1000) {
        t.format("%Y-%m-%dT%H:%M:%S+00:00").to_string()
    } else {
        t.format("%Y-%m-%dT%H:%M:%S%.3f+00:00").to_string()
    }
}

/// `YYYY-MM-DDTHH` (UTC) → epoch hour.
pub(crate) fn hour_text(h: &str) -> Option<u32> {
    let t =
        chrono::NaiveDateTime::parse_from_str(&format!("{h}:00:00"), "%Y-%m-%dT%H:%M:%S").ok()?;
    u32::try_from(t.and_utc().timestamp() / 3600).ok()
}
