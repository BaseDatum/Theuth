//! `t mesh add --k8s` and `t mesh k8s …` (design doc 224 §7): a theuth engine as a one-replica
//! Deployment in any cluster and namespace the user's kubeconfig reaches. The kustomize base in
//! `deploy/computer/k8s` is embedded; an overlay written here sets namespace, names, labels,
//! image, storage and the pairing-ticket Secret, then `kubectl kustomize` renders and
//! `kubectl apply` applies it. kubectl rather than a linked client: every kubeconfig auth plugin
//! (exec, OIDC, cloud CLIs) works as it does for the user, and `t` links no Kubernetes crates.
//!
//! The engine joins the mesh itself (`deploy/computer/entrypoint.sh` step 5) with a one-time
//! ticket from `t mesh pair --remote`: no `kubectl exec`, so the user needs no `pods/exec` right.

use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, bail};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

/// On every object this makes (`kubectl get … -l theuth.io/managed=true`).
pub const LABEL_MANAGED: &str = "theuth.io/managed";
/// The node name, on every object of one node (and in the Deployment's selector).
pub const LABEL_NODE: &str = "theuth.io/node";
/// The image `base/deployment.yaml` names; the overlay's `images:` entry rewrites it.
pub const IMAGE: &str = "ghcr.io/basedatum/theuth/engine";

/// The embedded kustomize tree, as `kubectl kustomize` reads it from a directory.
const FILES: &[(&str, &str)] = &[
    (
        "base/kustomization.yaml",
        include_str!("../../../deploy/computer/k8s/base/kustomization.yaml"),
    ),
    (
        "base/serviceaccount.yaml",
        include_str!("../../../deploy/computer/k8s/base/serviceaccount.yaml"),
    ),
    (
        "base/pvc.yaml",
        include_str!("../../../deploy/computer/k8s/base/pvc.yaml"),
    ),
    (
        "base/deployment.yaml",
        include_str!("../../../deploy/computer/k8s/base/deployment.yaml"),
    ),
    (
        "components/deny-cluster-network/kustomization.yaml",
        include_str!(
            "../../../deploy/computer/k8s/components/deny-cluster-network/kustomization.yaml"
        ),
    ),
    (
        "components/deny-cluster-network/networkpolicy.yaml",
        include_str!(
            "../../../deploy/computer/k8s/components/deny-cluster-network/networkpolicy.yaml"
        ),
    ),
];

/// Always in the NetworkPolicy's `except`: the cloud metadata address (instance credentials).
pub const METADATA_CIDR: &str = "169.254.169.254/32";

/// What `t mesh add --k8s` makes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct K8sSpec {
    /// Node name: object prefix `theuth-<name>-`, label `theuth.io/node`, `mesh.name`.
    pub name: String,
    /// kubeconfig context; `None` = the current one.
    pub context: Option<String>,
    /// Namespace; `None` = the context's.
    pub namespace: Option<String>,
    /// StorageClass of both PVCs; `None` = the cluster default.
    pub storage_class: Option<String>,
    /// Size of each PVC (`10Gi`).
    pub volume_size: String,
    /// Full image ref (`ghcr.io/basedatum/theuth/engine:2.31.2`).
    pub image: String,
    /// `Some`: add the NetworkPolicy with these CIDRs (plus [`METADATA_CIDR`]) denied.
    pub deny_cidrs: Option<Vec<String>>,
    /// The container's memory request and limit, GiB ([`DEFAULT_MEMORY_GB`]).
    pub memory_gb: u8,
}

/// A node's memory (request = limit): 2 GiB, the minimum the engine is sized for.
pub const DEFAULT_MEMORY_GB: u8 = 2;

/// A DNS-1123 label short enough that `theuth-<name>-workspace` stays one (≤ 40 characters).
pub fn check_name(name: &str) -> anyhow::Result<()> {
    let ok = (1..=40).contains(&name.len())
        && name.starts_with(|c: char| c.is_ascii_lowercase())
        && !name.ends_with('-')
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    if !ok {
        bail!("node name {name:?}: 1-40 of [a-z0-9-], starting with a letter, not ending in -");
    }
    Ok(())
}

/// `a.b.c.d/n` (IPv4 only: `ipBlock` in the policy is `0.0.0.0/0`).
fn check_cidr(c: &str) -> anyhow::Result<()> {
    let ok = c.split_once('/').is_some_and(|(ip, len)| {
        ip.parse::<std::net::Ipv4Addr>().is_ok() && len.parse::<u8>().is_ok_and(|n| n <= 32)
    });
    if !ok {
        bail!("{c:?} is not an IPv4 CIDR");
    }
    Ok(())
}

