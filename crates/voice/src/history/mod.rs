//! Dictation history and correction learning (153 §6): `<data_dir>/voice.db`
//! (sqlx, migrations under `crates/voice/migrations/`), audio under
//! `<data_dir>/voice/history/<id>.pcm16`, and the suggestion flow of §6.3.
//!
//! 153-C stub — A3 fills: `open` returns a store with no database; every
//! method is a no-op that succeeds.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{DictationId, Surface};

pub mod learn;

/// Identifies one pending dictionary suggestion (§6.3); answered with `dictionary_suggestion_answer`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct SuggestionId(pub uuid::Uuid);

impl SuggestionId {
    /// A fresh time-ordered id.
    pub fn new() -> Self {
        Self(uuid::Uuid::now_v7())
    }
}

impl Default for SuggestionId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for SuggestionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// The user's answer to a suggestion card (§6.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SuggestionAnswer {
    /// Add the learned entry.
    Add,
    /// Cooldown; ask again later.
    NotNow,
    /// Never for this pair: write a negative.
    Never,
}

/// One row of `dictations` (§6.5), as `dictation_history` returns it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct HistoryRow {
    /// Dictation.
    pub id: DictationId,
    /// Unix ms.
    pub started_ms: u64,
    /// Unix ms; `None` while running.
    pub finished_ms: Option<u64>,
    /// Where it was dictated.
    pub surface: Surface,
    /// `agent_prompt | document | message_reply | any_app:<bundle or exe>`.
    pub destination: String,
    /// Any-app: bundle id / exe.
    pub app: Option<String>,
    /// Finals joined.
    pub raw: String,
    /// Finals joined, shaped.
    pub shaped: String,
    /// `None` unless §9 ran.
    pub polished: Option<String>,
    /// What actually went out (`polished ?? shaped`), after end-command edits.
    pub delivered: Option<String>,
    /// §8.7, relative to press.
    pub press_ms: Option<u32>,
    /// §8.7.
    pub first_partial_ms: Option<u32>,
    /// §8.7.
    pub first_final_ms: Option<u32>,
    /// §8.7.
    pub final_ms: Option<u32>,
    /// `done | stopped | error:<message>`.
    pub reason: String,
    /// Bytes of PCM kept; `0` when evicted or never recorded.
    pub audio_bytes: u64,
    /// Audio is on disk (`dictation_audio_get` works).
    pub has_audio: bool,
}

/// One `corrections` row (§6.3): a single-word edit between what was dictated and what was sent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Correction {
    /// Utterance index within the dictation.
    pub utterance: u32,
    /// What the model heard.
    pub heard: String,
    /// What the user typed instead.
    pub typed: String,
    /// Unix ms.
    pub at_ms: u64,
}

/// The history store: `voice.db` + the audio directory.
#[derive(Clone, Debug)]
pub struct Store {
    /// `<data_dir>/voice/history`.
    audio_dir: PathBuf,
    /// `None` until A3 opens the database.
    db: Option<sqlx::SqlitePool>,
}

impl Store {
    /// Open (and migrate) `<data_dir>/voice.db`.
    pub async fn open(data_dir: &Path) -> anyhow::Result<Store> {
        // 153-C stub — A3 fills (sqlx pool + migrations).
        Ok(Store {
            audio_dir: data_dir.join("voice").join("history"),
            db: None,
        })
    }

    /// Insert or update the dictation's row.
    pub async fn record(&self, row: HistoryRow) -> anyhow::Result<()> {
        // 153-C stub — A3 fills.
        let _ = (row, &self.db);
        Ok(())
    }

    /// Append `pcm` (16 kHz i16) to the dictation's audio file, within the budget.
    pub async fn attach_audio(
        &self,
        id: DictationId,
        utterance: u32,
        pcm: &[i16],
    ) -> anyhow::Result<()> {
        // 153-C stub — A3 fills.
        let _ = (id, utterance, pcm, &self.audio_dir);
        Ok(())
    }

    /// Newest `limit` rows.
    pub async fn list(&self, limit: u32) -> anyhow::Result<Vec<HistoryRow>> {
        // 153-C stub — A3 fills.
        let _ = limit;
        Ok(Vec::new())
    }

    /// Delete one row (and its audio), or everything when `None`.
    pub async fn forget(&self, id: Option<DictationId>) -> anyhow::Result<()> {
        // 153-C stub — A3 fills.
        let _ = id;
        Ok(())
    }

    /// The dictation's PCM (one utterance, or all); `Err("no audio")` when evicted.
    pub async fn audio(&self, id: DictationId, utterance: Option<u32>) -> anyhow::Result<Vec<i16>> {
        // 153-C stub — A3 fills.
        let _ = (id, utterance);
        anyhow::bail!("no audio")
    }

    /// `dictation_correction` landed (§6.1): diff the delivered text against `sent_text`,
    /// store the single-word corrections, and return the suggestions that are due (§6.3).
    pub async fn correction(
        &self,
        id: DictationId,
        sent_text: &str,
    ) -> anyhow::Result<Vec<Correction>> {
        // 153-C stub — A3 fills.
        let _ = (id, sent_text);
        Ok(Vec::new())
    }

    /// §6.3 step 4: record the answer (cooldown / never / added).
    pub async fn answer(&self, id: SuggestionId, answer: SuggestionAnswer) -> anyhow::Result<()> {
        // 153-C stub — A3 fills.
        let _ = (id, answer);
        Ok(())
    }
}
