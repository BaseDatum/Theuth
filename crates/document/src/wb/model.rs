//! The whiteboard Y.Doc schema v1 (contract §2), the Rust twin of `canvas/model.ts`.
//!
//! Reading is tolerant: an entry this reader cannot parse is skipped (never deleted), unknown keys
//! are ignored, numbers are read from every `Any::Number` form (JS writes integers past 2³¹ as
//! float64). Writing sets one key per field on the nested maps, so keys this engine does not know
//! survive every write.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use yrs::types::map::MapPrelim;
use yrs::{Any, Array, ArrayPrelim, ArrayRef, Doc, In, Map, MapRef, Out, ReadTxn, Transact, TransactionMut};

pub use super::pack;

/// The seven top-level maps, exactly these names.
pub const MAPS: [&str; 7] = ["meta", "cards", "links", "frames", "ink", "marks", "aliases"];
/// Schema version written to `meta.v`.
pub const SCHEMA: u32 = 1;

/// What a card shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// An `.html` artifact.
    Page,
    /// json-render, OpenUI or Mermaid code.
    Diagram,
    /// A note document.
    Note,
    /// Lines of a file.
    Excerpt,
    /// A diff.
    Diff,
    /// A terminal job.
    Terminal,
    /// A web page.
    Web,
    /// An image.
    Image,
    /// A sticky note (markdown ≤ 280 chars).
    Sticky,
    /// A session.
    Session,
    /// A live app view: a route of the app (`/tasks/graph?project=…`) rendered inside the card.
    View,
}

impl CardKind {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Page => "page",
            Self::Diagram => "diagram",
            Self::Note => "note",
            Self::Excerpt => "excerpt",
            Self::Diff => "diff",
            Self::Terminal => "terminal",
            Self::Web => "web",
            Self::Image => "image",
            Self::Sticky => "sticky",
            Self::Session => "session",
            Self::View => "view",
        }
    }

    /// Kinds whose single version is replaced in place (no `vK/K` in the outline).
    pub fn in_place(self) -> bool {
        matches!(self, Self::Sticky | Self::Note | Self::Diagram)
    }

    fn parse(s: &str) -> Option<Self> {
        serde_json::from_value(Value::String(s.to_owned())).ok()
    }
}

/// `diagram` languages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub enum DiagramLang {
    /// json-render spec.
    #[serde(rename = "json-render")]
    JsonRender,
    /// OpenUI.
    #[serde(rename = "openui")]
    OpenUi,
    /// Mermaid.
    #[serde(rename = "mermaid")]
    Mermaid,
}

impl DiagramLang {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::JsonRender => "json-render",
            Self::OpenUi => "openui",
            Self::Mermaid => "mermaid",
        }
    }
}

/// A frozen artifact reference (`{ session, call_id }`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArtifactRef {
    /// Session id.
    pub session: String,
    /// The call that produced it.
    pub call_id: String,
}

/// What a version of a card shows; `t` equals the card's kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Source {
    /// An `.html` artifact version; `session` defaults to the calling session in tools.
    Page {
        /// Session that made it (defaults to the caller).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// The tool call that wrote the artifact.
        call_id: String,
    },
    /// Diagram code (≤ 24 000 chars).
    Diagram {
        /// Language.
        lang: DiagramLang,
        /// The code.
        code: String,
    },
    /// A note document.
    Note {
        /// Its DocId.
        doc: String,
    },
    /// File lines (text ≤ 16 000 chars).
    Excerpt {
        /// Path.
        path: String,
        /// First line (1-based).
        start: u32,
        /// Last line.
        end: u32,
        /// The text.
        text: String,
        /// Language for highlighting.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<String>,
    },
    /// A diff (212 spec, opaque).
    Diff {
        /// Session (defaults to the caller).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// The diff spec.
        spec: Value,
        /// Frozen copy of a working-tree diff.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        frozen: Option<ArtifactRef>,
        /// When the working tree was read (ms).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        as_of: Option<i64>,
    },
    /// A terminal job.
    Terminal {
        /// Session (defaults to the caller).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// Job id.
        job: String,
    },
    /// A web page.
    Web {
        /// URL.
        url: String,
    },
    /// An image: an artifact (`session`+`call_id`), a `path`, or a stored `blob` (`img/<ulid>`).
    Image {
        /// Session of the artifact.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<String>,
        /// Call of the artifact.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        call_id: Option<String>,
        /// File path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<String>,
        /// Blob key.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        blob: Option<String>,
    },
    /// Sticky text (markdown ≤ 280 chars).
    Sticky {
        /// Text.
        text: String,
    },
    /// A session.
    Session {
        /// Session id.
        session: String,
    },
    /// A live app view: one of the app's view routes (`/tasks/graph?project=…`, `/tasks`, `/agents`), rendered
    /// live and interactive inside the card.
    View {
        /// The route path, with its query.
        path: String,
    },
}