/// `repo[:tag]` or `repo@digest` → the overlay's `images:` entry.
fn image_entry(r: &str) -> Value {
    if let Some((name, digest)) = r.split_once('@') {
        return json!({"name": IMAGE, "newName": name, "digest": digest});
    }
    match r.rsplit_once(':') {
        Some((name, tag)) if !tag.contains('/') => {
            json!({"name": IMAGE, "newName": name, "newTag": tag})
        }
        _ => json!({"name": IMAGE, "newName": r, "newTag": "latest"}),
    }
}

/// The overlay's `kustomization.yaml` (as JSON, which YAML reads). `ticket`: a `ticket` file
/// beside it becomes the `pair` Secret.
pub fn kustomization(spec: &K8sSpec, ticket: bool) -> anyhow::Result<Value> {
    check_name(&spec.name)?;
    let mut pvc = vec![json!({
        "op": "replace",
        "path": "/spec/resources/requests/storage",
        "value": spec.volume_size,
    })];
    if let Some(sc) = &spec.storage_class {
        pvc.push(json!({"op": "add", "path": "/spec/storageClassName", "value": sc}));
    }
    if !(1..=64).contains(&spec.memory_gb) {
        bail!("memory {} GiB: 1-64", spec.memory_gb);
    }
    let mem = format!("{}Gi", spec.memory_gb);
    let env = json!([
        {
            "op": "add",
            "path": "/spec/template/spec/containers/0/env/-",
            "value": {"name": "THEUTH_NODE_NAME", "value": spec.name},
        },
        {"op": "replace", "path": "/spec/template/spec/containers/0/resources/requests/memory", "value": mem},
        {"op": "replace", "path": "/spec/template/spec/containers/0/resources/limits/memory", "value": mem},
    ]);
    let mut patches = vec![
        json!({"target": {"kind": "Deployment", "name": "engine"}, "patch": env.to_string()}),
        json!({"target": {"kind": "PersistentVolumeClaim"}, "patch": Value::from(pvc).to_string()}),
    ];
    let mut k = json!({
        "apiVersion": "kustomize.config.k8s.io/v1beta1",
        "kind": "Kustomization",
        "namePrefix": format!("theuth-{}-", spec.name),
        "resources": ["base"],
        // The base labels its own objects `managed`; this also reaches the Secret and the
        // NetworkPolicy the overlay and the component add.
        "labels": [
            {"pairs": {LABEL_NODE: spec.name}, "includeSelectors": true},
            {"pairs": {LABEL_MANAGED: "true"}, "includeTemplates": true},
        ],
        "images": [image_entry(&spec.image)],
    });
    if let Some(ns) = &spec.namespace {
        k["namespace"] = ns.as_str().into();
    }
    if ticket {
        k["secretGenerator"] = json!([{
            "name": "pair",
            "files": ["ticket"],
            "options": {"disableNameSuffixHash": true},
        }]);
    }
    if let Some(cidrs) = &spec.deny_cidrs {
        let mut except = vec![METADATA_CIDR.to_owned()];
        for c in cidrs {
            check_cidr(c)?;
            if !except.contains(c) {
                except.push(c.clone());
            }
        }
        let op = json!([{"op": "replace", "path": "/spec/egress/1/to/0/ipBlock/except", "value": except}]);
        patches.push(
            json!({"target": {"kind": "NetworkPolicy", "name": "egress"}, "patch": op.to_string()}),
        );
        k["components"] = json!(["components/deny-cluster-network"]);
    }
    k["patches"] = patches.into();
    Ok(k)
}

/// A private temp directory, removed on drop.
struct Tree(PathBuf);

impl Drop for Tree {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn write_private(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut o, 0o600);
    std::io::Write::write_all(&mut o.open(path)?, bytes).with_context(|| path.display().to_string())
}

/// The embedded base, the overlay and the ticket in a fresh 0700 directory.
fn tree(spec: &K8sSpec, ticket: Option<&str>) -> anyhow::Result<Tree> {
    let dir = std::env::temp_dir().join(format!(
        "theuth-k8s-{}-{:016x}",
        std::process::id(),
        rand::random::<u64>()
    ));
    let mut b = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut b, 0o700);
    b.create(&dir).with_context(|| dir.display().to_string())?;
    let t = Tree(dir);
    for (rel, text) in FILES {
        let p = t.0.join(rel);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        write_private(&p, text.as_bytes())?;
    }
    let k = serde_yaml::to_string(&kustomization(spec, ticket.is_some())?)?;
    write_private(&t.0.join("kustomization.yaml"), k.as_bytes())?;
    if let Some(ticket) = ticket {
        write_private(&t.0.join("ticket"), ticket.trim().as_bytes())?;
    }
    Ok(t)
}

