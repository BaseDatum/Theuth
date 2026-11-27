//! DigitalOcean (design doc 224 §6): tags, a Cloud Firewall with no inbound rule, a Block
//! Storage volume at `/data`, then a Droplet (Ubuntu 24.04) whose user data installs the engine
//! natively. No SSH key is ever uploaded or attached. Same calls, same order as the live-tested
//! `plugins/cloud-nodes/cloud_nodes.py` / `scripts/cloud-node-do-test.sh`.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};
use zeroize::Zeroizing;

use super::catalog;
use super::http::{Api, enc};
use super::{
    CloudError, CreateReq, Created, Listed, Pace, Provider, ProviderStatus, Result, Say,
    TAG_MANAGED, Transport, check_name, tag_node, userdata,
};
use crate::types::{ProviderKind, SizeOption, Stage};

/// DigitalOcean API v2.
pub const API: &str = "https://api.digitalocean.com/v2";
/// `vm-cloud-init.sh` names Ubuntu 24.04's packages.
pub const IMAGE: &str = "ubuntu-24-04-x64";

/// `GET /v2/account`: what a token belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    /// The account's email.
    pub email: Option<String>,
    /// `active`, `warning`, `locked`.
    pub status: Option<String>,
}

/// One DigitalOcean account.
pub struct DigitalOcean {
    api: Api,
    /// Polling of boots, actions and detaches.
    pub pace: Pace,
}

fn server_name(name: &str) -> String {
    format!("theuth-{name}")
}

fn volume_name(name: &str) -> String {
    format!("theuth-{name}-data")
}

/// The device DO gives an attached volume.
pub fn data_dev(name: &str) -> String {
    format!("/dev/disk/by-id/scsi-0DO_Volume_{}", volume_name(name))
}

fn public_ip(d: &Value) -> Option<String> {
    d["networks"]["v4"]
        .as_array()?
        .iter()
        .find(|n| n["type"] == "public")
        .and_then(|n| n["ip_address"].as_str())
        .map(str::to_owned)
}

/// The node name in a resource's `theuth-node:<name>` tag.
fn node_of(tags: &Value) -> Option<String> {
    tags.as_array()?
        .iter()
        .filter_map(Value::as_str)
        .find_map(|t| t.strip_prefix("theuth-node:"))
        .map(str::to_owned)
}

fn has_tag(v: &Value, tag: &str) -> bool {
    v["tags"]
        .as_array()
        .is_some_and(|a| a.iter().any(|t| t == tag))
}

fn id_str(v: &Value) -> Option<String> {
    match v {
        Value::Number(n) => Some(n.to_string()),
        Value::String(s) => Some(s.clone()),
        _ => None,
    }
}

fn volume_gb(v: &Value) -> Option<u32> {
    v["size_gigabytes"].as_u64().and_then(|n| u32::try_from(n).ok())
}

impl DigitalOcean {
    /// A client for `token` (OAuth or personal access token).
    pub fn new(token: Zeroizing<String>, transport: Arc<dyn Transport>) -> Self {
        Self {
            api: Api::new("DigitalOcean", API, token, transport),
            pace: Pace::default(),
        }
    }

    /// Override the polling pace (tests).
    pub fn with_pace(mut self, pace: Pace) -> Self {
        self.pace = pace;
        self
    }

    /// Validates the token.
    pub async fn account(&self) -> Result<Account> {
        let v = self.api.call("GET", "/account", None).await?;
        Ok(Account {
            email: v["account"]["email"].as_str().map(str::to_owned),
            status: v["account"]["status"].as_str().map(str::to_owned),
        })
    }

