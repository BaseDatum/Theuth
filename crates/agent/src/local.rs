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

/// 177 R2-5: a base URL on this machine — loopback (`127.0.0.0/8`, `::1`,
/// v4-mapped loopback), unspecified (`0.0.0.0`, `::`), `localhost` or
/// `*.localhost`. The narrow rule for behaviour (thinking-off kwargs, the
/// assumed window); [`private_url`] stays the rule for the local tag and the
/// optional key.
pub fn loopback_url(base_url: &str) -> bool {
    let Some(host) = url::Url::parse(base_url)
        .ok()
        .and_then(|u| u.host().map(|h| h.to_owned()))
    else {
        return false;
    };
    match host {
        url::Host::Domain(d) => {
            let d = d.to_ascii_lowercase();
            d == "localhost" || d.ends_with(".localhost")
        }
        url::Host::Ipv4(ip) => ip.is_loopback() || ip.is_unspecified(),
        url::Host::Ipv6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip
                    .to_ipv4_mapped()
                    .is_some_and(|v4| v4.is_loopback() || v4.is_unspecified())
        }
    }
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
    // R2-1: never follow a redirect off the probed port (another host, DNS).
    let client = match reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
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

/// `Ok(Ok(json))` on a 2xx JSON body, `Ok(Err(status))` when the server
/// answered otherwise, `Err` when nothing answered.
async fn get_json(client: &reqwest::Client, url: String) -> Result<Result<Value, u16>, String> {
    let r = client.get(url).send().await.map_err(|e| {
        if e.is_connect() {
            "connection refused".to_owned()
        } else if e.is_timeout() {
            "timed out".to_owned()
        } else {
            e.to_string()
        }
    })?;
    let status = r.status();
    if !status.is_success() {
        return Ok(Err(status.as_u16()));
    }
    Ok(r.json::<Value>().await.map_err(|_| status.as_u16()))
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
    // omlx with key verification on (or any keyed server) refuses the list.
    let refused = [&v1, &v0, &tags].into_iter().find_map(|r| match r {
        Ok(Err(s @ (401 | 403))) => Some(*s),
        _ => None,
    });
    let ok = |r: Result<Result<Value, u16>, String>| r.ok().and_then(Result::ok);
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
        server.error = Some(match refused {
            Some(s) => format!("answered {s}: needs an API key"),
            None => "answered, but lists no models at /v1/models".into(),
        });
        return server;
    };
    server.runtime = runtime;
    server.models = models as u32;
    server
}

/// The SERVED context window one `/v1/models` entry reports (R2-2: never a
/// model's maximum such as LM Studio's `max_context_length`).
fn entry_context(v: &Value, fields: &[&str]) -> Option<u32> {
    fields
        .iter()
        .find_map(|k| v.get(*k)?.as_u64())
        .and_then(|n| u32::try_from(n).ok())
        .filter(|&n| n > 0)
}

/// Served-window fields of an OpenAI-style `/v1/models` entry (vLLM
/// `max_model_len`, llama.cpp `n_ctx`, a loaded window, gateways' `context_length`).
const V1_FIELDS: &[&str] = &[
    "loaded_context_length",
    "n_ctx",
    "max_model_len",
    "context_length",
    "context_window",
];

/// `(id, context)` pairs of a `{data: [{id, …}]}` listing.
fn listing_contexts(v: &Value, fields: &[&str]) -> Vec<(String, u32)> {
    v.get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|e| Some((e.get("id")?.as_str()?.to_owned(), entry_context(e, fields)?)))
        .collect()
}

/// Ollama's served `num_ctx` from `/api/show`'s `parameters` text
/// (`"num_ctx                        8192\nstop …"`), if the model sets one.
fn ollama_num_ctx(show: &Value) -> Option<u32> {
    show.get("parameters")?
        .as_str()?
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            if it.next()? != "num_ctx" {
                return None;
            }
            it.next()?.parse::<u32>().ok()
        })
        .filter(|&n| n > 0)
}

