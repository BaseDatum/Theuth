//! Dynamic catalog (216 §7.1–§7.2): public wire topics split per `type`, plus plugin events.

use std::collections::BTreeMap;

use schemars::generate::SchemaSettings;
use serde_json::{Map, Value};
use wire::RuntimeWire as _;

use crate::manifest::Manifest;
use crate::types::{CatalogEntry, Scoped};

/// Delta-class types: delivered only to subscriptions with `deltas: true` (216 §7.3).
pub(crate) const DELTA_TYPES: &[&str] = &[
    "TextDelta",
    "ReasoningDelta",
    "ToolCallProgress",
    "VisionProgress",
];

/// Draft-07 definitions prefix of the generator [`wire_entries`] uses.
const DEFINITIONS: &str = "#/definitions/";

/// Public wire topics of `rt.wire_schema()`. An enum topic yields one entry per `oneOf` branch
/// (`topic#type`, the branch's `type.const`); any other topic yields one `topic` entry.
/// `plugins.events` is absent: its per-plugin names come from [`plugin_entries`].
pub(crate) fn wire_entries(rt: &runtime::Runtime) -> Vec<CatalogEntry> {
    let mut out = Vec::new();
    for d in rt.wire_schema() {
        if !public(&d) || d.topic == crate::EVENTS_TOPIC {
            continue;
        }
        let mut generator = SchemaSettings::draft07().for_serialize().into_generator();
        let mut root = (d.schema)(&mut generator);
        // subschema_for does not apply the dialect's transforms to the root (as rpc-core's discovery).
        for transform in generator.transforms_mut() {
            transform.transform(&mut root);
        }
        let defs = generator.take_definitions(true);
        topic_entries(&d, root.to_value(), &defs, &mut out);
    }
    out
}

/// The exposure check of one descriptor.
fn public(d: &wire::EventDescriptor) -> bool {
    d.exposure == wire::Exposure::Public
}

/// Entries of one topic from its root schema and the generator's definitions.
fn topic_entries(
    d: &wire::EventDescriptor,
    root: Value,
    defs: &Map<String, Value>,
    out: &mut Vec<CatalogEntry>,
) {
    let owner = d.topic.split('.').next().unwrap_or(d.topic).to_owned();
    // One level: the root is normally a `$ref` to the topic type's definition.
    let (top, root_name) = match ref_name(&root) {
        Some(name) => match defs.get(&name) {
            Some(def) => (def.clone(), Some(name)),
            None => (root, None),
        },
        None => (root, None),
    };
    let entry = |name: String, r#type: Option<String>, schema: Value| {
        let delta = r#type.as_deref().is_some_and(|t| DELTA_TYPES.contains(&t));
        CatalogEntry {
            name,
            topic: d.topic.to_owned(),
            version: d.version,
            r#type,
            owner: owner.clone(),
            exposure: wire::Exposure::Public.as_str().to_owned(),
            describe: schema
                .get("description")
                .and_then(Value::as_str)
                .map(first_sentence)
                .unwrap_or_default(),
            scoped: if has_property(&schema, "session") {
                Scoped::Session
            } else {
                Scoped::Global
            },
            delta,
            schema,
        }
    };
    let branches = top
        .get("oneOf")
        .or_else(|| top.get("anyOf"))
        .and_then(Value::as_array);
    let mut split = Vec::new();
    for branch in branches.into_iter().flatten() {
        let resolved = resolve(branch, defs, root_name.as_deref());
        if let Some(tag) = tag_of(&resolved) {
            split.push((tag, resolved));
        }
    }
    if split.is_empty() {
        let schema = resolve(&top, defs, root_name.as_deref());
        out.push(entry(d.topic.to_owned(), None, schema));
        return;
    }
    // Review S3: what `external_view` never delivers is never published either. A DROPped
    // variant has no entry (so a subscription to it fails validation), and every STRIP path is
    // pruned from its variant's schema.
    for (tag, mut schema) in split {
        if d.drop.contains(&tag.as_str()) {
            continue;
        }
        for path in d.strip {
            let mut segments = path.split('.');
            if segments.next() == Some(tag.as_str()) {
                prune(&mut schema, &segments.collect::<Vec<_>>());
            }
        }
        out.push(entry(format!("{}#{tag}", d.topic), Some(tag), schema));
    }
}

