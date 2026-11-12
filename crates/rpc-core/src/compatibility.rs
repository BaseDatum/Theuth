//! Conservative, offline compatibility checks for emitted OpenRPC 1.3.2 contracts.
//!
//! This is not a JSON Schema implication solver. Changed unsupported constructs
//! require review; unchanged opaque schemas cannot reveal semantic ABI changes.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value};

/// Compatibility of one structural change, independent of snapshot acceptance.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeClass {
    /// An existing wire contract was removed or changed incompatibly.
    Breaking,
    /// A contract was extended without changing existing slots or receiver needs.
    Additive,
    /// Documentation or a known non-wire representation changed.
    Nonbreaking,
    /// The changed construct has no proven compatibility rule.
    ReviewRequired,
}

/// One classified change at a JSON-pointer-like contract path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Change {
    /// Compatibility decision.
    pub class: ChangeClass,
    /// Method, parameter position, topic/version, or schema property path.
    pub path: String,
    /// Why the decision was made, including unsupported constructs.
    pub reason: String,
}

/// All observed changes; an empty report means identical documents.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// Deterministically ordered traversal results.
    pub changes: Vec<Change>,
}

impl Report {
    /// Whether the diff must block same-family snapshot acceptance.
    pub fn fails(&self) -> bool {
        self.changes.iter().any(|c| matches!(c.class, ChangeClass::Breaking | ChangeClass::ReviewRequired))
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum View { Input, Output, Event }

/// Compare resolved contracts, not component names or unordered catalog layout.
///
/// Only documentation, descriptions, summaries, titles and examples are ignored
/// as annotations. Defaults, formats, extensions and unknown keywords are not.
/// Local refs are followed without fetching external resources. Event extensions
/// are additive only if both documents explicitly promise unknown-variant ignore.
pub fn classify(old: &Value, new: &Value) -> Report {
    let mut check = Check { old, new, report: Report::default(), visited: BTreeSet::new() };
    for (doc, side) in [(old, "old"), (new, "new")] {
        if doc["openrpc"] != "1.3.2" || !doc["methods"].is_array() {
            check.add(ChangeClass::ReviewRequired, "/", format!("{side} document is not an OpenRPC 1.3.2 method catalog"));
        }
    }
    check.catalog(&old["methods"], &new["methods"], "/methods", &["name"], |c, a, b, p| c.method(a, b, p));
    let a = &old["x-theuth"];
    let b = &new["x-theuth"];
    check.catalog(&a["subscriptions"], &b["subscriptions"], "/x-theuth/subscriptions", &["subscribe"], |c, a, b, p| {
        c.params(&a["params"], &b["params"], &path(p, "params"));
        c.schema(&a["itemSchema"], &b["itemSchema"], &path(p, "itemSchema"), View::Output, Some(&b["receiverSchema"]));
        c.schema(&a["receiverSchema"], &b["receiverSchema"], &path(p, "receiverSchema"), View::Input, None);
        c.fields(a, b, p, &["subscribe", "params", "itemSchema", "receiverSchema"], &["unsubscribe", "notification", "aliases", "unsubscribeAliases"]);
    });
    check.catalog(&a["events"], &b["events"], "/x-theuth/events", &["topic", "version"], |c, a, b, p| {
        let view = if old["x-theuth"]["delivery"]["unknownTopicVersionVariant"] == "ignore"
            && new["x-theuth"]["delivery"]["unknownTopicVersionVariant"] == "ignore" { View::Event } else { View::Output };
        c.schema(&a["schema"], &b["schema"], &path(p, "schema"), view, Some(&b["receiverSchema"]));
        c.schema(&a["receiverSchema"], &b["receiverSchema"], &path(p, "receiverSchema"), view, None);
        c.fields(a, b, p, &["topic", "version", "schema", "receiverSchema"], &[]);
    });
    check.schema(&a["eventEnvelopeSchema"], &b["eventEnvelopeSchema"], "/x-theuth/eventEnvelopeSchema", View::Output, None);
    check.fields(a, b, "/x-theuth", &["subscriptions", "events", "eventEnvelopeSchema", "plugins"], &[]);
    // Plugin ordering is catalog layout, but changing declared availability needs review.
    if sorted(&a["plugins"]) != sorted(&b["plugins"]) {
        check.add(ChangeClass::ReviewRequired, "/x-theuth/plugins", "declared plugin set changed");
    }
    check.fields(&old["info"], &new["info"], "/info", &[], &[]);
    check.fields(&old["components"], &new["components"], "/components", &["schemas"], &[]);
    check.fields(old, new, "", &["methods", "x-theuth", "components", "info"], &[]);
    if old != new && check.report.changes.is_empty() {
        check.add(ChangeClass::Nonbreaking, "/", "catalog order or resolved component layout changed without changing the contract");
    }
    check.report
}

struct Check<'a> {
    old: &'a Value,
    new: &'a Value,
    report: Report,
    // Active pairs stop recursion, then are removed so shared types report every path.
    visited: BTreeSet<(usize, usize, View, usize)>,
}

fn path(base: &str, key: &str) -> String {
    format!("{base}/{}", key.replace('~', "~0").replace('/', "~1"))
}

fn docs(key: &str) -> bool {
    matches!(key, "description" | "summary" | "title" | "examples" | "example" | "externalDocs")
}

fn keys<'a>(a: &'a Value, b: &'a Value) -> BTreeSet<&'a str> {
    [a, b].into_iter().filter_map(Value::as_object).flat_map(Map::keys).map(String::as_str).collect()
}

fn sorted(value: &Value) -> Vec<String> {
    let mut entries: Vec<_> = value.as_array().into_iter().flatten().map(Value::to_string).collect();
    entries.sort();
    entries
}

fn required(schema: &Value, name: &str) -> bool {
    schema["required"].as_array().is_some_and(|items| items.iter().any(|v| v == name))
}

// A subtree with a ref is not identical merely because its ref spelling is:
// the target may have changed elsewhere in the document.
fn has_ref(value: &Value) -> bool {
    match value {
        Value::Object(map) => map.contains_key("$ref") || map.values().any(has_ref),
        Value::Array(items) => items.iter().any(has_ref),
        _ => false,
    }
}

fn resolve<'a>(doc: &'a Value, mut schema: &'a Value) -> Result<&'a Value, String> {
    let mut seen = BTreeSet::new();
    loop {
        if !seen.insert(schema as *const Value as usize) {
            return Err("reference-only cycle has no structural schema".into());
        }
        if let Some(reference) = schema.get("$ref") {
            if schema.as_object().is_some_and(|m| m.keys().any(|k| k != "$ref" && !docs(k))) {
                return Err("$ref siblings require review under Draft 7".into());
            }
            let reference = reference.as_str().and_then(|r| r.strip_prefix('#')).filter(|r| r.starts_with('/') || r.is_empty())
                .ok_or_else(|| "only local JSON-pointer $refs are supported".to_owned())?;
            let pointer = percent_encoding::percent_decode_str(reference).decode_utf8().map_err(|e| e.to_string())?;
            schema = doc.pointer(&pointer).ok_or_else(|| format!("unresolved local $ref #{reference}"))?;
        } else if let Some(items) = schema.get("allOf").and_then(Value::as_array)
            && items.len() == 1
            && schema.as_object().is_some_and(|m| m.keys().all(|k| k == "allOf" || docs(k))) {
            schema = &items[0];
        } else {
            return Ok(schema);
        }
    }
}

// Only a common required string discriminator proves that a union is a variant
// catalog. Arbitrary union growth can change nullability or oneOf overlap.
fn tagged_variants(doc: &Value, branches: &[Value]) -> bool {
    let Some(first) = branches.first().and_then(|s| resolve(doc, s).ok()) else { return false; };
    first["properties"].as_object().is_some_and(|properties| properties.keys().any(|tag| {
        let mut values = BTreeSet::new();
        branches.iter().all(|branch| {
            let Ok(branch) = resolve(doc, branch) else { return false; };
            let Some(schema) = branch["properties"].get(tag).and_then(|s| resolve(doc, s).ok()) else { return false; };
            let value = schema["const"].as_str().or_else(|| schema["enum"].as_array()
                .filter(|items| items.len() == 1).and_then(|items| items[0].as_str()));
            required(branch, tag) && value.is_some_and(|v| values.insert(v))
        })
    }))
}

impl Check<'_> {
    fn add(&mut self, class: ChangeClass, path: &str, reason: impl Into<String>) {
        self.report.changes.push(Change { class, path: path.to_owned(), reason: reason.into() });
    }