/// `kubectl [--context C] ARGS` with `stdin`; stdout, or an error carrying stderr.
pub async fn kubectl(
    context: Option<&str>,
    args: &[&str],
    stdin: Option<&[u8]>,
) -> anyhow::Result<String> {
    let mut cmd = tokio::process::Command::new("kubectl");
    if let Some(c) = context {
        cmd.arg("--context").arg(c);
    }
    cmd.args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .context("kubectl: not found on PATH (it reads your kubeconfig for `--k8s`)")?;
    if let (Some(bytes), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(bytes).await.context("kubectl stdin")?;
    }
    let out = child.wait_with_output().await.context("kubectl")?;
    if !out.status.success() {
        bail!(
            "kubectl {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The manifests `spec` (and `ticket`, as the `pair` Secret) render to.
pub async fn render(spec: &K8sSpec, ticket: Option<&str>) -> anyhow::Result<String> {
    let t = tree(spec, ticket)?;
    let dir = t.0.to_string_lossy().into_owned();
    kubectl(None, &["kustomize", &dir], None).await
}

/// `-n NS` when a namespace was given, else nothing (the context's namespace).
fn ns_args(namespace: Option<&str>) -> Vec<&str> {
    namespace
        .map(|n| vec!["--namespace", n])
        .unwrap_or_default()
}

/// The pod CIDRs (`node.spec.podCIDRs`) and service CIDRs (`ServiceCIDR`, Kubernetes 1.33+) the
/// cluster reports, IPv4 only. Either query may be forbidden or absent; what answered is
/// returned, and an empty list means "pass `--deny-cidr`".
pub async fn cluster_cidrs(context: Option<&str>) -> Vec<String> {
    let mut out = Vec::new();
    let mut take = |v: &Value| {
        if let Some(s) = v.as_str()
            && check_cidr(s).is_ok()
            && !out.iter().any(|o| o == s)
        {
            out.push(s.to_owned());
        }
    };
    if let Ok(s) = kubectl(context, &["get", "nodes", "-o", "json"], None).await
        && let Ok(v) = serde_json::from_str::<Value>(&s)
    {
        for n in v["items"].as_array().into_iter().flatten() {
            for c in n["spec"]["podCIDRs"].as_array().into_iter().flatten() {
                take(c);
            }
            take(&n["spec"]["podCIDR"]);
        }
    }
    if let Ok(s) = kubectl(context, &["get", "servicecidrs", "-o", "json"], None).await
        && let Ok(v) = serde_json::from_str::<Value>(&s)
    {
        for item in v["items"].as_array().into_iter().flatten() {
            for c in item["spec"]["cidrs"].as_array().into_iter().flatten() {
                take(c);
            }
        }
    }
    out
}

/// Apply `spec` with `ticket` (the namespace made first when `create_namespace`).
pub async fn apply(spec: &K8sSpec, ticket: &str, create_namespace: bool) -> anyhow::Result<()> {
    let ctx = spec.context.as_deref();
    if create_namespace {
        let ns = spec
            .namespace
            .as_deref()
            .context("--create-namespace needs --namespace")?;
        if kubectl(ctx, &["get", "namespace", ns, "-o", "name"], None)
            .await
            .is_err()
        {
            kubectl(ctx, &["create", "namespace", ns], None).await?;
        }
    }
    let yaml = render(spec, Some(ticket)).await?;
    kubectl(ctx, &["apply", "-f", "-"], Some(yaml.as_bytes())).await?;
    Ok(())
}

/// The ticket Secret's name for node `name`.
pub fn secret_name(name: &str) -> String {
    format!("theuth-{name}-pair")
}

/// The Deployment's name for node `name`.
pub fn deployment_name(name: &str) -> String {
    format!("theuth-{name}-engine")
}

/// Delete the (used or expired) ticket Secret.
pub async fn delete_ticket(
    context: Option<&str>,
    namespace: Option<&str>,
    name: &str,
) -> anyhow::Result<()> {
    let s = secret_name(name);
    let mut args = vec!["delete", "secret", &s, "--ignore-not-found"];
    args.extend(ns_args(namespace));
    kubectl(context, &args, None).await.map(drop)
}

/// The phase and readiness of the node's pod, one line (`Running 1/1`), for progress.
pub async fn pod_line(context: Option<&str>, namespace: Option<&str>, name: &str) -> String {
    let sel = format!("{LABEL_NODE}={name}");
    let mut args = vec!["get", "pods", "-l", &sel, "-o", "json"];
    args.extend(ns_args(namespace));
    let Ok(s) = kubectl(context, &args, None).await else {
        return "pod: unknown".into();
    };
    let v: Value = serde_json::from_str(&s).unwrap_or_default();
    let Some(p) = v["items"].as_array().and_then(|a| a.last()) else {
        return "pod: none yet".into();
    };
    let phase = p["status"]["phase"].as_str().unwrap_or("?");
    let waiting = p["status"]["containerStatuses"][0]["state"]["waiting"]["reason"]
        .as_str()
        .map(|r| format!(" ({r})"))
        .unwrap_or_default();
    format!("pod {phase}{waiting}")
}

/// One node as `t mesh k8s list` shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRow {
    /// Namespace.
    pub namespace: String,
    /// `theuth.io/node`.
    pub name: String,
    /// `ready/desired` replicas; `-` for a node whose Deployment is gone (volumes kept).
    pub ready: String,
    /// Container image; empty for kept volumes.
    pub image: String,
}

/// Managed Deployments and kept PVCs, in `namespace` or (None) all namespaces.
pub async fn list(context: Option<&str>, namespace: Option<&str>) -> anyhow::Result<Vec<NodeRow>> {
    let sel = format!("{LABEL_MANAGED}=true");
    let scope = match namespace {
        Some(n) => vec!["--namespace", n],
        None => vec!["--all-namespaces"],
    };
    let get = |kind: &'static str| {
        let mut args = vec!["get", kind, "-l", sel.as_str(), "-o", "json"];
        args.extend(scope.iter().copied());
        async move {
            let s = kubectl(context, &args, None).await?;
            anyhow::Ok(serde_json::from_str::<Value>(&s)?)
        }
    };
    let mut rows: Vec<NodeRow> = Vec::new();
    for d in get("deployments").await?["items"]
        .as_array()
        .into_iter()
        .flatten()
    {
        rows.push(NodeRow {
            namespace: d["metadata"]["namespace"].as_str().unwrap_or("").into(),
            name: d["metadata"]["labels"][LABEL_NODE]
                .as_str()
                .unwrap_or("")
                .into(),
            ready: format!(
                "{}/{}",
                d["status"]["readyReplicas"].as_u64().unwrap_or(0),
                d["spec"]["replicas"].as_u64().unwrap_or(0)
            ),
            image: d["spec"]["template"]["spec"]["containers"][0]["image"]
                .as_str()
                .unwrap_or("")
                .into(),
        });
    }
    for p in get("persistentvolumeclaims").await?["items"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let namespace = p["metadata"]["namespace"].as_str().unwrap_or("");
        let name = p["metadata"]["labels"][LABEL_NODE].as_str().unwrap_or("");
        if !rows
            .iter()
            .any(|r| r.namespace == namespace && r.name == name)
        {
            rows.push(NodeRow {
                namespace: namespace.into(),
                name: name.into(),
                ready: "-".into(),
                image: String::new(),
            });
        }
    }
    rows.sort_by(|a, b| (&a.namespace, &a.name).cmp(&(&b.namespace, &b.name)));
    Ok(rows)
}

/// `kubectl get` of every object of node `name`, as kubectl prints it.
pub async fn status(
    context: Option<&str>,
    namespace: Option<&str>,
    name: &str,
) -> anyhow::Result<String> {
    let sel = format!("{LABEL_NODE}={name}");
    let mut args = vec![
        "get",
        "deployments,pods,persistentvolumeclaims,secrets,networkpolicies,serviceaccounts",
        "-l",
        &sel,
        "-o",
        "wide",
    ];
    args.extend(ns_args(namespace));
    kubectl(context, &args, None).await
}

/// Delete node `name`; its PVCs too only with `delete_volume`. kubectl's output.
pub async fn destroy(
    context: Option<&str>,
    namespace: Option<&str>,
    name: &str,
    delete_volume: bool,
) -> anyhow::Result<String> {
    check_name(name)?;
    let sel = format!("{LABEL_NODE}={name}");
    let kinds = if delete_volume {
        "deployments,secrets,networkpolicies,serviceaccounts,persistentvolumeclaims"
    } else {
        "deployments,secrets,networkpolicies,serviceaccounts"
    };
    let mut args = vec!["delete", kinds, "-l", &sel, "--ignore-not-found", "--wait"];
    args.extend(ns_args(namespace));
    kubectl(context, &args, None).await
}

/// `(name, provisioner, default)` for each StorageClass.
pub async fn storage_classes(context: Option<&str>) -> anyhow::Result<Vec<(String, String, bool)>> {
    let s = kubectl(context, &["get", "storageclasses", "-o", "json"], None).await?;
    let v: Value = serde_json::from_str(&s)?;
    Ok(v["items"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            let a = &c["metadata"]["annotations"];
            let default = a["storageclass.kubernetes.io/is-default-class"] == "true"
                || a["storageclass.beta.kubernetes.io/is-default-class"] == "true";
            (
                c["metadata"]["name"].as_str().unwrap_or("").to_owned(),
                c["provisioner"].as_str().unwrap_or("").to_owned(),
                default,
            )
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> K8sSpec {
        K8sSpec {
            name: "lab".into(),
            context: None,
            namespace: Some("theuth".into()),
            storage_class: Some("fast".into()),
            volume_size: "20Gi".into(),
            image: "ghcr.io/basedatum/theuth/engine:2.31.2".into(),
            deny_cidrs: Some(vec!["10.42.0.0/16".into()]),
            memory_gb: 4,
        }
    }

    #[test]
    fn names_and_cidrs_are_checked() {
        for ok in ["lab", "a1-b2", &"a".repeat(40)] {
            check_name(ok).unwrap();
        }
        for bad in ["", "Lab", "1lab", "lab-", "la_b", &"a".repeat(41)] {
            assert!(check_name(bad).is_err(), "{bad}");
        }
        check_cidr("10.42.0.0/16").unwrap();
        for bad in ["10.42.0.0", "fd00::/8", "10.42.0.0/33", "x/8"] {
            assert!(check_cidr(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn image_refs_split_into_name_tag_or_digest() {
        assert_eq!(
            image_entry("ghcr.io/basedatum/theuth/engine:2.31.2"),
            json!({"name": IMAGE, "newName": IMAGE, "newTag": "2.31.2"})
        );
        assert_eq!(
            image_entry("localhost:5000/engine"),
            json!({"name": IMAGE, "newName": "localhost:5000/engine", "newTag": "latest"})
        );
        assert_eq!(
            image_entry("r.example/e@sha256:ab"),
            json!({"name": IMAGE, "newName": "r.example/e", "digest": "sha256:ab"})
        );
    }

    #[test]
    fn overlay_carries_ticket_policy_and_storage_only_when_asked() {
        let k = kustomization(&spec(), true).unwrap();
        assert_eq!(k["namePrefix"], "theuth-lab-");
        assert_eq!(k["namespace"], "theuth");
        assert_eq!(k["secretGenerator"][0]["name"], "pair");
        assert_eq!(k["components"][0], "components/deny-cluster-network");
        let patches = k["patches"].to_string();
        assert!(patches.contains("169.254.169.254/32") && patches.contains("10.42.0.0/16"));
        assert!(patches.contains("storageClassName") && patches.contains("20Gi"));
        assert!(patches.contains("resources/requests/memory") && patches.contains("4Gi"));

        let plain = K8sSpec {
            namespace: None,
            storage_class: None,
            deny_cidrs: None,
            ..spec()
        };
        let k = kustomization(&plain, false).unwrap();
        for key in ["namespace", "secretGenerator", "components"] {
            assert!(k.get(key).is_none(), "{key}");
        }
        assert!(!k["patches"].to_string().contains("storageClassName"));
        let bad = K8sSpec {
            deny_cidrs: Some(vec!["fd00::/8".into()]),
            ..spec()
        };
        assert!(kustomization(&bad, false).is_err());
    }

    #[test]
    fn the_tree_holds_base_overlay_and_a_private_ticket() {
        let t = tree(&spec(), Some("1.TICKET\n")).unwrap();
        for (rel, _) in FILES {
            assert!(t.0.join(rel).is_file(), "{rel}");
        }
        assert_eq!(
            std::fs::read_to_string(t.0.join("ticket")).unwrap(),
            "1.TICKET"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let mode = std::fs::metadata(t.0.join("ticket"))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        let dir = t.0.clone();
        drop(t);
        assert!(!dir.exists());
    }
}