/// Remove the property `path` names from a resolved schema, as `wire`'s `strip_path` removes the
/// value: arrays (`items`) fan out, and `allOf`/`anyOf`/`oneOf` parts (an `Option`, a flattened
/// struct) are each followed. The last segment also leaves `required`.
fn prune(schema: &mut Value, path: &[&str]) {
    let Some((first, rest)) = path.split_first() else {
        return;
    };
    let Some(map) = schema.as_object_mut() else {
        return;
    };
    for key in ["allOf", "anyOf", "oneOf"] {
        if let Some(Value::Array(parts)) = map.get_mut(key) {
            parts.iter_mut().for_each(|p| prune(p, path));
        }
    }
    if let Some(items) = map.get_mut("items") {
        match items {
            Value::Array(tuple) => tuple.iter_mut().for_each(|i| prune(i, path)),
            other => prune(other, path),
        }
    }
    if rest.is_empty() {
        if let Some(Value::Object(props)) = map.get_mut("properties") {
            props.remove(*first);
        }
        if let Some(Value::Array(required)) = map.get_mut("required") {
            required.retain(|r| r.as_str() != Some(*first));
        }
    } else if let Some(inner) = map.get_mut("properties").and_then(|p| p.get_mut(*first)) {
        prune(inner, rest);
    }
}

/// Plugin events a manifest declares: `<plugin>.<event>` on `plugins.events`, owner `plugin:<name>`.
pub(crate) fn plugin_entries(plugin: &str, m: &Manifest) -> Vec<CatalogEntry> {
    m.emits
        .iter()
        .map(|(event, decl)| {
            let name = format!("{plugin}.{event}");
            CatalogEntry {
                name: name.clone(),
                topic: crate::EVENTS_TOPIC.to_owned(),
                version: decl.version,
                r#type: Some(name),
                owner: format!("plugin:{plugin}"),
                exposure: wire::Exposure::Public.as_str().to_owned(),
                describe: decl.description.trim().to_owned(),
                scoped: decl.scope,
                delta: false,
                schema: decl.schema.clone(),
            }
        })
        .collect()
}

/// The first sentence of a doc comment: whitespace collapsed, cut after the first `.`, `!` or `?`
/// that ends the text or is followed by whitespace and a non-lowercase character (`e.g. this` does
/// not end a sentence). A blank line ends the first paragraph.
pub(crate) fn first_sentence(s: &str) -> String {
    let paragraph = s.trim().split("\n\n").next().unwrap_or_default();
    let flat = paragraph.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut chars = flat.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if !matches!(c, '.' | '!' | '?') {
            continue;
        }
        match chars.peek() {
            None => break,
            Some((_, ' ')) => {
                let next = flat[i + 2..].chars().next();
                if next.is_none_or(|n| !n.is_lowercase()) {
                    return flat[..=i].to_owned();
                }
            }
            Some(_) => {}
        }
    }
    flat
}

/// The serialized `type` tag of a resolved branch: `properties.type.const` (or a one-value
/// `enum`), looked up through `allOf`.
fn tag_of(schema: &Value) -> Option<String> {
    let ty = schema.get("properties").and_then(|p| p.get("type"));
    let tag = ty.and_then(|t| {
        t.get("const").and_then(Value::as_str).or_else(|| {
            match t.get("enum").and_then(Value::as_array).map(Vec::as_slice) {
                Some([one]) => one.as_str(),
                _ => None,
            }
        })
    });
    if let Some(tag) = tag {
        return Some(tag.to_owned());
    }
    schema
        .get("allOf")
        .and_then(Value::as_array)?
        .iter()
        .find_map(tag_of)
}

/// Whether a resolved object schema (or one of its `allOf` parts) declares `name`.
fn has_property(schema: &Value, name: &str) -> bool {
    schema.get("properties").and_then(|p| p.get(name)).is_some()
        || schema
            .get("allOf")
            .and_then(Value::as_array)
            .is_some_and(|parts| parts.iter().any(|p| has_property(p, name)))
}

/// The definition name of a `{"$ref": "#/definitions/<name>"}` schema.
fn ref_name(schema: &Value) -> Option<String> {
    let reference = schema.get("$ref")?.as_str()?.strip_prefix(DEFINITIONS)?;
    Some(
        percent_decode(reference)
            .replace("~1", "/")
            .replace("~0", "~"),
    )
}

