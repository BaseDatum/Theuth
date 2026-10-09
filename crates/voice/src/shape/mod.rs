//! The shaping layer (153 §4): a pure module — no I/O, no clock, no actor —
//! the dictation task calls once per piece. Spoken punctuation, literals,
//! numbers, the dictionary, spoken end commands (§7) and terminal spacing,
//! per-surface switches from `voice.shape.<surface>` (§4.5).
//!
//! Stages in §4.2 order over one token list (`tokens.rs`), each a function the
//! config switch skips; the end parser (`end.rs`) is owned by the actor per
//! utterance and fed through [`Shaper::shape_with`].

use serde::{Deserialize, Serialize};

use crate::dictionary::{Dictionary, Kind};
use crate::{AppProfile, Surface};

pub mod end;

mod dictionary_stage;
mod literals;
mod numbers;
mod punctuation;
mod tokens;

pub use end::{EndParser, NEAR_MISS_MIN_LEN, SETTLE_MS, SILENCE_MS};

pub(crate) use dictionary_stage::Lookup;

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
    surface: Surface,
    commands: Vec<String>,
    mentions: Vec<String>,
    tlds: Vec<String>,
    exts: Vec<String>,
}

impl<'a> Shaper<'a> {
    /// Resolved once per dictation at `StartDictation` (§4.6). TLD / extension
    /// terms are the dictionary's `Path` pairs tagged `tld` / `ext` (packaged + user +
    /// learned) on top of the built-in lists; mention terms are its `mention`-tagged
    /// user/learned entries.
    pub fn new(
        cfg: ShapeConfig,
        phrases: &'a EndPhrases,
        dict: &'a Dictionary,
        app: Option<&'a AppProfile>,
    ) -> Self {
        let tagged = |tag: &str| -> Vec<String> {
            dict.list(None)
                .into_iter()
                .filter(|e| e.tags.iter().any(|t| t == tag))
                .map(|e| e.canonical)
                .collect()
        };
        let paths = |tag: &str| -> Vec<String> {
            dict.pairs(Kind::Path)
                .filter(|e| e.tags.iter().any(|t| t == tag))
                .map(|e| e.canonical.trim_start_matches('.').to_string())
                .collect()
        };
        Self {
            cfg,
            phrases,
            dict,
            app,
            surface: Surface::Composer,
            commands: app.map(|a| a.commands.clone()).unwrap_or_default(),
            mentions: tagged("mention"),
            tlds: paths("tld"),
            exts: paths("ext"),
        }
    }

    /// The dictating surface (`DictationOpts.surface`; terminal spacing applies to `Tui`).
    pub fn with_surface(mut self, surface: Surface) -> Self {
        self.surface = surface;
        self
    }

    /// Slash commands the surface accepts (`DictationOpts.commands`), added to the app row's.
    pub fn with_commands(mut self, commands: impl IntoIterator<Item = String>) -> Self {
        self.commands.extend(commands);
        self
    }

    /// `@mention` names (`agents_list`), added to the dictionary's `mention`-tagged terms.
    pub fn with_mentions(mut self, names: impl IntoIterator<Item = String>) -> Self {
        self.mentions.extend(names);
        self
    }

    /// `prev`: the surface's text before this utterance (context for the first token); `raw`: the model's text so far;
    /// `is_final`: the VAD closed it. Deterministic; `shape(.., raw, true)` is the text delivered.
    ///
    /// Stateless: a fresh [`EndParser`], so only an exact end phrase is seen. The actor
    /// uses [`Shaper::shape_with`] with its per-utterance parser for arming and near misses.
    pub fn shape(&self, prev: &str, raw: &str, is_final: bool) -> Shaped {
        self.shape_with(&mut EndParser::new(self.phrases), prev, raw, is_final)
    }

    /// [`Shaper::shape`] with the utterance's end-command parser (§7.1 state across pieces).
    pub fn shape_with(
        &self,
        parser: &mut EndParser,
        prev: &str,
        raw: &str,
        is_final: bool,
    ) -> Shaped {
        self.ctx().run(parser, prev, raw, is_final)
    }

    fn ctx(&self) -> Ctx<'_> {
        Ctx {
            cfg: self.cfg,
            phrases: self.phrases,
            dict: self.dict,
            terminal: self.surface == Surface::Tui || self.app.is_some_and(|a| a.terminal),
            commands: &self.commands,
            mentions: &self.mentions,
            tlds: &self.tlds,
            exts: &self.exts,
        }
    }
}

