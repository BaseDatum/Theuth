//! Wire types of a `json-render` fence body (design doc 30 §3.2, §10).
//!
//! One definition feeds serde (the validator), schemars (the prompt catalog and the
//! `visualize` skill) and, behind `feature = "ts"`, ts-rs (desktop bindings).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// A fence body: `root` names the top-level element, `elements` holds every element by id.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// Id of the top-level element.
    pub root: String,
    /// Every element by id; ids are referenced from `root` and `children`.
    pub elements: BTreeMap<String, Element>,
}

/// One element: a catalog component (`type` + `props` on the wire) and the ids it contains.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Element {
    /// The component and its props.
    #[serde(flatten)]
    pub component: Component,
    /// Child element ids, in order; only containers may have any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<String>,
}

/// Catalog v1: fifty-seven components, adjacently tagged as `{"type": Name, "props": {...}}`.
///
/// Declaration order is the catalog order shown to the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(tag = "type", content = "props")]
pub enum Component {
    /// Framed container with an optional title and description.
    Card(CardProps),
    /// Children laid out in one direction.
    Stack(StackProps),
    /// Children in `columns` equal-width columns, row by row.
    Grid(GridProps),
    /// Horizontal rule.
    Separator(SeparatorProps),
    /// Heading text at a level.
    Heading(HeadingProps),
    /// A paragraph of text.
    Text(TextProps),
    /// Short inline label.
    Badge(BadgeProps),
    /// Callout with a tone.
    Alert(AlertProps),
    /// Column headers and rows of strings.
    Table(TableProps),
    /// Key/value pairs.
    KeyValueList(KeyValueListProps),
    /// One labelled figure with an optional trend.
    Metric(MetricProps),
    /// Several metrics side by side.
    MetricGrid(MetricGridProps),
    /// A progress bar.
    Progress(ProgressProps),
    /// Horizontal bars, one per item.
    Bars(BarsProps),
    /// Dated steps with a state each.
    Timeline(TimelineProps),
    /// Items that are done or not.
    Checklist(ChecklistProps),
    /// Labelled panes; `children[i]` is the pane for `labels[i]`.
    Tabs(TabsProps),
    /// Titled sections; `children[i]` is the body of `titles[i]`.
    Accordion(AccordionProps),
    /// Typed boxes and arrows: nodes, edges, optional groups (design doc 65).
    Graph(GraphProps),
    /// Actors exchanging messages over time; every step addressable and optionally bound to code by `ref` (design doc 71 §2.4).
    Sequence(SequenceProps),
    /// Numbered how-to steps.
    Steps(StepsProps),
    /// An aside with a variant icon: note, tip, question, important (not a severity; that is `Alert`).
    Callout(CalloutProps),
    /// A block of code with a copy button.
    Code(CodeProps),
    /// Before/after text side by side.
    Diff(DiffProps),
    /// A passage of real code pinned to its location: `path`, 1-based `start`, marked rows, the symbol it quotes (design doc 71 §2.5).
    Excerpt(ExcerptProps),
    /// Nested labels, expandable; nesting by `depth`.
    Tree(TreeProps),
    /// Terms and their definitions.
    Glossary(GlossaryProps),
    /// A quotation with its source.
    Quote(QuoteProps),
    /// A picture with alt text and a caption.
    Image(ImageProps),
    /// Links with a note each.
    LinkList(LinkListProps),
    /// Labels with a variant each.
    Tags(TagsProps),
    /// One value on an arc between `min` and `max`.
    Gauge(GaugeProps),
    /// A tiny line of values, no axes.
    Sparkline(SparklineProps),
    /// Rows × columns of cells shaded by value.
    Heatmap(HeatmapProps),
    /// A person: name, role, contact, initials avatar.
    Person(PersonProps),
    /// Files with kind and size.
    FileList(FileListProps),
    /// Read-only columns of cards.
    Kanban(KanbanProps),
    /// A strip of days with a marker each.
    Calendar(CalendarProps),
    // ---- 123 ELK family (W1) ----
    /// States and transitions of a lifecycle; `initial`/`final` dots, events on the arrows (design doc 123).
    StateMachine(StateMachineProps),
    /// Entities, tables or classes with their fields and typed relations (ER / class diagram).
    Entities(EntitiesProps),
    /// Steps in lanes (teams, systems), edges across lanes: a handoff or cross-functional flow.
    Swimlane(SwimlaneProps),
    /// Two graphs, before and after, over one layout; the added/removed/changed ledger is derived by id.
    GraphDelta(GraphDeltaProps),
    // ---- 123 chart family (W2) ----
    /// Bars per label, one or more series: grouped, stacked, dumbbell or horizontal.
    BarChart(BarChartProps),
    /// Values over ordered labels: line, area, slope, bump or step.
    LineChart(LineChartProps),
    /// Points on two numeric axes: scatter, bubble (size) or beeswarm (one axis).
    ScatterChart(ScatterChartProps),
    /// Several criteria compared on spokes: radar, or polar bars for one series.
    RadarChart(RadarChartProps),
    /// Deltas that carry a running total from a start level to an end level.
    Waterfall(WaterfallProps),
    /// Stages narrowing from top to bottom (funnel) or widening (pyramid).
    Funnel(FunnelProps),
    /// Tasks on a date scale with groups, dependencies, progress and a today line.
    Gantt(GanttProps),
    /// Items placed in a 2×2 by two 0..1 coordinates (effort/impact, urgency/importance).
    Quadrant(QuadrantProps),
    // ---- 123 part-of-whole + misc (W3) ----
    /// Part-of-whole as nested rectangles sized by value, grouped.
    Treemap(TreemapProps),
    /// Flows between staged nodes, ribbons sized by value; no cycles.
    Sankey(SankeyProps),
    /// Two or three overlapping sets with labelled overlaps.
    Venn(VennProps),
    /// A cycle of 2–8 steps drawn clockwise around an optional hub.
    Loop(LoopProps),
    /// Stacked horizontal layers (architecture tiers, medallion, a stack).
    Layers(LayersProps),
    /// Stages across, rows down (does / thinks / feels), a mood line and pain points.
    Journey(JourneyProps),
    /// Cause-and-effect: an effect with categorised causes on the bones.
    Fishbone(FishboneProps),
}

