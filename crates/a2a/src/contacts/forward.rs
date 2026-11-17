//! 191 (D3, P3): `a2a_forward`. Any machine of our mesh accepts a contact's request; the
//! machine that owns its session serves it. The receiving machine runs the request in its own
//! router first: that router's strict session resolve refuses `not owner` before any task
//! exists, naming the owner when it is another of our machines ([`OWNER_DETAIL`]); a task id
//! names the machine that minted it. The request is then carried to that owner over
//! `theuth/rpc/1` as one unary call ([`agent::communication::mesh::a2a_forward`]), and the
//! owner's router answers it there, after the owner's own check of the caller ([`serve`]).
//!
//! Only a JSON-RPC `POST` of `SendMessage`, `GetTask` or `CancelTask` is carried. The card,
//! the notice and the proof path never get here (`reach::Route`); a stream, a push-config
//! call or any other method whose session is elsewhere is refused with our client's own words
//! for streams. A forward is never forwarded again (the owner serves it in its router, not
//! through [`route`]), and the receiving machine resolves at most twice: a second `not owner`
//! goes back to the contact, whose client tries its next member.

use std::sync::{Arc, Weak};

use a2a_types::error_code;
use agent::communication::ContactRefusal;
use agent::communication::mesh::{self, A2aAnswer, FORWARD_BODY_MAX, ForwardFailure};
use agent::communication::network::{NOT_OWNER_CODE, NOT_OWNER_TEXT, OWNER_DETAIL, RETURN_ROUTE};
use axum::body::Body;
use axum::response::{IntoResponse as _, Response};
use bytes::Bytes;
use http_body_util::BodyExt as _;
use hyper::StatusCode;
use hyper::header::CONTENT_TYPE;
use tower::ServiceExt as _;
use tracing::Instrument as _;

use crate::Ctx;
use crate::server::{BODY_LIMIT, Charged, ContactCaller, IrohGate};

/// The unary methods `a2a_forward` carries (D3).
const CARRIED: [&str; 3] = ["SendMessage", "GetTask", "CancelTask"];
/// What the receiving machine answers for a call it cannot carry (D3: our client's own words).
const NOT_CARRIED: &str = "streaming is not carried to contacts yet; poll GetTask";
/// D3: the owner's gate does not list the caller yet (it answered 401): a transient refusal,
/// nothing ran and nothing is stored.
pub const NOT_ACCEPTED_THERE: &str =
    "the chat's machine has not accepted this machine of yours yet; try again shortly";
/// How often the receiving machine resolves one call's owner: once, and once more after the
/// owner answered `NOT_OWNER` (the chat moved meanwhile). Never a third time.
const RESOLVES: usize = 2;
/// The version the receiving machine checked before it forwarded (`server::check_version`).
const VERSION: &str = "1.0";

/// The plugin's context for the owner side, published at each start (as `reach::Gate` holds a
/// `Weak<Ctx>`; a restarted plugin replaces it): `rpc`'s `a2a_forward` reaches the gate and the
/// router through it, never through the a2a actor's mailbox.
static OWNER_SIDE: parking_lot::Mutex<Weak<Ctx>> = parking_lot::const_mutex(Weak::new());

/// Publish `c` as the owner side of `a2a_forward` (the plugin, at start).
pub(crate) fn publish(c: &Arc<Ctx>) {
    *OWNER_SIDE.lock() = Arc::downgrade(c);
}

/// The owner side of `a2a_forward` (`rpc` serves it, mesh-internal): `caller`'s request,
/// carried here by `forwarder`, the machine of our mesh that received it (logged with it).
pub async fn serve(
    forwarder: &str,
    caller: &str,
    path: &str,
    body: String,
) -> Result<A2aAnswer, ForwardFailure> {
    let Some(c) = OWNER_SIDE.lock().upgrade() else {
        return Err(ForwardFailure::Offline(
            "the a2a plugin is not running here".into(),
        ));
    };
    let span = tracing::info_span!("a2a_forward", forwarder = %short(forwarder));
    c.forwarded(caller, path, body).instrument(span).await
}

