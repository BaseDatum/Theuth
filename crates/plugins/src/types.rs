//! Public types of the plugins host (contract §5.1). B0, frozen.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tools::Risk;

/// Lifecycle of one plugin (216 §5).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PluginRunState {
    /// The manifest does not parse or validate.
    Invalid,
    /// No verified grant covers the manifest (and code): a card waits for a human press.
    AwaitingApproval,
    /// Spawned, waiting for `hello`.
    Starting,
    /// Answered `hello`; events flow.
    Running,
    /// Crashed; restarting after a delay.
    Backoff,
    /// Crash loop: not restarted until `plugin_restart` or a change.
    Failed,
    /// Exited with status 0 and stays down.
    Exited,
    /// Being stopped (SIGTERM, then SIGKILL after the grace).
    Stopping,
    /// The grant's `enabled` is false.
    Disabled,
}

/// A placement's status dot.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Dot {
    /// Green.
    Ok,
    /// Green, pulsing.
    Busy,
    /// Amber.
    Warn,
    /// Red.
    Error,
    /// Grey.
    #[default]
    Off,
    /// No dot drawn.
    None,
}

/// A badge: a count (0 hides it) or up to 4 characters.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(untagged)]
pub enum Badge {
    /// A count; 0 hides the badge.
    Count(u64),
    /// Up to 4 characters.
    Text(String),
}

/// v1 placement kinds (216 §9.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum PlacementKind {
    /// An icon in the sidebar rail / expanded sidebar.
    Sidebar,
    /// A segment in the status strip.
    Status,
    /// A tile beside a chat.
    Tile,
    /// A `/command` in the composer.
    Command,
}

/// What a sidebar icon opens.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum OpenMode {
    /// A popover beside the icon.
    Popover,
    /// A tile beside the focused chat.
    Tile,
}

/// Content format of a placement or a command card.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "kebab-case")]
pub enum SurfaceFormat {
    /// OpenUI source (`openui`).
    Openui,
    /// A json-render spec (`json-render`).
    JsonRender,
    /// Markdown, raw HTML off (`markdown`).
    Markdown,
}

/// What a surface without a renderer shows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Fallback {
    /// The plugin's text fallback.
    #[default]
    Text,
    /// Nothing.
    Hide,
}

/// Default size of a popover or tile, CSS px.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Size {
    /// Width, CSS px.
    pub width: u32,
    /// Height, CSS px.
    pub height: u32,
}

/// A `command` placement as clients list it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CommandInfo {
    /// As declared (`name:`).
    pub name: String,
    /// What the composer inserts: `/<plugin>` when `name == plugin`, else `/<plugin>:<name>`.
    pub slash: String,
    /// One line for the slash menu.
    pub description: String,
    /// Argument hint (`args:`), shown after the command.
    pub args: Option<String>,
}

/// One declared placement, as approved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PlacementInfo {
    /// The placement id (unique within the plugin).
    pub id: String,
    /// Where it is drawn.
    pub kind: PlacementKind,
    /// `label:` (tooltip / menu text / tile title).
    pub label: String,
    /// A lucide icon name (kebab-case, e.g. `gauge`), or `svg:<data-url>` for a sanitised SVG from the folder.
    pub icon: String,
    /// Draws a status dot fed by `status`.
    pub status: bool,
    /// Sidebar only.
    pub open: Option<OpenMode>,
    /// Default popover / tile size.
    pub size: Option<Size>,
    /// Absent for `status` and `command`.
    pub format: Option<SurfaceFormat>,
    /// What a client without a renderer for `format` shows.
    pub fallback: Fallback,
    /// The owner hid it (Settings › Plugins / context menu); hidden placements are not drawn, commands still work.
    pub hidden: bool,
    /// `command` placements only.
    pub command: Option<CommandInfo>,
}

