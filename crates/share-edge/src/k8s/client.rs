//! [`KubeApi`] over kube-rs (feature `k8s`): in-cluster config by `Config::infer` (the pod's service account),
//! kinds resolved by discovery (`pinned_kind`) and cached, `DynamicObject` APIs, server-side apply with the
//! field manager and `force = false`.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::Context as _;
use futures::future::BoxFuture;
use kube::api::{Api, ApiResource, DeleteParams, DynamicObject, ListParams, Patch, PatchParams};
use kube::core::GroupVersionKind;
use kube::discovery::{self, Scope};
use kube::{Client, Config};

use super::{KubeApi, Listed, Obj, ObjKey, SHARE_NAME, TypeKey};

/// The cluster client.
pub struct KubeClient {
    client: Client,
    field_manager: String,
    kinds: Mutex<HashMap<TypeKey, (ApiResource, bool)>>,
}

impl KubeClient {
    /// Infer the config (in cluster: the service-account token, CA and namespace file; else kubeconfig) and
    /// build the client. Returns the client and the inferred default namespace. Makes no API call.
    pub async fn infer(field_manager: &str) -> anyhow::Result<(KubeClient, String)> {
        let config = Config::infer().await.context("k8s: infer config")?;
        let namespace = config.default_namespace.clone();
        let client = Client::try_from(config).context("k8s: client")?;
        Ok((
            KubeClient {
                client,
                field_manager: field_manager.into(),
                kinds: Mutex::new(HashMap::new()),
            },
            namespace,
        ))
    }

    async fn resolve(&self, t: &TypeKey) -> anyhow::Result<(ApiResource, bool)> {
        if let Some(r) = self.kinds.lock().expect("kinds").get(t) {
            return Ok(r.clone());
        }
        let (group, version) = t
            .api_version
            .split_once('/')
            .unwrap_or(("", t.api_version.as_str()));
        let gvk = GroupVersionKind::gvk(group, version, &t.kind);
        let (ar, caps) = discovery::pinned_kind(&self.client, &gvk)
            .await
            .with_context(|| format!("k8s: discover {} {}", t.api_version, t.kind))?;
        let r = (ar, caps.scope == Scope::Namespaced);
        self.kinds
            .lock()
            .expect("kinds")
            .insert(t.clone(), r.clone());
        Ok(r)
    }

    async fn api(&self, t: &TypeKey, ns: Option<&str>) -> anyhow::Result<Api<DynamicObject>> {
        let (ar, namespaced) = self.resolve(t).await?;
        Ok(match (namespaced, ns) {
            (true, Some(ns)) => Api::namespaced_with(self.client.clone(), ns, &ar),
            (true, None) => anyhow::bail!("k8s: {} is namespaced; no namespace", t.kind),
            (false, _) => Api::all_with(self.client.clone(), &ar),
        })
    }
}

fn not_found(e: &kube::Error) -> bool {
    matches!(e, kube::Error::Api(s) if s.code == 404)
}

impl KubeApi for KubeClient {
    fn namespaced<'a>(&'a self, t: &'a TypeKey) -> BoxFuture<'a, anyhow::Result<bool>> {
        Box::pin(async move { Ok(self.resolve(t).await?.1) })
    }

    fn apply<'a>(&'a self, o: &'a Obj) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let api = self.api(&o.key.t, o.key.namespace.as_deref()).await?;
            // `force` stays false: a field another manager owns is a conflict to report, not to take.
            let pp = PatchParams::apply(&self.field_manager);
            api.patch(&o.key.name, &pp, &Patch::Apply(&o.body)).await?;
            Ok(())
        })
    }

    fn delete<'a>(&'a self, k: &'a ObjKey) -> BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move {
            let api = self.api(&k.t, k.namespace.as_deref()).await?;
            match api.delete(&k.name, &DeleteParams::default()).await {
                Ok(_) => Ok(()),
                Err(e) if not_found(&e) => Ok(()),
                Err(e) => Err(e.into()),
            }
        })
    }

    fn list<'a>(
        &'a self,
        t: &'a TypeKey,
        namespace: Option<&'a str>,
        selector: &'a str,
    ) -> BoxFuture<'a, anyhow::Result<Vec<Listed>>> {
        Box::pin(async move {
            let api = self.api(t, namespace).await?;
            let list = api.list(&ListParams::default().labels(selector)).await?;
            Ok(list
                .items
                .into_iter()
                .filter_map(|o| {
                    let name = o.metadata.name?;
                    Some(Listed {
                        key: ObjKey {
                            t: t.clone(),
                            namespace: o.metadata.namespace,
                            name,
                        },
                        share_name: o
                            .metadata
                            .labels
                            .and_then(|mut l| l.remove(SHARE_NAME)),
                    })
                })
                .collect())
        })
    }
}
