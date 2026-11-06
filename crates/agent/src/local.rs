//! 177: local model servers — the host rule that makes a provider URL local,
//! the loopback probe (E8), and the context windows servers report (E5).

use std::time::Duration;

use serde_json::Value;

use crate::{LocalRuntime, LocalServer};

/// 177 E7: the window a local model with an unknown context is budgeted as.
pub const ASSUMED_CONTEXT: u32 = models::ASSUMED_LOCAL_CONTEXT;

/// 177 E7: `r` names a local runtime (`ollama/…`, `omlx/…`, a provider called
/// `lmstudio` …) — for an id the catalog has no entry for.
pub fn runtime_named(r: &models::Resolved) -> bool {
    r.provider
        .as_deref()
        .is_some_and(crate::endless::compactor::local_name)
        || crate::endless::compactor::local_name(crate::loop_::ns(&r.genai))
}

/// 177 E8: the ports [`probe`] tries by default.
pub const PROBE_PORTS: [u16; 4] = [11434, 1234, 8000, 8080];

/// The ports `local_servers_probe` tries: `THEUTH_LOCAL_PROBE_PORTS`
/// (comma-separated, for a scratch engine pointed at fakes), else
/// [`PROBE_PORTS`]. Always on 127.0.0.1.
pub fn probe_ports() -> Vec<u16> {
    std::env::var("THEUTH_LOCAL_PROBE_PORTS")
        .ok()
        .map(|s| {
            s.split(',')
                .filter_map(|p| p.trim().parse().ok())
                .collect::<Vec<u16>>()
        })
        .filter(|p| !p.is_empty())
        .unwrap_or_else(|| PROBE_PORTS.to_vec())
}

/// A base URL whose host is loopback, private (RFC 1918, CGNAT 100.64/10,
/// link-local, ULA), `*.local` / `*.localhost` / `*.internal` / `*.lan` /
/// `*.home.arpa` / `*.ts.net`, or a single label (137F E6's host rule).
pub fn private_url(base_url: &str) -> bool {
    let Some(host) = url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host().map(|h| h.to_owned()))
    else {
        return false;
    };
    match host {
        url::Host::Domain(d) => {
            let d = d.to_ascii_lowercase();
            !d.contains('.')
                || [
                    ".localhost",
                    ".local",
                    ".internal",
                    ".lan",
                    ".home.arpa",
                    ".ts.net",
                ]
                .iter()
                .any(|s| d.ends_with(s))
        }
        url::Host::Ipv4(ip) => private_v4(ip),
        url::Host::Ipv6(ip) => {
            ip.is_loopback()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || ip.is_unspecified()
                || ip.to_ipv4_mapped().is_some_and(private_v4)
        }
    }
}

fn private_v4(ip: std::net::Ipv4Addr) -> bool {
    let [a, b, ..] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || (a == 100 && b & 0xC0 == 64)
}

/// `localhost` and `127.0.0.1` name the same server when matching `added_as`.
fn same_url(a: &str, b: &str) -> bool {
    let norm = |s: &str| {
        s.trim_end_matches('/')
            .replace("://localhost", "://127.0.0.1")
    };
    norm(a) == norm(b)
}

/// The runtime a default port usually belongs to (an unreachable port's label).
fn port_runtime(port: u16) -> LocalRuntime {
    match port {
        11434 => LocalRuntime::Ollama,
        1234 => LocalRuntime::Lmstudio,
        8000 => LocalRuntime::Omlx,
        8080 => LocalRuntime::Llamacpp,
        _ => LocalRuntime::Openai,
    }
}

