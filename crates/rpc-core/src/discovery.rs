//! Lazy owner contracts and OpenRPC 1.3.2 / JSON Schema Draft 7 assembly.

use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};
use schemars::{JsonSchema, Schema, SchemaGenerator, generate::SchemaSettings, transform::transform_subschemas};
use serde_json::{Value, json};

/// Consumers choose the schema dialect and serialization direction.
pub type SchemaFn = fn(&mut SchemaGenerator) -> Schema;

/// Typed callback used by the owner-trait macro; never generates a root `#` ref.
pub fn schema_for<T: ?Sized + JsonSchema>(generator: &mut SchemaGenerator) -> Schema {
    generator.subschema_for::<T>()
}

/// Visibility metadata, not authorization or a service-readiness claim.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Audience {
    /// Authenticated owner/device surface.
    #[default]
    Owner,
    /// Internal paired-engine surface; excluded from owner-only projections.
    Mesh,
}

impl Audience {
    fn name(self) -> &'static str {
        match self { Self::Owner => "owner", Self::Mesh => "mesh" }
    }
}

/// An ordered user argument, without injected Extensions or subscription sinks.
#[derive(Clone, Debug)]
pub struct ParamDescriptor {
    /// Named-input spelling after any `argument(rename = "...")`.
    pub name: &'static str,
    /// jsonrpsee's snake_case and lowerCamelCase named-input aliases.
    pub aliases: Vec<&'static str>,
    /// Only a trailing suffix of optional parameters may be omitted positionally.
    pub optional: bool,
    /// Accepted JSON, generated using the deserialize contract.
    pub schema: SchemaFn,
}

/// A unary request and its absolute wire aliases.
#[derive(Clone, Debug)]
pub struct MethodDescriptor {
    /// Stable owner/domain identifier, used to scope shared schema definitions.
    pub owner: &'static str,
    /// Canonical registered name, including any jsonrpsee namespace.
    pub name: &'static str,
    /// Method aliases are absolute names, unaffected by the trait's namespace.
    pub aliases: Vec<&'static str>,
    /// Declaration order is the positional wire ABI.
    pub params: Vec<ParamDescriptor>,
    /// Success payload inside RpcResult; used for output and receiver contracts.
    pub result: SchemaFn,
    /// Visibility metadata; serving code must still authorize every request.
    pub audience: Audience,
}

/// A direct stream; distinct from the generic wire event feed.
#[derive(Clone, Debug)]
pub struct SubscriptionDescriptor {
    /// Stable owner/domain identifier.
    pub owner: &'static str,
    /// Canonical subscription-start method.
    pub subscribe: &'static str,
    /// Canonical generated unsubscribe method.
    pub unsubscribe: &'static str,
    /// Method field on notifications, including any namespace.
    pub notification: &'static str,
    /// Absolute aliases of the subscription-start method.
    pub aliases: Vec<&'static str>,
    /// Absolute aliases of the unsubscribe method.
    pub unsubscribe_aliases: Vec<&'static str>,
    /// Ordered start parameters; the unsubscribe takes only a subscription ID.
    pub params: Vec<ParamDescriptor>,
    /// Stream payload, not the subscription-start response's ID.
    pub item: SchemaFn,
    /// Visibility shared by the subscription pair.
    pub audience: Audience,
}

/// One owner's generated declarations, including every subscription pair member.
#[derive(Clone, Debug, Default)]
pub struct Descriptors {
    /// Unary requests.
    pub methods: Vec<MethodDescriptor>,
    /// Streams and both members of each registered pair.
    pub subscriptions: Vec<SubscriptionDescriptor>,
}