/// `Card` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct CardProps {
    /// Title line.
    pub title: Option<String>,
    /// Muted line under the title.
    pub description: Option<String>,
}

/// `Stack` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct StackProps {
    /// Layout direction; default vertical.
    pub direction: Option<Direction>,
    /// Space between children.
    pub gap: Option<Gap>,
}

/// `Stack.direction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    /// Top to bottom.
    Vertical,
    /// Left to right.
    Horizontal,
}

/// `Stack.gap`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Gap {
    /// No space.
    None,
    /// Small.
    Sm,
    /// Medium.
    Md,
    /// Large.
    Lg,
}

/// `Grid` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GridProps {
    /// Number of columns, 1 to 6.
    #[schemars(range(min = 1, max = 6))]
    pub columns: u8,
}

/// `Separator` props: none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct SeparatorProps {}

/// `Heading` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct HeadingProps {
    /// Heading text.
    pub text: String,
    /// Level 1 (largest) to 4; default 2.
    #[schemars(range(min = 1, max = 4))]
    pub level: Option<u8>,
}

/// `Text` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TextProps {
    /// The text.
    pub text: String,
    /// Typographic variant; default body.
    pub variant: Option<TextVariant>,
}

/// `Text.variant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum TextVariant {
    /// Plain paragraph.
    Body,
    /// Dimmed.
    Muted,
    /// Small and dimmed.
    Caption,
    /// Larger intro text.
    Lead,
    /// Monospace.
    Code,
}

/// `Badge` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct BadgeProps {
    /// Badge text.
    pub text: String,
    /// Tone; default `default`.
    pub variant: Option<BadgeVariant>,
}

/// `Badge.variant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum BadgeVariant {
    /// Neutral, filled.
    Default,
    /// Neutral, subdued.
    Secondary,
    /// Positive.
    Success,
    /// Caution.
    Warning,
    /// Negative.
    Destructive,
}

/// `Alert` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct AlertProps {
    /// Bold first line.
    pub title: Option<String>,
    /// Body text.
    pub message: String,
    /// Tone; default info.
    #[serde(rename = "type")]
    pub kind: Option<AlertType>,
}

/// `Alert.type`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum AlertType {
    /// Informational.
    Info,
    /// Positive.
    Success,
    /// Caution.
    Warning,
    /// Failure.
    Error,
}

/// `Table` props; every row has exactly `columns.len()` cells.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TableProps {
    /// Header cells.
    pub columns: Vec<String>,
    /// Body rows.
    pub rows: Vec<Vec<String>>,
}

/// `KeyValueList` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct KeyValueListProps {
    /// Pairs, in order.
    pub items: Vec<KeyValue>,
}

/// One `KeyValueList` item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct KeyValue {
    /// Left column.
    pub key: String,
    /// Right column.
    pub value: String,
}

/// `Metric` props; also the item shape of `MetricGrid`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct MetricProps {
    /// What is measured.
    pub label: String,
    /// The figure, preformatted.
    pub value: String,
    /// Direction glyph beside the value.
    pub trend: Option<Trend>,
    /// Change text beside the value, e.g. `+3%`.
    pub delta: Option<String>,
    /// Recent values drawn as a sparkline beside the figure.
    pub spark: Option<Vec<f64>>,
}

/// `Metric.trend`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum Trend {
    /// Rising.
    Up,
    /// Falling.
    Down,
    /// Unchanged.
    Flat,
}

/// `MetricGrid` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct MetricGridProps {
    /// Metrics, in order.
    pub items: Vec<MetricProps>,
}

/// `Progress` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct ProgressProps {
    /// Current value.
    pub value: f64,
    /// Full-bar value; default 100.
    pub max: Option<f64>,
    /// Text before the bar.
    pub label: Option<String>,
}

/// `Bars` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct BarsProps {
    /// One bar per item.
    pub items: Vec<Bar>,
    /// Full-bar value; default the largest item.
    pub max: Option<f64>,
}

/// One `Bars` item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Bar {
    /// Row label.
    pub label: String,
    /// Bar length.
    pub value: f64,
}

/// `Timeline` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TimelineProps {
    /// Steps, in order.
    pub items: Vec<TimelineItem>,
    /// `list` (default) keeps `when` as free text; `time` places items on a date axis and requires ISO `when`/`until`.
    pub scale: Option<TimelineScale>,
    /// Title above the timeline.
    pub title: Option<String>,
}

/// One `Timeline` step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TimelineItem {
    /// Date or phase label.
    pub when: String,
    /// What happens.
    pub title: String,
    /// Second line under the title.
    pub detail: Option<String>,
    /// Marker state; default pending.
    pub state: Option<TimelineState>,
    /// End of a span (`scale: time`): ISO date or datetime.
    pub until: Option<String>,
    /// Unique id so boards and apps can point at this item.
    pub id: Option<String>,
    /// Code this item is about: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

/// `Timeline.scale`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum TimelineScale {
    /// Items in authored order, `when` as written.
    List,
    /// Items on a date axis; every `when` and `until` is an ISO date or datetime.
    Time,
}

/// `Timeline` item state.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum TimelineState {
    /// Finished.
    Done,
    /// In progress.
    Active,
    /// Not started.
    Pending,
}

/// `Checklist` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct ChecklistProps {
    /// Items, in order.
    pub items: Vec<ChecklistItem>,
}

/// One `Checklist` item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct ChecklistItem {
    /// Item text.
    pub text: String,
    /// Ticked or not.
    pub done: bool,
}

