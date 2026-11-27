//! Kubernetes (desktop only; design doc 224 §7): `sandbox::k8s` renders and applies the
//! one-replica Deployment with the ticket as the `pair` Secret, through the user's own kubectl
//! and kubeconfig. Memory request = limit = the node's size.

use async_trait::async_trait;
use serde_json::{Value, json};

use sandbox::k8s::{self, K8sSpec, LABEL_MANAGED, LABEL_NODE};

use super::catalog;
use super::{
    CloudError, CreateReq, Created, Listed, Pace, Provider, ProviderStatus, Result, Say,
    check_name,
};
use crate::types::{K8sInfo, K8sOptions, ProviderKind, Stage, StorageClass};

/// The user's cluster, through kubectl.
pub struct Kubernetes {
    opts: K8sOptions,
    /// Polling of the pod start.
    pub pace: Pace,
}

fn ns_args(ns: Option<&str>) -> Vec<&str> {
    ns.map(|n| vec!["--namespace", n]).unwrap_or_default()
}

fn gi(q: &str) -> Option<u32> {
    if let Some(n) = q.strip_suffix("Gi") {
        return n.parse().ok();
    }
    q.strip_suffix("Mi")
        .and_then(|n| n.parse::<u32>().ok())
        .map(|m| m / 1024)
}

fn container_memory(d: &Value) -> Option<u8> {
    let r = &d["spec"]["template"]["spec"]["containers"][0]["resources"];
    let q = r["limits"]["memory"].as_str().or(r["requests"]["memory"].as_str())?;
    u8::try_from(gi(q)?).ok()
}

fn replicas(d: &Value) -> String {
    format!(
        "ready {}/{}",
        d["status"]["readyReplicas"].as_u64().unwrap_or(0),
        d["spec"]["replicas"].as_u64().unwrap_or(0)
    )
}

/// The `/data` PVC's size (the larger of a node's claims).
fn pvc_gb<'a>(pvcs: impl Iterator<Item = &'a Value>) -> Option<u32> {
    pvcs.filter_map(|p| gi(p["spec"]["resources"]["requests"]["storage"].as_str()?))
        .max()
}

fn kind_is(v: &Value, k: &str) -> bool {
    v["kind"] == k
}

/// `kubectl get deployments,pvc -l theuth.io/node=<name> -o json` → the node's status.
pub fn parse_status(v: &Value) -> Option<ProviderStatus> {
    let items: Vec<&Value> = v["items"].as_array()?.iter().collect();
    let d = items.iter().find(|i| kind_is(i, "Deployment"));
    let pvcs = items.iter().filter(|i| kind_is(i, "PersistentVolumeClaim")).copied();
    let volume_gb = pvc_gb(pvcs);
    if d.is_none() && volume_gb.is_none() {
        return None;
    }
    let mem = d.and_then(|d| container_memory(d));
    Some(ProviderStatus {
        server_state: d.map(|d| replicas(d)),
        ip: None,
        region: None,
        size_slug: mem.map(|m| format!("{m}Gi")),
        memory_gb: mem,
        volume_gb,
        inbound_rules: 0,
    })
}

/// `kubectl get deployments,pvc -l theuth.io/managed=true -o json` → nodes (incl. PVC-only).
pub fn parse_list(v: &Value) -> Vec<Listed> {
    let items = v["items"].as_array().map(Vec::as_slice).unwrap_or_default();
    let node = |i: &Value| i["metadata"]["labels"][LABEL_NODE].as_str().map(str::to_owned);
    let mut out: Vec<Listed> = Vec::new();
    for d in items.iter().filter(|i| kind_is(i, "Deployment")) {
        let Some(name) = node(d) else { continue };
        let ns = d["metadata"]["namespace"].as_str();
        let pvcs = items.iter().filter(|p| {
            kind_is(p, "PersistentVolumeClaim")
                && node(p).as_deref() == Some(&name)
                && p["metadata"]["namespace"].as_str() == ns
        });
        out.push(Listed {
            server_state: Some(replicas(d)),
            region: ns.map(str::to_owned),
            ip: None,
            memory_gb: container_memory(d),
            volume_gb: pvc_gb(pvcs),
            volume_only: false,
            name,
        });
    }
    for p in items.iter().filter(|i| kind_is(i, "PersistentVolumeClaim")) {
        let Some(name) = node(p) else { continue };
        let ns = p["metadata"]["namespace"].as_str().map(str::to_owned);
        if let Some(l) = out.iter_mut().find(|l| l.name == name && l.region == ns) {
            if l.volume_only {
                l.volume_gb = l.volume_gb.max(pvc_gb(std::iter::once(p)));
            }
            continue;
        }
        out.push(Listed {
            name,
            server_state: None,
            region: ns,
            ip: None,
            memory_gb: None,
            volume_gb: pvc_gb(std::iter::once(p)),
            volume_only: true,
        });
    }
    out.sort_by(|a, b| (&a.region, &a.name).cmp(&(&b.region, &b.name)));
    out
}