/// 177 E8: probe `ports` on 127.0.0.1 in parallel — 300 ms to connect, 1.5 s
/// for the whole probe — and say what answers. `providers` (name, base URL)
/// fill `added_as`. Loopback only; nothing is added.
pub async fn probe(ports: &[u16], providers: &[(String, String)]) -> Vec<LocalServer> {
    let client = match reqwest::Client::builder()
        .no_proxy()
        .connect_timeout(Duration::from_millis(300))
        .timeout(Duration::from_millis(1_400))
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            return ports
                .iter()
                .map(|&p| unreachable(p, format!("http client: {e}")))
                .collect();
        }
    };
    let jobs = ports.iter().map(|&port| {
        let client = client.clone();
        async move {
            tokio::time::timeout(Duration::from_millis(1_500), probe_port(&client, port))
                .await
                .unwrap_or_else(|_| unreachable(port, "timed out".into()))
        }
    });
    let mut out = futures::future::join_all(jobs).await;
    for s in &mut out {
        s.added_as = providers
            .iter()
            .find(|(_, url)| same_url(url, &s.base_url))
            .map(|(n, _)| n.clone());
    }
    out
}

fn unreachable(port: u16, error: String) -> LocalServer {
    LocalServer {
        runtime: port_runtime(port),
        base_url: format!("http://127.0.0.1:{port}/v1/"),
        reachable: false,
        models: 0,
        added_as: None,
        error: Some(error),
    }
}

/// `Ok(Some(json))` on a 2xx JSON body, `Ok(None)` when the server answered
/// otherwise, `Err` when nothing answered.
async fn get_json(client: &reqwest::Client, url: String) -> Result<Option<Value>, String> {
    let r = client.get(url).send().await.map_err(|e| {
        if e.is_connect() {
            "connection refused".to_owned()
        } else if e.is_timeout() {
            "timed out".to_owned()
        } else {
            e.to_string()
        }
    })?;
    if !r.status().is_success() {
        return Ok(None);
    }
    Ok(r.json::<Value>().await.ok())
}

async fn probe_port(client: &reqwest::Client, port: u16) -> LocalServer {
    let root = format!("http://127.0.0.1:{port}");
    let (tags, v1, v0, props) = tokio::join!(
        get_json(client, format!("{root}/api/tags")),
        get_json(client, format!("{root}/v1/models")),
        get_json(client, format!("{root}/api/v0/models")),
        get_json(client, format!("{root}/props")),
    );
    let mut server = unreachable(port, String::new());
    if let (Err(e), Err(_), Err(_), Err(_)) = (&tags, &v1, &v0, &props) {
        server.error = Some(e.clone());
        return server;
    }
    server.reachable = true;
    server.error = None;
    let ok = |r: Result<Option<Value>, String>| r.ok().flatten();
    let (tags, v1, v0, props) = (ok(tags), ok(v1), ok(v0), ok(props));
    let list = |v: &Option<Value>, key: &str| -> Option<Vec<Value>> {
        v.as_ref()?.get(key)?.as_array().cloned()
    };
    let data = list(&v1, "data");
    let (runtime, models) = if let Some(m) = list(&tags, "models") {
        (LocalRuntime::Ollama, m.len())
    } else if let (Some(m), true) = (list(&v0, "data"), data.is_some()) {
        (LocalRuntime::Lmstudio, m.len())
    } else if let Some(d) = &data {
        let any = |f: &dyn Fn(&Value) -> bool| d.iter().any(f);
        let owned = |v: &Value, s: &str| {
            v.get("owned_by")
                .and_then(Value::as_str)
                .is_some_and(|o| o.to_ascii_lowercase().contains(s))
        };
        let runtime = if props
            .as_ref()
            .is_some_and(|p| p.get("default_generation_settings").is_some())
        {
            LocalRuntime::Llamacpp
        } else if any(&|v| v.get("max_model_len").is_some() || owned(v, "vllm")) {
            LocalRuntime::Vllm
        } else if any(&|v| owned(v, "omlx")) {
            LocalRuntime::Omlx
        } else {
            LocalRuntime::Openai
        };
        (runtime, d.len())
    } else {
        server.runtime = port_runtime(port);
        server.error = Some("answered, but lists no models at /v1/models".into());
        return server;
    };
    server.runtime = runtime;
    server.models = models as u32;
    server
}