/// `path` and `method` are a unary call `a2a_forward` carries: an export's JSON-RPC route
/// (`/agents/<export>/rpc`, the export a name of `[a-z0-9_-]`, or `~return`; never the bare
/// `/rpc`, which the receiving machine resolves first), 200 (D3): or a contact's
/// `/agents/session:<uuid>/rpc` (never `~session`), and one of [`CARRIED`].
fn carried(path: &str, method: &str) -> bool {
    let route = path
        .strip_prefix("/agents/")
        .and_then(|r| r.strip_suffix("/rpc"))
        .is_some_and(|e| {
            e == RETURN_ROUTE
                || crate::config::check_alias(e).is_ok()
                || agent::communication::network::access::session_route(e).is_some()
        });
    route && CARRIED.contains(&method)
}

/// A JSON-RPC error answer (HTTP 200, as the SDK's router answers).
fn rpc_error(id: &serde_json::Value, code: i32, message: &str) -> Response {
    let body = serde_json::json!({
        "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message }
    });
    ([(CONTENT_TYPE, "application/json")], body.to_string()).into_response()
}

/// `Some` when `body` is the JSON-RPC error `not owner`, with the owner it names, if any.
fn not_owner(body: &[u8]) -> Option<Option<String>> {
    #[derive(serde::Deserialize)]
    struct Answer {
        error: Option<Error>,
    }
    #[derive(serde::Deserialize)]
    struct Error {
        code: i32,
        #[serde(default)]
        data: Option<Vec<serde_json::Value>>,
    }
    let e = serde_json::from_slice::<Answer>(body).ok()?.error?;
    (e.code == NOT_OWNER_CODE).then(|| {
        e.data.iter().flatten().find_map(|d| {
            (d.get("@type")?.as_str()? == OWNER_DETAIL)
                .then(|| d.get("owner")?.as_str().map(str::to_owned))
                .flatten()
        })
    })
}

/// What this machine's own router made of a request.
enum Local {
    /// Served here (or refused for a reason of its own): the answer as it is.
    Answer(Response),
    /// `not owner`, naming the owner when it is another machine of our mesh.
    NotOwner(Option<String>),
}