/// `metadata.name` of each item.
pub fn parse_names(v: &Value) -> Vec<String> {
    v["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|i| i["metadata"]["name"].as_str().map(str::to_owned))
        .collect()
}

/// The CNI from kube-system daemonsets/pods (names and images) and whether it enforces
/// NetworkPolicy: flannel alone does not; calico/cilium/weave/antrea/kube-router do. Canal
/// (flannel + calico) counts as calico.
pub fn detect_cni(v: &Value) -> (Option<String>, Option<bool>) {
    let mut words = String::new();
    for i in v["items"].as_array().into_iter().flatten() {
        words.push_str(i["metadata"]["name"].as_str().unwrap_or(""));
        words.push(' ');
        for c in i["spec"]["template"]["spec"]["containers"]
            .as_array()
            .into_iter()
            .chain(i["spec"]["containers"].as_array())
            .flatten()
        {
            words.push_str(c["image"].as_str().unwrap_or(""));
            words.push(' ');
        }
    }
    let words = words.to_lowercase();
    for cni in ["cilium", "calico", "antrea", "weave", "kube-router"] {
        if words.contains(cni) {
            return (Some(cni.into()), Some(true));
        }
    }
    if words.contains("canal") {
        return (Some("calico".into()), Some(true));
    }
    if words.contains("flannel") {
        return (Some("flannel".into()), Some(false));
    }
    (None, None)
}

/// What the wizard's Kubernetes step needs, for `context` (`None`: the current one).
pub async fn info(context: Option<&str>) -> anyhow::Result<K8sInfo> {
    let contexts: Vec<String> = k8s::kubectl(None, &["config", "get-contexts", "-o", "name"], None)
        .await?
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect();
    let current_context = k8s::kubectl(None, &["config", "current-context"], None)
        .await
        .ok()
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty());
    let ctx = context.map(str::to_owned).or_else(|| current_context.clone());
    let ctx = ctx.as_deref();
    let json_of = |args: &'static [&'static str]| async move {
        let s = k8s::kubectl(ctx, args, None).await.ok()?;
        serde_json::from_str::<Value>(&s).ok()
    };
    let namespaces = json_of(&["get", "namespaces", "-o", "json"])
        .await
        .map(|v| parse_names(&v))
        .unwrap_or_default();
    let storage_classes: Vec<StorageClass> = k8s::storage_classes(ctx)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(name, provisioner, default)| StorageClass {
            name,
            provisioner,
            default,
        })
        .collect();
    let (cni, enforces_network_policy) =
        json_of(&["get", "ds,pods", "--namespace", "kube-system", "-o", "json"])
            .await
            .map(|v| detect_cni(&v))
            .unwrap_or((None, None));
    Ok(K8sInfo {
        contexts,
        current_context,
        namespaces,
        no_default_class: !storage_classes.is_empty() && !storage_classes.iter().any(|c| c.default),
        storage_classes,
        cni,
        enforces_network_policy,
    })
}

impl Kubernetes {
    /// Default placement (context/namespace) for status, list, restart, resize and destroy.
    pub fn new(opts: K8sOptions) -> Self {
        Self {
            opts,
            pace: Pace::default(),
        }
    }

    /// Override the polling pace (tests).
    pub fn with_pace(mut self, pace: Pace) -> Self {
        self.pace = pace;
        self
    }

    fn ctx(&self) -> Option<&str> {
        self.opts.context.as_deref()
    }

    fn ns(&self) -> Option<&str> {
        self.opts.namespace.as_deref()
    }

    async fn get_json(&self, args: &[&str]) -> Result<Value> {
        let s = k8s::kubectl(self.ctx(), args, None).await?;
        serde_json::from_str(&s).map_err(|e| CloudError(format!("kubectl output: {e}")))
    }
}

