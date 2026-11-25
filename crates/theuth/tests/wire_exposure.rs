//! 216 §7.3 guards for what external plugins may see of the wire: the exposure snapshot
//! (`wire_exposure.txt`, one line per `topic#type`), the sensitive-name lint over every public
//! variant's schema, and the same-`TOPIC` agreement rule.
//!
//! The snapshot is rewritten with `UPDATE_WIRE_EXPOSURE=1 cargo test -p theuth --test
//! wire_exposure` after reviewing its diff, so every new variant is reviewed in the change that
//! adds it.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use runtime::{Plugin, Runtime};
use schemars::generate::SchemaSettings;
use serde_json::Value;
use wire::{EventDescriptor, Exposure, RuntimeWire as _};

/// Property-name segments that suggest a credential (216 §7.3, review F5).
const SENSITIVE: &[&str] = &[
    "key",
    "token",
    "secret",
    "password",
    "passcode",
    "code",
    "pin",
    "pairing",
    "cookie",
    "credential",
];

fn no_methods(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    jsonrpsee::RpcModule::new(server)
}

/// The production plugin list (`engine_plugins` in `main.rs`), declaration only. A plugin
/// missing here shows up as a topic of the tracked contract this catalog lacks.
fn plugins() -> Vec<Box<dyn Plugin>> {
    vec![
        Box::new(tools::ToolsPlugin),
        Box::new(secrets::SecretsPlugin),
        Box::new(document::DocumentPlugin),
        Box::new(knowledge::KnowledgePlugin),
        Box::new(web::WebPlugin),
        Box::new(browser::BrowserPlugin),
        Box::new(scrape::ScrapePlugin),
        Box::new(codeindex::CodeindexPlugin),
        Box::new(routes::RoutesPlugin),
        Box::new(computer::ComputerPlugin),
        Box::new(artifact::ArtifactPlugin),
        Box::new(app::AppPlugin),
        Box::new(search_providers::SearchProvidersPlugin),
        Box::new(agent::AgentPlugin),
        Box::new(dreaming::DreamingPlugin),
        Box::new(autoresearch::AutoresearchPlugin),
        Box::new(cells::CellsPlugin),
        Box::new(rpc_compose::SearchToolsPlugin),
        Box::new(signal::SignalPlugin {
            channel: false,
            link: None,
        }),
        Box::new(slack::SlackPlugin { channel: false }),
        Box::new(telegram::TelegramPlugin { channel: false }),
        Box::new(link::LinkPlugin),
        Box::new(nodes::NodesPlugin),
        Box::new(schedule::SchedulePlugin),
        Box::new(mesh::MeshPlugin),
        Box::new(replica::ReplicaPlugin),
        Box::new(sessionsync::SessionSyncPlugin),
        Box::new(roster::RosterPlugin),
        Box::new(webprofile::WebProfilePlugin),
        Box::new(tasks::TasksPlugin),
        Box::new(sandbox::SandboxPlugin),
        Box::new(share::SharePlugin::engine()),
        Box::new(plugins::PluginsPlugin),
        Box::new(a2a::A2aPlugin),
        Box::new(update::UpdatePlugin),
        Box::new(voice::VoicePlugin),
        Box::new(talk::TalkPlugin),
        Box::new(decision::DecisionPlugin),
        Box::new(double::DoublePlugin),
        Box::new(meeting::MeetingPlugin),
        Box::new(rpc_compose::RpcPlugin {
            implicit: true,
            module: no_methods,
        }),
    ]
}

async fn catalog() -> Vec<EventDescriptor> {
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    for mut plugin in plugins() {
        let name = plugin.name();
        plugin
            .declare_wire(&rt)
            .await
            .unwrap_or_else(|e| panic!("{name}: {e:#}"));
    }
    rt.freeze_wire().await.unwrap();
    rt.wire_schema()
}

/// One topic's schema, its definitions resolved on demand.
struct Doc {
    root: Value,
    defs: serde_json::Map<String, Value>,
}

impl Doc {
    fn of(descriptor: &EventDescriptor) -> Self {
        let mut generator = SchemaSettings::draft07().into_generator();
        let root = (descriptor.schema)(&mut generator).to_value();
        let defs = generator.take_definitions(true);
        Self { root, defs }
    }