/// One contact request (not a contact-only route) on the receiving machine: its own router's
/// answer, or the owner's when the router resolved the session to another machine of our mesh
/// or the task id names one (D3).
pub(crate) async fn route(
    c: &Arc<Ctx>,
    remote: &str,
    req: hyper::Request<hyper::body::Incoming>,
) -> Response {
    // 191 (D3): the contact is charged once for the whole call (one rate token), and the
    // permit is held across this machine's router run and every forward (`Charged`: its
    // `dispatch` charges nothing again).
    let admitted = c.shared.read().admit_contact(remote);
    let _permit = match admitted {
        Ok(p) => p,
        Err(status) => return status.into_response(),
    };
    let (mut parts, body) = req.into_parts();
    parts.extensions.insert(Charged);
    let Ok(body) = http_body_util::Limited::new(body, BODY_LIMIT)
        .collect()
        .await
        .map(|b| b.to_bytes())
    else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    // `/rpc` reaches the first export granted to the caller: resolved here, so a forward carries
    // the export this machine resolved, never the bare `/rpc`; none granted is 404, as the
    // router answers it.
    if parts.uri.path() == "/rpc" {
        let first = c.shared.read().first_export(remote);
        let Some(uri) = first.and_then(|e| format!("/agents/{e}/rpc").parse().ok()) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        parts.uri = uri;
    }
    let head: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
    let id = head.get("id").cloned().unwrap_or_default();
    let method = head
        .get("method")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default()
        .to_owned();
    let path = parts.uri.path().to_owned();
    // A task id names the machine that admitted it: read for `GetTask` and `CancelTask` only
    // (a `SendMessage` resolves its session). A prefix naming no machine of this mesh answers
    // as an offline one does, so a crafted id tells nothing of which machines are connected.
    let task = match method.as_str() {
        "GetTask" | "CancelTask" => head
            .pointer("/params/id")
            .and_then(serde_json::Value::as_str),
        _ => None,
    };
    let mut owner = match task.map(|t| task_engine(c, t)) {
        Some(TaskHome::At(o)) => Some(o),
        Some(TaskHome::Unknown) => return offline(&id, "a task id naming no machine of this mesh"),
        Some(TaskHome::Here) | None => None,
    };
    for _ in 0..RESOLVES {
        let to = match owner.take() {
            Some(o) => o,
            None => match local(c, &parts, body.clone(), remote).await {
                Local::Answer(r) => return r,
                Local::NotOwner(None) => return rpc_error(&id, NOT_OWNER_CODE, NOT_OWNER_TEXT),
                Local::NotOwner(Some(o)) => o,
            },
        };
        if parts.method != hyper::Method::POST || !carried(&path, &method) {
            return rpc_error(&id, error_code::UNSUPPORTED_OPERATION, NOT_CARRIED);
        }
        if let Err(e) = crate::server::check_version_of(&parts.headers) {
            return rpc_error(&id, e.code, &e.message);
        }
        let Some(net) = c.net.get() else {
            return offline(&id, "no mesh endpoint");
        };
        // A row naming no machine of this mesh (one removed from it): nothing to carry it to;
        // `not owner`, so the contact's client tries its next member. A machine of the mesh
        // that is not connected is offline (`mesh::a2a_forward`).
        if !net.paired_ids().iter().any(|e| e.to_string() == to) {
            tracing::info!(owner = %short(&to), "191: a2a_forward: the owner is no machine of this mesh; not owner");
            return rpc_error(&id, NOT_OWNER_CODE, NOT_OWNER_TEXT);
        }
        tracing::info!(%remote, owner = %short(&to), %method, %path, "191: a2a_forward to the owner");
        let sent = mesh::a2a_forward(
            net.as_ref(),
            &replica::PeerId(to.clone()),
            remote,
            &path,
            &String::from_utf8_lossy(&body),
        )
        .await;
        match sent {
            Ok(a) => return answered(&id, a),
            Err(ForwardFailure::NotOwner) => {
                tracing::info!(owner = %short(&to), "191: a2a_forward: not owner there; resolving again");
            }
            Err(ForwardFailure::Offline(why)) => return offline(&id, &why),
            Err(ForwardFailure::Refused(why)) => {
                return rpc_error(&id, error_code::UNSUPPORTED_OPERATION, &why);
            }
        }
    }
    tracing::info!(%remote, %method, "191: a2a_forward: not owner twice; the contact tries its next member");
    rpc_error(&id, NOT_OWNER_CODE, NOT_OWNER_TEXT)
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

/// D3 (answer 4): the owner is not connected, or it did not answer before the forward's
/// deadline, or it is not `contacts_ready`: "the chat's machine is offline". Nothing queued.
fn offline(id: &serde_json::Value, why: &str) -> Response {
    tracing::info!("191: a2a_forward: the owner is offline ({why})");
    rpc_error(
        id,
        error_code::UNSUPPORTED_OPERATION,
        &ContactRefusal::OwnerOffline.to_string(),
    )
}

/// The owner's router's answer, as it is; a 401 there (the owner does not list the caller
/// yet, or already dropped it) is the transient refusal (D3).
fn answered(id: &serde_json::Value, a: A2aAnswer) -> Response {
    if a.status == StatusCode::UNAUTHORIZED.as_u16() {
        return rpc_error(id, error_code::UNSUPPORTED_OPERATION, NOT_ACCEPTED_THERE);
    }
    let mut r = Response::new(Body::from(a.body));
    *r.status_mut() = StatusCode::from_u16(a.status).unwrap_or(StatusCode::BAD_GATEWAY);
    if r.status() == StatusCode::OK {
        r.headers_mut().insert(
            CONTENT_TYPE,
            hyper::header::HeaderValue::from_static("application/json"),
        );
    }
    r
}

/// The request in this machine's own router, as `remote`.
async fn local(
    c: &Arc<Ctx>,
    parts: &hyper::http::request::Parts,
    body: Bytes,
    remote: &str,
) -> Local {
    let mut req = hyper::Request::from_parts(parts.clone(), Body::from(body));
    req.extensions_mut()
        .insert(ContactCaller(remote.to_owned()));
    let resp = match crate::server::app(c.shared.clone()).oneshot(req).await {
        Ok(r) => r,
        Err(never) => match never {},
    };
    let json = resp
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.starts_with("application/json"));
    if resp.status() != StatusCode::OK || !json {
        return Local::Answer(resp);
    }
    let (p, b) = resp.into_parts();
    let Ok(bytes) = b.collect().await.map(|b| b.to_bytes()) else {
        return Local::Answer(StatusCode::INTERNAL_SERVER_ERROR.into_response());
    };
    match not_owner(&bytes) {
        Some(owner) => Local::NotOwner(owner),
        None => Local::Answer(Response::from_parts(p, Body::from(bytes))),
    }
}

