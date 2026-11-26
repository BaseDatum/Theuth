//! `t mesh add --k8s …` and `t mesh k8s …` (design doc 224 §7): the CLI side of
//! [`sandbox::k8s`]. Adding needs the running engine (the ticket and the join check); listing,
//! status, destroy, storage classes and `--dry-run` only need kubectl.

use std::time::{Duration, Instant};

use anyhow::Context as _;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::rpc_params;
use sandbox::k8s::{self, K8sSpec};

/// How long `t mesh add --k8s` waits for the engine to join: the ticket's TTL plus a minute.
const JOIN_WAIT: Duration = Duration::from_millis(mesh::pairing::TTL_MS + 60_000);

/// The `--k8s` flags of `t mesh add` (`--name` and `--image` are shared with `--docker`).
#[derive(clap::Args, Debug, Clone, Default)]
pub struct K8sArgs {
    /// Make an engine in a Kubernetes cluster your kubeconfig reaches (a Deployment + 2 PVCs; kubectl on PATH).
    #[arg(long, conflicts_with_all = ["pairing", "ssh", "docker", "vm", "local_t"])]
    pub k8s: bool,
    /// --k8s: kubeconfig context (default: the current one).
    #[arg(long, requires = "k8s")]
    pub context: Option<String>,
    /// --k8s: namespace (default: the context's).
    #[arg(long, requires = "k8s")]
    pub namespace: Option<String>,
    /// --k8s: create --namespace if it does not exist.
    #[arg(long, requires = "namespace")]
    pub create_namespace: bool,
    /// --k8s: StorageClass of the volumes (default: the cluster's default; see `t mesh k8s storage-classes`).
    #[arg(long, requires = "k8s")]
    pub storage_class: Option<String>,
    /// --k8s: size of each volume (/data and /workspace).
    #[arg(long, requires = "k8s", default_value = "10Gi")]
    pub volume_size: String,
    /// --k8s: add a NetworkPolicy: no ingress; egress to DNS and the internet but not the cluster's pod/service CIDRs.
    #[arg(long, requires = "k8s")]
    pub deny_cluster_network: bool,
    /// --deny-cluster-network: a CIDR to deny besides the detected ones (repeatable; needed when the cluster does not report its pod CIDRs).
    #[arg(long, requires = "deny_cluster_network", value_name = "CIDR")]
    pub deny_cidr: Vec<String>,
    /// --k8s: print the manifests (no ticket, no engine needed) instead of applying them.
    #[arg(long, requires = "k8s")]
    pub dry_run: bool,
}

/// `t mesh k8s …`.
#[derive(clap::Subcommand, Debug)]
pub enum K8sCmd {
    /// Engines `t mesh add --k8s` made (label theuth.io/managed=true), and volumes kept after destroy.
    List {
        /// kubeconfig context (default: the current one).
        #[arg(long)]
        context: Option<String>,
        /// One namespace (default: all namespaces).
        #[arg(long)]
        namespace: Option<String>,
    },
    /// Every object of one node, as kubectl prints it.
    Status {
        /// Node name (`--name` of `t mesh add --k8s`).
        name: String,
        /// kubeconfig context (default: the current one).
        #[arg(long)]
        context: Option<String>,
        /// Namespace (default: the context's).
        #[arg(long)]
        namespace: Option<String>,
    },
    /// Delete a node's Deployment, Secret, NetworkPolicy and ServiceAccount; its volumes only with
    /// --delete-volume. `t mesh remove <id>` takes it out of the mesh.
    Destroy {
        /// Node name.
        name: String,
        /// kubeconfig context (default: the current one).
        #[arg(long)]
        context: Option<String>,
        /// Namespace (default: the context's).
        #[arg(long)]
        namespace: Option<String>,
        /// Also delete the PersistentVolumeClaims (the engine's data is gone for good).
        #[arg(long)]
        delete_volume: bool,
    },
    /// StorageClasses for `--storage-class`, the default one marked.
    StorageClasses {
        /// kubeconfig context (default: the current one).
        #[arg(long)]
        context: Option<String>,
    },
}

/// The spec `t mesh add --k8s` applies; with `--deny-cluster-network`, the cluster's CIDRs are
/// looked up now (an empty result without `--deny-cidr` is an error, not an open policy).
pub async fn spec(
    a: &K8sArgs,
    name: Option<String>,
    image: Option<String>,
) -> anyhow::Result<K8sSpec> {
    let name = name.unwrap_or_else(|| "engine".into());
    k8s::check_name(&name)?;
    // A PVC without a class waits forever in a cluster with no default one.
    if a.storage_class.is_none()
        && let Ok(classes) = k8s::storage_classes(a.context.as_deref()).await
        && !classes.is_empty()
        && !classes.iter().any(|(_, _, default)| *default)
    {
        let names: Vec<&str> = classes.iter().map(|(n, _, _)| n.as_str()).collect();
        anyhow::bail!(
            "the cluster has no default StorageClass; pick one with --storage-class ({})",
            names.join(", ")
        );
    }
    let deny_cidrs = if a.deny_cluster_network {
        let mut cidrs = k8s::cluster_cidrs(a.context.as_deref()).await;
        if cidrs.is_empty() && a.deny_cidr.is_empty() {
            anyhow::bail!(
                "--deny-cluster-network: the cluster reports no pod or service CIDRs to this user; \
                 pass them with --deny-cidr (e.g. --deny-cidr 10.0.0.0/8)"
            );
        }
        for c in &a.deny_cidr {
            if !cidrs.contains(c) {
                cidrs.push(c.clone());
            }
        }
        Some(cidrs)
    } else {
        None
    };
    Ok(K8sSpec {
        name,
        context: a.context.clone(),
        namespace: a.namespace.clone(),
        storage_class: a.storage_class.clone(),
        volume_size: a.volume_size.clone(),
        image: image.unwrap_or_else(|| format!("{}:{}", k8s::IMAGE, platform::version::get())),
        deny_cidrs,
    })
}