    /// Follow `$ref`s to a schema with content.
    fn resolve<'a>(&'a self, mut schema: &'a Value) -> &'a Value {
        for _ in 0..32 {
            let Some(name) = schema
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|r| r.strip_prefix("#/definitions/"))
            else {
                return schema;
            };
            schema = &self.defs[name];
        }
        panic!("$ref cycle without content")
    }

    /// The branches a value of `schema` may take (`oneOf`/`anyOf`, else itself).
    fn branches<'a>(&'a self, schema: &'a Value) -> Vec<&'a Value> {
        let schema = self.resolve(schema);
        for key in ["oneOf", "anyOf"] {
            if let Some(list) = schema.get(key).and_then(Value::as_array) {
                return list.iter().flat_map(|b| self.branches(b)).collect();
            }
        }
        vec![schema]
    }

    /// Every property of an object schema, `allOf` parts and `$ref`s merged.
    fn properties<'a>(&'a self, schema: &'a Value, out: &mut BTreeMap<String, &'a Value>) {
        let schema = self.resolve(schema);
        if let Some(props) = schema.get("properties").and_then(Value::as_object) {
            for (name, value) in props {
                out.entry(name.clone()).or_insert(value);
            }
        }
        for part in schema
            .get("allOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.properties(part, out);
        }
    }

    /// The variants of the topic as `(type tag, schema)`; `""` for an untagged topic.
    fn variants(&self) -> Vec<(String, &Value)> {
        let mut out: Vec<_> = self
            .branches(&self.root)
            .into_iter()
            .map(|branch| {
                let mut props = BTreeMap::new();
                self.properties(branch, &mut props);
                let tag = props
                    .get("type")
                    .map(|t| self.resolve(t))
                    .and_then(|t| {
                        t.get("const")
                            .or_else(|| t.get("enum").and_then(|e| e.get(0)))
                    })
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                (tag, branch)
            })
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.dedup_by(|a, b| a.0 == b.0);
        out
    }

    /// Paths (`field.sub`) under `schema` whose name has a sensitive segment.
    fn sensitive(
        &self,
        schema: &Value,
        path: &str,
        seen: &mut Vec<String>,
        hits: &mut Vec<String>,
    ) {
        if let Some(name) = schema.get("$ref").and_then(Value::as_str) {
            if seen.iter().any(|s| s == name) {
                return;
            }
            seen.push(name.to_owned());
            self.sensitive(self.resolve(schema), path, seen, hits);
            seen.pop();
            return;
        }
        for key in ["oneOf", "anyOf", "allOf"] {
            for branch in schema
                .get(key)
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                self.sensitive(branch, path, seen, hits);
            }
        }
        if let Some(items) = schema.get("items") {
            for item in items
                .as_array()
                .map_or_else(|| vec![items], |a| a.iter().collect())
            {
                self.sensitive(item, path, seen, hits);
            }
        }
        if let Some(extra) = schema.get("additionalProperties").filter(|v| v.is_object()) {
            self.sensitive(extra, path, seen, hits);
        }
        for (name, value) in schema
            .get("properties")
            .and_then(Value::as_object)
            .into_iter()
            .flatten()
        {
            let here = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            if snake(name).split('_').any(|seg| SENSITIVE.contains(&seg)) {
                hits.push(here.clone());
            }
            self.sensitive(value, &here, seen, hits);
        }
    }
}

/// `camelCase`/`kebab-case` → `snake_case`.
fn snake(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_ascii_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.push(c.to_ascii_lowercase());
        } else if c == '-' {
            out.push('_');
        } else {
            out.push(c);
        }
    }
    out
}

/// `path` is `rule` or lies under it.
fn covered(path: &str, rule: &str) -> bool {
    path == rule
        || path
            .strip_prefix(rule)
            .is_some_and(|rest| rest.starts_with('.'))
}

fn snapshot(catalog: &[EventDescriptor]) -> String {
    let mut out = String::new();
    for descriptor in catalog {
        let topic = descriptor.topic;
        if descriptor.exposure != Exposure::Public {
            out += &format!("{topic} {}\n", descriptor.exposure.as_str());
            continue;
        }
        for (tag, _) in Doc::of(descriptor).variants() {
            let name = if tag.is_empty() {
                topic.to_owned()
            } else {
                format!("{topic}#{tag}")
            };
            if descriptor.drop.contains(&tag.as_str()) {
                out += &format!("{name} dropped\n");
                continue;
            }
            let mine = |rules: &[&str]| -> Vec<String> {
                let prefix = format!("{tag}.");
                rules
                    .iter()
                    .filter_map(|r| r.strip_prefix(&prefix).map(str::to_owned))
                    .collect()
            };
            let mut line = format!("{name} public");
            let strip = mine(descriptor.strip);
            if !strip.is_empty() {
                line += &format!(" strip={}", strip.join(","));
            }
            let reviewed = mine(descriptor.reviewed_public);
            if !reviewed.is_empty() {
                line += &format!(" reviewed={}", reviewed.join(","));
            }
            out += &line;
            out.push('\n');
        }
        for rule in descriptor.conditional {
            out += &format!("{topic} conditional: {rule}\n");
        }
    }
    out
}