/// The spec `sandbox::k8s` applies for `req` with `opts`.
pub fn spec(req: &CreateReq, opts: &K8sOptions, deny_cidrs: Option<Vec<String>>) -> K8sSpec {
    K8sSpec {
        name: req.name.clone(),
        context: opts.context.clone(),
        namespace: opts.namespace.clone(),
        storage_class: opts.storage_class.clone(),
        volume_size: format!("{}Gi", req.volume_gb),
        image: req.image.clone(),
        deny_cidrs,
        memory_gb: req.memory_gb,
    }
}

#[async_trait]
impl Provider for Kubernetes {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Kubernetes
    }

    async fn create(&self, req: &CreateReq, say: Say<'_>) -> Result<Created> {
        check_name(&req.name)?;
        let size = catalog::slug(ProviderKind::Kubernetes, req.memory_gb)?;
        let opts = req.k8s.clone().unwrap_or_else(|| self.opts.clone());
        let ctx = opts.context.as_deref();
        let ns = opts.namespace.as_deref();
        let deny = if opts.deny_cluster_network {
            Some(k8s::cluster_cidrs(ctx).await)
        } else {
            None
        };
        let spec = spec(req, &opts, deny);
        k8s::apply(&spec, &req.ticket, opts.create_namespace).await?;
        let deployment = k8s::deployment_name(&req.name);
        say(Stage::Server, &format!("Deployment {deployment} applied"));
        let mut last = String::new();
        for _ in 0..self.pace.tries {
            let line = k8s::pod_line(ctx, ns, &req.name).await;
            if line != last {
                say(Stage::Server, &line);
                last.clone_from(&line);
            }
            // A running pod has its PVCs bound and mounted.
            if line == "pod Running" {
                say(Stage::Storage, &format!("Volume {}Gi mounted at /data", req.volume_gb));
                return Ok(Created {
                    server_id: Some(deployment),
                    volume_id: None,
                    extra: opts.namespace.clone(),
                    ip: None,
                    size_slug: size,
                });
            }
            self.pace.sleep().await;
        }
        Err(CloudError(format!("the pod did not start ({last})")))
    }

    async fn status(&self, name: &str) -> Result<Option<ProviderStatus>> {
        check_name(name)?;
        let sel = format!("{LABEL_NODE}={name}");
        let mut args = vec!["get", "deployments,persistentvolumeclaims", "-l", &sel, "-o", "json"];
        args.extend(ns_args(self.ns()));
        Ok(parse_status(&self.get_json(&args).await?))
    }

    async fn list(&self) -> Result<Vec<Listed>> {
        let sel = format!("{LABEL_MANAGED}=true");
        let mut args = vec!["get", "deployments,persistentvolumeclaims", "-l", &sel, "-o", "json"];
        match self.ns() {
            Some(n) => args.extend(["--namespace", n]),
            None => args.push("--all-namespaces"),
        }
        Ok(parse_list(&self.get_json(&args).await?))
    }

    async fn restart(&self, name: &str) -> Result<()> {
        check_name(name)?;
        let d = format!("deployment/{}", k8s::deployment_name(name));
        let mut args = vec!["rollout", "restart", d.as_str()];
        args.extend(ns_args(self.ns()));
        k8s::kubectl(self.ctx(), &args, None).await?;
        Ok(())
    }

    async fn resize(&self, name: &str, memory_gb: u8) -> Result<String> {
        check_name(name)?;
        let size = catalog::slug(ProviderKind::Kubernetes, memory_gb)?;
        let patch = json!([
            {"op": "replace", "path": "/spec/template/spec/containers/0/resources/requests/memory", "value": size},
            {"op": "replace", "path": "/spec/template/spec/containers/0/resources/limits/memory", "value": size},
        ])
        .to_string();
        let d = k8s::deployment_name(name);
        let mut args = vec!["patch", "deployment", d.as_str(), "--type", "json", "-p", patch.as_str()];
        args.extend(ns_args(self.ns()));
        k8s::kubectl(self.ctx(), &args, None).await?;
        Ok(size)
    }

    async fn destroy(&self, name: &str, keep_volume: bool) -> Result<Vec<String>> {
        let out = k8s::destroy(self.ctx(), self.ns(), name, !keep_volume).await?;
        let mut done: Vec<String> = out
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect();
        if keep_volume {
            done.push(format!("volumes of {name} kept"));
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::catalog::{ENGINE_IMAGE, MANIFEST};

    fn deployment(ns: &str, name: &str, mem: &str) -> Value {
        json!({"kind": "Deployment",
            "metadata": {"namespace": ns, "labels": {LABEL_NODE: name, LABEL_MANAGED: "true"}},
            "spec": {"replicas": 1, "template": {"spec": {"containers": [{"image": ENGINE_IMAGE,
                "resources": {"requests": {"memory": mem}, "limits": {"memory": mem}}}]}}},
            "status": {"readyReplicas": 1}})
    }

    fn pvc(ns: &str, name: &str, size: &str) -> Value {
        json!({"kind": "PersistentVolumeClaim",
            "metadata": {"namespace": ns, "labels": {LABEL_NODE: name, LABEL_MANAGED: "true"}},
            "spec": {"resources": {"requests": {"storage": size}}}})
    }

    #[test]
    fn status_and_list_parse_deployments_and_kept_pvcs() {
        let v = json!({"items": [deployment("t", "lab", "4Gi"), pvc("t", "lab", "10Gi"), pvc("t", "lab", "20Gi")]});
        let s = parse_status(&v).unwrap();
        assert_eq!(
            (s.server_state.as_deref(), s.memory_gb, s.size_slug.as_deref(), s.volume_gb),
            (Some("ready 1/1"), Some(4), Some("4Gi"), Some(20))
        );
        assert_eq!(parse_status(&json!({"items": []})), None);
        let kept = parse_status(&json!({"items": [pvc("t", "lab", "10Gi")]})).unwrap();
        assert_eq!((kept.server_state, kept.volume_gb), (None, Some(10)));

        let l = parse_list(&json!({"items": [
            deployment("t", "lab", "2Gi"), pvc("t", "lab", "10Gi"), pvc("u", "old", "5Gi"), pvc("u", "old", "10Gi"),
        ]}));
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].name.as_str(), l[0].memory_gb, l[0].volume_gb, l[0].volume_only), ("lab", Some(2), Some(10), false));
        assert_eq!((l[1].name.as_str(), l[1].region.as_deref(), l[1].volume_gb, l[1].volume_only), ("old", Some("u"), Some(10), true));
    }

    #[test]
    fn cni_detection() {
        let ds = |name: &str, image: &str| {
            json!({"items": [{"metadata": {"name": name},
                "spec": {"template": {"spec": {"containers": [{"image": image}]}}}}]})
        };
        assert_eq!(detect_cni(&ds("kube-flannel-ds", "docker.io/flannel/flannel:v0.25")), (Some("flannel".into()), Some(false)));
        assert_eq!(detect_cni(&ds("calico-node", "calico/node:v3")), (Some("calico".into()), Some(true)));
        assert_eq!(detect_cni(&ds("canal", "flannel/flannel")), (Some("calico".into()), Some(true)));
        assert_eq!(detect_cni(&ds("anetd", "cilium/cilium")), (Some("cilium".into()), Some(true)));
        let pods = json!({"items": [{"metadata": {"name": "weave-net-x"}, "spec": {"containers": [{"image": "weaveworks/weave-kube"}]}}]});
        assert_eq!(detect_cni(&pods), (Some("weave".into()), Some(true)));
        assert_eq!(detect_cni(&ds("kube-proxy", "registry.k8s.io/kube-proxy")), (None, None));
        assert_eq!(parse_names(&json!({"items": [{"metadata": {"name": "default"}}]})), ["default"]);
    }

    #[test]
    fn spec_carries_memory_and_volume() {
        let req = CreateReq {
            name: "lab".into(),
            region: String::new(),
            memory_gb: 2,
            volume_gb: 10,
            ticket: "1.ABC".into(),
            manifest: MANIFEST.into(),
            image: ENGINE_IMAGE.into(),
            k8s: None,
            desktop: true,
        };
        let opts = K8sOptions { namespace: Some("t".into()), ..Default::default() };
        let s = spec(&req, &opts, None);
        assert_eq!((s.memory_gb, s.volume_size.as_str(), s.namespace.as_deref()), (2, "10Gi", Some("t")));
        let k = k8s::kustomization(&s, true).unwrap().to_string();
        assert!(k.contains("2Gi") && k.contains("resources/limits/memory"));
    }
}