impl Descriptors {
    /// Reject duplicate descriptors, missing metadata and metadata without a method.
    pub fn validate_methods<'a>(&self, registered: impl IntoIterator<Item = &'a str>) -> Result<()> {
        let described = self.method_names()?;
        let registered: BTreeSet<_> = registered.into_iter().collect();
        let missing: Vec<_> = registered.difference(&described).copied().collect();
        let extra: Vec<_> = described.difference(&registered).copied().collect();
        ensure!(missing.is_empty() && extra.is_empty(),
            "RPC descriptor mismatch: missing descriptors {missing:?}; descriptors without methods {extra:?}");
        Ok(())
    }

    fn method_names(&self) -> Result<BTreeSet<&str>> {
        let mut names = BTreeSet::new();
        let unary = self.methods.iter().flat_map(|m| std::iter::once(m.name).chain(m.aliases.iter().copied()));
        let streams = self.subscriptions.iter().flat_map(|s| {
            [s.subscribe, s.unsubscribe].into_iter().chain(s.aliases.iter().copied()).chain(s.unsubscribe_aliases.iter().copied())
        });
        for name in unary.chain(streams) {
            ensure!(names.insert(name), "duplicate RPC descriptor: {name}");
        }
        Ok(names)
    }
}

/// A module and its contracts travel in the same composition-list entry.
/// `.module` has the existing ModuleFactory return type when Context = Server.
pub struct DescribedModule<Context = crate::Server> {
    /// Dispatch surface; `.module` preserves the existing ModuleFactory ABI.
    pub module: jsonrpsee::RpcModule<Context>,
    /// Contracts audited against this exact module.
    pub descriptors: Descriptors,
}

impl<Context> DescribedModule<Context> {
    /// Pair an owner module with descriptors after checking exact name equality.
    pub fn new(module: jsonrpsee::RpcModule<Context>, descriptors: Descriptors) -> Result<Self> {
        descriptors.validate_methods(module.method_names())?;
        Ok(Self { module, descriptors })
    }
}

/// Merge only audited owner units; reject empty lists and duplicate registrations.
pub fn merge_described<Context>(
    modules: impl IntoIterator<Item = DescribedModule<Context>>,
) -> Result<DescribedModule<Context>> {
    let mut modules = modules.into_iter();
    let Some(mut merged) = modules.next() else { bail!("RPC module list must not be empty") };
    merged.descriptors.validate_methods(merged.module.method_names())?;
    for unit in modules {
        unit.descriptors.validate_methods(unit.module.method_names())?;
        merged.module.merge(unit.module)?;
        merged.descriptors.methods.extend(unit.descriptors.methods);
        merged.descriptors.subscriptions.extend(unit.descriptors.subscriptions);
    }
    merged.descriptors.validate_methods(merged.module.method_names())?;
    Ok(merged)
}

/// Inert discovery inputs from the selected composition, never from running actors.
/// Declaration/freeze, audience projection and caching belong to the serving layer.
#[derive(Clone, Debug, Default)]
pub struct Discovery {
    /// Audited method/subscription contracts from module assembly.
    pub descriptors: Descriptors,
    /// Wire registry catalog; the consumer selects Draft 7 and direction.
    pub events: Vec<wire::EventDescriptor>,
    /// Declared composition, not configured or running service state.
    pub plugins: Vec<String>,
}