/// `Tabs` props; `labels.len()` must equal the element's `children.len()`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TabsProps {
    /// One label per child.
    pub labels: Vec<String>,
}

/// `Accordion` props; `titles.len()` must equal the element's `children.len()`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct AccordionProps {
    /// One title per child.
    pub titles: Vec<String>,
}

/// `Graph` props (design doc 65 §3.1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GraphProps {
    /// Nodes, in authored order.
    pub nodes: Vec<GraphNode>,
    /// Directed edges between node ids.
    pub edges: Vec<GraphEdge>,
    /// Groups a node may belong to; drawn as a labelled box.
    pub groups: Option<Vec<GraphGroup>>,
    /// Flow direction; default LR.
    pub direction: Option<GraphDirection>,
    /// Legend entries, ≤ 5: a label per state, edge style or node kind used.
    pub legend: Option<Vec<GraphLegendEntry>>,
    /// Title above the graph.
    pub title: Option<String>,
}

/// One `Graph` node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GraphNode {
    /// Unique within the graph; referenced by edges.
    pub id: String,
    /// Text inside the shape; ≤ 80 characters.
    pub label: String,
    /// Shape hint; default box.
    pub kind: Option<GraphKind>,
    /// Id of the group this node sits in.
    pub group: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Code this node explains: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
    /// Mono second line, ≤ 40 characters: a port, a version, a capacity.
    pub sub: Option<String>,
    /// Small rectangular tag, ≤ 12 characters: SERVICE, DB, QUEUE.
    pub tag: Option<String>,
    /// Icon drawn before the label; the TUI prints nothing for it.
    pub icon: Option<GraphIcon>,
}

/// `GraphNode.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum GraphKind {
    /// Rectangle.
    Box,
    /// Rounded rectangle.
    Rounded,
    /// Stadium.
    Pill,
    /// Decision.
    Diamond,
    /// Database.
    Cylinder,
    /// Person or external party.
    Actor,
    /// Hexagon: a service or a process.
    Hexagon,
    /// Circle: a start, an end or a point.
    Circle,
    /// Note: a folded-corner annotation.
    Note,
}

/// `GraphNode.state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum GraphState {
    /// Plain.
    Default,
    /// Highlighted.
    Active,
    /// Finished.
    Done,
    /// De-emphasised.
    Muted,
    /// Failing or risky.
    Danger,
}

/// One `Graph` edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GraphEdge {
    /// Source node id.
    pub from: String,
    /// Target node id; may equal `from`.
    pub to: String,
    /// Text on the arrow; ≤ 80 characters.
    pub label: Option<String>,
    /// Line style; default solid.
    pub style: Option<GraphEdgeStyle>,
    /// Semantic state of the arrow; default default.
    pub state: Option<GraphState>,
    /// Arrowheads; default forward.
    pub arrow: Option<GraphArrow>,
}

/// `GraphEdge.arrow`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum GraphArrow {
    /// Head at `to`.
    Forward,
    /// Heads at both ends.
    Both,
    /// A plain line.
    None,
}

/// `GraphEdge.style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum GraphEdgeStyle {
    /// Continuous line.
    Solid,
    /// Dashed line.
    Dashed,
}

/// One `Graph` group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GraphGroup {
    /// Unique within the graph; referenced by `GraphNode.group`.
    pub id: String,
    /// Title of the box; ≤ 80 characters.
    pub label: String,
    /// Id of the enclosing group (nested zones); depth ≤ 3.
    pub parent: Option<String>,
    /// Box treatment; default zone. `boundary` draws dashed.
    pub kind: Option<GraphGroupKind>,
}

/// `GraphGroup.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum GraphGroupKind {
    /// Solid labelled box.
    Zone,
    /// Dashed box: a trust or network boundary.
    Boundary,
    /// Light box: a loose cluster.
    Cluster,
}

/// One `Graph.legend` entry: a label for a state, an edge style or a node kind.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GraphLegendEntry {
    /// What the swatch means; ≤ 80 characters.
    pub label: String,
    /// The state this entry explains.
    pub state: Option<GraphState>,
    /// The edge style this entry explains.
    pub style: Option<GraphEdgeStyle>,
    /// The node kind this entry explains.
    pub kind: Option<GraphKind>,
}

/// `GraphNode.icon`: a closed set drawn from lucide.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "kebab-case")]
pub enum GraphIcon {
    /// A server.
    Server,
    /// A database.
    Database,
    /// A cloud service.
    Cloud,
    /// One person.
    User,
    /// A team.
    Users,
    /// The internet.
    Globe,
    /// A lock.
    Lock,
    /// A shield.
    Shield,
    /// A key.
    Key,
    /// A queue.
    Queue,
    /// A processor.
    Cpu,
    /// A box.
    Box,
    /// A package.
    Package,
    /// A file.
    File,
    /// A folder.
    Folder,
    /// A terminal.
    Terminal,
    /// A laptop.
    Laptop,
    /// A phone.
    Phone,
    /// Mail.
    Mail,
    /// A clock.
    Clock,
    /// Settings.
    Settings,
    /// Code.
    Code,
    /// A git branch.
    GitBranch,
    /// A bot.
    Bot,
    /// A browser.
    Browser,
}

/// `Graph.direction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub enum GraphDirection {
    /// Left to right.
    LR,
    /// Top to bottom.
    TB,
}

/// `Sequence` props (design doc 71 §2.4).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct SequenceProps {
    /// Lanes, left to right; 2 to 12.
    pub actors: Vec<SequenceActor>,
    /// Messages in time order; 1 to 40.
    pub steps: Vec<SequenceStep>,
    /// Title above the lanes.
    pub title: Option<String>,
    /// Combined fragments over step index ranges, ≤ 2, disjoint, not nested.
    pub fragments: Option<Vec<SequenceFragment>>,
}