    fn fields(&mut self, a: &Value, b: &Value, p: &str, handled: &[&str], breaking: &[&str]) {
        for key in keys(a, b) {
            if handled.contains(&key) || a.get(key) == b.get(key) { continue; }
            let class = if docs(key) { ChangeClass::Nonbreaking }
                else if breaking.contains(&key) { ChangeClass::Breaking } else { ChangeClass::ReviewRequired };
            self.add(class, &path(p, key), format!("{key} changed{}", if class == ChangeClass::ReviewRequired { "; unsupported construct" } else { "" }));
        }
    }

    fn index<'a>(&mut self, value: &'a Value, p: &str, identity: &[&str]) -> BTreeMap<String, &'a Value> {
        let mut map = BTreeMap::new();
        let Some(items) = value.as_array() else {
            if !value.is_null() { self.add(ChangeClass::ReviewRequired, p, "catalog must be an array"); }
            return map;
        };
        for item in items {
            if identity.iter().any(|k| item.get(k).is_none_or(|v| !(v.is_string() || (*k == "version" && v.is_u64())))) {
                self.add(ChangeClass::ReviewRequired, p, "invalid catalog identity");
                continue;
            }
            let name = identity.iter().map(|k| item[*k].as_str().map(str::to_owned).unwrap_or_else(|| item[*k].to_string())).collect::<Vec<_>>().join("@");
            if map.insert(name.clone(), item).is_some() {
                self.add(ChangeClass::ReviewRequired, &path(p, &name), "duplicate catalog identity");
            }
        }
        map
    }

    fn catalog(&mut self, a: &Value, b: &Value, p: &str, identity: &[&str], compare: impl Fn(&mut Self, &Value, &Value, &str)) {
        let old = self.index(a, p, identity);
        let new = self.index(b, p, identity);
        for (name, a) in &old {
            let p = path(p, name);
            if let Some(b) = new.get(name) { compare(self, a, b, &p); }
            else { self.add(ChangeClass::Breaking, &p, "existing catalog entry removed or renamed"); }
        }
        for name in new.keys().filter(|k| !old.contains_key(*k)) {
            self.add(ChangeClass::Additive, &path(p, name), "new catalog entry; consumers must check remote support");
        }
    }

    fn method(&mut self, a: &Value, b: &Value, p: &str) {
        self.params(&a["params"], &b["params"], &path(p, "params"));
        let a_result = &a["result"];
        let b_result = &b["result"];
        self.schema(&a_result["schema"], &b_result["schema"], &path(&path(p, "result"), "schema"), View::Output, Some(&b_result["x-theuth-receiverSchema"]));
        self.schema(&a_result["x-theuth-receiverSchema"], &b_result["x-theuth-receiverSchema"], &path(&path(p, "result"), "x-theuth-receiverSchema"), View::Input, None);
        self.fields(a_result, b_result, &path(p, "result"), &["schema", "x-theuth-receiverSchema"], &["name", "required"]);
        self.fields(a, b, p, &["name", "params", "result"], &["paramStructure", "x-theuth-namedParams", "x-theuth-aliasFor"]);
    }

    fn params(&mut self, a: &Value, b: &Value, p: &str) {
        let (Some(old), Some(new)) = (a.as_array(), b.as_array()) else {
            if a != b { self.add(ChangeClass::ReviewRequired, p, "params must be ordered arrays"); }
            return;
        };
        for (i, a) in old.iter().enumerate() {
            let p = path(p, &i.to_string());
            if let Some(b) = new.get(i) {
                self.schema(&a["schema"], &b["schema"], &path(&p, "schema"), View::Input, None);
                self.fields(a, b, &p, &["schema"], &["name", "required", "x-theuth-aliases"]);
            } else { self.add(ChangeClass::Breaking, &p, "existing parameter removed"); }
        }
        for (i, param) in new.iter().enumerate().skip(old.len()) {
            self.add(if param["required"] == false { ChangeClass::Additive } else { ChangeClass::Breaking },
                &path(p, &i.to_string()), "appended parameter; only an explicitly omittable trailing slot is additive (null is not omission)");
        }
    }

    fn schema(&mut self, a: &Value, b: &Value, p: &str, view: View, receiver: Option<&Value>) {
        if a == b && !has_ref(a) { return; }
        let (a, b) = match (resolve(self.old, a), resolve(self.new, b)) {
            (Ok(a), Ok(b)) => (a, b),
            (a, b) => {
                self.add(ChangeClass::ReviewRequired, p, a.err().or_else(|| b.err()).unwrap());
                return;
            }
        };
        let receiver = receiver.and_then(|r| resolve(self.new, r).ok());
        let pair = (a as *const Value as usize, b as *const Value as usize, view, receiver.map_or(0, |r| r as *const Value as usize));
        if !self.visited.insert(pair) { return; }
        self.schema_body(a, b, p, view, receiver);
        self.visited.remove(&pair);
    }

    fn schema_body(&mut self, a: &Value, b: &Value, p: &str, view: View, receiver: Option<&Value>) {
        if a == b && !has_ref(a) { return; }
        let (Some(_), Some(_)) = (a.as_object(), b.as_object()) else {
            self.add(if (a.is_boolean() || a.is_object()) && (b.is_boolean() || b.is_object()) { ChangeClass::Breaking } else { ChangeClass::ReviewRequired }, p, "boolean/opaque or invalid schema changed");
            return;
        };
        let props_a = &a["properties"];
        let props_b = &b["properties"];
        for key in ["properties", "required"] {
            let valid = |s: &Value| s.get(key).is_none_or(|v| if key == "properties" { v.is_object() }
                else { v.as_array().is_some_and(|items| items.iter().all(Value::is_string)) });
            if !valid(a) || !valid(b) {
                self.add(ChangeClass::ReviewRequired, &path(p, key), "malformed properties/required schema construct");
            }
        }
        for name in keys(props_a, props_b) {
            let property_path = path(&path(p, "properties"), name);
            match (props_a.get(name), props_b.get(name)) {
                (Some(a_prop), Some(b_prop)) => {
                    if required(a, name) != required(b, name) {
                        self.add(ChangeClass::Breaking, &property_path, "requiredness of an existing property changed");
                    }
                    self.schema(a_prop, b_prop, &property_path, view, receiver.and_then(|r| r["properties"].get(name)));
                }
                (Some(_), None) => self.add(ChangeClass::Breaking, &property_path, "existing property removed or renamed, including optional properties"),
                (None, Some(_)) => {
                    let optional = if view != View::Input {
                        receiver.map_or(!required(b, name), |r| r["properties"].get(name).is_some() && !required(r, name))
                    } else { !required(b, name) };
                    self.add(if optional { ChangeClass::Additive } else { ChangeClass::Breaking }, &property_path,
                        "property added; absence must be accepted by the input/receiver view");
                }
                _ => unreachable!(),
            }
        }
        for key in keys(a, b) {
            let x = &a[key];
            let y = &b[key];
            if matches!(key, "properties" | "required" | "definitions" | "$defs") { continue; }
            if x == y && !has_ref(x) { continue; }
            let p = path(p, key);
            match key {
                key if docs(key) => self.add(ChangeClass::Nonbreaking, &p, "documentation changed"),
                "type" | "const" => {
                    if key != "type" || sorted(x) != sorted(y) || !x.is_array() || !y.is_array() {
                        self.add(ChangeClass::Breaking, &p, "type, nullability or tag changed");
                    }
                }
                "enum" => {
                    let (Some(x), Some(y)) = (x.as_array(), y.as_array()) else {
                        self.add(ChangeClass::ReviewRequired, &p, "enum must be an array"); continue;
                    };
                    if x.iter().any(|v| !y.contains(v)) { self.add(ChangeClass::Breaking, &p, "enum member removed or renamed"); }
                    if y.iter().any(|v| !x.contains(v)) {
                        if y.contains(&Value::Null) && !x.contains(&Value::Null) {
                            self.add(ChangeClass::Breaking, &p, "enum nullability changed");
                        } else { self.variant(&p, view); }
                    }
                }
                "items" if x.is_array() && y.is_array() => {
                    let (x, y) = (x.as_array().unwrap(), y.as_array().unwrap());
                    if x.len() != y.len() { self.add(ChangeClass::Breaking, &p, "tuple length changed"); }
                    for (i, (x, y)) in x.iter().zip(y).enumerate() {
                        self.schema(x, y, &path(&p, &i.to_string()), view, receiver.and_then(|r| r["items"].get(i)));
                    }
                }
                "items" | "additionalProperties" if x.is_object() && y.is_object() => {
                    self.schema(x, y, &p, view, receiver.and_then(|r| r.get(key)));
                }
                "anyOf" | "oneOf" if x.is_array() && y.is_array() => {
                    let (x, y) = (x.as_array().unwrap(), y.as_array().unwrap());
                    if x.len() > y.len() { self.add(ChangeClass::Breaking, &p, "union variant removed"); }
                    if x.len() < y.len() {
                        if y.iter().skip(x.len()).any(|s| resolve(self.new, s).is_ok_and(|s| s["type"] == "null")) {
                            self.add(ChangeClass::Breaking, &p, "union nullability changed");
                        } else if tagged_variants(self.old, x) && tagged_variants(self.new, y) {
                            self.variant(&p, view);
                        } else {
                            self.add(ChangeClass::ReviewRequired, &p, "union extension is not a proven tagged variant");
                        }
                    }
                    for (i, (x, y)) in x.iter().zip(y).enumerate() {
                        self.schema(x, y, &path(&p, &i.to_string()), view, receiver.and_then(|r| r[key].get(i)));
                    }
                }
                "minimum" | "exclusiveMinimum" | "minLength" | "minItems" | "minProperties" => {
                    self.constraint(x, y, &p, true);
                }
                "maximum" | "exclusiveMaximum" | "maxLength" | "maxItems" | "maxProperties" => {
                    self.constraint(x, y, &p, false);
                }
                "pattern" | "format" | "additionalProperties" | "additionalItems" | "uniqueItems" | "items" => {
                    self.add(ChangeClass::Breaking, &p, "container, map value or accepted constraint changed");
                }
                // Do not erase defaults, validation extensions or unfamiliar
                // combinators: their compatibility is not proven.
                _ => self.add(ChangeClass::ReviewRequired, &p, format!("changed unsupported schema keyword {key}")),
            }
        }
        // Required names without property schemas cannot be handled by the property walk.
        if sorted(&a["required"]) != sorted(&b["required"]) {
            let names = [a, b].into_iter().filter_map(|s| s["required"].as_array()).flatten();
            if names.filter_map(Value::as_str).any(|n| props_a.get(n).is_none() && props_b.get(n).is_none()) {
                self.add(ChangeClass::ReviewRequired, &path(p, "required"), "required name has no property schema");
            }
        }
    }

    fn variant(&mut self, p: &str, view: View) {
        self.add(if view == View::Output { ChangeClass::ReviewRequired } else { ChangeClass::Additive }, p,
            if view == View::Output { "output variant added without a documented consumer fallback" }
            else { "variant added; inputs require remote-value support checks, events require explicit ignore policy" });
    }

    fn constraint(&mut self, a: &Value, b: &Value, p: &str, lower: bool) {
        let tightened = match (a.as_f64(), b.as_f64()) {
            (None, Some(_)) if a.is_null() => true,
            (Some(a), Some(b)) => if lower { b > a } else { b < a },
            _ => false,
        };
        self.add(if tightened { ChangeClass::Breaking } else { ChangeClass::ReviewRequired }, p,
            if tightened { "validation constraint tightened" } else { "constraint change not proven compatible in both peer directions" });
    }
}

#[cfg(test)]
#[path = "compatibility_tests.rs"]
mod tests;