impl Discovery {
    /// Assemble a deterministic document. Registration equality is checked by
    /// DescribedModule, including aliases and generated unsubscribers.
    pub fn document(&self) -> Result<Value> {
        self.descriptors.method_names()?;
        let mut schemas = Schemas::default();
        let mut methods = Vec::new();
        let mut subscriptions = Vec::new();
        let mut unary: Vec<_> = self.descriptors.methods.iter().collect();
        unary.sort_by_key(|m| (m.owner, m.name));
        for method in unary {
            let scope = format!("method.{}", component_name(method.owner));
            let params = schemas.params(&scope, &method.params)?;
            let result = schemas.schema(&scope, "output", method.result)?;
            let receiver = schemas.schema(&scope, "receiver", method.result)?;
            let entry = method_entry(method.name, method.owner, method.audience, params, result, receiver);
            methods.push(entry.clone());
            add_aliases(&mut methods, &entry, &method.aliases);
        }
        let mut streams: Vec<_> = self.descriptors.subscriptions.iter().collect();
        streams.sort_by_key(|s| (s.owner, s.subscribe));
        for stream in streams {
            let scope = format!("method.{}", component_name(stream.owner));
            let params = schemas.params(&scope, &stream.params)?;
            let id = json!({"type":["integer","string"]});
            let start = method_entry(stream.subscribe, stream.owner, stream.audience, params.clone(), id.clone(), id.clone());
            methods.push(start.clone());
            add_aliases(&mut methods, &start, &stream.aliases);
            let mut stop = method_entry(stream.unsubscribe, stream.owner, stream.audience,
                vec![json!({"name":"subscription_id","required":true,"schema":id})],
                json!({"type":"boolean"}), json!({"type":"boolean"}));
            // jsonrpsee's generated unsubscriber uses Params::one, not named decoding.
            stop["x-theuth-namedParams"] = json!(false);
            methods.push(stop.clone());
            add_aliases(&mut methods, &stop, &stream.unsubscribe_aliases);
            let mut aliases = stream.aliases.clone();
            aliases.sort_unstable();
            let mut unsubscribe_aliases = stream.unsubscribe_aliases.clone();
            unsubscribe_aliases.sort_unstable();
            subscriptions.push(json!({
                "subscribe":stream.subscribe, "unsubscribe":stream.unsubscribe,
                "notification":stream.notification, "params":params,
                "itemSchema":schemas.schema(&scope, "output", stream.item)?,
                "receiverSchema":schemas.schema(&scope, "receiver", stream.item)?,
                "aliases":aliases, "unsubscribeAliases":unsubscribe_aliases,
            }));
        }
        methods.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
        subscriptions.sort_by(|a, b| a["subscribe"].as_str().cmp(&b["subscribe"].as_str()));
        let mut events = Vec::new();
        let mut declared: Vec<_> = self.events.iter().collect();
        declared.sort_by_key(|e| (e.topic, e.version));
        let mut identities = BTreeSet::new();
        for event in declared {
            ensure!(identities.insert((event.topic, event.version)), "duplicate event descriptor: {} v{}", event.topic, event.version);
            let scope = format!("event.{}", component_name(event.topic));
            events.push(json!({"topic":event.topic,"version":event.version,
                "exposure":event.exposure.as_str(),
                "schema":schemas.schema(&scope, "output", event.schema)?,
                "receiverSchema":schemas.schema(&scope, "receiver", event.schema)?,
            }));
        }
        let mut plugins = self.plugins.clone();
        plugins.sort();
        plugins.dedup();
        let components = schemas.finish()?;
        Ok(json!({
            "openrpc":"1.3.2", "info":{"title":"theuth wire","version":"1.0.0"},
            "methods":methods, "components":{"schemas":components},
            "x-theuth":{
                "protocolVersion":{"major":1,"minor":0}, "plugins":plugins,
                "subscriptions":subscriptions, "events":events,
                "eventEnvelopeSchema":{"type":"object","required":["topic","version","event"],
                    "properties":{"topic":{"type":"string"},"version":{"type":"integer"},"event":true}},
                "delivery":{
                    "feed":"live", "unknownTopicVersionVariant":"ignore",
                    "controls":"snapshot-first; Lagged requires resynchronization and a fresh snapshot",
                    "durableReplay":false, "exactlyOnce":false,
                },
            },
        }))
    }
}

fn method_entry(name: &str, owner: &str, audience: Audience, params: Vec<Value>, result: Value, receiver: Value) -> Value {
    json!({"name":name,"paramStructure":"by-position","params":params,
        "result":{"name":"result","schema":result,"x-theuth-receiverSchema":receiver},
        "x-theuth-owner":owner,"x-theuth-audience":audience.name(),"x-theuth-namedParams":true})
}

fn add_aliases(methods: &mut Vec<Value>, entry: &Value, aliases: &[&str]) {
    for alias in aliases {
        let mut entry = entry.clone();
        entry["x-theuth-aliasFor"] = entry["name"].clone();
        entry["name"] = json!(alias);
        methods.push(entry);
    }
}