/// One `Sequence` fragment: a frame around steps `from..=to`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct SequenceFragment {
    /// Frame kind.
    pub kind: SequenceFragmentKind,
    /// First step index (0-based).
    pub from: usize,
    /// Last step index (0-based), ≥ `from`.
    pub to: usize,
    /// Condition or caption in the frame's corner; ≤ 80 characters.
    pub label: Option<String>,
    /// `alt` only: the step index where the else region starts, inside `from..=to`.
    pub r#else: Option<usize>,
}

/// `SequenceFragment.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum SequenceFragmentKind {
    /// Alternatives; `else` splits the regions.
    Alt,
    /// Optional.
    Opt,
    /// Repeated.
    Loop,
    /// Parallel.
    Par,
}

/// One `Sequence` actor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct SequenceActor {
    /// Unique within the sequence; referenced by steps.
    pub id: String,
    /// Lane title; ≤ 80 characters.
    pub label: String,
}

/// One `Sequence` step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct SequenceStep {
    /// Sending actor id.
    pub from: String,
    /// Receiving actor id; may equal `from`.
    pub to: String,
    /// Text on the arrow; ≤ 80 characters.
    pub label: String,
    /// Arrow style; default call.
    pub style: Option<SequenceStyle>,
    /// Code this step runs: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
    /// Note beside the step, ≤ 80 characters; ≤ 2 notes per sequence.
    pub note: Option<String>,
    /// Semantic state of the arrow; default default.
    pub state: Option<GraphState>,
}

/// `SequenceStep.style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum SequenceStyle {
    /// Solid arrow, closed head.
    Call,
    /// Dashed arrow: a reply.
    Return,
    /// Open arrowhead: fire and forget.
    Async,
}

/// `Steps` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct StepsProps {
    /// Steps, in order; numbered from 1.
    pub items: Vec<Step>,
}

/// One `Steps` item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// What to do.
    pub title: String,
    /// How, or why.
    pub detail: Option<String>,
}

/// `Callout` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct CalloutProps {
    /// The aside.
    pub text: String,
    /// Bold first line.
    pub title: Option<String>,
    /// Icon; default note.
    pub variant: Option<CalloutVariant>,
}

/// `Callout.variant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum CalloutVariant {
    /// Background information.
    Note,
    /// A shortcut or best practice.
    Tip,
    /// Something to decide or find out.
    Question,
    /// Must not be missed.
    Important,
}

/// `Code` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct CodeProps {
    /// The code, verbatim.
    pub text: String,
    /// Language name, e.g. `rust`.
    pub lang: Option<String>,
    /// File name or caption above the block.
    pub title: Option<String>,
}

/// `Diff` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct DiffProps {
    /// Left column text.
    pub before: String,
    /// Right column text.
    pub after: String,
    /// Left heading; default Before.
    pub from: Option<String>,
    /// Right heading; default After.
    pub to: Option<String>,
}

/// `Excerpt` props (design doc 71 §2.5): ≤ 120 lines and ≤ 16 KiB of `text`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct ExcerptProps {
    /// Root-relative file path; no `..`, no scheme.
    pub path: String,
    /// The quoted lines, verbatim.
    pub text: String,
    /// 1-based line number of the first quoted line; default 1.
    #[schemars(range(min = 1))]
    pub start: Option<u32>,
    /// Language name, e.g. `rust`.
    pub lang: Option<String>,
    /// One line under the code: the claim these lines carry.
    pub caption: Option<String>,
    /// Marked rows as `[from, to]` line-number pairs, inclusive, inside the quoted range.
    pub mark: Option<Vec<[u32; 2]>>,
    /// Name of the definition the range quotes; enables `moved` detection.
    pub symbol: Option<String>,
}

/// `Tree` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TreeProps {
    /// Rows in reading order; a row nests under the nearest earlier row of smaller depth.
    pub items: Vec<TreeItem>,
    /// `list` (default) is the indented list; `nodes`/`org` draw a node tree, `nested` nested boxes.
    pub layout: Option<TreeLayout>,
    /// Title above the tree.
    pub title: Option<String>,
}

/// `Tree.layout`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum TreeLayout {
    /// Indented list.
    List,
    /// Top-down node tree.
    Nodes,
    /// Nested boxes.
    Nested,
    /// Org chart: top-down, siblings aligned.
    Org,
}

/// One `Tree` row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TreeItem {
    /// Row text.
    pub label: String,
    /// Nesting level, 0 to 6; default 0.
    #[schemars(range(min = 0, max = 6))]
    pub depth: Option<u8>,
    /// Dim text after the label.
    pub detail: Option<String>,
    /// Unique id so boards and apps can point at this row.
    pub id: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Code this row is about: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

/// `Glossary` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GlossaryProps {
    /// Terms, in order.
    pub items: Vec<Definition>,
}

/// One `Glossary` item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Definition {
    /// The term.
    pub term: String,
    /// Its meaning.
    pub text: String,
}

/// `Quote` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct QuoteProps {
    /// The quoted words.
    pub text: String,
    /// Who or what said them.
    pub source: Option<String>,
}

/// `Image` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct ImageProps {
    /// `http(s)://` or `data:image/` URL.
    pub src: String,
    /// What the picture shows, for readers who cannot see it.
    pub alt: String,
    /// Text under the picture.
    pub caption: Option<String>,
}

/// `LinkList` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct LinkListProps {
    /// Links, in order.
    pub items: Vec<Link>,
}

/// One `LinkList` item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Link {
    /// Link text.
    pub title: String,
    /// `http(s)://` URL.
    pub url: String,
    /// Why follow it.
    pub note: Option<String>,
}

/// `Tags` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TagsProps {
    /// Labels, in order.
    pub items: Vec<Tag>,
}

/// One `Tags` item; the same variants as `Badge`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Tag {
    /// Label text.
    pub text: String,
    /// Tone; default default.
    pub variant: Option<BadgeVariant>,
}