/// One row of Settings › Plugins and `plugin_list`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PluginInfo {
    /// Folder name = plugin name.
    pub name: String,
    /// `version:` (free text); empty when the manifest does not parse.
    pub version: String,
    /// `description:`; empty when the manifest does not parse.
    pub description: String,
    /// Lifecycle state.
    pub state: PluginRunState,
    /// Last error: manifest error (`invalid`), crash reason, stderr's last line.
    pub error: Option<String>,
    /// The grant's `enabled` (false when there is no grant yet).
    pub enabled: bool,
    /// A card waits for a press (install, widen or code-changed), whatever the state.
    pub needs_approval: bool,
    /// Attention item id of that card (`plugin:<name>:<m8>:<c8>:<seq>`).
    pub card: Option<String>,
    /// Approved placements (all declared ones while awaiting the first approval).
    pub placements: Vec<PlacementInfo>,
    /// Names of the tools it offers the model.
    pub tools: Vec<String>,
    /// Absolute folder path.
    pub dir: String,
    /// Absolute path of today's log file.
    pub log: String,
    /// The grant's `approved_at` (RFC 3339), none before the first approval.
    pub approved_at: Option<String>,
    /// Position in the sidebar/status order (0 = first).
    pub order: u32,
}

/// One plain-words line of an install card (216 §10.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct GrantLine {
    /// Which manifest entry the line stands for.
    pub key: GrantKey,
    /// The card wording, e.g. "Reads agent.events: TurnFinished — a turn finished; …".
    pub text: String,
    /// The line's risk.
    pub risk: Risk,
    /// On a widen card: this line is new since the last approval.
    pub new: bool,
}

/// Which manifest entry a [`GrantLine`] stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum GrantKey {
    /// `run` / `cwd` / `env`: the command line.
    Run,
    /// A vault secret referenced by `env`.
    Secret,
    /// A `subscribe` entry.
    Subscribe,
    /// An `emits` entry.
    Emit,
    /// A placement.
    Placement,
    /// A tool offered to the model.
    Tool,
    /// The `call` capability.
    Call,
    /// The `input` capability.
    Input,
    /// The `notify` capability.
    Notify,
}

/// Which card it is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// No grant yet.
    Install,
    /// The manifest asks for more than the grant holds.
    Widen,
    /// The code hash changed.
    CodeChanged,
}

/// A pending card, as the install screen and `t plugin approve` show it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct InstallCard {
    /// Attention item id `plugin:<name>:<m8>:<c8>:<seq>` (m8/c8: first 8 hex of the hashes, `00000000` for no code hash; seq: 0, bumped by each `RequestApproval`).
    pub id: String,
    /// Plugin name.
    pub plugin: String,
    /// The manifest's `version:`.
    pub version: String,
    /// Install, widen or code-changed.
    pub kind: CardKind,
    /// Pass back verbatim to `plugin_approve`.
    pub manifest_sha256: String,
    /// Pass back verbatim to `plugin_approve`; `None` when the grant will carry no code hash.
    pub code_sha256: Option<String>,
    /// The highest line's risk (always at least `destructive`, because `run` is).
    pub risk: Risk,
    /// Install: every line. Widen: the new lines, plus "Runs …" when run/cwd/env changed.
    pub lines: Vec<GrantLine>,
    /// Code-changed: the relative paths whose bytes changed.
    pub changed_paths: Vec<String>,
    /// The saved approval did not verify ("the saved approval for <name> could not be verified").
    pub unverified: bool,
    /// Absolute folder path.
    pub dir: String,
    /// When the card was filed (ms since epoch).
    pub at_ms: u64,
}