// Encode scope/name parts injectively, retaining OpenRPC's component-key alphabet.
fn component_name(value: &str) -> String {
    use std::fmt::Write;
    let mut name = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() { name.push(char::from(byte)); }
        else { write!(name, "_{byte:02x}").unwrap(); }
    }
    name
}

#[derive(Default)]
struct Schemas {
    generators: BTreeMap<(String, &'static str), SchemaGenerator>,
    references: BTreeSet<String>,
}

impl Schemas {
    fn params(&mut self, scope: &str, params: &[ParamDescriptor]) -> Result<Vec<Value>> {
        let required_end = params.iter().rposition(|p| !p.optional).map_or(0, |i| i + 1);
        params.iter().enumerate().map(|(i, param)| {
            let mut aliases = param.aliases.clone();
            aliases.sort_unstable();
            aliases.dedup();
            Ok(json!({"name":param.name,"required":i < required_end,
                "schema":self.schema(scope, "input", param.schema)?,"x-theuth-aliases":aliases}))
        }).collect()
    }

    fn schema(&mut self, scope: &str, direction: &'static str, callback: SchemaFn) -> Result<Value> {
        let generator = self.generators.entry((scope.to_owned(), direction)).or_insert_with(|| {
            let settings = SchemaSettings::draft07();
            if direction == "output" { settings.for_serialize() } else { settings.for_deserialize() }.into_generator()
        });
        let mut schema = callback(generator);
        // subschema_for does not apply Draft 7's tuple/ref-sibling transforms.
        for transform in generator.transforms_mut() { transform.transform(&mut schema); }
        rewrite_refs(&mut schema, &format!("{scope}.{direction}"), &mut self.references)?;
        Ok(schema.to_value())
    }

    fn finish(mut self) -> Result<BTreeMap<String, Value>> {
        let mut components = BTreeMap::new();
        for ((scope, direction), mut generator) in self.generators {
            let prefix = format!("{scope}.{direction}");
            for (name, value) in generator.take_definitions(true) {
                let mut schema = Schema::try_from(value)?;
                rewrite_refs(&mut schema, &prefix, &mut self.references)?;
                let key = format!("{prefix}.{}", component_name(&name));
                ensure!(components.insert(key.clone(), schema.to_value()).is_none(), "schema component collision: {key}");
            }
        }
        for reference in self.references {
            ensure!(components.contains_key(&reference), "unresolved schema reference: {reference}");
        }
        Ok(components)
    }
}

// Walk schema keywords, not arbitrary default/example objects that may contain `$ref`.
fn rewrite_refs(schema: &mut Schema, prefix: &str, references: &mut BTreeSet<String>) -> Result<()> {
    let mut error = None;
    let mut visit = |schema: &mut Schema| {
        if schema.get("$id").is_some() {
            error = Some(anyhow::anyhow!("schema $id changes reference scope; unsupported in discovery"));
        }
        if let Some(reference) = schema.get("$ref") {
            let rewritten = (|| -> Result<String> {
                let reference = reference.as_str().ok_or_else(|| anyhow::anyhow!("schema $ref must be a string"))?;
                let name = reference.strip_prefix("#/definitions/").ok_or_else(|| anyhow::anyhow!("nonlocal or unsupported schema reference: {reference}"))?;
                ensure!(!name.contains('/'), "unsupported subschema reference: {reference}");
                let decoded = percent_encoding::percent_decode_str(name).decode_utf8()?;
                let name = decoded.replace("~1", "/").replace("~0", "~");
                Ok(format!("{prefix}.{}", component_name(&name)))
            })();
            match rewritten {
                Ok(key) => {
                    schema.insert("$ref".into(), json!(format!("#/components/schemas/{key}")));
                    references.insert(key);
                }
                Err(err) => error = Some(err),
            }
        }
    };
    fn walk(schema: &mut Schema, visit: &mut impl FnMut(&mut Schema)) {
        visit(schema);
        transform_subschemas(&mut |schema: &mut Schema| walk(schema, visit), schema);
    }
    walk(schema, &mut visit);
    if let Some(error) = error { return Err(error); }
    Ok(())
}