/// `Gauge` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GaugeProps {
    /// Current value.
    pub value: f64,
    /// Text under the arc.
    pub label: Option<String>,
    /// Full-arc value; default 100.
    pub max: Option<f64>,
    /// Empty-arc value; default 0.
    pub min: Option<f64>,
}

/// `Sparkline` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct SparklineProps {
    /// Values in order, left to right.
    pub values: Vec<f64>,
    /// Text beside the line.
    pub label: Option<String>,
}

/// `Heatmap` props; `cells` has `rows.len()` rows of `cols.len()` values.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct HeatmapProps {
    /// Row labels.
    pub rows: Vec<String>,
    /// Column labels.
    pub cols: Vec<String>,
    /// One row of values per row label.
    pub cells: Vec<Vec<f64>>,
    /// Darkest-cell value; default the largest cell.
    pub max: Option<f64>,
    /// `sequential` (default) shades from light to dark; `diverging` runs negative→neutral→positive about 0.
    pub palette: Option<HeatmapPalette>,
    /// Title above the grid.
    pub title: Option<String>,
}

/// `Heatmap.palette`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum HeatmapPalette {
    /// Light to dark.
    Sequential,
    /// Two hues about zero.
    Diverging,
}

/// `Person` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct PersonProps {
    /// Full name; the avatar shows its initials.
    pub name: String,
    /// Title or team.
    pub role: Option<String>,
    /// Email, handle or phone.
    pub contact: Option<String>,
}

/// `FileList` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct FileListProps {
    /// Files, in order.
    pub items: Vec<FileItem>,
}

/// One `FileList` item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct FileItem {
    /// Path; a trailing `/` or `kind: "dir"` is a folder.
    pub path: String,
    /// Type word, e.g. `dir`, `rust`, `image`.
    pub kind: Option<String>,
    /// Size, preformatted, e.g. `12 KB`.
    pub size: Option<String>,
}

/// `Kanban` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct KanbanProps {
    /// Columns, left to right.
    pub columns: Vec<KanbanColumn>,
    /// Story-map rows; cards pick one by `row` index.
    pub rows: Option<Vec<String>>,
    /// Story map: the release line is drawn after row `cut`.
    pub cut: Option<usize>,
    /// Title above the board.
    pub title: Option<String>,
}

/// One `Kanban` column.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct KanbanColumn {
    /// Column heading.
    pub title: String,
    /// Cards, top to bottom.
    pub cards: Vec<KanbanCard>,
    /// WIP limit; a column over it is marked.
    pub limit: Option<usize>,
}

/// One `Kanban` card.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct KanbanCard {
    /// Card text.
    pub title: String,
    /// Second line.
    pub detail: Option<String>,
    /// Short label on the card.
    pub tag: Option<String>,
    /// Card state; default default.
    pub state: Option<KanbanState>,
    /// Unique id so boards and apps can point at this card.
    pub id: Option<String>,
    /// Code this card is about: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
    /// Story-map row index into `rows`.
    pub row: Option<usize>,
}

/// `KanbanCard.state`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum KanbanState {
    /// Plain.
    Default,
    /// Waiting on something.
    Blocked,
    /// Finished.
    Done,
}

/// `Calendar` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct CalendarProps {
    /// Days, left to right.
    pub days: Vec<CalendarDay>,
    /// Text above the strip, e.g. the month.
    pub title: Option<String>,
}

/// One `Calendar` day.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct CalendarDay {
    /// Day text, e.g. `Mon 3` or `2026-09-25`.
    pub day: String,
    /// What happens that day.
    pub label: Option<String>,
    /// Marker; the same states as `Timeline`.
    pub state: Option<TimelineState>,
}

// ---- 123 ELK family (W1) ----

/// `StateMachine` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct StateMachineProps {
    /// States, in authored order; ≤ 40, at most one `initial`.
    pub states: Vec<MachineState>,
    /// Transitions between state ids; ≤ 80.
    pub transitions: Vec<Transition>,
    /// Flow direction; default LR.
    pub direction: Option<GraphDirection>,
    /// Title above the diagram.
    pub title: Option<String>,
}

/// One `StateMachine` state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct MachineState {
    /// Unique within the machine; referenced by transitions.
    pub id: String,
    /// Text in the state; ≤ 80 characters.
    pub label: String,
    /// Shape; default state.
    pub kind: Option<MachineStateKind>,
    /// Mono second line, ≤ 40 characters: an entry action, a timeout.
    pub sub: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Code this state is: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

/// `MachineState.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum MachineStateKind {
    /// The entry dot; at most one.
    Initial,
    /// A terminal ringed dot.
    Final,
    /// A rounded box.
    State,
    /// A diamond: a guarded branch.
    Choice,
}

/// One `StateMachine` transition.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Transition {
    /// Source state id.
    pub from: String,
    /// Target state id; may equal `from`.
    pub to: String,
    /// The event; ≤ 80 characters.
    pub label: Option<String>,
    /// The condition, drawn in brackets; ≤ 40 characters.
    pub guard: Option<String>,
    /// Line style; default solid.
    pub style: Option<GraphEdgeStyle>,
    /// Semantic state of the arrow; default default.
    pub state: Option<GraphState>,
}

/// `Entities` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct EntitiesProps {
    /// Entities, in authored order; ≤ 24.
    pub entities: Vec<Entity>,
    /// Relations between entity ids; ≤ 48.
    pub relations: Vec<Relation>,
    /// How much to show; default model.
    pub detail: Option<EntitiesDetail>,
    /// Title above the diagram.
    pub title: Option<String>,
}

/// `Entities.detail`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum EntitiesDetail {
    /// Names and relations only.
    Model,
    /// Fields with types and keys.
    Schema,
    /// Fields and methods.
    Class,
}