/// `%XX` escapes of a JSON pointer segment (schemars encodes generic names).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = bytes
            .get(i + 1..i + 3)
            .and_then(|h| std::str::from_utf8(h).ok());
        match (bytes[i], hex.and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(b)) => {
                out.push(b);
                i += 3;
            }
            (b, _) => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `schema` with every `$ref` inlined. A reference back into a definition being inlined (a
/// recursive type, or the topic type itself) stays a `$ref`, and its definition is carried under
/// the entry's own `definitions`, so each entry is self-contained.
fn resolve(schema: &Value, defs: &Map<String, Value>, root: Option<&str>) -> Value {
    let mut r = Resolver {
        defs,
        stack: root.map(str::to_owned).into_iter().collect(),
        keep: BTreeMap::new(),
    };
    let mut out = r.inline(schema);
    if !r.keep.is_empty()
        && let Value::Object(map) = &mut out
    {
        map.insert(
            "definitions".into(),
            Value::Object(r.keep.into_iter().collect()),
        );
    }
    out
}

/// Inlining state of [`resolve`].
struct Resolver<'a> {
    defs: &'a Map<String, Value>,
    /// Definitions being inlined, outermost first.
    stack: Vec<String>,
    /// Recursive definitions kept by reference.
    keep: BTreeMap<String, Value>,
}

