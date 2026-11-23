//! Per-name Kubernetes objects (EDGE-K8S): the edge applies plain YAML templates (any `apiVersion`/`kind`) for
//! every name that can serve and deletes them when the name goes, so a passthrough ingress in front of the
//! edge can route `*.<name>.<D>`. Nothing here knows a controller: templates, variables and labels only.
//!
//! The pure part (config, substitution, slug, desired set, diff, the reconcile loop over a [`KubeApi`]) is
//! always compiled; the kube-rs client ([`client`]) is behind the cargo feature `k8s`. A disabled section
//! constructs no client and makes no call.

#[cfg(feature = "k8s")]
pub mod client;

use std::collections::BTreeSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use anyhow::{Context as _, bail};
use futures::future::BoxFuture;
use serde::Deserialize;
use serde_json::{Map, Value};
use tokio::sync::{Mutex, Notify};

use crate::state::Edge;

/// Label naming the edge's field manager on every object it applies; the only objects it lists or deletes.
pub const MANAGED_BY: &str = "app.kubernetes.io/managed-by";
/// Label naming the share name a per-name object belongs to.
pub const SHARE_NAME: &str = "theuth.dev/share-name";
/// Longest `${slug}`, leaving room for a prefix inside a 63-character DNS-1123 label.
pub const SLUG_MAX: usize = 40;

/// `k8s:`.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct K8sConfig {
    /// Off by default: no client, no call.
    pub enabled: bool,
    /// Namespace of namespaced objects without their own; empty = the pod's (service-account file).
    pub namespace: String,
    /// Server-side-apply field manager and the `app.kubernetes.io/managed-by` value.
    pub field_manager: String,
    /// Reconcile period.
    #[serde(with = "crate::config::dur")]
    pub resync: Duration,
    /// Objects applied once at start (variables `${domain}`, `${namespace}`).
    #[serde(rename = "static")]
    pub static_objects: Vec<Value>,
    /// Objects applied per name that can serve, deleted when it goes.
    pub per_name: Vec<Value>,
}

impl Default for K8sConfig {
    fn default() -> Self {
        K8sConfig {
            enabled: false,
            namespace: String::new(),
            field_manager: "theuth-share-edge".into(),
            resync: Duration::from_secs(300),
            static_objects: Vec::new(),
            per_name: Vec::new(),
        }
    }
}

const STATIC_VARS: &[&str] = &["domain", "namespace"];
const NAME_VARS: &[&str] = &["name", "domain", "host", "wildcard", "slug", "namespace"];

impl K8sConfig {
    /// Config-time checks: every template is an object with `apiVersion`, `kind` and `metadata.name`; every
    /// `${…}` is known (static templates know only `${domain}` and `${namespace}`); a per-name template's
    /// `metadata.namespace` does not depend on the name; the field manager is a valid label value.
    pub fn validate(&self, domain: &str) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        if !label_value_ok(&self.field_manager) {
            bail!("`k8s.field_manager` must be a label value (≤63 of [A-Za-z0-9-_.], alphanumeric ends)");
        }
        if self.resync < Duration::from_secs(10) {
            bail!("`k8s.resync` must be at least 10s");
        }
        for (i, t) in self.static_objects.iter().enumerate() {
            let v = Vars::statics(domain, "ns");
            let r = render(t, &v, STATIC_VARS).with_context(|| format!("k8s.static[{i}]"))?;
            ident(&r).with_context(|| format!("k8s.static[{i}]"))?;
        }
        for (i, t) in self.per_name.iter().enumerate() {
            let a = render(t, &Vars::name("aaa", domain, "ns"), NAME_VARS)
                .with_context(|| format!("k8s.per_name[{i}]"))?;
            let b = render(t, &Vars::name("bbb", domain, "ns"), NAME_VARS)?;
            ident(&a).with_context(|| format!("k8s.per_name[{i}]"))?;
            if a.pointer("/metadata/namespace") != b.pointer("/metadata/namespace") {
                bail!("k8s.per_name[{i}]: `metadata.namespace` must not depend on the name");
            }
            if ident(&a)?.2 == ident(&b)?.2 {
                bail!("k8s.per_name[{i}]: `metadata.name` must depend on the name (use ${{slug}})");
            }
        }
        Ok(())
    }
}