/// One `Entities` entity.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Entity {
    /// Unique within the diagram; referenced by relations.
    pub id: String,
    /// Header text; ≤ 80 characters.
    pub label: String,
    /// Header treatment; default entity.
    pub kind: Option<EntityKind>,
    /// Fields, in order; ≤ 32 with unique names.
    pub fields: Vec<Field>,
    /// Methods; only on `class` or `interface`.
    pub methods: Option<Vec<Method>>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Code this entity is: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

/// `Entity.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum EntityKind {
    /// A domain entity.
    Entity,
    /// A database table.
    Table,
    /// A class.
    Class,
    /// An interface (italic header).
    Interface,
    /// An enum.
    Enum,
}

/// One `Entity` field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Field {
    /// Field name; unique within the entity.
    pub name: String,
    /// Type, as written.
    pub r#type: Option<String>,
    /// Key role.
    pub key: Option<FieldKey>,
    /// Nullable; default false.
    pub nullable: Option<bool>,
    /// Short remark after the type.
    pub note: Option<String>,
}

/// `Field.key`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum FieldKey {
    /// Primary key.
    Pk,
    /// Foreign key.
    Fk,
    /// Unique key.
    Uk,
}

/// One `Entity` method.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Method {
    /// Method name.
    pub name: String,
    /// Signature after the name, e.g. `(id) -> User`.
    pub sig: Option<String>,
}

/// One `Entities` relation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Relation {
    /// Source entity id.
    pub from: String,
    /// Target entity id.
    pub to: String,
    /// Cardinality or UML relation; chooses the line ends.
    pub kind: RelationKind,
    /// Text on the line; ≤ 80 characters.
    pub label: Option<String>,
    /// A field of `from` the line leaves.
    pub from_field: Option<String>,
    /// A field of `to` the line enters.
    pub to_field: Option<String>,
}

/// `Relation.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "kebab-case")]
pub enum RelationKind {
    /// 1 : 1.
    OneOne,
    /// 1 : n (crow's foot at `to`).
    OneMany,
    /// n : m.
    ManyMany,
    /// `from` extends `to`.
    Inherits,
    /// `from` implements `to`.
    Implements,
    /// `from` owns `to` (filled diamond).
    Composes,
    /// `from` has `to` (open diamond).
    Aggregates,
    /// `from` uses `to` (dashed).
    Depends,
    /// `from` points at `to`.
    References,
}

/// `Swimlane` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct SwimlaneProps {
    /// Lanes, in order; ≤ 8.
    pub lanes: Vec<Lane>,
    /// Steps, each in a lane; ≤ 40.
    pub steps: Vec<LaneStep>,
    /// Edges between step ids; ≤ 80.
    pub edges: Vec<LaneEdge>,
    /// Flow direction; default LR (lanes are rows).
    pub direction: Option<GraphDirection>,
    /// Title above the diagram.
    pub title: Option<String>,
}

/// One `Swimlane` lane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Lane {
    /// Unique within the diagram; referenced by `LaneStep.lane`.
    pub id: String,
    /// Lane title; ≤ 80 characters.
    pub label: String,
}

/// One `Swimlane` step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct LaneStep {
    /// Unique within the diagram; referenced by edges.
    pub id: String,
    /// The lane this step sits in.
    pub lane: String,
    /// Text in the step; ≤ 80 characters.
    pub label: String,
    /// Shape; default step.
    pub kind: Option<LaneStepKind>,
    /// Mono second line, ≤ 40 characters.
    pub sub: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Code this step runs: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

/// `LaneStep.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum LaneStepKind {
    /// A rounded box.
    Step,
    /// A diamond.
    Decision,
    /// The entry pill.
    Start,
    /// The exit pill.
    End,
    /// A parallelogram: a document or a dataset.
    Data,
}

/// One `Swimlane` edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct LaneEdge {
    /// Source step id.
    pub from: String,
    /// Target step id.
    pub to: String,
    /// Text on the arrow; ≤ 80 characters.
    pub label: Option<String>,
    /// Line style; default solid.
    pub style: Option<GraphEdgeStyle>,
}

/// `GraphDelta` props (design doc 123 §4.2): ids are the identity across sides.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GraphDeltaProps {
    /// The graph as it was.
    pub before: GraphSide,
    /// The graph as it is; nodes with the same id keep their place.
    pub after: GraphSide,
    /// Flow direction; default LR.
    pub direction: Option<GraphDirection>,
    /// Title above both panels.
    pub title: Option<String>,
}

/// One side of a `GraphDelta`: a `Graph` without direction.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GraphSide {
    /// Nodes, in authored order.
    pub nodes: Vec<GraphNode>,
    /// Directed edges between node ids.
    pub edges: Vec<GraphEdge>,
    /// Groups a node may belong to.
    pub groups: Option<Vec<GraphGroup>>,
}

// ---- 123 chart family (W2) ----

/// One chart series: `values[i]` belongs to the i-th label or axis.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Series {
    /// Legend name; ≤ 80 characters.
    pub name: String,
    /// One value per label.
    pub values: Vec<f64>,
    /// Semantic state colouring the series; default default.
    pub state: Option<GraphState>,
}

/// `BarChart` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct BarChartProps {
    /// Category labels; ≤ 40.
    pub labels: Vec<String>,
    /// Series; ≤ 8, each with `labels.len()` values. `dumbbell` needs exactly 2.
    pub series: Vec<Series>,
    /// Layout; default grouped.
    pub variant: Option<BarVariant>,
    /// Category axis title.
    pub x_label: Option<String>,
    /// Value axis title.
    pub y_label: Option<String>,
    /// Unit after values, e.g. `ms`, `USD`.
    pub unit: Option<String>,
    /// Title above the chart.
    pub title: Option<String>,
}

/// `BarChart.variant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum BarVariant {
    /// Series side by side per label.
    Grouped,
    /// Series stacked per label.
    Stacked,
    /// Two series as the ends of one connector per label.
    Dumbbell,
    /// Bars run left to right, labels down the side.
    Horizontal,
}