impl Source {
    /// The card kind this source belongs to.
    pub fn kind(&self) -> CardKind {
        match self {
            Self::Page { .. } => CardKind::Page,
            Self::Diagram { .. } => CardKind::Diagram,
            Self::Note { .. } => CardKind::Note,
            Self::Excerpt { .. } => CardKind::Excerpt,
            Self::Diff { .. } => CardKind::Diff,
            Self::Terminal { .. } => CardKind::Terminal,
            Self::Web { .. } => CardKind::Web,
            Self::Image { .. } => CardKind::Image,
            Self::Sticky { .. } => CardKind::Sticky,
            Self::Session { .. } => CardKind::Session,
            Self::View { .. } => CardKind::View,
        }
    }
}

/// One version of a card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Version {
    /// What it shows.
    pub source: Source,
    /// Written at (ms).
    pub at: i64,
    /// `owner` | `session:<uuid>`.
    pub by: String,
    /// What changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// A `page` card's device viewport.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct DeviceViewport {
    /// CSS px.
    pub w: f64,
    /// CSS px.
    pub h: f64,
    /// `phone` | `tablet` | `desktop` | `auto` | `custom`.
    pub preset: String,
}

/// Device presets (CSS px).
pub fn preset_size(preset: &str) -> Option<(f64, f64)> {
    match preset {
        "phone" => Some((390.0, 844.0)),
        "tablet" => Some((820.0, 1180.0)),
        "desktop" => Some((1280.0, 800.0)),
        _ => None,
    }
}

/// A card's life on the canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CardState {
    /// Shown.
    #[default]
    Live,
    /// Shown at 40 % with its `tag`.
    Dimmed,
    /// Hidden, restorable.
    Archived,
}

impl CardState {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Dimmed => "dimmed",
            Self::Archived => "archived",
        }
    }
}

/// The newest stored snapshot of a card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Snap {
    /// Version it shows.
    pub v: u32,
    /// Captured at (ms).
    pub at: i64,
    /// `light` | `dark`.
    pub theme: String,
}

/// A card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Card {
    /// ULID.
    pub id: String,
    /// Kind (immutable).
    pub kind: CardKind,
    /// World x (top-left, absolute).
    pub x: f64,
    /// World y.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
    /// Stacking.
    pub z: i64,
    /// Title (≤ 120).
    pub title: String,
    /// Versions, oldest first; the current one is the last.
    pub versions: Vec<Version>,
    /// `page` viewport.
    pub viewport: Option<DeviceViewport>,
    /// Life.
    pub state: CardState,
    /// Reason under a dimmed card.
    pub tag: Option<String>,
    /// Frame membership.
    pub frame: Option<String>,
    /// Placeholder label.
    pub placeholder: Option<String>,
    /// Newest stored snapshot.
    pub snap: Option<Snap>,
    /// Pinned live.
    pub pin: bool,
    /// Who placed it.
    pub by: String,
}

impl Card {
    /// The box `[x, y, w, h]`.
    pub fn rect(&self) -> [f64; 4] {
        [self.x, self.y, self.w, self.h]
    }
}

/// A link endpoint side.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Port {
    /// Top.
    Top,
    /// Right.
    Right,
    /// Bottom.
    Bottom,
    /// Left.
    Left,
}

/// Link line style.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkStyle {
    /// Solid.
    #[default]
    Solid,
    /// Dashed.
    Dashed,
}

/// Link arrows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Arrow {
    /// At `to`.
    #[default]
    End,
    /// Both ends.
    Both,
    /// None.
    None,
}

impl Arrow {
    /// The outline glyph.
    pub fn glyph(self) -> &'static str {
        match self {
            Self::End => "→",
            Self::Both => "↔",
            Self::None => "—",
        }
    }
}

/// A link between two cards.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Link {
    /// ULID.
    pub id: String,
    /// Card.
    pub from: String,
    /// Card.
    pub to: String,
    /// Side at `from`.
    pub from_port: Option<Port>,
    /// Side at `to`.
    pub to_port: Option<Port>,
    /// Label (≤ 80).
    pub label: Option<String>,
    /// Style.
    pub style: LinkStyle,
    /// Arrows.
    pub arrow: Arrow,
    /// Who drew it.
    pub by: String,
}

/// A frame.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Frame {
    /// ULID.
    pub id: String,
    /// Title (≤ 80).
    pub title: String,
    /// World x.
    pub x: f64,
    /// World y.
    pub y: f64,
    /// Width.
    pub w: f64,
    /// Height.
    pub h: f64,
    /// Theme colour token.
    pub colour: String,
    /// `phone` | `tablet` | `desktop`.
    pub preset: Option<String>,
    /// Who made it.
    pub by: String,
}

impl Frame {
    /// The box `[x, y, w, h]`.
    pub fn rect(&self) -> [f64; 4] {
        [self.x, self.y, self.w, self.h]
    }
}

/// A stroke without its points: the engine needs boxes (placement, bake), never the path.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct StrokeInfo {
    /// ULID.
    pub id: String,
    /// Points.
    pub n: u32,
    /// Packed bytes.
    pub bytes: u32,
    /// Bounding box in the stroke's space.
    pub bb: [f64; 4],
    /// Anchor card.
    pub card: Option<String>,
    /// Version marked.
    pub ver: Option<u32>,
    /// Commit time.
    pub at: i64,
}

