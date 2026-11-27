//! Fly.io (design doc 224 §6): one app per node, `theuth-<name>-<6 hex of sha256(org)>`
//! (app names are global), an encrypted volume `theuth_data` at `/data`, and a Machine running
//! the engine image with `restart: always`. The ticket reaches the machine only as a file
//! (`/run/theuth/pair/ticket`). No service, no port.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};
use zeroize::Zeroizing;

use super::catalog;
use super::http::{Api, OK, enc};
use super::{
    CloudError, CreateReq, Created, Listed, Pace, Provider, ProviderStatus, Result, Say,
    TAG_MANAGED, Transport, check_name,
};
use crate::types::{ProviderKind, Stage};

/// Fly.io Machines API.
pub const API: &str = "https://api.machines.dev/v1";
const VOLUME: &str = "theuth_data";

/// One Fly.io organization (or an org-scoped token when `org` is `None`).
pub struct Fly {
    api: Api,
    org: Option<String>,
    /// Polling of machine starts and volume detaches.
    pub pace: Pace,
}

fn guest(memory_gb: u8) -> Result<Value> {
    let (cpus, memory_mb) = catalog::guest(ProviderKind::Fly, memory_gb)?;
    Ok(json!({"cpu_kind": "shared", "cpus": cpus, "memory_mb": memory_mb}))
}

fn memory_gb(m: &Value) -> Option<u8> {
    let mb = m["config"]["guest"]["memory_mb"].as_u64()?;
    u8::try_from(mb / 1024).ok()
}

fn live(m: &&Value) -> bool {
    !matches!(m["state"].as_str(), Some("destroyed" | "destroying"))
}

fn live_volume(v: &Value) -> bool {
    !matches!(v["state"].as_str(), Some("destroyed" | "destroying"))
}

impl Fly {
    /// A client for `token` in `org` (`None` omits `org_slug` so an org-scoped token works).
    pub fn new(token: Zeroizing<String>, org: Option<String>, transport: Arc<dyn Transport>) -> Self {
        Self {
            api: Api::new("Fly.io", API, token, transport),
            org: org.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()),
            pace: Pace::default(),
        }
    }

    /// Override the polling pace (tests).
    pub fn with_pace(mut self, pace: Pace) -> Self {
        self.pace = pace;
        self
    }

    fn suffix(&self) -> String {
        let h = Sha256::digest(self.org.as_deref().unwrap_or("personal").as_bytes());
        format!("-{:02x}{:02x}{:02x}", h[0], h[1], h[2])
    }

    /// The node's app name.
    pub fn app(&self, name: &str) -> String {
        format!("theuth-{name}{}", self.suffix())
    }

    /// A list endpoint of the app; a missing app is an empty list.
    async fn items(&self, app: &str, what: &str) -> Result<Vec<Value>> {
        let (status, v) = self
            .api
            .call_any("GET", &format!("/apps/{app}/{what}"), None, &[200, 404])
            .await?;
        Ok(if status == 404 {
            Vec::new()
        } else {
            v.as_array().cloned().unwrap_or_default()
        })
    }

    async fn machine_of(&self, name: &str) -> Result<(String, Value)> {
        let app = self.app(name);
        let ms = self.items(&app, "machines").await?;
        let m = ms
            .iter()
            .find(live)
            .cloned()
            .ok_or_else(|| CloudError(format!("no Fly machine for node {name}")))?;
        Ok((app, m))
    }
}