/// `LineChart` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct LineChartProps {
    /// Ordered x labels; ≤ 60.
    pub labels: Vec<String>,
    /// Series; ≤ 8, each with `labels.len()` values. `slope` needs exactly 2 labels; `bump` values are ranks ≥ 1.
    pub series: Vec<Series>,
    /// Mark; default line.
    pub variant: Option<LineVariant>,
    /// X axis title.
    pub x_label: Option<String>,
    /// Y axis title.
    pub y_label: Option<String>,
    /// Unit after values.
    pub unit: Option<String>,
    /// Title above the chart.
    pub title: Option<String>,
}

/// `LineChart.variant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum LineVariant {
    /// Plain lines.
    Line,
    /// Filled under the line.
    Area,
    /// Two labels, one segment per series.
    Slope,
    /// Ranks over time, 1 at the top.
    Bump,
    /// Horizontal runs with vertical jumps.
    Step,
}

/// `ScatterChart` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ScatterChartProps {
    /// Points; ≤ 200.
    pub points: Vec<Point>,
    /// X axis title.
    pub x_label: String,
    /// Y axis title.
    pub y_label: String,
    /// Point groups a point may name; ≤ 8, each a legend entry.
    pub groups: Option<Vec<PointGroup>>,
    /// Mark; default scatter. `bubble` needs `size` on every point; `beeswarm` ignores `y`.
    pub variant: Option<ScatterVariant>,
    /// Title above the chart.
    pub title: Option<String>,
}

/// One `ScatterChart` point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Point {
    /// X value.
    pub x: f64,
    /// Y value.
    pub y: f64,
    /// Text beside the point; ≤ 80 characters.
    pub label: Option<String>,
    /// Bubble size (any positive scale).
    pub size: Option<f64>,
    /// Group id from `groups`.
    pub group: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Code this point is about: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

/// One `ScatterChart` group.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct PointGroup {
    /// Unique within the chart; referenced by `Point.group`.
    pub id: String,
    /// Legend text; ≤ 80 characters.
    pub label: String,
}

/// `ScatterChart.variant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum ScatterVariant {
    /// Dots.
    Scatter,
    /// Dots sized by `size`.
    Bubble,
    /// Dots spread along x only.
    Beeswarm,
}

/// `RadarChart` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct RadarChartProps {
    /// Spoke labels; 3 to 8.
    pub axes: Vec<String>,
    /// Series; ≤ 5, each with `axes.len()` values. `polar` needs exactly 1.
    pub series: Vec<Series>,
    /// Outer ring value; default the largest value.
    pub max: Option<f64>,
    /// Mark; default radar.
    pub variant: Option<RadarVariant>,
    /// Title above the chart.
    pub title: Option<String>,
}

/// `RadarChart.variant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum RadarVariant {
    /// A polygon per series.
    Radar,
    /// Radial bars, one series.
    Polar,
}

/// `Waterfall` props (design doc 123 §4.2): the running total is computed.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct WaterfallProps {
    /// Steps, in order; ≤ 20.
    pub steps: Vec<WaterfallStep>,
    /// Unit after values.
    pub unit: Option<String>,
    /// Title above the chart.
    pub title: Option<String>,
}

/// One `Waterfall` step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct WaterfallStep {
    /// Step text; ≤ 80 characters.
    pub label: String,
    /// A delta for `delta`; the absolute level for `total`/`subtotal`.
    pub value: f64,
    /// Bar kind; default delta.
    pub kind: Option<WaterfallKind>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
}

/// `WaterfallStep.kind`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum WaterfallKind {
    /// A floating change.
    Delta,
    /// An absolute level from the baseline.
    Total,
    /// An absolute level mid-way.
    Subtotal,
}

/// `Funnel` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct FunnelProps {
    /// Stages, top to bottom; ≤ 10. Values on all or none.
    pub stages: Vec<FunnelStage>,
    /// Shape; default funnel.
    pub variant: Option<FunnelVariant>,
    /// Unit after values.
    pub unit: Option<String>,
    /// Title above the chart.
    pub title: Option<String>,
}

/// One `Funnel` stage.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct FunnelStage {
    /// Stage text; ≤ 80 characters.
    pub label: String,
    /// Count at this stage.
    pub value: Option<f64>,
    /// Short remark.
    pub note: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
}

/// `Funnel.variant`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum FunnelVariant {
    /// Narrowing downwards.
    Funnel,
    /// Widening downwards.
    Pyramid,
}

/// `Gantt` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GanttProps {
    /// Tasks, top to bottom; ≤ 60.
    pub tasks: Vec<GanttTask>,
    /// The today line: ISO date or datetime.
    pub today: Option<String>,
    /// Title above the chart.
    pub title: Option<String>,
}

/// One `Gantt` task.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct GanttTask {
    /// Unique within the chart; referenced by `depends`.
    pub id: String,
    /// Row text; ≤ 80 characters.
    pub label: String,
    /// Start: ISO date or datetime.
    pub start: String,
    /// End: ISO date or datetime, ≥ `start`.
    pub end: String,
    /// Band the task sits in; equal groups are adjacent.
    pub group: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Done fraction, 0 to 1.
    #[schemars(range(min = 0.0, max = 1.0))]
    pub progress: Option<f64>,
    /// Ids of tasks this one waits for.
    pub depends: Option<Vec<String>>,
    /// Code this task is about: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

/// `Quadrant` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct QuadrantProps {
    /// X axis title.
    pub x_label: String,
    /// Y axis title.
    pub y_label: String,
    /// Items; ≤ 40.
    pub items: Vec<QuadrantItem>,
    /// Quadrant captions, exactly 4: top-left, top-right, bottom-left, bottom-right.
    pub quadrants: Option<Vec<String>>,
    /// X end labels, exactly 2: low, high.
    pub x_ticks: Option<Vec<String>>,
    /// Y end labels, exactly 2: low, high.
    pub y_ticks: Option<Vec<String>>,
    /// Title above the chart.
    pub title: Option<String>,
}