/// The values of the template variables.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vars {
    pairs: Vec<(&'static str, String)>,
}

impl Vars {
    /// Variables of a static template.
    pub fn statics(domain: &str, namespace: &str) -> Vars {
        Vars {
            pairs: vec![("domain", domain.into()), ("namespace", namespace.into())],
        }
    }

    /// Variables of a per-name template.
    pub fn name(name: &str, domain: &str, namespace: &str) -> Vars {
        Vars {
            pairs: vec![
                ("name", name.into()),
                ("domain", domain.into()),
                ("host", format!("{name}.{domain}")),
                ("wildcard", format!("*.{name}.{domain}")),
                ("slug", slug(name)),
                ("namespace", namespace.into()),
            ],
        }
    }

    fn get(&self, k: &str) -> Option<&str> {
        self.pairs
            .iter()
            .find(|(n, _)| *n == k)
            .map(|(_, v)| v.as_str())
    }
}

/// A DNS-1123-label-safe fragment of `name`: lowercase, every character outside `[a-z0-9-]` as `-`, no
/// leading or trailing `-`, at most [`SLUG_MAX`]; a longer one keeps 31 characters plus `-` and 8 hex digits
/// of FNV-1a over the whole name, so two long names do not collide. Never empty (`x`).
pub fn slug(name: &str) -> String {
    let mapped: String = name
        .chars()
        .map(|c| match c.to_ascii_lowercase() {
            c @ ('a'..='z' | '0'..='9' | '-') => c,
            _ => '-',
        })
        .collect();
    let trimmed = mapped.trim_matches('-');
    let out = if trimmed.len() > SLUG_MAX {
        let h = name.bytes().fold(0x811c_9dc5_u32, |h, b| {
            (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
        });
        format!("{}-{h:08x}", trimmed[..31].trim_end_matches('-'))
    } else {
        trimmed.to_string()
    };
    if out.is_empty() { "x".into() } else { out }
}

/// Substitute `${var}` in every string value of `t` (never in keys); a `${` without `}` or an unknown or
/// disallowed variable is an error. `$` not followed by `{` is literal. Nothing is evaluated.
pub fn render(t: &Value, vars: &Vars, allowed: &[&str]) -> anyhow::Result<Value> {
    Ok(match t {
        Value::String(s) => Value::String(subst(s, vars, allowed)?),
        Value::Array(a) => Value::Array(
            a.iter()
                .map(|v| render(v, vars, allowed))
                .collect::<anyhow::Result<_>>()?,
        ),
        Value::Object(o) => Value::Object(
            o.iter()
                .map(|(k, v)| Ok((k.clone(), render(v, vars, allowed)?)))
                .collect::<anyhow::Result<Map<_, _>>>()?,
        ),
        v => v.clone(),
    })
}

fn subst(s: &str, vars: &Vars, allowed: &[&str]) -> anyhow::Result<String> {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("${") {
        out.push_str(&rest[..i]);
        let after = &rest[i + 2..];
        let Some(end) = after.find('}') else {
            bail!("unterminated `${{` in {s:?}");
        };
        let var = &after[..end];
        if !allowed.contains(&var) {
            bail!("unknown variable `${{{var}}}` in {s:?} (allowed: {})", allowed.join(", "));
        }
        out.push_str(vars.get(var).context("variable without a value")?);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// A kind: `apiVersion` and `kind`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TypeKey {
    /// `v1`, `networking.k8s.io/v1`, …
    pub api_version: String,
    /// `Ingress`, …
    pub kind: String,
}

/// One object's identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ObjKey {
    /// Its kind.
    pub t: TypeKey,
    /// `None` for a cluster-scoped kind.
    pub namespace: Option<String>,
    /// `metadata.name`.
    pub name: String,
}

/// A rendered, stamped object.
#[derive(Debug, Clone, PartialEq)]
pub struct Obj {
    /// Identity.
    pub key: ObjKey,
    /// The full body sent with server-side apply.
    pub body: Value,
}

/// A managed object as listed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    /// Identity.
    pub key: ObjKey,
    /// Its [`SHARE_NAME`] label; `None` for a static object.
    pub share_name: Option<String>,
}