/// `--dry-run`: the manifests, without the Secret.
pub async fn dry_run(
    a: &K8sArgs,
    name: Option<String>,
    image: Option<String>,
) -> anyhow::Result<()> {
    let spec = spec(a, name, image).await?;
    print!("{}", k8s::render(&spec, None).await?);
    Ok(())
}

/// `t mesh add --k8s`: mint a ticket (`t mesh pair --remote`), apply, wait until an engine of
/// that name with a new id is a peer, delete the ticket Secret.
pub async fn add(
    client: &jsonrpsee::ws_client::WsClient,
    a: &K8sArgs,
    name: Option<String>,
    image: Option<String>,
) -> anyhow::Result<()> {
    let spec = spec(a, name, image).await?;
    let (ctx, ns) = (spec.context.as_deref(), spec.namespace.as_deref());
    let before: mesh::MeshStatus = client.request("mesh_status", rpc_params![]).await?;
    let pairing: String = client.request("mesh_pairing_string", rpc_params![]).await?;
    let ticket = mesh::pairing::without_vault_key(&pairing)
        .context("the engine returned a malformed pairing string")?;
    println!(
        "apply: {} in {} ({})",
        k8s::deployment_name(&spec.name),
        ns.unwrap_or("the context's namespace"),
        spec.image
    );
    k8s::apply(&spec, &ticket, a.create_namespace).await?;
    let start = Instant::now();
    let mut last = String::new();
    let joined = loop {
        let status: mesh::MeshStatus = client.request("mesh_status", rpc_params![]).await?;
        if let Some(p) = status
            .peers
            .iter()
            .find(|p| p.name == spec.name && !before.peers.iter().any(|b| b.id == p.id))
        {
            break Some(p.clone());
        }
        if start.elapsed() > JOIN_WAIT {
            break None;
        }
        let line = k8s::pod_line(ctx, ns, &spec.name).await;
        if line != last {
            println!("wait: {line}");
            last = line;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    };
    // Used, or expired: either way nothing should keep it.
    if let Err(e) = k8s::delete_ticket(ctx, ns, &spec.name).await {
        eprintln!(
            "could not delete the ticket Secret {}: {e:#}",
            k8s::secret_name(&spec.name)
        );
    }
    match joined {
        Some(p) => {
            println!("engine {} ({}) joined the mesh", p.name, p.id);
            Ok(())
        }
        None => {
            eprintln!(
                "{} did not join within {} s; see `kubectl logs deployment/{}`{}. \
                 A fresh ticket: `t mesh pair --remote` into /data/pair-ticket in the pod.",
                spec.name,
                JOIN_WAIT.as_secs(),
                k8s::deployment_name(&spec.name),
                ns.map(|n| format!(" -n {n}")).unwrap_or_default()
            );
            std::process::exit(1);
        }
    }
}

/// `t mesh k8s …`.
pub async fn run(cmd: K8sCmd) -> anyhow::Result<()> {
    match cmd {
        K8sCmd::List { context, namespace } => {
            let rows = k8s::list(context.as_deref(), namespace.as_deref()).await?;
            if rows.is_empty() {
                println!("no theuth nodes");
            }
            for r in rows {
                let state = if r.ready == "-" {
                    "volumes kept".to_owned()
                } else {
                    format!("ready {}", r.ready)
                };
                println!("{}/{}  {state}  {}", r.namespace, r.name, r.image);
            }
        }
        K8sCmd::Status {
            name,
            context,
            namespace,
        } => print!(
            "{}",
            k8s::status(context.as_deref(), namespace.as_deref(), &name).await?
        ),
        K8sCmd::Destroy {
            name,
            context,
            namespace,
            delete_volume,
        } => {
            print!(
                "{}",
                k8s::destroy(
                    context.as_deref(),
                    namespace.as_deref(),
                    &name,
                    delete_volume
                )
                .await?
            );
            if !delete_volume {
                println!("volumes kept: `t mesh k8s destroy {name} --delete-volume` removes them");
            }
            println!("`t mesh status` → `t mesh remove <id>` takes {name} out of the mesh");
        }
        K8sCmd::StorageClasses { context } => {
            for (name, provisioner, default) in k8s::storage_classes(context.as_deref()).await? {
                let mark = if default { "  (default)" } else { "" };
                println!("{name}  {provisioner}{mark}");
            }
        }
    }
    Ok(())
}