/// Where a task id says its task is (`<first 16 hex of its engine id>-<uuid>`).
enum TaskHome {
    /// No such prefix (an id minted before 191), this machine's own, or no mesh here.
    Here,
    /// Another machine of our mesh.
    At(String),
    /// A well-formed prefix naming no machine of our mesh (one removed from it, or crafted).
    Unknown,
}

/// 191 (D3): the machine a task id names.
fn task_engine(c: &Ctx, task: &str) -> TaskHome {
    let Some((prefix, _)) = task.split_once('-') else {
        return TaskHome::Here;
    };
    if prefix.len() != 16 || !prefix.bytes().all(|b| b.is_ascii_hexdigit()) {
        return TaskHome::Here;
    }
    let Some(net) = c.net.get() else {
        return TaskHome::Here;
    };
    if net.id.0.starts_with(prefix) {
        return TaskHome::Here;
    }
    net.paired_ids()
        .into_iter()
        .map(|e| e.to_string())
        .find(|e| e.starts_with(prefix))
        .map_or(TaskHome::Unknown, TaskHome::At)
}

impl Ctx {
    /// The owner side of `a2a_forward` (D3): `caller`'s request, carried here. The owner checks
    /// the caller itself, on its own tables, as `reach::serve` does: not before
    /// `contacts_ready` (`OWNER_UNREACHABLE`: the receiving machine reads an offline owner),
    /// and only an `IrohGate::Contact` (a stranger inside an open Add contact window, a blocked
    /// or an unlisted caller: 401). Its router then applies the contact's rate and concurrency
    /// bounds (`admit_contact`'s, on the same principal) and serves the request as the
    /// contact's own stream would be. A `not owner` there is `NOT_OWNER`: a forward is never
    /// forwarded again. An answer over [`FORWARD_BODY_MAX`] fails, never cut. The whole run has
    /// the forward's own deadline ([`mesh::FORWARD_TIMEOUT`], where the receiving machine gives
    /// up too), so a blocking call never holds the caller's permit past it.
    pub(crate) async fn forwarded(
        self: &Arc<Self>,
        caller: &str,
        path: &str,
        body: String,
    ) -> Result<A2aAnswer, ForwardFailure> {
        if !self.ready.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(ForwardFailure::Offline(
                "contacts are not ready on the owner".into(),
            ));
        }
        let method = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|v| v.get("method")?.as_str().map(str::to_owned))
            .unwrap_or_default();
        if !carried(path, &method) {
            return Err(ForwardFailure::Refused(format!(
                "{NOT_CARRIED} ({method} on {path} is not carried)"
            )));
        }
        if !matches!(self.gate(caller), IrohGate::Contact { .. }) {
            tracing::info!(caller = %short(caller), "191: a2a_forward refused: not a contact here");
            return Ok(A2aAnswer {
                status: StatusCode::UNAUTHORIZED.as_u16(),
                body: String::new(),
            });
        }
        let mut req = hyper::Request::post(path)
            .header(CONTENT_TYPE, "application/json")
            .header("a2a-version", VERSION)
            .body(Body::from(body))
            .map_err(|e| ForwardFailure::Refused(e.to_string()))?;
        req.extensions_mut()
            .insert(ContactCaller(caller.to_owned()));
        let served = async {
            let resp = match crate::server::app(self.shared.clone()).oneshot(req).await {
                Ok(r) => r,
                Err(never) => match never {},
            };
            let status = resp.status();
            Ok::<_, ForwardFailure>((status, capped(resp.into_body()).await?))
        };
        let (status, bytes) = tokio::time::timeout(mesh::FORWARD_TIMEOUT, served)
            .await
            .map_err(|_| {
                tracing::info!(caller = %short(caller), %path, "191: a2a_forward: no answer within the forward's deadline");
                ForwardFailure::Offline("no answer within the forward's deadline".into())
            })??;
        if status == StatusCode::OK && not_owner(&bytes).is_some() {
            tracing::info!(caller = %short(caller), %path, "191: a2a_forward: not owner here either");
            return Err(ForwardFailure::NotOwner);
        }
        let body = String::from_utf8(bytes.to_vec())
            .map_err(|_| ForwardFailure::Refused("the answer is not text".into()))?;
        Ok(A2aAnswer {
            status: status.as_u16(),
            body,
        })
    }
}