/// Everything the stages read; tests build one directly (with a fake [`Lookup`]).
pub(crate) struct Ctx<'a> {
    pub(crate) cfg: ShapeConfig,
    pub(crate) phrases: &'a EndPhrases,
    pub(crate) dict: &'a dyn Lookup,
    pub(crate) terminal: bool,
    pub(crate) commands: &'a [String],
    pub(crate) mentions: &'a [String],
    pub(crate) tlds: &'a [String],
    pub(crate) exts: &'a [String],
}

// The packaged `Path` pairs (§5.2) are the talon lists: 6 TLDs, 35 extensions — no `dev`,
// `io`, `rs`, `ts`. Table rows 5/7 (`theuth.dev`, `main.rs`) need these on top of them.
const TLDS: &[&str] = &[
    "com", "org", "net", "io", "dev", "ai", "co", "app", "sh", "me", "us", "uk", "de", "fr", "eu",
    "edu", "gov", "info", "xyz", "cloud", "tech", "rs", "ch", "nl", "se", "no", "jp", "au", "ca",
];
const EXTS: &[&str] = &[
    "sh", "rs", "py", "js", "ts", "tsx", "jsx", "mjs", "md", "json", "yaml", "yml", "toml", "txt",
    "html", "css", "go", "java", "c", "h", "cpp", "hpp", "cs", "swift", "kt", "rb", "php", "sql",
    "csv", "png", "jpg", "jpeg", "gif", "svg", "pdf", "zip", "gz", "tar", "lock", "env", "log",
    "exe", "dll", "so", "dylib", "wasm", "xml", "ini", "cfg",
];

fn has(list: &[&str], extra: &[String], w: &str) -> bool {
    list.iter().any(|x| x.eq_ignore_ascii_case(w))
        || extra.iter().any(|x| x.eq_ignore_ascii_case(w))
}

impl Ctx<'_> {
    fn is_tld(&self, w: &str) -> bool {
        has(TLDS, self.tlds, w)
    }
    fn is_ext(&self, w: &str) -> bool {
        has(EXTS, self.exts, w)
    }
    fn is_command(&self, w: &str) -> bool {
        has(&[], self.commands, w)
    }
    fn is_mention(&self, w: &str) -> bool {
        has(&[], self.mentions, w)
    }
    fn number_word(&self, w: &str) -> bool {
        numbers::is_number_word(w)
    }

    /// The §4.2 stages in order; every switch skips its stage.
    pub(crate) fn run(
        &self,
        parser: &mut EndParser,
        prev: &str,
        raw: &str,
        is_final: bool,
    ) -> Shaped {
        let cfg = self.cfg;
        let mut toks = tokens::tokenize(raw);
        if cfg.punctuation || cfg.literals || cfg.end_commands {
            punctuation::literal_escape(&mut toks, self);
        }
        if cfg.punctuation {
            punctuation::spoken(&mut toks);
            punctuation::spelled_letters(&mut toks);
            punctuation::gates(&mut toks, self, is_final);
        }
        if cfg.literals {
            literals::literals(&mut toks, self);
        }
        if cfg.numbers {
            numbers::numbers(&mut toks);
        }
        if cfg.punctuation {
            punctuation::cleanup(&mut toks);
        }
        if cfg.dictionary {
            dictionary_stage::dictionary(&mut toks, self.dict);
        }
        let mut command = None;
        if cfg.end_commands {
            let feed = toks
                .iter()
                .filter(|t| matches!(t.kind, tokens::Kind::Word | tokens::Kind::Hidden))
                .map(|t| t.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            command = parser.feed(&feed, is_final);
            let mut n = parser.consumed();
            if command.is_some() && n > 0 {
                // Drop from the phrase's first word to the end, then a trailing model mark.
                let mut cut = toks.len();
                while n > 0 && cut > 0 {
                    cut -= 1;
                    if matches!(toks[cut].kind, tokens::Kind::Word | tokens::Kind::Hidden) {
                        n -= 1;
                    }
                }
                toks.truncate(cut);
                while toks.last().is_some_and(|t| t.asr) {
                    toks.pop();
                }
            }
        }
        let (mut text, actions, replaced) = tokens::render(&toks, prev);
        if cfg.terminal_spacing && self.terminal && is_final {
            let last = text.rsplit(char::is_whitespace).next().unwrap_or("");
            if last.starts_with(['/', '@']) {
                text.truncate(text.trim_end().len());
            }
        }
        Shaped {
            text,
            command,
            actions,
            replaced,
        }
    }
}

#[cfg(test)]
mod tests;