/// 177 E5: the context windows a server SERVES, by model id (R2-2: never a
/// model's maximum) — `{base}models` entry fields for any custom provider;
/// for a `local` one also LM Studio's `/api/v0/models` `loaded_context_length`
/// (a loaded model only), llama.cpp's `/props` `n_ctx` (one window for every
/// id in `ids`), and Ollama's `num_ctx` (`/api/show` `parameters`, else
/// `/api/ps` `context_length` of a loaded model, else unknown). At most 2 s;
/// what arrived before the bound is kept; a failure is ignored. Redirects are
/// not followed.
pub async fn server_contexts(
    base_url: &str,
    key: &str,
    local: bool,
    ids: &[String],
) -> std::collections::HashMap<String, u32> {
    let Ok(client) = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_millis(500))
        .timeout(Duration::from_secs(2))
        .build()
    else {
        return std::collections::HashMap::new();
    };
    let send = |r: reqwest::RequestBuilder| {
        let r = if key.is_empty() {
            r
        } else {
            r.bearer_auth(key)
        };
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
    let get = |url: String| send(client.get(url));
    let root = base_url
        .trim_end_matches('/')
        .trim_end_matches("/v1")
        .to_owned();
    let local_get = |path: &str| {
        let fut = get(format!("{root}{path}"));
        async move { if local { fut.await } else { None } }
    };
    // Each source writes as it lands (R2-8: what arrived before the bound is
    // kept); the lower rank wins whatever the arrival order.
    let found = parking_lot::Mutex::new(std::collections::HashMap::<String, (u8, u32)>::new());
    let put = |rank: u8, id: String, n: u32| {
        let mut found = found.lock();
        let slot = found.entry(id).or_insert((rank, n));
        if rank < slot.0 {
            *slot = (rank, n);
        }
    };
    let work = async {
        tokio::join!(
            async {
                if let Some(v) = get(format!("{base_url}models")).await {
                    for (id, n) in listing_contexts(&v, V1_FIELDS) {
                        put(0, id, n);
                    }
                }
            },
            async {
                if let Some(v) = local_get("/api/v0/models").await {
                    for (id, n) in listing_contexts(&v, &["loaded_context_length"]) {
                        put(1, id, n);
                    }
                }
            },
            async {
                if let Some(n) = local_get("/props").await.and_then(|p| {
                    p.pointer("/default_generation_settings/n_ctx")?
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                }) && n > 0
                {
                    for id in ids {
                        put(2, id.clone(), n);
                    }
                }
            },
            async {
                let (tags, ps) = tokio::join!(local_get("/api/tags"), local_get("/api/ps"));
                if !tags.is_some_and(|t| t.get("models").is_some()) {
                    return;
                }
                // A loaded model's served window, by name.
                let loaded: std::collections::HashMap<String, u32> = ps
                    .as_ref()
                    .and_then(|p| p.get("models")?.as_array().cloned())
                    .into_iter()
                    .flatten()
                    .filter_map(|m| {
                        let name = m.get("name").or_else(|| m.get("model"))?.as_str()?;
                        let n = u32::try_from(m.get("context_length")?.as_u64()?).ok()?;
                        (n > 0).then(|| (name.to_owned(), n))
                    })
                    .collect();
                for id in ids {
                    if found.lock().contains_key(id) {
                        continue;
                    }
                    let show = send(
                        client
                            .post(format!("{root}/api/show"))
                            .json(&serde_json::json!({"model": id})),
                    )
                    .await;
                    if let Some(n) = show
                        .as_ref()
                        .and_then(ollama_num_ctx)
                        .or_else(|| loaded.get(id).copied())
                    {
                        put(3, id.clone(), n);
                    }
                }
            },
        );
    };
    let _ = tokio::time::timeout(Duration::from_secs(2), work).await;
    found
        .into_inner()
        .into_iter()
        .map(|(id, (_, n))| (id, n))
        .collect()
}

#[cfg(test)]
mod tests;