/// The owner's answer body, up to [`FORWARD_BODY_MAX`]; a larger one fails, never cut.
async fn capped(body: Body) -> Result<Bytes, ForwardFailure> {
    http_body_util::Limited::new(body, FORWARD_BODY_MAX)
        .collect()
        .await
        .map(|b| b.to_bytes())
        .map_err(|_| ForwardFailure::Refused("the answer is larger than 2 MiB".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D3: only a unary JSON-RPC call of an export's route is carried: never the bare `/rpc`,
    /// and an export segment only of `[a-z0-9_-]` (no query, no escape, no other route).
    #[test]
    fn only_unary_calls_of_a_route_are_carried() {
        for m in CARRIED {
            assert!(carried("/agents/proj/rpc", m), "{m}");
            assert!(carried("/agents/~return/rpc", m), "{m}");
            // 200 (D3): a contact's session path, by its canonical id only.
            assert!(
                carried(
                    "/agents/session:0193f0a2-6b1c-7d3e-9f40-1a2b3c4d5e6f/rpc",
                    m
                ),
                "{m}"
            );
        }
        for m in [
            "SendStreamingMessage",
            "SubscribeToTask",
            "CreateTaskPushNotificationConfig",
            "ListTasks",
            "",
        ] {
            assert!(!carried("/agents/proj/rpc", m), "{m}");
        }
        for p in [
            super::super::offers::CARD_PATH,
            super::super::offers::NOTICE_PATH,
            super::super::rosters::ROSTER_PATH,
            "/agents/proj",
            "/agents//rpc",
            "/agents/a/b/rpc",
            "/rpc/x",
            "/rpc",
            "/agents/proj?x=1/rpc",
            "/agents/pro%6a/rpc",
            "/agents/Proj/rpc",
            "/agents/../rpc",
            "/agents/~session/rpc",
            "/agents/session:0193F0A2-6B1C-7D3E-9F40-1A2B3C4D5E6F/rpc",
            "/agents/session:nope/rpc",
        ] {
            assert!(!carried(p, "SendMessage"), "{p}");
        }
    }

    /// D3: an owner's answer over 2 MiB fails, never cut; one of exactly 2 MiB is carried whole.
    #[tokio::test]
    async fn an_answer_over_two_mib_fails_never_cut() {
        let at = vec![b'a'; FORWARD_BODY_MAX];
        assert_eq!(
            capped(Body::from(at)).await.unwrap().len(),
            FORWARD_BODY_MAX
        );
        let over = vec![b'a'; FORWARD_BODY_MAX + 1];
        assert_eq!(
            capped(Body::from(over)).await,
            Err(ForwardFailure::Refused(
                "the answer is larger than 2 MiB".into()
            ))
        );
    }

    /// The owner a `not owner` names is read from its own detail only; any other error is not
    /// `not owner`.
    #[test]
    fn not_owner_names_the_owner_only_through_its_detail() {
        let err = |data: serde_json::Value| {
            serde_json::json!({"jsonrpc": "2.0", "id": 1,
                "error": {"code": NOT_OWNER_CODE, "message": NOT_OWNER_TEXT, "data": data}})
            .to_string()
        };
        let named = err(serde_json::json!([{"@type": OWNER_DETAIL, "owner": "abc"}]));
        assert_eq!(not_owner(named.as_bytes()), Some(Some("abc".into())));
        let other = err(serde_json::json!([{"@type": "x", "owner": "abc"}]));
        assert_eq!(not_owner(other.as_bytes()), Some(None));
        let plain = serde_json::json!({"jsonrpc": "2.0", "id": 1,
            "error": {"code": NOT_OWNER_CODE, "message": NOT_OWNER_TEXT}})
        .to_string();
        assert_eq!(not_owner(plain.as_bytes()), Some(None));
        let found = serde_json::json!({"jsonrpc": "2.0", "id": 1, "error": {"code": -32001, "message": "x"}})
            .to_string();
        assert_eq!(not_owner(found.as_bytes()), None);
        assert_eq!(not_owner(br#"{"jsonrpc":"2.0","id":1,"result":{}}"#), None);
    }
}