/// A mark's state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MarkState {
    /// Sent, not yet dealt with.
    Sent,
    /// Dealt with.
    Answered,
    /// Kept by the owner.
    Pinned,
}

impl MarkState {
    /// The wire string.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sent => "sent",
            Self::Answered => "answered",
            Self::Pinned => "pinned",
        }
    }
}

/// A mark's caption source (already sanitised by the client).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Region {
    /// `page` | `lines` | `hunk` | `text`.
    pub kind: String,
    /// Caption text (≤ 120, no newlines, no `[ ] < >`).
    pub label: String,
    /// CSS selector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selector: Option<String>,
    /// ARIA role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// Rect in the card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rect: Option<[f64; 4]>,
    /// Line span.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<[u32; 2]>,
}

/// A cluster of strokes a send carried.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Mark {
    /// ULID.
    pub id: String,
    /// Stroke ids.
    pub strokes: Vec<String>,
    /// Anchor card (absent = canvas).
    pub card: Option<String>,
    /// Version marked.
    pub ver: Option<u32>,
    /// Union box.
    pub bb: [f64; 4],
    /// State.
    pub state: MarkState,
    /// Caption source.
    pub region: Option<Region>,
    /// Handwriting text.
    pub hw: Option<String>,
    /// Sent at (ms).
    pub sent_at: i64,
    /// Image stored.
    pub blob: bool,
    /// Strokes baked away.
    pub baked: bool,
}

/// `meta.focus` ([stretch P4]).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Focus {
    /// Target id.
    pub target: String,
    /// Dim everything else.
    pub spotlight: bool,
    /// When (ms).
    pub at: i64,
    /// Who.
    pub by: String,
}

/// The `meta` map.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Meta {
    /// Schema version.
    pub v: u32,
    /// Created (ms).
    pub created: i64,
    /// Background.
    pub bg: String,
    /// Follow target.
    pub focus: Option<Focus>,
}

/// A plain read of a whiteboard; every `Vec` sorted by id.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    /// Cards.
    pub cards: Vec<Card>,
    /// Links.
    pub links: Vec<Link>,
    /// Frames.
    pub frames: Vec<Frame>,
    /// Strokes (boxes only).
    pub strokes: Vec<StrokeInfo>,
    /// Marks.
    pub marks: Vec<Mark>,
    /// ULID → alias.
    pub aliases: BTreeMap<String, String>,
}

impl Snapshot {
    /// The card `id`.
    pub fn card(&self, id: &str) -> Option<&Card> {
        self.cards.binary_search_by(|c| c.id.as_str().cmp(id)).ok().map(|i| &self.cards[i])
    }

    /// The frame `id`.
    pub fn frame(&self, id: &str) -> Option<&Frame> {
        self.frames.iter().find(|f| f.id == id)
    }

    /// The mark `id`.
    pub fn mark(&self, id: &str) -> Option<&Mark> {
        self.marks.iter().find(|m| m.id == id)
    }

    /// The link `id`.
    pub fn link(&self, id: &str) -> Option<&Link> {
        self.links.iter().find(|l| l.id == id)
    }

    /// The alias of `id`, or `id` itself.
    pub fn name<'a>(&'a self, id: &'a str) -> &'a str {
        self.aliases.get(id).map(String::as_str).unwrap_or(id)
    }

    /// Resolve an alias (`c3`) or a full ULID to the ULID; `None` when neither names an entry.
    pub fn resolve(&self, s: &str) -> Option<String> {
        let s = s.trim();
        if let Some((id, _)) = self.aliases.iter().find(|(_, a)| a.as_str() == s) {
            return Some(id.clone());
        }
        let upper = s.to_ascii_uppercase();
        let known = self.card(&upper).is_some()
            || self.frame(&upper).is_some()
            || self.mark(&upper).is_some()
            || self.link(&upper).is_some();
        known.then_some(upper)
    }
}

// ---------------------------------------------------------------- reading

/// `Any` → JSON; a float with an integral value in the safe range becomes an integer (JS writes
/// integers past 2³¹ as float64), a buffer becomes its length (strokes are read by hand).
pub fn any_to_json(a: &Any) -> Value {
    match a {
        Any::Null | Any::Undefined => Value::Null,
        Any::Bool(b) => Value::Bool(*b),
        Any::Number(n) => num_json(n.as_f64().or_else(|| n.as_i64().map(|i| i as f64)).unwrap_or(0.0)),
        Any::String(s) => Value::String(s.to_string()),
        Any::Buffer(b) => Value::from(b.len()),
        Any::Array(xs) => Value::Array(xs.iter().map(any_to_json).collect()),
        Any::Map(m) => Value::Object(m.iter().map(|(k, v)| (k.clone(), any_to_json(v))).collect()),
    }
}

fn num_json(f: f64) -> Value {
    if f.fract() == 0.0 && f.abs() <= 9_007_199_254_740_991.0 {
        Value::from(f as i64)
    } else {
        serde_json::Number::from_f64(f).map(Value::Number).unwrap_or(Value::Null)
    }
}