impl Resolver<'_> {
    fn inline(&mut self, v: &Value) -> Value {
        let defs = self.defs;
        match v {
            Value::Array(items) => Value::Array(items.iter().map(|i| self.inline(i)).collect()),
            Value::Object(map) => {
                if let Some(name) = ref_name(v)
                    && let Some(def) = defs.get(&name)
                {
                    if self.stack.contains(&name) {
                        self.keep_def(&name, def);
                        return v.clone();
                    }
                    self.stack.push(name);
                    let mut resolved = self.inline(def);
                    self.stack.pop();
                    if let Value::Object(target) = &mut resolved {
                        for (k, sibling) in map.iter().filter(|(k, _)| k.as_str() != "$ref") {
                            target.insert(k.clone(), self.inline(sibling));
                        }
                    }
                    return resolved;
                }
                Value::Object(
                    map.iter()
                        .map(|(k, x)| (k.clone(), self.inline(x)))
                        .collect(),
                )
            }
            other => other.clone(),
        }
    }

    /// Carry a recursive definition, itself inlined with only its own name on the stack.
    fn keep_def(&mut self, name: &str, def: &Value) {
        if self.keep.contains_key(name) {
            return;
        }
        self.keep.insert(name.to_owned(), Value::Null);
        let outer = std::mem::replace(&mut self.stack, vec![name.to_owned()]);
        let inlined = self.inline(def);
        self.stack = outer;
        self.keep.insert(name.to_owned(), inlined);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Probe events of the catalog tests.
    #[expect(dead_code, reason = "only its schema is used")]
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    #[serde(tag = "type")]
    enum Probe {
        /// The first thing happened, e.g. a probe. It carries text.
        First {
            /// The session.
            session: String,
            /// What was said.
            text: String,
            /// A nested value.
            inner: Inner,
        },
        /// Second thing!  Engine-wide.
        #[serde(rename = "second_thing")]
        Second {
            /// A count.
            n: u32,
        },
        /// Streamed text.
        TextDelta {
            /// The session.
            session: String,
        },
    }

    /// A nested struct, inlined into the branch.
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    struct Inner {
        /// A flag.
        flag: bool,
        /// Recursion stays a reference.
        next: Option<Box<Inner>>,
    }

    impl wire::WireEvent for Probe {
        const TOPIC: &'static str = "test.catalog-probe";
    }

    /// A private topic: never in the catalog.
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    struct Hidden {
        /// Something private.
        sites: Vec<String>,
    }

    impl wire::WireEvent for Hidden {
        const TOPIC: &'static str = "test.catalog-hidden";
        const EXPOSURE: wire::Exposure = wire::Exposure::Private;
    }

    /// A single-struct topic. Not split.
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    struct Single {
        /// A path.
        path: String,
    }

    impl wire::WireEvent for Single {
        const TOPIC: &'static str = "test.catalog-single";
    }

    #[test]
    fn first_sentence() {
        assert_eq!(
            super::first_sentence("A turn finished; carries usage. More here."),
            "A turn finished; carries usage."
        );
        assert_eq!(
            super::first_sentence("  Wrapped\n   doc line. Next"),
            "Wrapped doc line."
        );
        assert_eq!(
            super::first_sentence("Uses e.g. this form. Then"),
            "Uses e.g. this form."
        );
        assert_eq!(super::first_sentence("Ready?"), "Ready?");
        assert_eq!(super::first_sentence("No terminator"), "No terminator");
        assert_eq!(
            super::first_sentence("First para\n\nSecond. para"),
            "First para"
        );
        assert_eq!(
            super::first_sentence("version 1.2 shipped. Ok"),
            "version 1.2 shipped."
        );
        assert_eq!(super::first_sentence(""), "");
    }

    #[tokio::test]
    async fn enum_topic_splits_per_type() {
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(Default::default(), dir.path().to_path_buf());
        rt.register_wire_event::<Probe>().await.unwrap();
        rt.register_wire_event::<Hidden>().await.unwrap();
        rt.register_wire_event::<Single>().await.unwrap();
        crate::declare_wire(&rt).await.unwrap();
        let entries = wire_entries(&rt);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(
            !names.iter().any(|n| n.starts_with("test.catalog-hidden")),
            "{names:?}"
        );
        assert!(
            !names.iter().any(|n| n.starts_with(crate::EVENTS_TOPIC)),
            "{names:?}"
        );
        assert!(names.contains(&"plugins.ui#PluginStatus"), "{names:?}");

        let find = |n: &str| {
            entries
                .iter()
                .find(|e| e.name == n)
                .unwrap_or_else(|| panic!("{n} in {names:?}"))
        };
        let first = find("test.catalog-probe#First");
        assert_eq!(first.r#type.as_deref(), Some("First"));
        assert_eq!(first.topic, "test.catalog-probe");
        assert_eq!(first.owner, "test");
        assert_eq!(first.exposure, "public");
        assert_eq!(first.describe, "The first thing happened, e.g. a probe.");
        assert_eq!(first.scoped, Scoped::Session);
        assert!(!first.delta);
        // Inlined: the nested struct's properties are present, its recursion stays a reference.
        // schemars wraps a described `$ref` in `allOf`; the inlined body sits inside it.
        let inner = &first.schema["properties"]["inner"];
        let inner = if inner["allOf"].is_array() {
            &inner["allOf"][0]
        } else {
            inner
        };
        assert!(inner["properties"]["flag"].is_object(), "{}", first.schema);
        assert!(
            first.schema["definitions"]["Inner"].is_object(),
            "{}",
            first.schema
        );
        assert!(
            !first.schema.to_string().contains("#/definitions/Probe"),
            "{}",
            first.schema
        );

        let second = find("test.catalog-probe#second_thing");
        assert_eq!(second.describe, "Second thing!");
        assert_eq!(second.scoped, Scoped::Global);
        assert!(find("test.catalog-probe#TextDelta").delta);

        let single = find("test.catalog-single");
        assert_eq!(single.r#type, None);
        assert_eq!(single.describe, "A single-struct topic.");
        assert!(
            single.schema["properties"]["path"].is_object(),
            "{}",
            single.schema
        );
    }

    /// An enum topic with a DROPped variant and STRIP paths (one through an array).
    #[expect(dead_code, reason = "only its schema is used")]
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    #[serde(tag = "type")]
    enum Exposed {
        /// Shown.
        Shown {
            /// The session.
            session: String,
            /// Others' words.
            text: String,
            /// Kept.
            kept: u32,
            /// Rows.
            rows: Vec<Row>,
        },
        /// Never shown.
        Secret {
            /// A value.
            value: String,
        },
    }

    /// One row.
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    struct Row {
        /// A link.
        url: String,
        /// A title.
        title: String,
    }

    impl wire::WireEvent for Exposed {
        const TOPIC: &'static str = "test.catalog-exposed";
        const DROP: &'static [&'static str] = &["Secret"];
        const STRIP: &'static [&'static str] = &["Shown.text", "Shown.rows.url"];
    }

    /// S3: the catalog publishes exactly what `external_view` delivers.
    #[tokio::test]
    async fn dropped_variants_and_stripped_fields_are_not_published() {
        let dir = tempfile::tempdir().unwrap();
        let rt = runtime::Runtime::new(Default::default(), dir.path().to_path_buf());
        rt.register_wire_event::<Exposed>().await.unwrap();
        let entries = wire_entries(&rt);
        let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        assert!(names.contains(&"test.catalog-exposed#Shown"), "{names:?}");
        assert!(!names.contains(&"test.catalog-exposed#Secret"), "{names:?}");
        let shown = &entries
            .iter()
            .find(|e| e.name == "test.catalog-exposed#Shown")
            .unwrap()
            .schema;
        let text = shown.to_string();
        assert!(shown["properties"]["kept"].is_object(), "{text}");
        assert!(shown["properties"].get("text").is_none(), "{text}");
        assert!(
            !shown["required"]
                .as_array()
                .unwrap()
                .iter()
                .any(|r| r == "text"),
            "{text}"
        );
        assert!(!text.contains("\"url\""), "{text}");
        assert!(text.contains("\"title\""), "{text}");
    }
}
