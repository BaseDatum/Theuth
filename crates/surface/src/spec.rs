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

/// Catalog v0: thirty-six components, adjacently tagged as `{"type": Name, "props": {...}}`.
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
    /// Numbered how-to steps.
    Steps(StepsProps),
    /// An aside with a variant icon: note, tip, question, important (not a severity; that is `Alert`).
    Callout(CalloutProps),
    /// A block of code with a copy button.
    Code(CodeProps),
    /// Before/after text side by side.
    Diff(DiffProps),
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

/// `Tree` props.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(deny_unknown_fields)]
pub struct TreeProps {
    /// Rows in reading order; a row nests under the nearest earlier row of smaller depth.
    pub items: Vec<TreeItem>,
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
