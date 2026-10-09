//! The dictionary (153 §5): the packaged technical dataset, user entries and
//! learned entries behind one matcher. The actor holds an `Arc<Dictionary>`
//! snapshot rebuilt on every write and handed to each new dictation (§4.6).
//!
//! 153-C stub — A2 fills: `load` builds an empty dictionary, `lookup` never matches.

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::Surface;

pub mod matcher;

/// §5.4 W-k: accept a match whose entry already has this `heard` variant (or is a user entry).
pub const ACCEPT_KNOWN: f32 = 0.70;
/// §5.4 W-k: accept a match for an entry without this variant.
pub const ACCEPT_NEW: f32 = 0.85;
/// §5.4 W-k: a phonetic hit counts only within this Levenshtein distance.
pub const EDIT_MAX: usize = 2;

/// Identifies an [`Entry`] across layers (packaged, user, learned).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct EntryId(pub String);

/// What kind of thing an entry's canonical is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    /// A technical term or product name.
    Term,
    /// A symbol ("comma", "open paren").
    Symbol,
    /// A casing formatter ("camel", "snake").
    Formatter,
    /// An abbreviation.
    Abbrev,
    /// An @mention.
    Mention,
    /// A slash command.
    Command,
    /// A file path / extension / TLD.
    Path,
}

/// Which layer an entry came from (§5); the matcher's tie-break order is User > Learned > Packaged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Shipped dataset (§5.1).
    Packaged,
    /// Settings / `dictionary_put`.
    User,
    /// §6.3's Add.
    Learned,
}

/// Per-term scope; an any-app term can be limited to one app.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "scope", rename_all = "snake_case")]
pub enum Scope {
    /// Every surface.
    Global,
    /// One surface.
    Surface {
        /// The surface.
        surface: Surface,
    },
    /// One app (bundle id / exe).
    App {
        /// `AppProfile.id`.
        app: String,
    },
}

/// One heard form of an entry, with its phonetic codes and usage counts.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Variant {
    /// What the model said.
    pub heard: String,
    /// Double Metaphone (primary, alternate) of `heard`.
    pub metaphone: (String, String),
    /// Times matched.
    pub seen: u32,
    /// Last match, Unix ms.
    pub last_ms: u64,
}

/// One dictionary entry (§5.3).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Entry {
    /// Stable id.
    pub id: EntryId,
    /// The written form; its casing wins.
    pub canonical: String,
    /// What it is.
    pub kind: Kind,
    /// Spoken forms (exact lookups).
    pub spoken: Vec<String>,
    /// Heard forms with phonetic codes.
    pub variants: Vec<Variant>,
    /// Where it applies.
    pub scope: Scope,
    /// Which layer.
    pub source: Source,
    /// "tld", "ext", "mention", "path", "terminal", "keep_case" …
    pub tags: Vec<String>,
}

/// "This heard form must NOT become canonical" (Never, or an unchanged delivery).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Negative {
    /// Lowercased heard form.
    pub heard_lc: String,
    /// The canonical it must not become.
    pub canonical: String,
}

/// One accepted match of a token window (§5.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Match {
    /// The entry that matched.
    pub entry: EntryId,
    /// Its written form.
    pub canonical: String,
    /// The §5.4 score.
    pub score: f32,
    /// Tokens of the window consumed (1–3).
    pub tokens: u8,
}

/// The three layers behind one matcher (§5). Read-only once snapshotted.
#[derive(Debug, Default)]
pub struct Dictionary {
    entries: Vec<Entry>,
    negatives: Vec<Negative>,
}

impl Dictionary {
    /// The packaged set plus the user/learned layers from `<data_dir>/voice.db`.
    pub fn load(data_dir: &Path) -> anyhow::Result<Dictionary> {
        // 153-C stub — A2 fills (§5.2 packaged index, §5.5 storage).
        let _ = data_dir;
        Ok(Dictionary::default())
    }

    /// A read-only snapshot for one dictation (§4.6).
    pub fn snapshot(&self) -> Arc<Dictionary> {
        Arc::new(Dictionary {
            entries: self.entries.clone(),
            negatives: self.negatives.clone(),
        })
    }

    /// The best accepted match for `heard` (one 1–3-token window), per §5.4.
    pub fn lookup(&self, heard: &str) -> Option<Match> {
        // 153-C stub — A2 fills.
        let _ = heard;
        None
    }

    /// Upsert by canonical + scope (§5.6); returns the stored entry.
    pub fn put(&mut self, entry: Entry) -> Entry {
        // 153-C stub — A2 fills (persist, rebuild index).
        self.entries
            .retain(|e| !(e.canonical == entry.canonical && e.scope == entry.scope));
        self.entries.push(entry.clone());
        entry
    }

    /// Delete one entry; `false` when unknown.
    pub fn delete(&mut self, id: &EntryId) -> bool {
        // 153-C stub — A2 fills (persist).
        let before = self.entries.len();
        self.entries.retain(|e| &e.id != id);
        self.entries.len() != before
    }

    /// User + learned entries, optionally filtered to `scope`.
    pub fn list(&self, scope: Option<&Scope>) -> Vec<Entry> {
        self.entries
            .iter()
            .filter(|e| e.source != Source::Packaged)
            .filter(|e| scope.is_none_or(|s| &e.scope == s))
            .cloned()
            .collect()
    }

    /// Record a veto (§5.3).
    pub fn negative(&mut self, n: Negative) {
        // 153-C stub — A2 fills (persist).
        self.negatives.push(n);
    }

    /// Packaged dataset version string, for `voice_status` (§5.6).
    pub fn packaged_version(&self) -> &str {
        ""
    }

    /// Counts for the Settings header line: (packaged, user, learned).
    pub fn counts(&self) -> (u32, u32, u32) {
        let count = |s: Source| self.entries.iter().filter(|e| e.source == s).count() as u32;
        (
            count(Source::Packaged),
            count(Source::User),
            count(Source::Learned),
        )
    }
}