fn out_to_json<T: ReadTxn>(txn: &T, o: &Out) -> Value {
    match o {
        Out::Any(a) => any_to_json(a),
        Out::YMap(m) => map_to_json(txn, m),
        Out::YArray(a) => Value::Array(a.iter(txn).map(|v| out_to_json(txn, &v)).collect()),
        _ => Value::Null,
    }
}

fn map_to_json<T: ReadTxn>(txn: &T, m: &MapRef) -> Value {
    Value::Object(m.iter(txn).map(|(k, v)| (k.to_owned(), out_to_json(txn, &v))).collect())
}

fn f(v: &Value, k: &str) -> Option<f64> {
    v.get(k)?.as_f64()
}
fn s(v: &Value, k: &str) -> Option<String> {
    v.get(k)?.as_str().map(str::to_owned)
}
fn u(v: &Value, k: &str) -> Option<u32> {
    v.get(k)?.as_f64().filter(|n| *n >= 0.0).map(|n| n as u32)
}
fn i(v: &Value, k: &str) -> Option<i64> {
    v.get(k)?.as_f64().map(|n| n as i64)
}
fn boxed(v: &Value, k: &str) -> Option<[f64; 4]> {
    let a = v.get(k)?.as_array()?;
    if a.len() != 4 {
        return None;
    }
    Some([a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?, a[3].as_f64()?])
}
fn parse<T: serde::de::DeserializeOwned>(v: Option<&Value>) -> Option<T> {
    serde_json::from_value(v?.clone()).ok()
}

fn card_of(id: &str, v: &Value) -> Option<Card> {
    let kind = CardKind::parse(v.get("kind")?.as_str()?)?;
    let versions = v
        .get("versions")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(|x| parse::<Version>(Some(x))).collect())
        .unwrap_or_default();
    Some(Card {
        id: id.to_owned(),
        kind,
        x: f(v, "x")?,
        y: f(v, "y")?,
        w: f(v, "w")?,
        h: f(v, "h")?,
        z: i(v, "z").unwrap_or(0),
        title: s(v, "title").unwrap_or_default(),
        versions,
        viewport: parse(v.get("viewport")),
        state: parse(v.get("state")).unwrap_or_default(),
        tag: s(v, "tag"),
        frame: s(v, "frame"),
        placeholder: s(v, "placeholder"),
        snap: parse(v.get("snap")),
        pin: v.get("pin").and_then(Value::as_bool).unwrap_or(false),
        by: s(v, "by").unwrap_or_default(),
    })
}

fn link_of(id: &str, v: &Value) -> Option<Link> {
    Some(Link {
        id: id.to_owned(),
        from: s(v, "from")?,
        to: s(v, "to")?,
        from_port: parse(v.get("from_port")),
        to_port: parse(v.get("to_port")),
        label: s(v, "label"),
        style: parse(v.get("style")).unwrap_or_default(),
        arrow: parse(v.get("arrow")).unwrap_or_default(),
        by: s(v, "by").unwrap_or_default(),
    })
}

fn frame_of(id: &str, v: &Value) -> Option<Frame> {
    Some(Frame {
        id: id.to_owned(),
        title: s(v, "title").unwrap_or_default(),
        x: f(v, "x")?,
        y: f(v, "y")?,
        w: f(v, "w")?,
        h: f(v, "h")?,
        colour: s(v, "colour").unwrap_or_else(|| "neutral".into()),
        preset: s(v, "preset"),
        by: s(v, "by").unwrap_or_default(),
    })
}

fn stroke_of(id: &str, a: &Any) -> Option<StrokeInfo> {
    let Any::Map(m) = a else { return None };
    let bytes = match m.get("pts") {
        Some(Any::Buffer(b)) => b.len() as u32,
        _ => 0,
    };
    let v = any_to_json(a);
    Some(StrokeInfo {
        id: id.to_owned(),
        n: u(&v, "n").unwrap_or(bytes / 6),
        bytes,
        bb: boxed(&v, "bb")?,
        card: s(&v, "card"),
        ver: u(&v, "ver"),
        at: i(&v, "at").unwrap_or(0),
    })
}

fn mark_of(id: &str, v: &Value) -> Option<Mark> {
    Some(Mark {
        id: id.to_owned(),
        strokes: parse(v.get("strokes")).unwrap_or_default(),
        card: s(v, "card"),
        ver: u(v, "ver"),
        bb: boxed(v, "bb").unwrap_or_default(),
        state: parse(v.get("state"))?,
        region: parse(v.get("region")),
        hw: s(v, "hw"),
        sent_at: i(v, "sent_at").unwrap_or(0),
        blob: v.get("blob").and_then(Value::as_bool).unwrap_or(false),
        baked: v.get("baked").and_then(Value::as_bool).unwrap_or(false),
    })
}