    /// The 2/4/8 GB sizes with live list prices (`GET /v2/sizes`), only those offered in
    /// `region` when given.
    pub async fn sizes(&self, region: Option<&str>) -> Result<Vec<SizeOption>> {
        let v = self.api.call("GET", "/sizes?per_page=200", None).await?;
        let live = v["sizes"].as_array().map(Vec::as_slice).unwrap_or_default();
        Ok(catalog::sizes(ProviderKind::Digitalocean)
            .into_iter()
            .filter_map(|mut s| {
                let l = live.iter().find(|l| l["slug"] == s.slug.as_str())?;
                if l["available"] == false {
                    return None;
                }
                if let Some(r) = region
                    && !l["regions"].as_array().is_some_and(|a| a.iter().any(|x| x == r))
                {
                    return None;
                }
                s.vcpus = l["vcpus"]
                    .as_u64()
                    .and_then(|n| u8::try_from(n).ok())
                    .unwrap_or(s.vcpus);
                s.monthly_usd = l["price_monthly"].as_f64();
                Some(s)
            })
            .collect())
    }

    async fn droplet(&self, name: &str) -> Result<Option<Value>> {
        let v = self
            .api
            .call("GET", &format!("/droplets?tag_name={}", enc(&tag_node(name))), None)
            .await?;
        Ok(v["droplets"].as_array().and_then(|a| a.first()).cloned())
    }

    async fn volume(&self, name: &str, region: Option<&str>) -> Result<Option<Value>> {
        let mut q = format!("/volumes?name={}", volume_name(name));
        if let Some(r) = region {
            q += &format!("&region={}", enc(r));
        }
        let v = self.api.call("GET", &q, None).await?;
        Ok(v["volumes"].as_array().and_then(|a| a.first()).cloned())
    }

    async fn firewall(&self, name: &str) -> Result<Option<Value>> {
        let v = self.api.call("GET", "/firewalls?per_page=200", None).await?;
        let want = server_name(name);
        Ok(v["firewalls"]
            .as_array()
            .and_then(|a| a.iter().find(|f| f["name"] == want.as_str()))
            .cloned())
    }

    async fn droplet_of(&self, name: &str) -> Result<Value> {
        self.droplet(name)
            .await?
            .ok_or_else(|| CloudError(format!("no Droplet for node {name}")))
    }

    async fn action(&self, droplet: &str, body: Value) -> Result<()> {
        let v = self
            .api
            .call("POST", &format!("/droplets/{droplet}/actions"), Some(body))
            .await?;
        let id = id_str(&v["action"]["id"])
            .ok_or_else(|| CloudError("DigitalOcean: action without an id".into()))?;
        for _ in 0..self.pace.tries {
            let a = self.api.call("GET", &format!("/actions/{id}"), None).await?;
            match a["action"]["status"].as_str() {
                Some("completed") => return Ok(()),
                Some("errored") => {
                    return Err(CloudError(format!(
                        "DigitalOcean: {} failed",
                        a["action"]["type"].as_str().unwrap_or("action")
                    )));
                }
                _ => self.pace.sleep().await,
            }
        }
        Err(CloudError(format!("DigitalOcean: action {id} did not finish")))
    }
}

