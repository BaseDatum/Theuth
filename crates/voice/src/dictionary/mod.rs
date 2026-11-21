//! The dictionary (153 §5): the packaged technical dataset, user entries and
//! learned entries behind one matcher. The actor holds an `Arc<Dictionary>`
//! snapshot rebuilt on every write and handed to each new dictation (§4.6).
//!
//! Layers: [`packaged`] (read-only, in the binary), user + learned entries and
//! negatives in `<data_dir>/voice/dictionary.json` ([`store`]). [`matcher`]
//! is the pure §5.4 algorithm; [`Dictionary::lookup`] drives it over one window.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::Surface;

pub mod matcher;
pub mod packaged;
pub mod store;

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
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "EntryKind"))]
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
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, rename = "EntrySource"))]
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
#[derive(Clone, Debug, Default)]
pub struct Dictionary {
    packaged: Arc<packaged::Packaged>,
    /// User + learned, catalog order.
    entries: Vec<Entry>,
    negatives: Vec<Negative>,
    /// Over `entries`; the packaged index lives in `packaged`.
    index: matcher::Index,
    /// The user file; `None` for an unsaved (test / snapshot) dictionary.
    path: Option<PathBuf>,
}

impl Dictionary {
    /// The packaged set plus the user/learned layers from `<data_dir>/voice/dictionary.json`.
    pub fn load(data_dir: &Path) -> anyhow::Result<Dictionary> {
        let path = store::path(data_dir);
        let (entries, negatives) = store::read(&path)?;
        let mut d = Dictionary {
            packaged: Arc::new(packaged::load()?),
            entries,
            negatives,
            index: matcher::Index::default(),
            path: Some(path),
        };
        d.index = matcher::Index::build(&d.entries);
        Ok(d)
    }

    /// The packaged layer only, no user file (tests; the shaper's fixtures).
    pub fn packaged_only() -> anyhow::Result<Dictionary> {
        Ok(Dictionary {
            packaged: Arc::new(packaged::load()?),
            ..Dictionary::default()
        })
    }

    /// A read-only snapshot for one dictation (§4.6); the packaged layer is shared, not copied.
    pub fn snapshot(&self) -> Arc<Dictionary> {
        Arc::new(self.clone())
    }

    /// The best accepted match for `heard` (one 1–3-token window), per §5.4; `Global` entries only.
    pub fn lookup(&self, heard: &str) -> Option<Match> {
        self.lookup_scoped(heard, None, None)
    }