fn ident(v: &Value) -> anyhow::Result<(String, String, String)> {
    let s = |p: &str| {
        v.pointer(p)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .with_context(|| format!("template needs a string `{}`", p[1..].replace('/', ".")))
    };
    if !v.is_object() {
        bail!("template must be an object");
    }
    Ok((s("/apiVersion")?, s("/kind")?, s("/metadata/name")?))
}

fn label_value_ok(v: &str) -> bool {
    v.len() <= 63
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        && v.bytes().next().is_none_or(|b| b.is_ascii_alphanumeric())
        && v.bytes().last().is_none_or(|b| b.is_ascii_alphanumeric())
}

/// The cluster side: apply, delete, list, scope lookup. Implemented by [`client::KubeClient`] (feature `k8s`)
/// and by fakes in tests.
pub trait KubeApi: Send + Sync {
    /// Whether the kind is namespaced (discovery).
    fn namespaced<'a>(&'a self, t: &'a TypeKey) -> BoxFuture<'a, anyhow::Result<bool>>;
    /// Server-side apply with the field manager, `force = false`.
    fn apply<'a>(&'a self, o: &'a Obj) -> BoxFuture<'a, anyhow::Result<()>>;
    /// Delete; an absent object is success.
    fn delete<'a>(&'a self, k: &'a ObjKey) -> BoxFuture<'a, anyhow::Result<()>>;
    /// Objects of `t` in `namespace` (`None`: cluster scope) matching the label selector.
    fn list<'a>(
        &'a self,
        t: &'a TypeKey,
        namespace: Option<&'a str>,
        selector: &'a str,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Listed>>>;
}

/// The managed per-name objects to delete: those whose share name is set and whose key is not desired.
/// Static objects (no share-name label) are never deleted here.
pub fn diff(desired: &[Obj], existing: &[Listed]) -> Vec<ObjKey> {
    let want: BTreeSet<&ObjKey> = desired.iter().map(|o| &o.key).collect();
    existing
        .iter()
        .filter(|l| l.share_name.is_some() && !want.contains(&l.key))
        .map(|l| l.key.clone())
        .collect()
}

/// What one reconcile pass did.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Pass {
    /// Objects applied.
    pub applied: usize,
    /// Objects deleted.
    pub deleted: usize,
    /// Calls that failed (logged, counted).
    pub failed: usize,
}

/// The per-name object manager: templates, the resolved namespace and a [`KubeApi`]. Every operation holds
/// one async lock, so a claim, a release and a reconcile pass never interleave their reads and writes.
pub struct K8s {
    cfg: K8sConfig,
    domain: String,
    namespace: String,
    api: Arc<dyn KubeApi>,
    lock: Mutex<()>,
    nudge: Notify,
    /// `share_edge_k8s_errors_total`.
    pub errors: AtomicU64,
}

impl std::fmt::Debug for K8s {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("K8s")
            .field("namespace", &self.namespace)
            .field("errors", &self.errors)
            .finish_non_exhaustive()
    }
}

impl K8s {
    /// A manager over `api`; `namespace` is the resolved default namespace.
    pub fn new(cfg: K8sConfig, domain: &str, namespace: &str, api: Arc<dyn KubeApi>) -> K8s {
        K8s {
            cfg,
            domain: domain.into(),
            namespace: namespace.into(),
            api,
            lock: Mutex::new(()),
            nudge: Notify::new(),
            errors: AtomicU64::new(0),
        }
    }

    fn selector(&self, name: Option<&str>) -> String {
        let mut s = format!("{MANAGED_BY}={}", self.cfg.field_manager);
        if let Some(n) = name {
            s.push_str(&format!(",{SHARE_NAME}={n}"));
        }
        s
    }