#[async_trait]
impl Provider for DigitalOcean {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Digitalocean
    }

    async fn create(&self, req: &CreateReq, say: Say<'_>) -> Result<Created> {
        let name = req.name.as_str();
        check_name(name)?;
        let size = catalog::slug(ProviderKind::Digitalocean, req.memory_gb)?;
        let user_data = userdata::render(name, &data_dev(name), &req.ticket, &req.manifest, req.desktop)?;
        if self.droplet(name).await?.is_some() {
            return Err(CloudError(format!(
                "a Droplet tagged {} exists; destroy it first",
                tag_node(name)
            )));
        }
        let tags = [TAG_MANAGED.to_owned(), tag_node(name)];
        for t in &tags {
            self.api
                .call_any("POST", "/tags", Some(json!({"name": t})), &[201, 422])
                .await?;
        }
        // The firewall first: the Droplet is never reachable inbound, not even while booting.
        let firewall = match self.firewall(name).await? {
            Some(f) => f,
            None => {
                let out = json!({"addresses": ["0.0.0.0/0", "::/0"]});
                let v = self
                    .api
                    .call(
                        "POST",
                        "/firewalls",
                        Some(json!({
                            "name": server_name(name),
                            "tags": [tag_node(name)],
                            "inbound_rules": [],
                            "outbound_rules": [
                                {"protocol": "tcp", "ports": "all", "destinations": out},
                                {"protocol": "udp", "ports": "all", "destinations": out},
                                {"protocol": "icmp", "destinations": out},
                            ],
                        })),
                    )
                    .await?;
                v["firewall"].clone()
            }
        };
        say(Stage::Server, "Firewall: no inbound connections");
        let volume = match self.volume(name, Some(&req.region)).await? {
            Some(v) => {
                say(Stage::Server, &format!("Reusing volume {}", volume_name(name)));
                v
            }
            None => {
                let v = self
                    .api
                    .call(
                        "POST",
                        "/volumes",
                        Some(json!({
                            "name": volume_name(name),
                            "region": req.region,
                            "size_gigabytes": req.volume_gb,
                            "filesystem_type": "ext4",
                            "filesystem_label": "theuth-data",
                            "tags": tags,
                            "description": "theuth node /data (design doc 224)",
                        })),
                    )
                    .await?;
                v["volume"].clone()
            }
        };
        let volume_id = volume["id"]
            .as_str()
            .ok_or_else(|| CloudError("DigitalOcean: volume without an id".into()))?
            .to_owned();
        let v = self
            .api
            .call(
                "POST",
                "/droplets",
                Some(json!({
                    "name": server_name(name),
                    "region": req.region,
                    "size": size,
                    "image": IMAGE,
                    "volumes": [volume_id],
                    "tags": tags,
                    "ipv6": true,
                    "monitoring": false,
                    "user_data": user_data,
                })),
            )
            .await?;
        let id = id_str(&v["droplet"]["id"])
            .ok_or_else(|| CloudError("DigitalOcean: Droplet without an id".into()))?;
        say(Stage::Server, &format!("Droplet {} created", server_name(name)));
        let mut last = String::new();
        for _ in 0..self.pace.tries {
            let d = self.api.call("GET", &format!("/droplets/{id}"), None).await?;
            let d = &d["droplet"];
            let status = d["status"].as_str().unwrap_or("?");
            if status != last {
                say(Stage::Server, &format!("Droplet {status}"));
                last = status.to_owned();
            }
            if status == "active" {
                let attached = d["volume_ids"]
                    .as_array()
                    .is_some_and(|a| a.iter().any(|v| v == volume_id.as_str()));
                let attached = attached || {
                    let v = self.api.call("GET", &format!("/volumes/{volume_id}"), None).await?;
                    v["volume"]["droplet_ids"]
                        .as_array()
                        .is_some_and(|a| a.iter().any(|x| id_str(x).as_deref() == Some(&id)))
                };
                if attached {
                    say(
                        Stage::Storage,
                        &format!("Volume {} attached at /data", volume_name(name)),
                    );
                    return Ok(Created {
                        server_id: Some(id),
                        volume_id: Some(volume_id),
                        extra: firewall["id"].as_str().map(str::to_owned),
                        ip: public_ip(d),
                        size_slug: size,
                    });
                }
            }
            self.pace.sleep().await;
        }
        Err(CloudError(format!(
            "Droplet {} did not become active with its volume attached",
            server_name(name)
        )))
    }

    async fn status(&self, name: &str) -> Result<Option<ProviderStatus>> {
        let d = self.droplet(name).await?;
        let v = self.volume(name, None).await?;
        if d.is_none() && v.is_none() {
            return Ok(None);
        }
        let f = self.firewall(name).await?;
        let slug = d.as_ref().and_then(|d| d["size_slug"].as_str().map(str::to_owned));
        Ok(Some(ProviderStatus {
            server_state: d.as_ref().and_then(|d| d["status"].as_str().map(str::to_owned)),
            ip: d.as_ref().and_then(public_ip),
            region: d
                .as_ref()
                .or(v.as_ref())
                .and_then(|x| x["region"]["slug"].as_str().map(str::to_owned)),
            memory_gb: slug
                .as_deref()
                .and_then(|s| catalog::memory_of(ProviderKind::Digitalocean, s)),
            size_slug: slug,
            volume_gb: v.as_ref().and_then(volume_gb),
            inbound_rules: f
                .as_ref()
                .and_then(|f| f["inbound_rules"].as_array().map(Vec::len))
                .unwrap_or(0),
        }))
    }

    async fn list(&self) -> Result<Vec<Listed>> {
        let ds = self
            .api
            .call("GET", &format!("/droplets?tag_name={TAG_MANAGED}&per_page=200"), None)
            .await?;
        let vs = self.api.call("GET", "/volumes?per_page=200", None).await?;
        let mut volumes: BTreeMap<String, &Value> = BTreeMap::new();
        for v in vs["volumes"].as_array().into_iter().flatten() {
            if has_tag(v, TAG_MANAGED)
                && let Some(n) = node_of(&v["tags"])
            {
                volumes.insert(n, v);
            }
        }
        let mut nodes: BTreeMap<String, Listed> = BTreeMap::new();
        for d in ds["droplets"].as_array().into_iter().flatten() {
            let Some(n) = node_of(&d["tags"]) else {
                continue;
            };
            nodes.insert(
                n.clone(),
                Listed {
                    server_state: d["status"].as_str().map(str::to_owned),
                    region: d["region"]["slug"].as_str().map(str::to_owned),
                    ip: public_ip(d),
                    memory_gb: d["size_slug"]
                        .as_str()
                        .and_then(|s| catalog::memory_of(ProviderKind::Digitalocean, s)),
                    volume_gb: volumes.get(&n).and_then(|v| volume_gb(v)),
                    volume_only: false,
                    name: n,
                },
            );
        }
        for (n, v) in volumes {
            nodes.entry(n.clone()).or_insert_with(|| Listed {
                name: n,
                server_state: None,
                region: v["region"]["slug"].as_str().map(str::to_owned),
                ip: None,
                memory_gb: None,
                volume_gb: volume_gb(v),
                volume_only: true,
            });
        }
        Ok(nodes.into_values().collect())
    }

    async fn restart(&self, name: &str) -> Result<()> {
        let d = self.droplet_of(name).await?;
        let id = id_str(&d["id"]).unwrap_or_default();
        self.api
            .call(
                "POST",
                &format!("/droplets/{id}/actions"),
                Some(json!({"type": "reboot"})),
            )
            .await?;
        Ok(())
    }

    async fn resize(&self, name: &str, memory_gb: u8) -> Result<String> {
        let size = catalog::slug(ProviderKind::Digitalocean, memory_gb)?;
        let d = self.droplet_of(name).await?;
        if d["size_slug"] == size.as_str() {
            return Ok(size);
        }
        let id = id_str(&d["id"]).unwrap_or_default();
        if d["status"] != "off" {
            self.action(&id, json!({"type": "power_off"})).await?;
        }
        // `disk: false`: RAM/CPU only, so the size can go down again later.
        self.action(&id, json!({"type": "resize", "size": size, "disk": false}))
            .await?;
        self.action(&id, json!({"type": "power_on"})).await?;
        Ok(size)
    }

    async fn destroy(&self, name: &str, keep_volume: bool) -> Result<Vec<String>> {
        check_name(name)?;
        let mut done = Vec::new();
        if let Some(d) = self.droplet(name).await? {
            let id = id_str(&d["id"]).unwrap_or_default();
            self.api.call("DELETE", &format!("/droplets/{id}"), None).await?;
            done.push(format!("Droplet {} deleted", server_name(name)));
        }
        if let Some(f) = self.firewall(name).await? {
            let id = f["id"].as_str().unwrap_or_default();
            self.api.call("DELETE", &format!("/firewalls/{id}"), None).await?;
            done.push(format!("firewall {} deleted", server_name(name)));
        }
        if let Some(v) = self.volume(name, None).await? {
            let id = v["id"].as_str().unwrap_or_default().to_owned();
            if keep_volume {
                let gb = volume_gb(&v).unwrap_or(0);
                done.push(format!(
                    "volume {} kept (${:.2}/month)",
                    volume_name(name),
                    catalog::DO_VOLUME_USD_PER_GB_MONTH * f64::from(gb)
                ));
            } else {
                // Detaches with the Droplet's deletion, asynchronously.
                let mut detached = false;
                for _ in 0..self.pace.tries {
                    let cur = self.api.call("GET", &format!("/volumes/{id}"), None).await?;
                    if cur["volume"]["droplet_ids"]
                        .as_array()
                        .is_none_or(Vec::is_empty)
                    {
                        detached = true;
                        break;
                    }
                    self.pace.sleep().await;
                }
                if !detached {
                    return Err(CloudError(format!(
                        "volume {} is still attached; destroy again in a minute",
                        volume_name(name)
                    )));
                }
                self.api.call("DELETE", &format!("/volumes/{id}"), None).await?;
                done.push(format!("volume {} deleted", volume_name(name)));
            }
        }
        if !keep_volume {
            self.api
                .call_any(
                    "DELETE",
                    &format!("/tags/{}", enc(&tag_node(name))),
                    None,
                    &[204, 404, 422],
                )
                .await?;
        }
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::catalog::{ENGINE_IMAGE, MANIFEST};
    use crate::providers::http::testing::FakeTransport;

    const TICKET: &str = "1.ABCDEFGHIJKLMNOP234567";

    fn client(t: &Arc<FakeTransport>) -> DigitalOcean {
        DigitalOcean::new(Zeroizing::new("dop_v1_tok".into()), t.clone()).with_pace(Pace::NOW)
    }

    fn req() -> CreateReq {
        CreateReq {
            name: "lab".into(),
            region: "lon1".into(),
            memory_gb: 2,
            volume_gb: 10,
            ticket: TICKET.into(),
            manifest: MANIFEST.into(),
            image: ENGINE_IMAGE.into(),
            k8s: None,
            desktop: true,
        }
    }

    fn droplet(status: &str, volumes: &[&str]) -> Value {
        json!({"droplet": {
            "id": 42, "status": status, "size_slug": "s-1vcpu-2gb", "volume_ids": volumes,
            "region": {"slug": "lon1"}, "tags": [TAG_MANAGED, "theuth-node:lab"],
            "networks": {"v4": [{"type": "private", "ip_address": "10.0.0.2"},
                                {"type": "public", "ip_address": "203.0.113.7"}]},
        }})
    }

    #[tokio::test]
    async fn create_makes_the_firewall_first_with_no_inbound_and_no_ssh_key() {
        let t = FakeTransport::new();
        t.on("GET", "/droplets?tag_name=", 200, json!({"droplets": []}))
            .on("POST", "/tags", 201, json!({}))
            .on("GET", "/firewalls", 200, json!({"firewalls": []}))
            .on("POST", "/firewalls", 202, json!({"firewall": {"id": "fw-1"}}))
            .on("GET", "/volumes?name=", 200, json!({"volumes": []}))
            .on("POST", "/volumes", 201, json!({"volume": {"id": "vol-1"}}))
            .on("POST", "/droplets", 202, json!({"droplet": {"id": 42}}))
            .once("GET", "/droplets/42", 200, droplet("new", &[]))
            .on("GET", "/droplets/42", 200, droplet("active", &["vol-1"]));
        let lines = parking_lot::Mutex::new(Vec::new());
        let say = |s: Stage, l: &str| lines.lock().push((s, l.to_owned()));
        let c = client(&t).create(&req(), &say).await.unwrap();
        assert_eq!(
            c,
            Created {
                server_id: Some("42".into()),
                volume_id: Some("vol-1".into()),
                extra: Some("fw-1".into()),
                ip: Some("203.0.113.7".into()),
                size_slug: "s-1vcpu-2gb".into(),
            }
        );
        let calls = t.lines();
        let pos = |p: &str| calls.iter().position(|c| c.starts_with(p)).unwrap();
        assert!(pos("POST https://api.digitalocean.com/v2/tags") < pos("POST https://api.digitalocean.com/v2/firewalls"));
        assert!(pos("POST https://api.digitalocean.com/v2/firewalls") < pos("POST https://api.digitalocean.com/v2/volumes"));
        assert!(pos("POST https://api.digitalocean.com/v2/volumes") < pos("POST https://api.digitalocean.com/v2/droplets"));

        let reqs = t.requests();
        let body = |p: &str| {
            reqs.iter()
                .find(|r| r.method == "POST" && r.url.ends_with(p))
                .and_then(|r| r.json.clone())
                .unwrap()
        };
        let fw = body("/firewalls");
        assert_eq!(fw["inbound_rules"], json!([]));
        assert_eq!(fw["outbound_rules"].as_array().unwrap().len(), 3);
        assert_eq!(fw["tags"], json!(["theuth-node:lab"]));
        let vol = body("/volumes");
        assert_eq!(vol["tags"], json!([TAG_MANAGED, "theuth-node:lab"]));
        let d = body("/droplets");
        assert!(d.get("ssh_keys").is_none());
        assert_eq!(d["tags"], json!([TAG_MANAGED, "theuth-node:lab"]));
        assert_eq!((d["image"].as_str(), d["size"].as_str()), (Some(IMAGE), Some("s-1vcpu-2gb")));
        assert_eq!(d["volumes"], json!(["vol-1"]));
        // The ticket appears in the user data and nowhere else.
        for r in &reqs {
            let j = r.json.as_ref().map(Value::to_string).unwrap_or_default();
            let in_ud = r.url.ends_with("/droplets") && r.method == "POST";
            assert_eq!(j.contains(TICKET), in_ud, "{} {}", r.method, r.url);
            assert!(!r.url.contains(TICKET));
            assert!(!format!("{r:?}").contains("dop_v1_tok"));
        }
        assert!(d["user_data"].as_str().unwrap().contains(TICKET));
        let lines = lines.into_inner();
        assert!(lines.iter().any(|(s, l)| *s == Stage::Server && l == "Droplet new"));
        assert_eq!(lines.last().unwrap().0, Stage::Storage);
    }

    #[tokio::test]
    async fn create_refuses_an_existing_droplet_and_reuses_a_kept_volume() {
        let t = FakeTransport::new();
        t.once("GET", "/droplets?tag_name=", 200, json!({"droplets": [{"id": 1}]}));
        assert!(client(&t).create(&req(), &|_, _| {}).await.is_err());

        let t = FakeTransport::new();
        t.on("GET", "/droplets?tag_name=", 200, json!({"droplets": []}))
            .on("POST", "/tags", 422, json!({}))
            .on("GET", "/firewalls", 200, json!({"firewalls": [{"id": "fw-1", "name": "theuth-lab"}]}))
            .on("GET", "/volumes?name=theuth-lab-data&region=lon1", 200, json!({"volumes": [{"id": "vol-9"}]}))
            .on("POST", "/droplets", 202, json!({"droplet": {"id": 42}}))
            .on("GET", "/droplets/42", 200, droplet("active", &[]))
            .on("GET", "/volumes/vol-9", 200, json!({"volume": {"droplet_ids": [42]}}));
        let c = client(&t).create(&req(), &|_, _| {}).await.unwrap();
        assert_eq!(c.volume_id.as_deref(), Some("vol-9"));
        let calls = t.lines().join("\n");
        assert!(!calls.contains("POST https://api.digitalocean.com/v2/volumes"));
        assert!(!calls.contains("POST https://api.digitalocean.com/v2/firewalls"));
    }

    #[tokio::test]
    async fn list_finds_orphans_and_volume_only_nodes() {
        let t = FakeTransport::new();
        t.on("GET", "/droplets?tag_name=theuth-managed", 200, json!({"droplets": [droplet("active", &[])["droplet"]]}))
            .on("GET", "/volumes?per_page", 200, json!({"volumes": [
                {"id": "v1", "size_gigabytes": 10, "region": {"slug": "lon1"}, "tags": [TAG_MANAGED, "theuth-node:lab"]},
                {"id": "v2", "size_gigabytes": 20, "region": {"slug": "fra1"}, "tags": [TAG_MANAGED, "theuth-node:old"]},
                {"id": "v3", "size_gigabytes": 5, "region": {"slug": "fra1"}, "tags": ["theuth-node:foreign"]},
            ]}));
        let l = client(&t).list().await.unwrap();
        assert_eq!(
            l,
            vec![
                Listed {
                    name: "lab".into(),
                    server_state: Some("active".into()),
                    region: Some("lon1".into()),
                    ip: Some("203.0.113.7".into()),
                    memory_gb: Some(2),
                    volume_gb: Some(10),
                    volume_only: false,
                },
                Listed {
                    name: "old".into(),
                    server_state: None,
                    region: Some("fra1".into()),
                    ip: None,
                    memory_gb: None,
                    volume_gb: Some(20),
                    volume_only: true,
                },
            ]
        );
    }

    #[tokio::test]
    async fn status_reports_inbound_rules_and_none_when_gone() {
        let t = FakeTransport::new();
        t.on("GET", "/droplets?tag_name=", 200, json!({"droplets": [droplet("active", &[])["droplet"]]}))
            .on("GET", "/volumes?name=", 200, json!({"volumes": [{"id": "v", "size_gigabytes": 10}]}))
            .on("GET", "/firewalls", 200, json!({"firewalls": [{"id": "f", "name": "theuth-lab", "inbound_rules": []}]}));
        let s = client(&t).status("lab").await.unwrap().unwrap();
        assert_eq!((s.server_state.as_deref(), s.memory_gb, s.volume_gb, s.inbound_rules), (Some("active"), Some(2), Some(10), 0));

        let t = FakeTransport::new();
        t.on("GET", "/droplets?", 200, json!({"droplets": []}))
            .on("GET", "/volumes?", 200, json!({"volumes": []}));
        assert_eq!(client(&t).status("lab").await.unwrap(), None);
    }

    fn destroy_fake() -> Arc<FakeTransport> {
        let t = FakeTransport::new();
        t.on("GET", "/droplets?tag_name=", 200, json!({"droplets": [{"id": 42}]}))
            .on("DELETE", "/droplets/42", 204, Value::Null)
            .on("GET", "/firewalls", 200, json!({"firewalls": [{"id": "fw-1", "name": "theuth-lab"}]}))
            .on("DELETE", "/firewalls/fw-1", 204, Value::Null)
            .on("GET", "/volumes?name=", 200, json!({"volumes": [{"id": "vol-1", "size_gigabytes": 10}]}))
            .once("GET", "/volumes/vol-1", 200, json!({"volume": {"droplet_ids": [42]}}))
            .on("GET", "/volumes/vol-1", 200, json!({"volume": {"droplet_ids": []}}))
            .on("DELETE", "/volumes/vol-1", 204, Value::Null)
            .on("DELETE", "/tags/", 204, Value::Null);
        t
    }

    #[tokio::test]
    async fn destroy_deletes_or_keeps_the_volume() {
        let t = destroy_fake();
        let done = client(&t).destroy("lab", false).await.unwrap();
        assert_eq!(done, ["Droplet theuth-lab deleted", "firewall theuth-lab deleted", "volume theuth-lab-data deleted"]);
        let calls = t.lines();
        assert!(calls.contains(&"DELETE https://api.digitalocean.com/v2/volumes/vol-1".to_owned()));
        assert!(calls.contains(&"DELETE https://api.digitalocean.com/v2/tags/theuth-node%3Alab".to_owned()));

        let t = destroy_fake();
        let done = client(&t).destroy("lab", true).await.unwrap();
        assert_eq!(done[2], "volume theuth-lab-data kept ($1.00/month)");
        let calls = t.lines().join("\n");
        assert!(!calls.contains("DELETE https://api.digitalocean.com/v2/volumes"));
        assert!(!calls.contains("DELETE https://api.digitalocean.com/v2/tags"));
    }

    #[tokio::test]
    async fn sizes_come_live_from_the_api() {
        // Recorded shape of GET /v2/sizes (test data only).
        let t = FakeTransport::new();
        t.on("GET", "/sizes?per_page=200", 200, json!({"sizes": [
            {"slug": "s-1vcpu-1gb", "memory": 1024, "vcpus": 1, "price_monthly": 6.0, "price_hourly": 0.00893, "regions": ["lon1"], "available": true},
            {"slug": "s-1vcpu-2gb", "memory": 2048, "vcpus": 1, "price_monthly": 12.0, "price_hourly": 0.01786, "regions": ["lon1", "nyc3"], "available": true},
            {"slug": "s-2vcpu-4gb", "memory": 4096, "vcpus": 2, "price_monthly": 24.0, "price_hourly": 0.03571, "regions": ["nyc3"], "available": true},
            {"slug": "s-4vcpu-8gb", "memory": 8192, "vcpus": 4, "price_monthly": 48.0, "price_hourly": 0.07143, "regions": ["lon1"], "available": false},
        ], "meta": {"total": 4}}));
        let c = client(&t);
        let all = c.sizes(None).await.unwrap();
        let got: Vec<(&str, Option<f64>)> = all.iter().map(|s| (s.slug.as_str(), s.monthly_usd)).collect();
        assert_eq!(got, [("s-1vcpu-2gb", Some(12.0)), ("s-2vcpu-4gb", Some(24.0))]);
        let lon = c.sizes(Some("lon1")).await.unwrap();
        assert_eq!(lon.len(), 1);
        let q = catalog::quote(ProviderKind::Digitalocean, &lon, 2, 10).unwrap();
        assert_eq!(q.total_usd, Some(13.0));
    }

    #[tokio::test]
    async fn restart_resize_and_account() {
        let t = FakeTransport::new();
        t.on("GET", "/droplets?tag_name=", 200, json!({"droplets": [droplet("active", &[])["droplet"]]}))
            .on("POST", "/droplets/42/actions", 201, json!({"action": {"id": 7}}))
            .on("GET", "/actions/7", 200, json!({"action": {"status": "completed"}}))
            .on("GET", "/account", 200, json!({"account": {"email": "a@b.c", "status": "active"}}));
        let c = client(&t);
        c.restart("lab").await.unwrap();
        assert_eq!(c.resize("lab", 4).await.unwrap(), "s-2vcpu-4gb");
        assert_eq!(c.resize("lab", 2).await.unwrap(), "s-1vcpu-2gb");
        let types: Vec<String> = t
            .requests()
            .iter()
            .filter(|r| r.url.ends_with("/actions"))
            .map(|r| r.json.as_ref().unwrap()["type"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(types, ["reboot", "power_off", "resize", "power_on"]);
        let resize = t.requests().into_iter().find(|r| r.json.as_ref().is_some_and(|j| j["type"] == "resize")).unwrap();
        assert_eq!(resize.json.unwrap(), json!({"type": "resize", "size": "s-2vcpu-4gb", "disk": false}));
        assert_eq!(c.account().await.unwrap().email.as_deref(), Some("a@b.c"));
    }
}