/// What a plugin may do (grants.yaml `grants:`, `hello.params.grants` minus run/cwd/env/secrets).
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Grants {
    /// Catalog names: `topic`, `topic#type`, `<plugin>.<event>`, `<plugin>.*`.
    pub subscribe: Vec<String>,
    /// Declared event names it may emit.
    pub emits: Vec<String>,
    /// Placement ids.
    pub placements: Vec<String>,
    /// Tools it offers the model, with their approved risk.
    pub tools: Vec<ToolGrant>,
    /// Host-brokered powers.
    pub capabilities: Capabilities,
    /// Vault secret names referenced by `env`.
    pub secrets: Vec<String>,
    /// As approved, unexpanded (a change is widening).
    pub run: Vec<String>,
    /// Working directory as approved, relative to the folder.
    pub cwd: String,
    /// Environment as approved, unexpanded.
    pub env: BTreeMap<String, String>,
}

/// A manifest tool and the risk it was approved at.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ToolGrant {
    /// Tool name.
    pub name: String,
    /// Approved risk.
    pub risk: Risk,
}

/// Host-brokered powers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Capabilities {
    /// May raise inbox items and OS notifications.
    pub notify: bool,
    /// May send input to a session.
    pub input: bool,
    /// May call tools in a headless session.
    pub call: Option<CallGrant>,
}

/// `call: {tools, max_risk}`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CallGrant {
    /// Tool names it may call.
    pub tools: Vec<String>,
    /// Highest risk of a call.
    pub max_risk: Risk,
}

/// Everything about one plugin (`plugin_get`, `plugin_status` tool).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct PluginDetail {
    /// The list row.
    pub info: PluginInfo,
    /// The parsed manifest as JSON (env values unexpanded); `null` when it does not parse.
    pub manifest: Value,
    /// What the manifest asks for now.
    pub requested: Grants,
    /// What the verified grant holds; `None` before the first approval or when it does not verify.
    pub granted: Option<Grants>,
    /// The card waiting for a press, if any.
    pub card: Option<InstallCard>,
    /// Catalog entries of the events it emits.
    pub catalog: Vec<CatalogEntry>,
    /// Last 40 stderr lines, redacted.
    pub stderr_tail: Vec<String>,
    /// Validation warnings (reserved keys, unknown top-level keys).
    pub warnings: Vec<String>,
}

/// How an event is scoped.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum Scoped {
    /// Carries a session.
    Session,
    /// Engine-wide.
    Global,
}

/// One catalog entry (216 §7.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CatalogEntry {
    /// `topic#type`, `topic` or `<plugin>.<event>`.
    pub name: String,
    /// The wire topic.
    pub topic: String,
    /// The topic's (or plugin event's) version.
    pub version: u32,
    /// The serialized `type` tag (or `<plugin>.<event>`); `None` for single-struct topics.
    #[serde(rename = "type")]
    pub r#type: Option<String>,
    /// Owner crate (`agent`) or `plugin:<name>`.
    pub owner: String,
    /// Always `"public"` in the catalog (private and internal topics are absent).
    pub exposure: String,
    /// First sentence of the schema description, or the manifest's `description`.
    pub describe: String,
    /// Session-scoped or global.
    pub scoped: Scoped,
    /// Delta-class: needs `deltas: true` on the subscription.
    pub delta: bool,
    /// JSON Schema: inlined (resolved) for wire topics, the manifest schema for plugin events.
    pub schema: Value,
}

/// The latest status of one placement (snapshot-first for clients, `plugin_surfaces`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SurfaceSnapshot {
    /// Plugin name.
    pub plugin: String,
    /// Placement id.
    pub placement: String,
    /// Already overridden by state (backoff/failed/exited/awaiting_approval → off/error/off/off).
    pub dot: Dot,
    /// The badge, if any.
    pub badge: Option<Badge>,
    /// The plugin's label override.
    pub label: Option<String>,
    /// The plugin's tooltip, or the state when overridden ("starting", "exited", …).
    pub tooltip: Option<String>,
    /// Content revision, 0 = no content held.
    pub revision: u64,
    /// Format of the held content.
    pub format: Option<SurfaceFormat>,
}

/// Whether a surface has content.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum SurfaceLoad {
    /// Content held.
    Ready,
    /// `render` sent; a `PluginSurface` event follows.
    Loading,
    /// See `error`.
    Error,
}

