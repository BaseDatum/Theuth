//! The catalog, read off [`Component`]'s JSON Schema so the validator, the prompt line and
//! the skill can never disagree with the types (design doc 30 §0.3).

use std::sync::LazyLock;

use schemars::generate::SchemaSettings;
use serde_json::Value;

use crate::spec::{Component, Spec};

/// One catalog component: its `type` name and the (inlined) JSON Schema of its `props`.
pub(crate) struct Entry {
    /// The `type` string on the wire.
    pub name: String,
    /// Schema of `props`; an object schema with `properties`, `required`, `additionalProperties`.
    pub props: Value,
}

/// Every component, in declaration (= prompt) order.
pub(crate) struct Catalog {
    /// Entries in `Component` variant order.
    pub entries: Vec<Entry>,
    /// Names sorted and comma-joined, for the unknown-component message.
    pub names: String,
}

impl Catalog {
    /// The entry named `name`, if it is a catalog component.
    pub fn entry(&self, name: &str) -> Option<&Entry> {
        self.entries.iter().find(|e| e.name == name)
    }
}

/// The catalog, built once from the schema of [`Component`].
pub(crate) fn catalog() -> &'static Catalog {
    static CATALOG: LazyLock<Catalog> = LazyLock::new(|| {
        let schema = SchemaSettings::default()
            .with(|s| s.inline_subschemas = true)
            .into_generator()
            .into_root_schema_for::<Component>()
            .to_value();
        let entries: Vec<Entry> = schema["oneOf"]
            .as_array()
            .expect("adjacently tagged enum schema is a oneOf")
            .iter()
            .map(|v| Entry {
                name: v["properties"]["type"]["const"]
                    .as_str()
                    .expect("variant tag is a const string")
                    .to_owned(),
                props: v["properties"]["props"].clone(),
            })
            .collect();
        let mut names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        let names = names.join(", ");
        Catalog { entries, names }
    });
    &CATALOG
}

/// The one-line catalog for the prompt block: `Card(title?, description?) · Grid(columns) · …`
/// in catalog order; required props first in declaration order, optional props (`?`) after,
/// item shapes of object arrays inline as `items[{key, value}]`.
pub fn catalog_compact() -> String {
    catalog()
        .entries
        .iter()
        .map(|e| format!("{}({})", e.name, fields(&e.props)))
        .collect::<Vec<_>>()
        .join(" · ")
}

/// JSON Schema of [`Spec`] for the `visualize` skill.
pub fn catalog_schema() -> Value {
    schemars::schema_for!(Spec).to_value()
}

fn fields(schema: &Value) -> String {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|a| a.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(props) = schema["properties"].as_object() else {
        return String::new();
    };
    let mut optional: Vec<&str> = props
        .keys()
        .map(String::as_str)
        .filter(|k| !required.contains(k))
        .collect();
    optional.sort_unstable();
    required
        .iter()
        .map(|k| (*k, ""))
        .chain(optional.iter().map(|k| (*k, "?")))
        .map(|(k, opt)| {
            let items = &props[k]["items"];
            if items["properties"].is_object() {
                format!("{k}[{{{}}}]{opt}", fields(items))
            } else {
                format!("{k}{opt}")
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract's thirty-six names, in catalog order.
    const NAMES: [&str; 36] = [
        "Card",
        "Stack",
        "Grid",
        "Separator",
        "Heading",
        "Text",
        "Badge",
        "Alert",
        "Table",
        "KeyValueList",
        "Metric",
        "MetricGrid",
        "Progress",
        "Bars",
        "Timeline",
        "Checklist",
        "Tabs",
        "Accordion",
        "Graph",
        "Steps",
        "Callout",
        "Code",
        "Diff",
        "Tree",
        "Glossary",
        "Quote",
        "Image",
        "LinkList",
        "Tags",
        "Gauge",
        "Sparkline",
        "Heatmap",
        "Person",
        "FileList",
        "Kanban",
        "Calendar",
    ];

    #[test]
    fn catalog_matches_component_variants() {
        let names: Vec<&str> = catalog().entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, NAMES);
        assert!(catalog().names.starts_with("Accordion, Alert, "));
        for e in &catalog().entries {
            assert_eq!(e.props["type"], "object", "{}: {}", e.name, e.props);
            assert_eq!(e.props["additionalProperties"], false, "{}", e.name);
        }
    }

    #[test]
    fn compact_lists_every_component_with_props() {
        let compact = catalog_compact();
        let parts: Vec<&str> = compact.split(" · ").collect();
        assert_eq!(parts.len(), 36, "{compact}");
        assert!(compact.starts_with("Card(description?, title?) · Stack(direction?, gap?) · Grid(columns) · Separator() · Heading(text, level?)"), "{compact}");
        assert!(
            compact.contains("KeyValueList(items[{key, value}])"),
            "{compact}"
        );
        assert!(
            compact.contains("Metric(label, value, delta?, spark?, trend?)"),
            "{compact}"
        );
        assert!(compact.contains("Table(columns, rows)"), "{compact}");
        assert!(
            compact.contains("Tabs(labels) · Accordion(titles) · Graph(nodes[{id, label, group?, kind?, state?}], edges[{from, to, label?, style?}], direction?, groups[{id, label}]?) · Steps(items[{title, detail?}]) · Callout(text, title?, variant?)"),
            "{compact}"
        );
        assert!(
            compact.ends_with("Kanban(columns[{title, cards[{title, detail?, tag?}]}]) · Calendar(days[{day, label?, state?}], title?)"),
            "{compact}"
        );
    }

    #[test]
    fn schema_is_the_spec_schema() {
        let schema = catalog_schema();
        assert!(schema["properties"]["root"].is_object());
        assert!(schema["properties"]["elements"].is_object());
    }
}