    /// Render, stamp the labels (they win over the template's), and fix the namespace by the kind's scope.
    async fn build(&self, t: &Value, name: Option<&str>) -> anyhow::Result<Obj> {
        let mut body = match name {
            Some(n) => render(t, &Vars::name(n, &self.domain, &self.namespace), NAME_VARS)?,
            None => render(t, &Vars::statics(&self.domain, &self.namespace), STATIC_VARS)?,
        };
        let (api_version, kind, obj_name) = ident(&body)?;
        let t = TypeKey { api_version, kind };
        let namespace = if self.api.namespaced(&t).await? {
            let ns = body
                .pointer("/metadata/namespace")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .unwrap_or(&self.namespace)
                .to_string();
            Some(ns)
        } else {
            None
        };
        let meta = body
            .get_mut("metadata")
            .and_then(Value::as_object_mut)
            .context("metadata")?;
        match &namespace {
            Some(ns) => meta.insert("namespace".into(), Value::String(ns.clone())),
            None => meta.remove("namespace"),
        };
        let labels = meta
            .entry("labels")
            .or_insert_with(|| Value::Object(Map::new()));
        if !labels.is_object() {
            *labels = Value::Object(Map::new());
        }
        let labels = labels.as_object_mut().expect("object");
        labels.insert(MANAGED_BY.into(), self.cfg.field_manager.clone().into());
        if let Some(n) = name {
            labels.insert(SHARE_NAME.into(), n.into());
        }
        Ok(Obj {
            key: ObjKey {
                t,
                namespace,
                name: obj_name,
            },
            body,
        })
    }

    /// Templates × `names`.
    pub async fn desired(&self, names: &[String]) -> anyhow::Result<Vec<Obj>> {
        let mut out = Vec::with_capacity(names.len() * self.cfg.per_name.len());
        for n in names {
            for t in &self.cfg.per_name {
                out.push(self.build(t, Some(n)).await?);
            }
        }
        Ok(out)
    }

    /// Where per-name objects live: each per-name kind with its (name-independent) namespace.
    async fn scopes(&self) -> anyhow::Result<Vec<(TypeKey, Option<String>)>> {
        let mut s = BTreeSet::new();
        for t in &self.cfg.per_name {
            let o = self.build(t, Some("x")).await?;
            s.insert((o.key.t, o.key.namespace));
        }
        Ok(s.into_iter().collect())
    }

    fn failed(&self, what: &str, e: &anyhow::Error) {
        self.errors.fetch_add(1, Ordering::Relaxed);
        tracing::warn!("k8s {what}: {e:#}");
    }

    /// Apply the static objects. Errors are counted and returned.
    pub async fn apply_static(&self) -> anyhow::Result<()> {
        let _g = self.lock.lock().await;
        let mut first = None;
        for t in &self.cfg.static_objects {
            let r = async {
                let o = self.build(t, None).await?;
                self.api.apply(&o).await.with_context(|| describe(&o.key))
            }
            .await;
            if let Err(e) = r {
                self.failed("apply static", &e);
                first.get_or_insert(e);
            }
        }
        first.map_or(Ok(()), Err)
    }

    /// One pass: apply every desired object (server-side apply is a no-op when nothing drifted, and restores
    /// what did), then delete managed per-name objects of names not in `names`.
    pub async fn reconcile(&self, names: &[String]) -> anyhow::Result<Pass> {
        let _g = self.lock.lock().await;
        let desired = self.desired(names).await?;
        let mut pass = Pass::default();
        for o in &desired {
            match self.api.apply(o).await {
                Ok(()) => pass.applied += 1,
                Err(e) => {
                    pass.failed += 1;
                    self.failed(&format!("apply {}", describe(&o.key)), &e);
                }
            }
        }
        let selector = self.selector(None);
        let mut existing = Vec::new();
        for (t, ns) in self.scopes().await? {
            existing.extend(self.api.list(&t, ns.as_deref(), &selector).await?);
        }
        for k in diff(&desired, &existing) {
            match self.api.delete(&k).await {
                Ok(()) => pass.deleted += 1,
                Err(e) => {
                    pass.failed += 1;
                    self.failed(&format!("delete {}", describe(&k)), &e);
                }
            }
        }
        Ok(pass)
    }

    /// Apply `name`'s objects if `still_held` (checked under the lock) says the name can still serve.
    pub async fn apply_name(
        &self,
        name: &str,
        still_held: impl Future<Output = bool>,
    ) -> anyhow::Result<()> {
        let _g = self.lock.lock().await;
        if !still_held.await {
            return Ok(());
        }
        for o in self.desired(&[name.to_string()]).await? {
            self.api.apply(&o).await.with_context(|| describe(&o.key))?;
        }
        Ok(())
    }