/// One `Quadrant` item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct QuadrantItem {
    /// Unique id so boards and apps can point at this item.
    pub id: Option<String>,
    /// Text beside the dot; ≤ 80 characters.
    pub label: String,
    /// Horizontal position, 0 (left) to 1 (right).
    #[schemars(range(min = 0.0, max = 1.0))]
    pub x: f64,
    /// Vertical position, 0 (bottom) to 1 (top).
    #[schemars(range(min = 0.0, max = 1.0))]
    pub y: f64,
    /// Dot size (any positive scale).
    pub size: Option<f64>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Code this item is about: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

// ---- 123 part-of-whole + misc (W3) ----

/// `Treemap` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TreemapProps {
    /// Cells; ≤ 60 over ≤ 12 groups.
    pub items: Vec<TreemapCell>,
    /// Title above the map.
    pub title: Option<String>,
}

/// One `Treemap` cell.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TreemapCell {
    /// Cell text; ≤ 80 characters.
    pub label: String,
    /// Area weight, > 0.
    pub value: f64,
    /// Group name; equal groups share a headed rectangle.
    pub group: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Code this cell is about: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

/// `Sankey` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct SankeyProps {
    /// Nodes; ≤ 24.
    pub nodes: Vec<SankeyNode>,
    /// Flows between node ids; ≤ 48, no self-flow, no cycle.
    pub flows: Vec<SankeyFlow>,
    /// Unit after values.
    pub unit: Option<String>,
    /// Title above the diagram.
    pub title: Option<String>,
}

/// One `Sankey` node.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct SankeyNode {
    /// Unique within the diagram; referenced by flows.
    pub id: String,
    /// Node text; ≤ 80 characters.
    pub label: String,
    /// Column, 0-based; derived from the flows when absent.
    pub stage: Option<usize>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
}

/// One `Sankey` flow.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct SankeyFlow {
    /// Source node id.
    pub from: String,
    /// Target node id; ≠ `from`.
    pub to: String,
    /// Ribbon width, > 0.
    pub value: f64,
    /// Text on the ribbon; ≤ 80 characters.
    pub label: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
}

/// `Venn` props (design doc 123 §4.2): exactly 2 or 3 sets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct VennProps {
    /// Sets; exactly 2 or 3.
    pub sets: Vec<VennSet>,
    /// Labelled overlaps of 2 or 3 distinct sets.
    pub overlaps: Option<Vec<VennOverlap>>,
    /// Title above the diagram.
    pub title: Option<String>,
}

/// One `Venn` set.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct VennSet {
    /// Unique within the diagram; referenced by overlaps.
    pub id: String,
    /// Set text; ≤ 80 characters.
    pub label: String,
    /// Size, shown after the label.
    pub size: Option<f64>,
}

/// One `Venn` overlap.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct VennOverlap {
    /// The intersecting set ids; 2 or 3, distinct.
    pub sets: Vec<String>,
    /// Text in the lens; ≤ 80 characters.
    pub label: String,
}

/// `Loop` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct LoopProps {
    /// Steps clockwise from the top; 2 to 8.
    pub steps: Vec<LoopStep>,
    /// Text in the centre.
    pub hub: Option<String>,
    /// Title above the diagram.
    pub title: Option<String>,
}

/// One `Loop` step.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct LoopStep {
    /// Step text; ≤ 80 characters.
    pub label: String,
    /// Short remark under the label.
    pub note: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Code this step runs: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

/// `Layers` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct LayersProps {
    /// Layers in reading order; ≤ 12, each ≤ 12 items.
    pub layers: Vec<Layer>,
    /// Which way the stack reads; default top-down.
    pub direction: Option<LayersDirection>,
    /// Title above the stack.
    pub title: Option<String>,
}

/// One `Layers` layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct Layer {
    /// Layer text; ≤ 80 characters.
    pub label: String,
    /// Items in the layer; ≤ 12.
    pub items: Option<Vec<String>>,
    /// Dim remark at the right.
    pub note: Option<String>,
    /// Semantic state; default default.
    pub state: Option<GraphState>,
    /// Code this layer is: `path`, `path#L10` or `path#L10-L24` (root-relative), or `block:<id>`.
    pub r#ref: Option<String>,
}

/// `Layers.direction`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "kebab-case")]
pub enum LayersDirection {
    /// First layer on top.
    TopDown,
    /// First layer at the bottom.
    BottomUp,
}

/// `Journey` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct JourneyProps {
    /// Stages across the top; ≤ 8.
    pub stages: Vec<String>,
    /// Rows down the side; ≤ 4, each with `stages.len()` cells.
    pub rows: Vec<JourneyRow>,
    /// One mood per stage, −1 to 1.
    pub mood: Option<Vec<f64>>,
    /// Pain points under a stage.
    pub pains: Option<Vec<JourneyPain>>,
    /// Title above the map.
    pub title: Option<String>,
}

/// One `Journey` row.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct JourneyRow {
    /// Row title, e.g. Does, Thinks, Feels.
    pub label: String,
    /// One cell per stage.
    pub cells: Vec<String>,
}

/// One `Journey` pain point.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct JourneyPain {
    /// Stage index, 0-based.
    pub stage: usize,
    /// The pain; ≤ 80 characters.
    pub text: String,
}

/// `Fishbone` props (design doc 123 §4.2).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct FishboneProps {
    /// The effect at the head; ≤ 80 characters.
    pub effect: String,
    /// Cause categories on the bones; ≤ 8, each ≤ 6 items.
    pub causes: Vec<FishboneCause>,
    /// Title above the diagram.
    pub title: Option<String>,
}

/// One `Fishbone` category.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct FishboneCause {
    /// Bone title; ≤ 80 characters.
    pub category: String,
    /// Causes on the bone; ≤ 6.
    pub items: Vec<String>,
}