fn entries<T: ReadTxn, R>(txn: &T, name: &str, of: impl Fn(&str, &Value) -> Option<R>) -> Vec<R> {
    let Some(m) = txn.get_map(name) else {
        return Vec::new();
    };
    let mut keyed: Vec<(String, R)> = m
        .iter(txn)
        .filter_map(|(k, o)| {
            let v = out_to_json(txn, &o);
            of(k, &v).map(|r| (k.to_owned(), r))
        })
        .collect();
    keyed.sort_by(|a, b| a.0.cmp(&b.0));
    keyed.into_iter().map(|(_, r)| r).collect()
}

/// Read the whole whiteboard; tolerant (skips what it cannot parse).
pub fn read<T: ReadTxn>(txn: &T) -> Snapshot {
    let mut strokes: Vec<StrokeInfo> = match txn.get_map("ink") {
        Some(m) => m
            .iter(txn)
            .filter_map(|(k, o)| match o {
                Out::Any(a) => stroke_of(k, &a),
                _ => None,
            })
            .collect(),
        None => Vec::new(),
    };
    strokes.sort_by(|a, b| a.id.cmp(&b.id));
    let aliases = match txn.get_map("aliases") {
        Some(m) => m
            .iter(txn)
            .filter_map(|(k, o)| match o {
                Out::Any(Any::String(s)) => Some((k.to_owned(), s.to_string())),
                _ => None,
            })
            .collect(),
        None => BTreeMap::new(),
    };
    Snapshot {
        cards: entries(txn, "cards", card_of),
        links: entries(txn, "links", link_of),
        frames: entries(txn, "frames", frame_of),
        strokes,
        marks: entries(txn, "marks", mark_of),
        aliases,
    }
}

/// Read `meta`; defaults for what is absent.
pub fn read_meta<T: ReadTxn>(txn: &T) -> Meta {
    let v = txn.get_map("meta").map(|m| map_to_json(txn, &m)).unwrap_or(Value::Null);
    Meta {
        v: u(&v, "v").unwrap_or(SCHEMA),
        created: i(&v, "created").unwrap_or(0),
        bg: s(&v, "bg").unwrap_or_else(|| "dots".into()),
        focus: parse(v.get("focus")),
    }
}

// ---------------------------------------------------------------- writing

/// JSON → `Any`; every number is written as a float (`Any::Number`).
pub fn json_to_any(v: &Value) -> Any {
    match v {
        Value::Null => Any::Null,
        Value::Bool(b) => Any::Bool(*b),
        Value::Number(n) => Any::from(n.as_f64().unwrap_or(0.0)),
        Value::String(s) => Any::from(s.as_str()),
        Value::Array(a) => Any::Array(a.iter().map(json_to_any).collect::<Vec<_>>().into()),
        Value::Object(o) => Any::Map(Arc::new(
            o.iter().map(|(k, v)| (k.clone(), json_to_any(v))).collect::<HashMap<_, _>>(),
        )),
    }
}

/// A serialisable value as a plain `Any`.
pub fn plain<T: Serialize>(t: &T) -> Any {
    json_to_any(&serde_json::to_value(t).unwrap_or(Value::Null))
}

/// One process-wide client id for the engine's own writes (each write is built on a scratch copy
/// of the document, so its clock continues from the document's state).
pub static ENGINE_CLIENT: LazyLock<u64> =
    LazyLock::new(|| u64::from(uuid::Uuid::new_v4().as_u128() as u32) | 1);

/// The seven root maps of `doc`.
pub struct Maps {
    /// `meta`.
    pub meta: MapRef,
    /// `cards`.
    pub cards: MapRef,
    /// `links`.
    pub links: MapRef,
    /// `frames`.
    pub frames: MapRef,
    /// `ink`.
    pub ink: MapRef,
    /// `marks`.
    pub marks: MapRef,
    /// `aliases`.
    pub aliases: MapRef,
}

impl Maps {
    /// Get or create the root maps (no transaction may be open on `doc`).
    pub fn of(doc: &Doc) -> Self {
        Self {
            meta: doc.get_or_insert_map("meta"),
            cards: doc.get_or_insert_map("cards"),
            links: doc.get_or_insert_map("links"),
            frames: doc.get_or_insert_map("frames"),
            ink: doc.get_or_insert_map("ink"),
            marks: doc.get_or_insert_map("marks"),
            aliases: doc.get_or_insert_map("aliases"),
        }
    }

    fn root(&self, name: &str) -> &MapRef {
        match name {
            "meta" => &self.meta,
            "cards" => &self.cards,
            "links" => &self.links,
            "frames" => &self.frames,
            "ink" => &self.ink,
            "marks" => &self.marks,
            _ => &self.aliases,
        }
    }

    /// The nested map of entry `id` in `root`, if it is one.
    pub fn entry<T: ReadTxn>(&self, txn: &T, root: &str, id: &str) -> Option<MapRef> {
        match self.root(root).get(txn, id)? {
            Out::YMap(m) => Some(m),
            _ => None,
        }
    }

    /// Set (or, with `None`, delete) one field of entry `id` in `root`. False when the entry is gone.
    pub fn set(&self, txn: &mut TransactionMut, root: &str, id: &str, key: &str, v: Option<Any>) -> bool {
        let Some(m) = self.entry(txn, root, id) else {
            return false;
        };
        match v {
            Some(v) => {
                m.insert(txn, key, v);
            }
            None => {
                m.remove(txn, key);
            }
        }
        true
    }