/// The context window one `/v1/models` (or LM Studio `/api/v0/models`) entry
/// reports: the loaded window first, then the served, then the model's maximum.
fn entry_context(v: &Value) -> Option<u32> {
    [
        "loaded_context_length",
        "n_ctx",
        "max_model_len",
        "context_length",
        "context_window",
        "max_context_length",
    ]
    .iter()
    .find_map(|k| v.get(*k)?.as_u64())
    .and_then(|n| u32::try_from(n).ok())
    .filter(|&n| n > 0)
}

/// `id → context` from a `{data: [{id, …}]}` listing.
fn listing_contexts(v: &Value, out: &mut std::collections::HashMap<String, u32>) {
    for e in v
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let (Some(id), Some(c)) = (e.get("id").and_then(Value::as_str), entry_context(e)) {
            out.entry(id.to_owned()).or_insert(c);
        }
    }
}

/// 177 E5: the context windows a server reports, by model id — `{base}models`
/// entry fields for any custom provider; for a `local` one also LM Studio's
/// `/api/v0/models`, llama.cpp's `/props` (one window for every id in `ids`)
/// and Ollama's `/api/show`. At most 2 s; any failure is ignored (empty).
pub async fn server_contexts(
    base_url: &str,
    key: &str,
    local: bool,
    ids: &[String],
) -> std::collections::HashMap<String, u32> {
    let mut out = std::collections::HashMap::new();
    let Ok(client) = reqwest::Client::builder()
        .connect_timeout(Duration::from_millis(500))
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return out;
    };
    let get = |url: String| {
        let mut r = client.get(url);
        if !key.is_empty() {
            r = r.bearer_auth(key);
        }
        async move {
            r.send()
                .await
                .ok()?
                .error_for_status()
                .ok()?
                .json::<Value>()
                .await
                .ok()
        }
    };
    let root = base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_owned();
    let work = async {
        let (v1, v0, props, tags) = tokio::join!(
            get(format!("{base_url}models")),
            async {
                if local {
                    get(format!("{root}/api/v0/models")).await
                } else {
                    None
                }
            },
            async {
                if local {
                    get(format!("{root}/props")).await
                } else {
                    None
                }
            },
            async {
                if local {
                    get(format!("{root}/api/tags")).await
                } else {
                    None
                }
            },
        );
        if let Some(v) = &v1 {
            listing_contexts(v, &mut out);
        }
        if let Some(v) = &v0 {
            listing_contexts(v, &mut out);
        }
        if let Some(n) = props.as_ref().and_then(|p| {
            p.pointer("/default_generation_settings/n_ctx")?
                .as_u64()
                .and_then(|n| u32::try_from(n).ok())
        }) && n > 0
        {
            for id in ids {
                out.entry(id.clone()).or_insert(n);
            }
        }
        if tags.is_some_and(|t| t.get("models").is_some()) {
            let shows = ids.iter().filter(|id| !out.contains_key(*id)).map(|id| {
                let mut r = client
                    .post(format!("{root}/api/show"))
                    .json(&serde_json::json!({"model": id}));
                if !key.is_empty() {
                    r = r.bearer_auth(key);
                }
                async move {
                    let v: Value = r
                        .send()
                        .await
                        .ok()?
                        .error_for_status()
                        .ok()?
                        .json()
                        .await
                        .ok()?;
                    let info = v.get("model_info")?;
                    let arch = info.get("general.architecture")?.as_str()?;
                    let n = info.get(format!("{arch}.context_length"))?.as_u64()?;
                    Some((id.clone(), u32::try_from(n).ok().filter(|&n| n > 0)?))
                }
            });
            out.extend(futures::future::join_all(shows).await.into_iter().flatten());
        }
    };
    // A slow server yields what arrived before the bound — nothing, here.
    if tokio::time::timeout(Duration::from_secs(2), work)
        .await
        .is_err()
    {
        return std::collections::HashMap::new();
    }
    out
}

#[cfg(test)]
mod tests;