#[async_trait]
impl Provider for Fly {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Fly
    }

    async fn create(&self, req: &CreateReq, say: Say<'_>) -> Result<Created> {
        let name = req.name.as_str();
        check_name(name)?;
        let size = catalog::slug(ProviderKind::Fly, req.memory_gb)?;
        let guest = guest(req.memory_gb)?;
        let app = self.app(name);
        let app_exists = match self
            .api
            .call_any("GET", &format!("/apps/{app}"), None, &[200, 404])
            .await?
        {
            (200, v) => {
                if let (Some(want), Some(got)) = (
                    self.org.as_deref(),
                    v["organization"]["slug"].as_str(),
                ) && got != want
                {
                    return Err(CloudError(format!(
                        "Fly.io: app {app} belongs to organization {got}, not {want}"
                    )));
                }
                true
            }
            _ => false,
        };
        if app_exists {
            if self.items(&app, "machines").await?.iter().any(|m| live(&m)) {
                return Err(CloudError(format!(
                    "app {app} has a machine; destroy it first"
                )));
            }
        } else {
            let mut body = json!({"app_name": app});
            if let Some(org) = &self.org {
                body["org_slug"] = json!(org);
            }
            self.api.call("POST", "/apps", Some(body)).await?;
        }
        say(Stage::Server, &format!("App {app}"));
        let existing = if app_exists {
            self.items(&app, "volumes")
                .await?
                .into_iter()
                .find(|v| {
                    live_volume(v) && v["name"] == VOLUME && v["region"] == req.region.as_str()
                })
        } else {
            None
        };
        let vol = match existing {
            Some(v) => v,
            None => {
                self.api
                    .call(
                        "POST",
                        &format!("/apps/{app}/volumes"),
                        Some(json!({
                            "name": VOLUME,
                            "region": req.region,
                            "size_gb": req.volume_gb,
                            "encrypted": true,
                        })),
                    )
                    .await?
            }
        };
        let vol_id = vol["id"]
            .as_str()
            .ok_or_else(|| CloudError("Fly.io: volume without an id".into()))?
            .to_owned();
        let mut config = json!({
            "image": req.image,
            "guest": guest,
            "env": {"THEUTH_NODE_NAME": name},
            "mounts": [{"volume": vol_id, "path": "/data"}],
            "restart": {"policy": "always"},
            // The image defaults to USER engine (uid 1000, needed for Kubernetes runAsNonRoot),
            // while a new Fly volume is root-owned. Start theuth-entrypoint as root so it chowns
            // /data and drops to uid 1000 via setpriv.
            "processes": [{"entrypoint": ["theuth-entrypoint"], "user": "root"}],
            "metadata": {TAG_MANAGED: "true", "theuth-node": name},
        });
        if !req.ticket.is_empty() {
            config["files"] = json!([{
                "guest_path": "/run/theuth/pair/ticket",
                "raw_value": base64::engine::general_purpose::STANDARD.encode(&req.ticket),
                "mode": 0o444,
            }]);
        }
        let m = self
            .api
            .call(
                "POST",
                &format!("/apps/{app}/machines"),
                Some(json!({"name": name, "region": req.region, "config": config})),
            )
            .await?;
        let id = m["id"]
            .as_str()
            .ok_or_else(|| CloudError("Fly.io: machine without an id".into()))?
            .to_owned();
        say(Stage::Server, &format!("Machine {id} created"));
        let mut last = String::new();
        for _ in 0..self.pace.tries {
            let m = self
                .api
                .call("GET", &format!("/apps/{app}/machines/{id}"), None)
                .await?;
            let state = m["state"].as_str().unwrap_or("?");
            if state != last {
                say(Stage::Server, &format!("Machine {state}"));
                last = state.to_owned();
            }
            match state {
                "started" => {
                    say(Stage::Storage, &format!("Volume {VOLUME} at /data"));
                    return Ok(Created {
                        server_id: Some(id),
                        volume_id: Some(vol_id),
                        extra: Some(app),
                        ip: None,
                        size_slug: size,
                    });
                }
                "failed" | "destroyed" => {
                    return Err(CloudError(format!(
                        "Fly.io: machine {id} failed to start (state: {state})"
                    )));
                }
                _ => {}
            }
            self.pace.sleep().await;
        }
        Err(CloudError(format!("machine {id} did not start")))
    }

    async fn status(&self, name: &str) -> Result<Option<ProviderStatus>> {
        let app = self.app(name);
        let ms = self.items(&app, "machines").await?;
        let vs: Vec<Value> = self
            .items(&app, "volumes")
            .await?
            .into_iter()
            .filter(live_volume)
            .collect();
        let m = ms.iter().find(live);
        if m.is_none() && vs.is_empty() {
            return Ok(None);
        }
        let v = vs.iter().find(|v| v["name"] == VOLUME);
        let mem = m.and_then(memory_gb);
        Ok(Some(ProviderStatus {
            server_state: m.and_then(|m| m["state"].as_str().map(str::to_owned)),
            ip: None,
            region: m
                .or(v)
                .and_then(|x| x["region"].as_str().map(str::to_owned)),
            size_slug: mem.and_then(|g| catalog::slug(ProviderKind::Fly, g).ok()),
            memory_gb: mem,
            volume_gb: v
                .and_then(|v| v["size_gb"].as_u64())
                .and_then(|n| u32::try_from(n).ok()),
            inbound_rules: 0,
        }))
    }

    async fn list(&self) -> Result<Vec<Listed>> {
        let path = match &self.org {
            Some(org) => format!("/apps?org_slug={}", enc(org)),
            None => "/apps".to_owned(),
        };
        let apps = self.api.call("GET", &path, None).await?;
        let suffix = self.suffix();
        let mut out = Vec::new();
        for a in apps["apps"].as_array().into_iter().flatten() {
            let Some(name) = a["name"]
                .as_str()
                .and_then(|n| n.strip_prefix("theuth-"))
                .and_then(|n| n.strip_suffix(suffix.as_str()))
            else {
                continue;
            };
            if check_name(name).is_err() {
                continue;
            }
            let Some(s) = self.status(name).await? else {
                continue;
            };
            out.push(Listed {
                name: name.to_owned(),
                volume_only: s.server_state.is_none(),
                server_state: s.server_state,
                region: s.region,
                ip: None,
                memory_gb: s.memory_gb,
                volume_gb: s.volume_gb,
            });
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(out)
    }

    async fn restart(&self, name: &str) -> Result<()> {
        let (app, m) = self.machine_of(name).await?;
        let id = m["id"].as_str().unwrap_or_default();
        self.api
            .call("POST", &format!("/apps/{app}/machines/{id}/restart"), None)
            .await?;
        Ok(())
    }

    async fn resize(&self, name: &str, memory_gb: u8) -> Result<String> {
        let size = catalog::slug(ProviderKind::Fly, memory_gb)?;
        let (app, m) = self.machine_of(name).await?;
        let id = m["id"].as_str().unwrap_or_default();
        let mut config = m["config"].clone();
        if !config.is_object() {
            return Err(CloudError(format!("Fly.io: machine {id} has no config")));
        }
        config["guest"] = guest(memory_gb)?;
        self.api
            .call(
                "POST",
                &format!("/apps/{app}/machines/{id}"),
                Some(json!({"config": config})),
            )
            .await?;
        Ok(size)
    }

    async fn destroy(&self, name: &str, keep_volume: bool) -> Result<Vec<String>> {
        check_name(name)?;
        let app = self.app(name);
        let mut done = Vec::new();
        let mut ok_del = OK.to_vec();
        ok_del.push(404);
        for m in self
            .items(&app, "machines")
            .await?
            .into_iter()
            .filter(|m| live(&m))
        {
            let id = m["id"].as_str().unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            self.api
                .call_any(
                    "DELETE",
                    &format!("/apps/{app}/machines/{id}?force=true"),
                    None,
                    &ok_del,
                )
                .await?;
            done.push(format!("machine {id} deleted"));
        }
        if keep_volume {
            done.push(format!(
                "app {app} and its volume kept (Fly bills the volume)"
            ));
            return Ok(done);
        }
        let mut vol_ok = ok_del.clone();
        vol_ok.extend_from_slice(&[409, 412]);
        for v in self
            .items(&app, "volumes")
            .await?
            .into_iter()
            .filter(live_volume)
        {
            let id = v["id"].as_str().unwrap_or_default();
            if id.is_empty() {
                continue;
            }
            let mut deleted = false;
            for _ in 0..self.pace.tries {
                let (status, _) = self
                    .api
                    .call_any("DELETE", &format!("/apps/{app}/volumes/{id}"), None, &vol_ok)
                    .await?;
                if status != 409 && status != 412 {
                    deleted = true;
                    break;
                }
                self.pace.sleep().await;
            }
            if !deleted {
                return Err(CloudError(format!(
                    "Fly.io: volume {id} is still attached; destroy again in a minute"
                )));
            }
            done.push(format!("volume {id} deleted"));
        }
        self.api
            .call_any("DELETE", &format!("/apps/{app}"), None, &ok_del)
            .await?;
        done.push(format!("app {app} deleted"));
        Ok(done)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::catalog::{ENGINE_IMAGE, MANIFEST};
    use crate::providers::digitalocean::DigitalOcean;
    use crate::providers::http::testing::FakeTransport;

    const TICKET: &str = "1.ABCDEFGHIJKLMNOP234567";

    fn client(t: &Arc<FakeTransport>) -> Fly {
        Fly::new(Zeroizing::new("fo1_tok".into()), Some("acme".into()), t.clone())
            .with_pace(Pace::NOW)
    }

    fn req() -> CreateReq {
        CreateReq {
            name: "lab".into(),
            region: "lhr".into(),
            memory_gb: 2,
            volume_gb: 10,
            ticket: TICKET.into(),
            manifest: MANIFEST.into(),
            image: ENGINE_IMAGE.into(),
            k8s: None,
            desktop: true,
        }
    }

    #[test]
    fn app_names_carry_the_org_hash() {
        let t = FakeTransport::new();
        let h = Sha256::digest(b"acme");
        let hex: String = h.iter().take(3).map(|b| format!("{b:02x}")).collect();
        assert_eq!(client(&t).app("lab"), format!("theuth-lab-{hex}"));
    }

    #[tokio::test]
    async fn auth_headers_match_flyv1_and_bearer_rules() {
        // Already prefixed FlyV1 token -> sent verbatim, never `Bearer FlyV1 ...`.
        let t = FakeTransport::new();
        t.on("GET", "/apps", 200, json!({"apps": []}));
        let c = Fly::new(
            Zeroizing::new("FlyV1 fm2_lJPECtest".into()),
            None,
            t.clone(),
        )
        .with_pace(Pace::NOW);
        c.list().await.unwrap();
        assert_eq!(t.auth_headers(), [Some("FlyV1 fm2_lJPECtest".to_owned())]);

        // Bare fm2_ macaroon -> prefixed with `FlyV1 `, never `Bearer`.
        let t = FakeTransport::new();
        t.on("GET", "/apps", 200, json!({"apps": []}));
        let c = Fly::new(Zeroizing::new("fm2_lJPECtest".into()), None, t.clone())
            .with_pace(Pace::NOW);
        c.list().await.unwrap();
        assert_eq!(t.auth_headers(), [Some("FlyV1 fm2_lJPECtest".to_owned())]);

        // fo1_ token -> stays `Bearer`.
        let t = FakeTransport::new();
        t.on("GET", "/apps", 200, json!({"apps": []}));
        client(&t).list().await.unwrap();
        assert_eq!(t.auth_headers(), [Some("Bearer fo1_tok".to_owned())]);

        // DigitalOcean token -> stays `Bearer`.
        let t = FakeTransport::new();
        t.on("GET", "/account", 200, json!({"account": {"status": "active"}}));
        DigitalOcean::new(Zeroizing::new("dop_v1_tok".into()), t.clone())
            .account()
            .await
            .unwrap();
        assert_eq!(t.auth_headers(), [Some("Bearer dop_v1_tok".to_owned())]);
    }

    #[tokio::test]
    async fn create_sends_app_volume_and_machine() {
        let t = FakeTransport::new();
        let c = client(&t);
        let app = c.app("lab");
        t.once("GET", "/machines/m1", 200, json!({"state": "starting"}))
            .on("GET", "/machines/m1", 200, json!({"state": "started"}))
            .on("GET", "/machines", 404, json!({"error": "not found"}))
            .on("GET", "/volumes", 404, json!({}))
            .on("POST", "/volumes", 200, json!({"id": "vol_1"}))
            .on("POST", "/machines", 200, json!({"id": "m1"}))
            .on("POST", "/apps", 201, json!({}));
        let stages = parking_lot::Mutex::new(Vec::new());
        let created = c
            .create(&req(), &|s, _| stages.lock().push(s))
            .await
            .unwrap();
        assert_eq!(created.extra.as_deref(), Some(app.as_str()));
        assert_eq!(created.size_slug, "shared-cpu-1x");
        assert_eq!(stages.into_inner().last(), Some(&Stage::Storage));
        let reqs = t.requests();
        assert_eq!(reqs[0].method, "GET");
        assert!(reqs[0].url.ends_with(&format!("/apps/{app}")));
        let body = |suffix: &str| {
            reqs.iter()
                .find(|r| r.method == "POST" && r.url.ends_with(suffix))
                .and_then(|r| r.json.clone())
                .unwrap()
        };
        assert_eq!(
            body("/v1/apps"),
            json!({"app_name": app, "org_slug": "acme"})
        );
        assert_eq!(
            body("/volumes"),
            json!({"name": "theuth_data", "region": "lhr", "size_gb": 10, "encrypted": true})
        );
        let m = body("/machines");
        assert_eq!(
            (m["name"].as_str(), m["region"].as_str()),
            (Some("lab"), Some("lhr"))
        );
        let cfg = &m["config"];
        assert_eq!(cfg["image"], ENGINE_IMAGE);
        assert_eq!(
            cfg["guest"],
            json!({"cpu_kind": "shared", "cpus": 1, "memory_mb": 2048})
        );
        assert_eq!(cfg["env"], json!({"THEUTH_NODE_NAME": "lab"}));
        assert_eq!(
            cfg["mounts"],
            json!([{"volume": "vol_1", "path": "/data"}])
        );
        assert_eq!(cfg["restart"], json!({"policy": "always"}));
        assert_eq!(
            cfg["processes"],
            json!([{"entrypoint": ["theuth-entrypoint"], "user": "root"}])
        );
        assert!(cfg.get("services").is_none());
        assert_eq!(
            cfg["metadata"],
            json!({TAG_MANAGED: "true", "theuth-node": "lab"})
        );
        assert_eq!(cfg["files"][0]["guest_path"], "/run/theuth/pair/ticket");
        assert_eq!(cfg["files"][0]["mode"], 0o444);
        let raw = cfg["files"][0]["raw_value"].as_str().unwrap();
        assert_eq!(
            base64::engine::general_purpose::STANDARD.decode(raw).unwrap(),
            TICKET.as_bytes()
        );
        for r in &reqs {
            assert!(!r.url.contains(TICKET));
            assert!(!r.json.as_ref().map(Value::to_string).unwrap_or_default().contains(TICKET));
        }
    }

    #[tokio::test]
    async fn create_surfaces_422_and_reuses_existing_owned_app() {
        // Real 422 on POST /apps (app does not belong to us) must fail.
        let t = FakeTransport::new();
        let c = client(&t);
        t.on(
            "POST",
            "/apps",
            422,
            json!({"error": "Name has already been taken"}),
        );
        let err = c.create(&req(), &|_, _| {}).await.unwrap_err().0;
        assert!(err.contains("422"), "{err}");
        assert!(err.contains("Name has already been taken"), "{err}");
        assert!(!t.lines().iter().any(|l| l.contains("/volumes")));

        // When GET /apps/{app} shows the app is ours (e.g. kept volume), skip POST /apps and reuse volume.
        let t = FakeTransport::new();
        let c = client(&t);
        let app = c.app("lab");
        t.on("GET", "/machines/m2", 200, json!({"state": "started"}))
            .on("GET", "/machines", 200, json!([]))
            .on(
                "GET",
                "/volumes",
                200,
                json!([{"id": "vol_kept", "name": "theuth_data", "region": "lhr", "state": "created"}]),
            )
            .on(
                "GET",
                &format!("/apps/{app}"),
                200,
                json!({"name": app, "organization": {"slug": "acme"}}),
            )
            .on("POST", "/machines", 200, json!({"id": "m2"}));
        let created = c.create(&req(), &|_, _| {}).await.unwrap();
        assert_eq!(created.volume_id.as_deref(), Some("vol_kept"));
        assert!(!t.lines().iter().any(|l| l.starts_with("POST ") && l.ends_with("/v1/apps")));
        assert!(!t.lines().iter().any(|l| l.starts_with("POST ") && l.ends_with("/volumes")));
    }

    #[tokio::test]
    async fn create_fails_fast_on_terminal_machine_states() {
        for terminal in ["failed", "destroyed"] {
            let t = FakeTransport::new();
            let c = client(&t);
            t.on("POST", "/volumes", 200, json!({"id": "vol_1"}))
                .on("POST", "/machines", 200, json!({"id": "m1"}))
                .on("POST", "/apps", 201, json!({}))
                .on("GET", "/machines/m1", 200, json!({"state": terminal}));
            let err = c.create(&req(), &|_, _| {}).await.unwrap_err().0;
            assert!(err.contains(terminal), "state={terminal}: {err}");
            let polls = t
                .lines()
                .iter()
                .filter(|l| l.starts_with("GET ") && l.ends_with("/machines/m1"))
                .count();
            assert_eq!(polls, 1, "state={terminal} should fail on first poll");
        }

        // Transient `stopped` before `starting` -> `started` must not abort create.
        let t = FakeTransport::new();
        let c = client(&t);
        t.on("POST", "/volumes", 200, json!({"id": "vol_1"}))
            .on("POST", "/machines", 200, json!({"id": "m1"}))
            .on("POST", "/apps", 201, json!({}))
            .once("GET", "/machines/m1", 200, json!({"state": "stopped"}))
            .once("GET", "/machines/m1", 200, json!({"state": "starting"}))
            .on("GET", "/machines/m1", 200, json!({"state": "started"}));
        let created = c.create(&req(), &|_, _| {}).await.unwrap();
        assert_eq!(created.server_id.as_deref(), Some("m1"));

        // Staying `stopped` exhausts the poll budget and errors.
        let t = FakeTransport::new();
        let c = client(&t);
        t.on("POST", "/volumes", 200, json!({"id": "vol_1"}))
            .on("POST", "/machines", 200, json!({"id": "m1"}))
            .on("POST", "/apps", 201, json!({}))
            .on("GET", "/machines/m1", 200, json!({"state": "stopped"}));
        let err = c.create(&req(), &|_, _| {}).await.unwrap_err().0;
        assert!(err.contains("did not start"), "{err}");
        let polls = t
            .lines()
            .iter()
            .filter(|l| l.starts_with("GET ") && l.ends_with("/machines/m1"))
            .count();
        assert_eq!(polls, Pace::NOW.tries as usize);
    }

    #[tokio::test]
    async fn list_and_create_omit_org_slug_when_org_is_none() {
        let t = FakeTransport::new();
        let c = Fly::new(
            Zeroizing::new("FlyV1 fm2_org_scoped".into()),
            None,
            t.clone(),
        )
        .with_pace(Pace::NOW);
        let app = c.app("lab");
        t.on("GET", "/apps", 200, json!({"apps": []}));
        assert!(c.list().await.unwrap().is_empty());
        assert_eq!(t.lines(), ["GET https://api.machines.dev/v1/apps"]);
        assert!(!t.lines()[0].contains("org_slug"));

        // Separate transport for create so GET /apps/{app} returns 404.
        let t = FakeTransport::new();
        let c = Fly::new(
            Zeroizing::new("FlyV1 fm2_org_scoped".into()),
            None,
            t.clone(),
        )
        .with_pace(Pace::NOW);
        t.on("POST", "/volumes", 200, json!({"id": "vol_1"}))
            .on("POST", "/machines", 200, json!({"id": "m1"}))
            .on("POST", "/apps", 201, json!({}))
            .on("GET", "/machines/m1", 200, json!({"state": "started"}));
        c.create(&req(), &|_, _| {}).await.unwrap();
        let post_app = t
            .requests()
            .into_iter()
            .find(|r| r.method == "POST" && r.url.ends_with("/v1/apps"))
            .unwrap();
        assert_eq!(post_app.json, Some(json!({"app_name": app})));
        assert!(post_app.json.unwrap().get("org_slug").is_none());
    }

    #[tokio::test]
    async fn list_status_restart_resize() {
        let t = FakeTransport::new();
        let c = client(&t);
        let app = c.app("lab");
        let old = c.app("old");
        let machine = json!({"id": "m1", "state": "started", "region": "lhr",
            "config": {"image": ENGINE_IMAGE, "guest": {"cpu_kind": "shared", "cpus": 1, "memory_mb": 2048}}});
        t.on("GET", "/apps?org_slug=acme", 200, json!({"apps": [{"name": app}, {"name": old}, {"name": "other-app"}]}))
            .on("GET", &format!("/apps/{app}/machines"), 200, json!([machine]))
            .on("GET", &format!("/apps/{app}/volumes"), 200, json!([{"id": "v1", "name": "theuth_data", "size_gb": 10, "region": "lhr"}]))
            .on("GET", &format!("/apps/{old}/machines"), 200, json!([]))
            .on("GET", &format!("/apps/{old}/volumes"), 200, json!([{"id": "v2", "name": "theuth_data", "size_gb": 20, "region": "ams"}]))
            .on("POST", "/restart", 200, json!({}))
            .on("POST", "/machines/m1", 200, json!({"id": "m1"}));
        let l = c.list().await.unwrap();
        assert_eq!(l.len(), 2);
        assert_eq!((l[0].name.as_str(), l[0].memory_gb, l[0].volume_only), ("lab", Some(2), false));
        assert_eq!((l[1].name.as_str(), l[1].volume_gb, l[1].volume_only), ("old", Some(20), true));
        c.restart("lab").await.unwrap();
        assert_eq!(c.resize("lab", 4).await.unwrap(), "shared-cpu-2x");
        let r = t.requests().into_iter().rfind(|r| r.method == "POST").unwrap();
        assert!(r.url.ends_with(&format!("/apps/{app}/machines/m1")));
        let cfg = &r.json.unwrap()["config"];
        assert_eq!(cfg["guest"], json!({"cpu_kind": "shared", "cpus": 2, "memory_mb": 4096}));
        assert_eq!(cfg["image"], ENGINE_IMAGE);
        assert!(t.lines().iter().any(|l| l.ends_with("/machines/m1/restart")));
    }

    #[tokio::test]
    async fn destroy_keeps_or_deletes_app_and_volume() {
        let setup = || {
            let t = FakeTransport::new();
            t.on(
                "GET",
                "/machines",
                200,
                json!([
                    {"id": "m1", "state": "started"},
                    {"id": "m_gone", "state": "destroyed"},
                    {"id": "m_dying", "state": "destroying"}
                ]),
            )
            .on("GET", "/volumes", 200, json!([{"id": "v1", "state": "created"}]))
            .once("DELETE", "/volumes/v1", 409, json!({"error": "volume attached"}))
            .once("DELETE", "/volumes/v1", 412, json!({"error": "precondition failed"}))
            .on("DELETE", "/", 200, Value::Null);
            t
        };
        let t = setup();
        let done = client(&t).destroy("lab", true).await.unwrap();
        assert_eq!(done.len(), 2);
        assert!(!t.lines().iter().any(|l| l.contains("/volumes/v1")));
        assert!(!t.lines().iter().any(|l| l.contains("m_gone")));
        assert!(!t.lines().iter().any(|l| l.contains("m_dying")));

        let t = setup();
        let done = client(&t).destroy("lab", false).await.unwrap();
        assert_eq!(&done[..2], ["machine m1 deleted", "volume v1 deleted"]);
        let calls = t.lines();
        assert!(calls.iter().any(|l| l.ends_with("/machines/m1?force=true")));
        assert!(!calls.iter().any(|l| l.contains("m_gone")));
        assert!(!calls.iter().any(|l| l.contains("m_dying")));
        let vol_deletes = calls.iter().filter(|l| l.contains("DELETE ") && l.ends_with("/volumes/v1")).count();
        assert_eq!(vol_deletes, 3, "retried 409 and 412 then succeeded");
        assert!(calls.last().unwrap().starts_with("DELETE https://api.machines.dev/v1/apps/theuth-lab-"));
    }
}