    /// Create entry `id` in `root` from `fields` (absent options are simply not written).
    pub fn put(&self, txn: &mut TransactionMut, root: &str, id: &str, fields: Vec<(&str, In)>) {
        let prelim: MapPrelim = fields.into_iter().collect();
        self.root(root).insert(txn, id, prelim);
    }

    /// Delete entry `id` of `root`.
    pub fn delete(&self, txn: &mut TransactionMut, root: &str, id: &str) {
        self.root(root).remove(txn, id);
    }

    /// The `versions` array of card `id`.
    pub fn versions<T: ReadTxn>(&self, txn: &T, id: &str) -> Option<ArrayRef> {
        match self.entry(txn, "cards", id)?.get(txn, "versions")? {
            Out::YArray(a) => Some(a),
            _ => None,
        }
    }

    /// Set a meta key.
    pub fn set_meta(&self, txn: &mut TransactionMut, key: &str, v: Any) {
        self.meta.insert(txn, key, v);
    }

    /// Set an alias.
    pub fn set_alias(&self, txn: &mut TransactionMut, id: &str, alias: &str) {
        self.aliases.insert(txn, id, Any::from(alias));
    }

    /// Set a whole stroke (plain value).
    pub fn set_stroke(&self, txn: &mut TransactionMut, id: &str, v: Any) {
        self.ink.insert(txn, id, v);
    }
}

fn num(x: f64) -> In {
    In::Any(Any::from(x))
}
fn text(s: &str) -> In {
    In::Any(Any::from(s))
}

/// The fields of a new card entry.
pub fn card_fields(c: &Card) -> Vec<(&'static str, In)> {
    let mut f: Vec<(&'static str, In)> = vec![
        ("kind", text(c.kind.as_str())),
        ("x", num(c.x)),
        ("y", num(c.y)),
        ("w", num(c.w)),
        ("h", num(c.h)),
        ("z", num(c.z as f64)),
        ("title", text(&c.title)),
        (
            "versions",
            In::Array(ArrayPrelim::from(c.versions.iter().map(|v| In::Any(plain(v))).collect::<Vec<_>>())),
        ),
        ("state", text(c.state.as_str())),
        ("by", text(&c.by)),
    ];
    if let Some(v) = &c.viewport {
        f.push(("viewport", In::Any(plain(v))));
    }
    if let Some(t) = &c.tag {
        f.push(("tag", text(t)));
    }
    if let Some(t) = &c.frame {
        f.push(("frame", text(t)));
    }
    if let Some(t) = &c.placeholder {
        f.push(("placeholder", text(t)));
    }
    if let Some(s) = &c.snap {
        f.push(("snap", In::Any(plain(s))));
    }
    if c.pin {
        f.push(("pin", In::Any(Any::Bool(true))));
    }
    f
}

/// The fields of a new link entry.
pub fn link_fields(l: &Link) -> Vec<(&'static str, In)> {
    let mut f: Vec<(&'static str, In)> = vec![
        ("from", text(&l.from)),
        ("to", text(&l.to)),
        ("style", In::Any(plain(&l.style))),
        ("arrow", In::Any(plain(&l.arrow))),
        ("by", text(&l.by)),
    ];
    if let Some(p) = l.from_port {
        f.push(("from_port", In::Any(plain(&p))));
    }
    if let Some(p) = l.to_port {
        f.push(("to_port", In::Any(plain(&p))));
    }
    if let Some(t) = &l.label {
        f.push(("label", text(t)));
    }
    f
}

/// The fields of a new frame entry.
pub fn frame_fields(fr: &Frame) -> Vec<(&'static str, In)> {
    let mut f: Vec<(&'static str, In)> = vec![
        ("title", text(&fr.title)),
        ("x", num(fr.x)),
        ("y", num(fr.y)),
        ("w", num(fr.w)),
        ("h", num(fr.h)),
        ("colour", text(&fr.colour)),
        ("by", text(&fr.by)),
    ];
    if let Some(p) = &fr.preset {
        f.push(("preset", text(p)));
    }
    f
}

/// The fields of a new mark entry (`by` and `to` are display fields the reader does not keep).
pub fn mark_fields(m: &Mark, by: &str) -> Vec<(&'static str, In)> {
    let mut f: Vec<(&'static str, In)> = vec![
        ("strokes", In::Any(plain(&m.strokes))),
        ("bb", In::Any(plain(&m.bb))),
        ("state", text(m.state.as_str())),
        ("sent_at", num(m.sent_at as f64)),
        ("by", text(by)),
    ];
    if let Some(c) = &m.card {
        f.push(("card", text(c)));
    }
    if let Some(v) = m.ver {
        f.push(("ver", num(v as f64)));
    }
    if let Some(r) = &m.region {
        f.push(("region", In::Any(plain(r))));
    }
    if let Some(h) = &m.hw {
        f.push(("hw", text(h)));
    }
    if m.blob {
        f.push(("blob", In::Any(Any::Bool(true))));
    }
    if m.baked {
        f.push(("baked", In::Any(Any::Bool(true))));
    }
    f
}