    /// Delete every managed object labelled with `name`, whatever template made it, if `still_gone` (checked
    /// under the lock) says the name still cannot serve: a name claimed again since keeps its objects
    /// (FIX4-EDGE #14).
    pub async fn delete_name(&self, name: &str, still_gone: impl Future<Output = bool>) -> anyhow::Result<()> {
        let _g = self.lock.lock().await;
        if !still_gone.await {
            return Ok(());
        }
        let selector = self.selector(Some(name));
        for (t, ns) in self.scopes().await? {
            for l in self.api.list(&t, ns.as_deref(), &selector).await? {
                self.api.delete(&l.key).await.with_context(|| describe(&l.key))?;
            }
        }
        Ok(())
    }
}

fn describe(k: &ObjKey) -> String {
    match &k.namespace {
        Some(ns) => format!("{} {}/{}", k.t.kind, ns, k.name),
        None => format!("{} {}", k.t.kind, k.name),
    }
}

fn retry() -> impl Iterator<Item = Duration> {
    use backon::BackoffBuilder as _;
    backon::ExponentialBuilder::default()
        .with_min_delay(Duration::from_secs(1))
        .with_max_delay(Duration::from_secs(30))
        .with_max_times(4)
        .with_jitter()
        .build()
}

/// The names that can serve (held, not banned) under the edge's domain.
async fn serving(edge: &Edge) -> anyhow::Result<Vec<String>> {
    edge.store.serving_names(&edge.cfg.domain).await
}

/// A name was granted: apply its objects now (before the certificate job needs the routing), retrying with
/// backoff in the background; never blocks or fails the claim. A final failure wakes the reconcile loop.
pub fn granted(edge: &Arc<Edge>, name: &str) {
    let Some(k) = edge.k8s.get().cloned() else {
        return;
    };
    let (edge, name) = (edge.clone(), name.to_string());
    tokio::spawn(async move {
        let mut delays = retry();
        loop {
            let held = async {
                edge.store
                    .name(&edge.cfg.domain, &name)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.held())
            };
            match k.apply_name(&name, held).await {
                Ok(()) => return,
                Err(e) => k.failed(&format!("apply name {name}"), &e),
            }
            match delays.next() {
                Some(d) => tokio::time::sleep(d).await,
                None => return k.nudge.notify_one(),
            }
        }
    });
}

/// A name was released, banned, evicted or expired: delete its objects in the background, retrying; a final
/// failure wakes the reconcile loop, which deletes objects of names that cannot serve.
pub fn gone(edge: &Arc<Edge>, name: &str) {
    let Some(k) = edge.k8s.get().cloned() else {
        return;
    };
    let (edge, name) = (edge.clone(), name.to_string());
    tokio::spawn(async move {
        let mut delays = retry();
        loop {
            // A store error counts as "still gone": the reconcile loop re-applies a name that can serve.
            let gone = async {
                !edge
                    .store
                    .name(&edge.cfg.domain, &name)
                    .await
                    .ok()
                    .flatten()
                    .is_some_and(|r| r.held())
            };
            match k.delete_name(&name, gone).await {
                Ok(()) => return,
                Err(e) => k.failed(&format!("delete name {name}"), &e),
            }
            match delays.next() {
                Some(d) => tokio::time::sleep(d).await,
                None => return k.nudge.notify_one(),
            }
        }
    });
}