    /// [`Dictionary::lookup`] plus the entries scoped to `surface` / `app` (§5.4 step 6).
    pub fn lookup_scoped(
        &self,
        heard: &str,
        surface: Option<Surface>,
        app: Option<&str>,
    ) -> Option<Match> {
        let heard = heard.trim();
        let (core, possessive) = split_possessive(heard);
        let key = matcher::norm(core);
        if key.is_empty() {
            return None;
        }
        let codes = matcher::metaphone(&key);
        let heard_lc = core.to_lowercase();
        let in_scope = |e: &Entry| match &e.scope {
            Scope::Global => true,
            Scope::Surface { surface: s } => surface == Some(*s),
            Scope::App { app: a } => app == Some(a.as_str()),
        };
        // User > Learned > Packaged, then catalog order: scan in that order and
        // replace only on a strictly better score; an exact hit (1.0) ends the scan.
        let mut best: Option<(f32, &Entry)> = None;
        let layers = [
            (self.index.candidates(&key, &codes), self.entries.as_slice()),
            (
                self.packaged.index.candidates(&key, &codes),
                self.packaged.entries.as_slice(),
            ),
        ];
        'scan: for rank in [Source::User, Source::Learned, Source::Packaged] {
            for (cands, entries) in &layers {
                for c in cands {
                    let e = &entries[c.idx as usize];
                    if e.source != rank || !in_scope(e) || !replaces(e) {
                        continue;
                    }
                    // Packaged terms match on the normalised form only: fuzzy matching 40k
                    // package/app names against prose rewrote "and" → wand, "plan" → Plane
                    // (A2 as-built). Fuzzy stays for the user's and learned words.
                    if e.source == Source::Packaged && !c.exact {
                        continue;
                    }
                    let s = if c.exact {
                        1.0
                    } else {
                        best_form_score(e, &key, c.phonetic)
                    };
                    let known = e.source == Source::User
                        || e.variants.iter().any(|v| matcher::norm(&v.heard) == key);
                    let threshold = if known { ACCEPT_KNOWN } else { ACCEPT_NEW };
                    if s < threshold {
                        continue;
                    }
                    if self
                        .negatives
                        .iter()
                        .any(|n| n.heard_lc == heard_lc && n.canonical == e.canonical)
                    {
                        continue;
                    }
                    if best.is_none_or(|(b, _)| s > b) {
                        best = Some((s, e));
                        if s >= 1.0 {
                            break 'scan;
                        }
                    }
                }
            }
        }
        let (score, e) = best?;
        // Self-expansion guard: a completed occurrence of the canonical is never rewritten. A
        // case-only difference is left alone for `keep_case` terms (lowercase command/package
        // names) and for packaged terms whose casing says nothing — Title-case app names (Play,
        // Tomorrow) and ALLCAPS language names (STAR) would recase prose; GitHub / PostgreSQL /
        // macOS and the curated list (JSON, AWS) still win.
        let keep_case = e.tags.iter().any(|t| t == "keep_case");
        let case_only = e.canonical.to_lowercase() == core.to_lowercase();
        let casing_informative = e.source != Source::Packaged
            || e.tags.iter().any(|t| t == "curated")
            || mixed_case(&e.canonical);
        if e.canonical == core || (case_only && (keep_case || !casing_informative)) {
            return None;
        }
        // 201 (shaper audit): a packaged term rewrites prose only on positive evidence — a
        // recase (gated above) or a multi-word window joined into a curated / mixed-case name
        // ("git hub" → GitHub). Joining ordinary words into a lowercase or Title-case name
        // ("a lot" → alot, "go to" → goto, "be an" → Bean, "up dog" → updog), dropping the
        // canonical's punctuation ("agent" → Agent!) and single-word respellings ("node" →
        // Node.js) rewrote correctly heard speech. User and learned entries are unaffected.
        if e.source == Source::Packaged
            && !case_only
            && !(core.split_whitespace().count() > 1 && casing_informative)
        {
            return None;
        }
        let mut canonical = e.canonical.clone();
        let lower = canonical == canonical.to_lowercase();
        let heard_capital = core.chars().next().is_some_and(char::is_uppercase);
        if lower && heard_capital && !keep_case {
            let mut cs = canonical.chars();
            if let Some(first) = cs.next() {
                canonical = first.to_uppercase().chain(cs).collect();
            }
        }
        canonical.push_str(possessive);
        Some(Match {
            entry: e.id.clone(),
            canonical,
            score,
            tokens: heard.split_whitespace().count().clamp(1, 3) as u8,
        })
    }

    /// Upsert by canonical + scope (§5.6); returns the stored entry (id and variant codes filled).
    pub fn put(&mut self, mut entry: Entry) -> Entry {
        if entry.id.0.is_empty() {
            entry.id = EntryId(uuid::Uuid::new_v4().to_string());
        }
        for v in &mut entry.variants {
            if v.metaphone.0.is_empty() {
                v.metaphone = matcher::metaphone(&matcher::norm(&v.heard));
            }
        }
        self.entries
            .retain(|e| !(e.canonical == entry.canonical && e.scope == entry.scope));
        self.entries.push(entry.clone());
        self.changed();
        entry
    }

    /// Delete one entry; `false` when unknown.
    pub fn delete(&mut self, id: &EntryId) -> bool {
        let before = self.entries.len();
        self.entries.retain(|e| &e.id != id);
        let removed = self.entries.len() != before;
        if removed {
            self.changed();
        }
        removed
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

    /// Record a veto (§5.3); `heard_lc` is lowercased here so callers may pass the heard form as is.
    pub fn negative(&mut self, mut n: Negative) {
        n.heard_lc = n.heard_lc.trim().to_lowercase();
        if !self.negatives.contains(&n) {
            self.negatives.push(n);
            self.changed();
        }
    }

    /// Rebuild the user index and persist; a write failure is logged, the in-memory state stands.
    fn changed(&mut self) {
        self.index = matcher::Index::build(&self.entries);
        if let Some(path) = &self.path
            && let Err(e) = store::write(path, &self.entries, &self.negatives)
        {
            tracing::warn!(path = %path.display(), error = %e, "dictionary: write failed");
        }
    }

    /// Packaged dataset version string, for `voice_status` (§5.6).
    pub fn packaged_version(&self) -> &str {
        &self.packaged.version
    }

    /// Counts for the Settings header line: (packaged, user, learned).
    pub fn counts(&self) -> (u32, u32, u32) {
        let count = |s: Source| self.entries.iter().filter(|e| e.source == s).count() as u32;
        (
            self.packaged.entries.len() as u32,
            count(Source::User),
            count(Source::Learned),
        )
    }

    /// One entry by id, any layer (the shaper reads `kind` / `tags` of a [`Match`]).
    pub fn get(&self, id: &EntryId) -> Option<&Entry> {
        self.entries
            .iter()
            .chain(&self.packaged.entries)
            .find(|e| &e.id == id)
    }

    /// The spoken-form pairs of one kind (symbols, formatters, abbreviations) and the tagged
    /// lists (`tld`, `ext`, `number`): plain entries the shaper's own stages consume (§4.2 2–5).
    /// `lookup` never returns these — see [`replaces`].
    pub fn pairs(&self, kind: Kind) -> impl Iterator<Item = &Entry> {
        self.entries
            .iter()
            .chain(&self.packaged.entries)
            .filter(move |e| e.kind == kind)
    }
}

/// Which entries `lookup` may rewrite a window into: terms, mentions and commands. Symbols,
/// formatters, abbreviations, paths (TLDs, extensions) and number words are the shaper's pairs.
fn replaces(e: &Entry) -> bool {
    matches!(e.kind, Kind::Term | Kind::Mention | Kind::Command)
        && !e.tags.iter().any(|t| t == "number")
}

/// Best score of the heard key against the entry's canonical, spoken and heard forms.
fn best_form_score(e: &Entry, key: &str, phonetic: bool) -> f32 {
    std::iter::once(e.canonical.as_str())
        .chain(e.spoken.iter().map(String::as_str))
        .chain(e.variants.iter().map(|v| v.heard.as_str()))
        .map(|f| matcher::score_norm(key, &matcher::norm(f), phonetic))
        .fold(0.0, f32::max)
}

/// `kubernetes's` → (`kubernetes`, `'s`); the suffix is carried to the output (§5.4 step 5).
fn split_possessive(heard: &str) -> (&str, &str) {
    for suffix in ["'s", "’s", "'S", "’S"] {
        if let Some(core) = heard.strip_suffix(suffix)
            && !core.is_empty()
        {
            return (core, suffix);
        }
    }
    (heard, "")
}

#[cfg(test)]
mod tests;

/// `GitHub`, `macOS`, `PostgreSQL`: an uppercase letter after the first character and some lowercase.
fn mixed_case(s: &str) -> bool {
    s.chars().skip(1).any(char::is_uppercase) && s.chars().any(char::is_lowercase)
}