/// OpenUI statement kind of a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum BindingKind {
    /// `Query(...)`: delivered as `query`.
    Query,
    /// `Mutation(...)`: delivered as `action`, needs a press.
    Mutation,
}

/// One `Query(...)` / `Mutation(...)` statement of a validated OpenUI source (from
/// `app::helper::CheckResult.bindings`). The plugin provider routes by it (§12.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SurfaceBinding {
    /// The statement id the renderer reports as `ProviderContext.statement_id`.
    pub statement_id: String,
    /// The query or mutation name the plugin receives (`query` / `action`).
    pub name: String,
    /// Query or mutation.
    pub kind: BindingKind,
}

/// A placement's content (`plugin_surface`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct SurfaceContent {
    /// Plugin name.
    pub plugin: String,
    /// Placement id.
    pub placement: String,
    /// Content revision.
    pub revision: u64,
    /// `loading`: the host sent `render` and waits for the plugin's `ui` (a `PluginSurface` event follows).
    pub state: SurfaceLoad,
    /// Content format.
    pub format: Option<SurfaceFormat>,
    /// Validated source; `None` while loading.
    pub source: Option<String>,
    /// Text fallback.
    pub text: Option<String>,
    /// OpenUI only; empty for json-render and markdown.
    pub bindings: Vec<SurfaceBinding>,
    /// "plugin did not render", "plugin <name> is not running", …
    pub error: Option<String>,
}

/// A `/command` reply.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CommandReply {
    /// A note shown in the chat.
    pub text: Option<String>,
    /// An ephemeral card.
    pub ui: Option<CommandUi>,
}

/// An ephemeral card in a command reply (json-render or markdown; OpenUI is refused in v1 replies).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CommandUi {
    /// json-render or markdown.
    pub format: SurfaceFormat,
    /// The card source.
    pub source: String,
    /// Text fallback.
    pub text: String,
}

/// One located validation problem.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CheckIssue {
    /// JSON Pointer into the manifest (`/placements/0/kind`), or `""` for the whole folder.
    pub path: String,
    /// What is wrong.
    pub message: String,
}

/// `plugin_check` / `t plugin check`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct CheckReport {
    /// No errors.
    pub ok: bool,
    /// The plugin name, when the folder has one.
    pub name: Option<String>,
    /// Errors (any makes `ok` false).
    pub errors: Vec<CheckIssue>,
    /// Warnings.
    pub warnings: Vec<CheckIssue>,
    /// Manifest hash, when it parses.
    pub manifest_sha256: Option<String>,
    /// Code hash, when computable.
    pub code_sha256: Option<String>,
    /// The install card's lines for this manifest.
    pub lines: Vec<GrantLine>,
}

/// SDK language of `plugin_new` / `t plugin new --lang`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    /// Python.
    #[default]
    Py,
    /// JavaScript (node).
    Js,
    /// POSIX shell.
    Sh,
    /// Go.
    Go,
    /// Ruby.
    Rb,
    /// Rust.
    Rs,
}

/// What `plugin_new` wrote.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ScaffoldReport {
    /// Plugin name.
    pub name: String,
    /// Absolute folder path.
    pub dir: String,
    /// SDK language.
    pub lang: Lang,
    /// No SDK (raw protocol template).
    pub raw: bool,
    /// Relative paths written.
    pub files: Vec<String>,
    /// Always "awaiting approval: press Approve in theuth or run `t plugin approve <name>`".
    pub next: String,
}

/// What `plugin_install` did.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct InstallReport {
    /// Plugin name.
    pub name: String,
    /// Absolute folder path under plugins/.
    pub dir: String,
    /// How to approve it.
    pub next: String,
}

/// A log tail (`plugin_logs`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema, kameo::Reply)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct LogTail {
    /// Plugin name.
    pub name: String,
    /// Absolute path of the log file.
    pub path: String,
    /// The lines, oldest first, redacted.
    pub lines: Vec<String>,
}
