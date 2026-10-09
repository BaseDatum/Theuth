//! The shaping layer (153 §4): a pure module — no I/O, no clock, no actor —
//! the dictation task calls once per piece. Spoken punctuation, literals,
//! numbers, the dictionary, spoken end commands (§7) and terminal spacing,
//! per-surface switches from `voice.shape.<surface>` (§4.5).
//!
//! 153-C stub — A1 fills: every body here returns its input unchanged.

use serde::{Deserialize, Serialize};

use crate::dictionary::Dictionary;
use crate::{AppProfile, Surface};

pub mod end;

/// Per-surface switches; resolved once at `StartDictation` from `voice.shape.<surface>` (§4.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShapeConfig {
    /// Spoken punctuation → symbols (stage 2).
    pub punctuation: bool,
    /// "literal" escapes, spelled letters, identifier formatters (stage 3).
    pub literals: bool,
    /// Number words → digits (stage 5).
    pub numbers: bool,
    /// Dictionary matches (stage 7, §5).
    pub dictionary: bool,
    /// Spoken end commands (§7).
    pub end_commands: bool,
    /// Terminal autocomplete spacing (stage 8).
    pub terminal_spacing: bool,
}

/// What the surface receives beside the raw text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Shaped {
    /// The written form.
    pub text: String,
    /// An end command armed/fired on this piece.
    pub command: Option<EndCommand>,
    /// Actions the surface applies after `text`.
    pub actions: Vec<Action>,
    /// Ranges of `text` the dictionary rewrote (for the muted/underline render and §6's diff).
    pub replaced: Vec<Replaced>,
}

/// A spoken end command (§7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum EndCommand {
    /// "send it": armed; `near_miss` when accepted by tolerance at the final.
    Send {
        /// Accepted by the §7.1 near-miss tolerance, not an exact phrase.
        near_miss: bool,
    },
    /// "scratch that": drop the previous closed utterance.
    ScratchThat,
}

/// A shaping action the surface performs (W-j: not an end command).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// "new line".
    NewLine,
    /// "new paragraph".
    NewParagraph,
    /// "tab".
    Tab,
}

/// One dictionary rewrite inside [`Shaped::text`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Replaced {
    /// Byte offset where the rewrite starts.
    pub start: usize,
    /// Byte offset where it ends (exclusive).
    pub end: usize,
    /// What the model said.
    pub heard: String,
    /// The rule that rewrote it.
    pub rule: RuleId,
}

/// Identifies a shaping/dictionary rule (an entry id, or a stage's rule name).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(transparent)]
pub struct RuleId(pub String);

/// Spoken end phrases, configurable (§7); `literal` escapes the next phrase.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct EndPhrases {
    /// Phrases that arm `Send`.
    pub send: Vec<String>,
    /// Phrases that arm `ScratchThat`.
    pub scratch: Vec<String>,
    /// The escape word: the next phrase or symbol word is written as said.
    pub literal: String,
}

impl Default for EndPhrases {
    /// §7.2's defaults.
    fn default() -> Self {
        Self {
            send: vec!["send it".into()],
            scratch: vec!["scratch that".into()],
            literal: "literal".into(),
        }
    }
}

/// `voice.shape.<surface>` (§4.5): `plain` = punctuation + literals + numbers, no dictionary/end commands.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Preset {
    /// Every stage on.
    #[default]
    Full,
    /// Punctuation, literals, numbers only.
    Plain,
    /// Raw model text.
    Off,
}

impl From<Preset> for ShapeConfig {
    fn from(p: Preset) -> Self {
        let full = p == Preset::Full;
        let on = p != Preset::Off;
        Self {
            punctuation: on,
            literals: on,
            numbers: on,
            dictionary: full,
            end_commands: full,
            terminal_spacing: full,
        }
    }
}

/// `voice.shape` (§4.5): one preset per surface.
#[derive(
    Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(default)]
pub struct ShapePresets {
    /// Desktop Composer and per-field mics.
    pub composer: Preset,
    /// Terminal UI.
    pub tui: Preset,
    /// Companion's own fields.
    pub companion: Preset,
    /// Phone webview.
    pub phone: Preset,
    /// Delivery into another app.
    pub any_app: Preset,
}

impl ShapePresets {
    /// The preset for `surface`.
    pub fn get(&self, surface: Surface) -> Preset {
        match surface {
            Surface::Composer => self.composer,
            Surface::Tui => self.tui,
            Surface::Companion => self.companion,
            Surface::Phone => self.phone,
            Surface::AnyApp => self.any_app,
        }
    }
}

/// The shaper for one dictation: config + the dictionary snapshot + phrases + the app row.
pub struct Shaper<'a> {
    cfg: ShapeConfig,
    phrases: &'a EndPhrases,
    dict: &'a Dictionary,
    app: Option<&'a AppProfile>,
}

impl<'a> Shaper<'a> {
    /// Resolved once per dictation at `StartDictation` (§4.6).
    pub fn new(
        cfg: ShapeConfig,
        phrases: &'a EndPhrases,
        dict: &'a Dictionary,
        app: Option<&'a AppProfile>,
    ) -> Self {
        Self {
            cfg,
            phrases,
            dict,
            app,
        }
    }

    /// `prev`: the surface's text before this utterance (context for the first token); `raw`: the model's text so far;
    /// `is_final`: the VAD closed it. Deterministic; `shape(.., raw, true)` is the text delivered.
    pub fn shape(&self, prev: &str, raw: &str, is_final: bool) -> Shaped {
        // 153-C stub — A1 fills (§4.2 stages).
        let _ = (prev, is_final, self.cfg, self.phrases, self.dict, self.app);
        Shaped {
            text: raw.to_owned(),
            command: None,
            actions: Vec::new(),
            replaced: Vec::new(),
        }
    }
}