/// The exposure snapshot: any change fails until the file is rewritten after review.
#[tokio::test]
async fn wire_exposure_snapshot() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/wire_exposure.txt");
    let generated = snapshot(&catalog().await);
    if std::env::var_os("UPDATE_WIRE_EXPOSURE").is_some() {
        std::fs::write(&path, &generated).unwrap();
        return;
    }
    let tracked = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        tracked == generated,
        "wire exposure changed; review, then rewrite with UPDATE_WIRE_EXPOSURE=1:\n{}",
        diff(&tracked, &generated)
    );
}

fn diff(old: &str, new: &str) -> String {
    let (old, new): (BTreeSet<_>, BTreeSet<_>) = (old.lines().collect(), new.lines().collect());
    let removed = old.difference(&new).map(|l| format!("- {l}"));
    let added = new.difference(&old).map(|l| format!("+ {l}"));
    removed.chain(added).collect::<Vec<_>>().join("\n")
}

/// Every public, undropped variant: a property named like a credential is stripped or reviewed.
#[tokio::test]
async fn sensitive_names_are_stripped_or_reviewed() {
    let mut unreviewed = Vec::new();
    let mut stale = Vec::new();
    for descriptor in catalog()
        .await
        .iter()
        .filter(|d| d.exposure == Exposure::Public)
    {
        let doc = Doc::of(descriptor);
        let mut hits = Vec::new();
        for (tag, schema) in doc.variants() {
            if descriptor.drop.contains(&tag.as_str()) {
                continue;
            }
            let mut found = Vec::new();
            doc.sensitive(schema, &tag, &mut Vec::new(), &mut found);
            // A tagged variant's paths start with its tag; an untagged topic's do not.
            hits.extend(found.into_iter().filter(|p| p != "type"));
        }
        for hit in &hits {
            let rules = descriptor.strip.iter().chain(descriptor.reviewed_public);
            if !rules.clone().any(|rule| covered(hit, rule)) {
                unreviewed.push(format!("{}: {hit}", descriptor.topic));
            }
        }
        for rule in descriptor.reviewed_public {
            if !hits.iter().any(|hit| covered(hit, rule)) {
                stale.push(format!("{}: {rule}", descriptor.topic));
            }
        }
    }
    unreviewed.sort();
    unreviewed.dedup();
    assert!(
        unreviewed.is_empty(),
        "public fields named like credentials; STRIP them or list them in the owner's \
         REVIEWED_PUBLIC after review (216 §7.3):\n{}",
        unreviewed.join("\n")
    );
    assert!(
        stale.is_empty(),
        "REVIEWED_PUBLIC entries that match no field:\n{}",
        stale.join("\n")
    );
}

/// The catalog this test declares is the production one, and the tracked contract carries the
/// same exposure for every topic.
#[tokio::test]
async fn catalog_matches_the_tracked_contract() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/wire/openrpc-v1.json");
    let document: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    let tracked: BTreeMap<&str, Option<&str>> = document["x-theuth"]["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|e| (e["topic"].as_str().unwrap(), e["exposure"].as_str()))
        .collect();
    let catalog = catalog().await;
    let declared: BTreeMap<&str, Option<&str>> = catalog
        .iter()
        .map(|d| (d.topic, Some(d.exposure.as_str())))
        .collect();
    assert_eq!(tracked, declared);
}

/// A second impl of a declared `TOPIC` whose exposure consts disagree (a relay test twin left
/// public) cannot be declared, in any runtime of the process.
#[tokio::test]
async fn same_topic_impls_must_agree() {
    #[derive(Clone, serde::Serialize, schemars::JsonSchema)]
    struct Twin;
    impl wire::WireEvent for Twin {
        const TOPIC: &'static str = "browser.web-profile-changed";
    }
    catalog().await;
    let rt = Runtime::new(figment::Figment::new(), std::env::temp_dir());
    let err = rt.register_wire_event::<Twin>().await.unwrap_err();
    assert!(err.to_string().contains("disagreeing exposure"), "{err}");
}