/// Seed `meta` of a new whiteboard (`v`, `created`, `bg`).
pub fn seed(doc: &Doc, now: i64) {
    let maps = Maps::of(doc);
    let mut txn = doc.transact_mut();
    maps.set_meta(&mut txn, "v", Any::from(SCHEMA as f64));
    maps.set_meta(&mut txn, "created", Any::from(now as f64));
    maps.set_meta(&mut txn, "bg", Any::from("dots"));
}

/// A ULID: 48-bit ms time + 80 random bits (from a v4 uuid), Crockford base32, 26 chars.
pub fn ulid() -> String {
    ulid_at(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0),
    )
}

/// [`ulid`] at `ms`.
pub fn ulid_at(ms: u64) -> String {
    const A: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";
    let rand = uuid::Uuid::new_v4().as_u128() & ((1u128 << 80) - 1);
    let v: u128 = ((ms as u128 & ((1 << 48) - 1)) << 80) | rand;
    (0..26).rev().map(|i| A[((v >> (i * 5)) & 31) as usize] as char).collect()
}

/// `^[0-9A-HJKMNP-TV-Z]{26}$`.
pub fn is_ulid(s: &str) -> bool {
    s.len() == 26 && s.bytes().all(|b| b"0123456789ABCDEFGHJKMNPQRSTVWXYZ".contains(&b))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use yrs::updates::decoder::Decode;
    use yrs::{Doc, Transact};

    pub(crate) fn card(id: &str, kind: CardKind, rect: [f64; 4], title: &str, versions: Vec<Version>) -> Card {
        Card {
            id: id.into(),
            kind,
            x: rect[0],
            y: rect[1],
            w: rect[2],
            h: rect[3],
            z: 1,
            title: title.into(),
            versions,
            viewport: None,
            state: CardState::Live,
            tag: None,
            frame: None,
            placeholder: None,
            snap: None,
            pin: false,
            by: "owner".into(),
        }
    }

    #[test]
    fn write_read_round_trip() {
        let doc = Doc::new();
        seed(&doc, 1_795_570_000_000);
        let maps = Maps::of(&doc);
        let mut c = card(
            "01KAR3ZC4M0000000000000001",
            CardKind::Page,
            [250.0, 80.0, 180.0, 390.0],
            "Mock B — flat list",
            vec![Version {
                source: Source::Page { session: Some("s".into()), call_id: "toolu_01AAA".into() },
                at: 1_795_570_100_000,
                by: "session:s".into(),
                note: Some("first".into()),
            }],
        );
        c.viewport = Some(DeviceViewport { w: 390.0, h: 844.0, preset: "phone".into() });
        c.frame = Some("01KAR3ZC4M0000000000000009".into());
        c.snap = Some(Snap { v: 1, at: 5, theme: "dark".into() });
        c.pin = true;
        c.state = CardState::Dimmed;
        c.tag = Some("why".into());
        let l = Link {
            id: "01KAR3ZC4M0000000000000005".into(),
            from: c.id.clone(),
            to: c.id.clone(),
            from_port: Some(Port::Right),
            to_port: None,
            label: Some("drives".into()),
            style: LinkStyle::Dashed,
            arrow: Arrow::Both,
            by: "owner".into(),
        };
        let fr = Frame {
            id: "01KAR3ZC4M0000000000000009".into(),
            title: "Variants".into(),
            x: 36.0,
            y: 48.0,
            w: 610.0,
            h: 424.0,
            colour: "neutral".into(),
            preset: Some("phone".into()),
            by: "owner".into(),
        };
        let m = Mark {
            id: "01KAR3ZC4M0000000000000008".into(),
            strokes: vec!["01KAR3ZC4M0000000000000007".into()],
            card: Some(c.id.clone()),
            ver: Some(1),
            bb: [98.1, 47.6, 4.8, 4.05],
            state: MarkState::Sent,
            region: Some(Region {
                kind: "page".into(),
                label: "tab bar, item 3".into(),
                selector: None,
                role: Some("tab".into()),
                rect: None,
                lines: None,
            }),
            hw: Some("no third tab".into()),
            sent_at: 1_795_570_300_000,
            blob: true,
            baked: false,
        };
        {
            let mut txn = doc.transact_mut();
            maps.put(&mut txn, "cards", &c.id, card_fields(&c));
            maps.put(&mut txn, "links", &l.id, link_fields(&l));
            maps.put(&mut txn, "frames", &fr.id, frame_fields(&fr));
            maps.put(&mut txn, "marks", &m.id, mark_fields(&m, "owner"));
            let pts = pack::encode([100.0, 50.0], &[(100.0, 50.0, 0.5, 0), (101.5, 50.25, 1.0, 8)]);
            let stroke = Any::Map(Arc::new(HashMap::from([
                ("pts".to_owned(), Any::Buffer(pts.into())),
                ("n".to_owned(), Any::from(2.0)),
                ("bb".to_owned(), plain(&[98.0, 47.0, 5.0, 5.0])),
                ("card".to_owned(), Any::from(c.id.as_str())),
                ("ver".to_owned(), Any::from(1.0)),
                ("at".to_owned(), Any::from(1_795_570_200_000.0)),
            ])));
            maps.set_stroke(&mut txn, "01KAR3ZC4M0000000000000007", stroke);
            maps.set_alias(&mut txn, &c.id, "c1");
        }
        let snap = read(&doc.transact());
        assert_eq!(snap.cards, vec![c]);
        assert_eq!(snap.links, vec![l]);
        assert_eq!(snap.frames, vec![fr]);
        assert_eq!(snap.marks, vec![m]);
        assert_eq!(snap.strokes.len(), 1);
        assert_eq!(snap.strokes[0].bytes, 12);
        assert_eq!(snap.strokes[0].n, 2);
        assert_eq!(snap.strokes[0].ver, Some(1));
        assert_eq!(snap.strokes[0].at, 1_795_570_200_000);
        assert_eq!(snap.aliases.get("01KAR3ZC4M0000000000000001").map(String::as_str), Some("c1"));
        let meta = read_meta(&doc.transact());
        assert_eq!((meta.v, meta.created, meta.bg.as_str()), (1, 1_795_570_000_000, "dots"));
    }

    #[test]
    fn tolerates_unknown_fields() {
        let doc = Doc::new();
        let maps = Maps::of(&doc);
        {
            let mut txn = doc.transact_mut();
            let mut good = card_fields(&card("A", CardKind::Sticky, [0.0, 0.0, 10.0, 10.0], "ok", vec![]));
            good.push(("future", text("kept")));
            maps.put(&mut txn, "cards", "A", good);
            // An unknown kind: skipped on read, never deleted.
            let mut odd = card_fields(&card("B", CardKind::Sticky, [0.0, 0.0, 10.0, 10.0], "odd", vec![]));
            odd[0] = ("kind", text("hologram"));
            maps.put(&mut txn, "cards", "B", odd);
            // An unknown version source: that version is skipped, the card stays.
            maps.put(&mut txn, "cards", "C", card_fields(&card("C", CardKind::Web, [0.0, 0.0, 1.0, 1.0], "w", vec![])));
            let v = maps.versions(&txn, "C").unwrap();
            v.push_back(&mut txn, plain(&serde_json::json!({"source": {"t": "web", "url": "https://x"}, "at": 1, "by": "owner"})));
            v.push_back(&mut txn, plain(&serde_json::json!({"source": {"t": "teleport"}, "at": 2, "by": "owner"})));
            maps.set_meta(&mut txn, "mystery", Any::from(3.0));
        }
        let snap = read(&doc.transact());
        assert_eq!(snap.cards.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), ["A", "C"]);
        assert_eq!(snap.cards[1].versions.len(), 1);
        // A field write leaves the unknown key alone.
        {
            let mut txn = doc.transact_mut();
            maps.set(&mut txn, "cards", "A", "title", Some(Any::from("renamed")));
        }
        let txn = doc.transact();
        let a = maps.entry(&txn, "cards", "A").unwrap();
        assert_eq!(a.get(&txn, "future"), Some(Out::Any(Any::from("kept"))));
        assert!(maps.entry(&txn, "cards", "B").is_some());
    }

    #[test]
    fn reads_bigint_numbers() {
        let doc = Doc::new();
        let maps = Maps::of(&doc);
        {
            let mut txn = doc.transact_mut();
            let mut f = card_fields(&card("A", CardKind::Sticky, [0.0, 0.0, 10.0, 10.0], "t", vec![]));
            f[1] = ("x", In::Any(Any::Number(yrs::any::Number::Int(120))));
            f[5] = ("z", In::Any(Any::Number(yrs::any::Number::Int(7))));
            maps.put(&mut txn, "cards", "A", f);
            maps.put(&mut txn, "marks", "M", vec![
                ("strokes", In::Any(plain(&Vec::<String>::new()))),
                ("bb", In::Any(plain(&[0.0, 0.0, 1.0, 1.0]))),
                ("state", text("sent")),
                ("sent_at", In::Any(Any::Number(yrs::any::Number::Int(1_795_570_300_000)))),
                ("ver", In::Any(Any::Number(yrs::any::Number::Int(2)))),
            ]);
        }
        // Through the wire (BigInt/varint encodings) and back.
        let update = doc.transact().encode_state_as_update_v1(&Default::default());
        let other = Doc::new();
        other
            .transact_mut()
            .apply_update(yrs::Update::decode_v1(&update).unwrap())
            .unwrap();
        let snap = read(&other.transact());
        assert_eq!((snap.cards[0].x, snap.cards[0].z), (120.0, 7));
        assert_eq!((snap.marks[0].sent_at, snap.marks[0].ver), (1_795_570_300_000, Some(2)));
    }

    #[test]
    fn ulids_are_well_formed_and_ordered() {
        let a = ulid_at(1_000);
        let b = ulid_at(2_000);
        assert!(is_ulid(&a) && is_ulid(&b));
        assert!(a < b);
    }
}