/// The loop: static objects until they succeed once, then a reconcile at start and every `k8s.resync`; after
/// a failing pass the next comes sooner (30 s doubling up to `resync`). A nudge runs a pass at once.
pub async fn run(edge: Arc<Edge>, k: Arc<K8s>) {
    let resync = k.cfg.resync;
    let mut statics_done = k.cfg.static_objects.is_empty();
    let mut backoff = Duration::from_secs(30);
    loop {
        if !statics_done {
            statics_done = k.apply_static().await.is_ok();
        }
        let ok = match serving(&edge).await {
            Ok(names) => match k.reconcile(&names).await {
                Ok(p) => {
                    tracing::debug!(?p, "k8s reconcile");
                    p.failed == 0
                }
                Err(e) => {
                    k.failed("reconcile", &e);
                    false
                }
            },
            Err(e) => {
                k.failed("reconcile: names", &e);
                false
            }
        };
        let wait = if ok && statics_done {
            backoff = Duration::from_secs(30);
            resync
        } else {
            let w = backoff.min(resync);
            backoff = (backoff * 2).min(resync);
            w
        };
        tokio::select! {
            () = tokio::time::sleep(wait) => {}
            () = k.nudge.notified() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex as StdMutex;

    use super::*;

    fn yaml(s: &str) -> Value {
        serde_yaml::from_str(s).unwrap()
    }

    const INGRESS: &str = "
apiVersion: networking.k8s.io/v1
kind: Ingress
metadata:
  name: share-${slug}
  labels: {team: edge, app.kubernetes.io/managed-by: someone-else}
  annotations: {haproxy.org/ssl-passthrough: 'true', note: '$5 ${host}'}
spec:
  ingressClassName: haproxy
  rules:
  - host: '${wildcard}'
    http: {paths: [{path: /, pathType: Prefix, backend: {service: {name: theuth-share-edge, port: {number: 443}}}}]}
  tls: [{hosts: ['${host}', '${name}.${domain}']}]
";

    #[test]
    fn substitutes_every_variable_in_values_only() {
        let t = yaml(
            "k-${name}: ['${name}', '${domain}', '${host}', '${wildcard}', '${slug}', '${namespace}', 'a$b', 7]",
        );
        let r = render(&t, &Vars::name("App_X", "tun.zip", "theuth"), NAME_VARS).unwrap();
        assert_eq!(
            r,
            serde_json::json!({"k-${name}": ["App_X", "tun.zip", "App_X.tun.zip", "*.App_X.tun.zip",
                "app-x", "theuth", "a$b", 7]})
        );
    }

    #[test]
    fn unknown_or_unterminated_variables_are_refused() {
        let v = Vars::name("sky", "tun.zip", "ns");
        let e = render(&yaml("a: ${nmae}"), &v, NAME_VARS).unwrap_err();
        assert!(e.to_string().contains("${nmae}"), "{e}");
        assert!(render(&yaml("a: x${name"), &v, NAME_VARS).is_err());
        assert!(render(&yaml("a: ${}"), &v, NAME_VARS).is_err());
        // Static templates have no name.
        let s = Vars::statics("tun.zip", "ns");
        assert!(render(&yaml("a: ${host}"), &s, STATIC_VARS).is_err());
        assert!(render(&yaml("a: '*.${domain}'"), &s, STATIC_VARS).is_ok());
    }

    #[test]
    fn config_validation() {
        let cfg = |per_name: &str, statics: &str| K8sConfig {
            enabled: true,
            per_name: vec![yaml(per_name)],
            static_objects: if statics.is_empty() { vec![] } else { vec![yaml(statics)] },
            ..K8sConfig::default()
        };
        cfg(INGRESS, "").validate("tun.zip").unwrap();
        let bad_var = INGRESS.replace("${slug}", "${sluggy}");
        assert!(cfg(&bad_var, "").validate("tun.zip").is_err());
        // The object name must vary with the name, the namespace must not.
        let fixed = INGRESS.replace("share-${slug}", "share");
        assert!(cfg(&fixed, "").validate("tun.zip").is_err());
        let ns = INGRESS.replace("  labels:", "  namespace: ns-${slug}\n  labels:");
        assert!(cfg(&ns, "").validate("tun.zip").is_err());
        assert!(cfg("kind: Ingress\nmetadata: {name: 'a-${slug}'}", "").validate("tun.zip").is_err());
        let st = "apiVersion: v1\nkind: ConfigMap\nmetadata: {name: wild}\ndata: {h: '*.${domain}'}";
        cfg(INGRESS, st).validate("tun.zip").unwrap();
        assert!(cfg(INGRESS, &st.replace("${domain}", "${name}")).validate("tun.zip").is_err());
        // Disabled: never looked at.
        let mut off = cfg(&bad_var, "");
        off.enabled = false;
        off.validate("tun.zip").unwrap();
    }

    #[test]
    fn slug_rules() {
        assert_eq!(slug("sky"), "sky");
        assert_eq!(slug("Sky.Blue_1"), "sky-blue-1");
        assert_eq!(slug("--a--"), "a");
        assert_eq!(slug("..."), "x");
        let long = "a".repeat(63);
        let s = slug(&long);
        assert!(s.len() <= SLUG_MAX, "{s}");
        assert_ne!(s, slug(&format!("{}b", "a".repeat(62))));
        assert!(s.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'));
        assert_eq!(slug(&"b".repeat(SLUG_MAX)), "b".repeat(SLUG_MAX));
    }

    /// An in-memory cluster: Ingress and ConfigMap namespaced, ClusterRole cluster-scoped.
    #[derive(Default)]
    struct Fake {
        objs: StdMutex<BTreeMap<ObjKey, Value>>,
        fail_apply: StdMutex<bool>,
        calls: AtomicU64,
    }

    impl KubeApi for Fake {
        fn namespaced<'a>(&'a self, t: &'a TypeKey) -> BoxFuture<'a, anyhow::Result<bool>> {
            Box::pin(async move { Ok(t.kind != "ClusterRole") })
        }
        fn apply<'a>(&'a self, o: &'a Obj) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::Relaxed);
                if *self.fail_apply.lock().unwrap() {
                    bail!("conflict");
                }
                self.objs.lock().unwrap().insert(o.key.clone(), o.body.clone());
                Ok(())
            })
        }
        fn delete<'a>(&'a self, k: &'a ObjKey) -> BoxFuture<'a, anyhow::Result<()>> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::Relaxed);
                self.objs.lock().unwrap().remove(k);
                Ok(())
            })
        }
        fn list<'a>(
            &'a self,
            t: &'a TypeKey,
            namespace: Option<&'a str>,
            selector: &'a str,
        ) -> BoxFuture<'a, anyhow::Result<Vec<Listed>>> {
            Box::pin(async move {
                let want: Vec<(&str, &str)> =
                    selector.split(',').filter_map(|p| p.split_once('=')).collect();
                Ok(self
                    .objs
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(k, _)| &k.t == t && k.namespace.as_deref() == namespace)
                    .filter(|(_, v)| {
                        want.iter().all(|(l, val)| {
                            v.pointer("/metadata/labels").and_then(|m| m.get(*l))
                                == Some(&Value::String((*val).into()))
                        })
                    })
                    .map(|(k, v)| Listed {
                        key: k.clone(),
                        share_name: v
                            .pointer("/metadata/labels")
                            .and_then(|m| m.get(SHARE_NAME))
                            .and_then(Value::as_str)
                            .map(str::to_owned),
                    })
                    .collect())
            })
        }
    }

    fn manager(fake: Arc<Fake>) -> K8s {
        let cfg = K8sConfig {
            enabled: true,
            per_name: vec![
                yaml(INGRESS),
                yaml("apiVersion: rbac.authorization.k8s.io/v1\nkind: ClusterRole\nmetadata: {name: 'r-${slug}', namespace: ignored}"),
            ],
            static_objects: vec![yaml(
                "apiVersion: v1\nkind: ConfigMap\nmetadata: {name: wild}\ndata: {h: '*.${domain}'}",
            )],
            ..K8sConfig::default()
        };
        cfg.validate("tun.zip").unwrap();
        K8s::new(cfg, "tun.zip", "theuth", fake)
    }

    fn names(n: &[&str]) -> Vec<String> {
        n.iter().map(|s| (*s).to_string()).collect()
    }

    #[tokio::test]
    async fn desired_set_is_templates_times_names_with_stamped_labels() {
        let k = manager(Arc::new(Fake::default()));
        let d = k.desired(&names(&["sky", "blue"])).await.unwrap();
        assert_eq!(d.len(), 4);
        let ing = &d[0];
        assert_eq!(
            ing.key,
            ObjKey {
                t: TypeKey {
                    api_version: "networking.k8s.io/v1".into(),
                    kind: "Ingress".into()
                },
                namespace: Some("theuth".into()),
                name: "share-sky".into(),
            }
        );
        let labels = ing.body.pointer("/metadata/labels").unwrap();
        assert_eq!(labels[MANAGED_BY], "theuth-share-edge");
        assert_eq!(labels[SHARE_NAME], "sky");
        assert_eq!(labels["team"], "edge");
        assert_eq!(ing.body.pointer("/metadata/namespace").unwrap(), "theuth");
        assert_eq!(ing.body.pointer("/spec/rules/0/host").unwrap(), "*.sky.tun.zip");
        assert_eq!(ing.body.pointer("/metadata/annotations/note").unwrap(), "$5 sky.tun.zip");
        // Cluster-scoped: no namespace in the key or the body.
        assert_eq!(d[1].key.namespace, None);
        assert!(d[1].body.pointer("/metadata/namespace").is_none());
        assert_eq!(d[3].key.name, "r-blue");
    }

    #[test]
    fn diff_deletes_only_undesired_per_name_objects() {
        let t = TypeKey {
            api_version: "v1".into(),
            kind: "Service".into(),
        };
        let key = |n: &str| ObjKey {
            t: t.clone(),
            namespace: Some("theuth".into()),
            name: n.into(),
        };
        let desired = vec![Obj {
            key: key("share-sky"),
            body: Value::Null,
        }];
        let existing = vec![
            Listed { key: key("share-sky"), share_name: Some("sky".into()) },
            Listed { key: key("share-old"), share_name: Some("old".into()) },
            Listed { key: key("wild"), share_name: None },
        ];
        assert_eq!(diff(&desired, &existing), vec![key("share-old")]);
        assert_eq!(diff(&[], &existing).len(), 2);
    }

    #[tokio::test]
    async fn reconcile_applies_and_deletes_and_lifecycle_ops() {
        let fake = Arc::new(Fake::default());
        let k = manager(fake.clone());
        k.apply_static().await.unwrap();
        // A foreign object with the same kind but without our label is never touched.
        let foreign = ObjKey {
            t: TypeKey { api_version: "networking.k8s.io/v1".into(), kind: "Ingress".into() },
            namespace: Some("theuth".into()),
            name: "share-zzz".into(),
        };
        fake.objs.lock().unwrap().insert(
            foreign.clone(),
            serde_json::json!({"metadata": {"labels": {SHARE_NAME: "zzz"}}}),
        );
        let p = k.reconcile(&names(&["sky", "blue"])).await.unwrap();
        assert_eq!(p, Pass { applied: 4, deleted: 0, failed: 0 });
        assert_eq!(fake.objs.lock().unwrap().len(), 6);
        // blue goes away: its two objects are deleted, the static and the foreign one stay.
        let p = k.reconcile(&names(&["sky"])).await.unwrap();
        assert_eq!(p, Pass { applied: 2, deleted: 2, failed: 0 });
        let keys: Vec<String> =
            fake.objs.lock().unwrap().keys().map(|k| k.name.clone()).collect();
        assert_eq!(keys, ["share-sky", "share-zzz", "r-sky", "wild"]);
        // Claim and release.
        k.apply_name("blue", async { true }).await.unwrap();
        assert_eq!(fake.objs.lock().unwrap().len(), 6);
        k.delete_name("sky", async { true }).await.unwrap();
        let keys: Vec<String> =
            fake.objs.lock().unwrap().keys().map(|k| k.name.clone()).collect();
        assert_eq!(keys, ["share-blue", "share-zzz", "r-blue", "wild"]);
        // A name released before its apply ran is not applied.
        k.apply_name("late", async { false }).await.unwrap();
        assert_eq!(fake.objs.lock().unwrap().len(), 4);
        // A name claimed again before its delete ran keeps its objects (FIX4-EDGE #14).
        k.delete_name("blue", async { false }).await.unwrap();
        assert_eq!(fake.objs.lock().unwrap().len(), 4);
        // Apply failures are counted, not fatal to the pass.
        *fake.fail_apply.lock().unwrap() = true;
        let p = k.reconcile(&names(&["blue"])).await.unwrap();
        assert_eq!(p.failed, 2);
        assert_eq!(k.errors.load(Ordering::Relaxed), 2);
        assert!(k.apply_name("blue", async { true }).await.is_err());
    }
}
