//! Public web sharing (design 210 §7.4 item 8, D12, G4): a red `● LIVE N · tun.zip` status segment while any
//! share is active, fed by `share.changed@1`/`share.status@1` and seeded by `share_list`; `/share [list]` and
//! `/share stop [id|all]`. Stop is never gated: no confirmation here, no approval on the engine.
//!
//! The wire records are read through a local serde mirror of the fields shown, so the TUI does not depend on
//! the `share` crate (the dependency ratchet in `tests/dependency_limit.rs`).
use super::*;
use std::collections::BTreeMap;
use serde::Deserialize;
use rpc_compose::api::clients::ShareApiClient;

/// One share as the TUI shows it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Live {
    /// Share id.
    pub id: String,
    /// `https://{host}{path}/`.
    pub url: String,
    /// The edge's domain `D`.
    pub domain: String,
    /// `pending | live | offline | stopped`.
    pub state: String,
    /// Expiry, Unix ms.
    #[serde(default)]
    pub expires_ms: Option<u64>,
}

impl Live {
    /// Counts for the indicator: live, offline (comes back) and pending (goes live by itself), as on the desktop.
    pub fn active(&self) -> bool {
        matches!(self.state.as_str(), "live" | "offline" | "pending")
    }
}

/// Active shares of every machine of the mesh (the list is replicated), by id.
#[derive(Default)]
pub struct Share {
    shares: BTreeMap<String, Live>,
    /// Domain announced by `share.status`, when known.
    domain: Option<String>,
}

/// Typed request retained on the effect for tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    /// `share_list`; `print` answers `/share`.
    List {
        /// Print the list (a command), or only refresh the indicator.
        print: bool,
    },
    /// `share_stop {id}`; `"all"` is Stop all.
    Stop(String),
}

/// Owner-local messages.
pub enum Msg {
    /// `share.changed@1`.
    Changed(Live),
    /// `share.status@1`: the edge's domain.
    Domain(Option<String>),
    /// `share_list` answer.
    Listed {
        /// The shares, or the error text.
        shares: Result<Vec<Live>, String>,
        /// Print them.
        print: bool,
    },
    /// `share_stop` answer: stopped count and the pending edge stop.
    Stopped(Result<(u32, Option<String>), String>),
}

#[derive(Deserialize)]
struct Changed {
    share: Live,
}
#[derive(Deserialize)]
struct Status {
    #[serde(default)]
    domain: Option<String>,
}

/// What `/share <args>` asks for; `Err` is the usage line.
pub fn parse(args: &str) -> Result<Request, &'static str> {
    let mut words = args.split_whitespace();
    let request = match (words.next(), words.next()) {
        (None | Some("list"), None) => Request::List { print: true },
        (Some("stop"), None | Some("all")) => Request::Stop("all".into()),
        (Some("stop"), Some(id)) => Request::Stop(id.to_owned()),
        _ => return Err("usage: /share [list] | /share stop [id|all]"),
    };
    if words.next().is_some() {
        return Err("usage: /share [list] | /share stop [id|all]");
    }
    Ok(request)
}

/// `3h05m`, `12m`, `40s` until `at`; `expired` when past.
fn until(at: u64, now_ms: u64) -> String {
    let s = at.saturating_sub(now_ms) / 1000;
    match (s / 3600, s / 60 % 60, s % 60) {
        _ if at <= now_ms => "expired".into(),
        (0, 0, s) => format!("{s}s"),
        (0, m, _) => format!("{m}m"),
        (h, m, _) => format!("{h}h{m:02}m"),
    }
}

/// The `/share` reply: one line per active share, or `nothing is public`.
pub fn list_text(shares: &[Live], now_ms: u64) -> String {
    let active: Vec<_> = shares.iter().filter(|s| s.active()).collect();
    if active.is_empty() {
        return "public sharing: nothing is public".into();
    }
    let mut out = format!("public sharing: {} live  ·  /share stop stops all", active.len());
    for s in active {
        let expiry = s
            .expires_ms
            .map_or_else(|| "no expiry".to_owned(), |at| format!("expires in {}", until(at, now_ms)));
        let state = if s.state == "live" { String::new() } else { format!(" ({})", s.state) };
        out.push_str(&format!("\n  {}{state}  ·  {expiry}  ·  {}", s.url, s.id));
    }
    out
}

/// The `/share stop` reply.
pub fn stopped_text(stopped: u32, edge_pending: Option<&str>) -> String {
    let base = match stopped {
        0 => "public sharing: nothing was live".to_owned(),
        1 => "public sharing: stopped 1 share".to_owned(),
        n => format!("public sharing: stopped {n} shares"),
    };
    match edge_pending {
        Some(why) => format!("{base}; the edge stop is pending and will be retried ({why})"),
        None => base,
    }
}

impl Share {
    /// The status segment, `● LIVE 2 · tun.zip`; `None` while nothing is active.
    pub fn segment(&self) -> Option<String> {
        let n = self.shares.values().filter(|s| s.active()).count();
        if n == 0 {
            return None;
        }
        let domain = self
            .domain
            .as_deref()
            .or_else(|| self.shares.values().find(|s| s.active()).map(|s| s.domain.as_str()))
            .filter(|d| !d.is_empty());
        Some(match domain {
            Some(d) => format!("  ● LIVE {n} · {d}"),
            None => format!("  ● LIVE {n}"),
        })
    }

    fn put(&mut self, share: Live) {
        if share.active() {
            self.shares.insert(share.id.clone(), share);
        } else {
            self.shares.remove(&share.id);
        }
    }
}

fn send(cx: &mut Cx<'_, '_, Msg>, request: Request) {
    cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
        match request {
            Request::List { print } => Msg::Listed {
                shares: api
                    .share_list()
                    .await
                    .map(|shares| {
                        shares
                            .into_iter()
                            .map(|s| Live {
                                id: s.id,
                                url: s.url,
                                domain: s.domain,
                                state: s.state.as_str().to_owned(),
                                expires_ms: s.expires_ms,
                            })
                            .collect()
                    })
                    .map_err(crate::reason),
                print,
            },
            Request::Stop(id) => Msg::Stopped(
                api.share_stop(id)
                    .await
                    .map(|r| (r.stopped, r.edge_pending))
                    .map_err(crate::reason),
            ),
        }
    });
}

fn now_ms() -> u64 {
    u64::try_from(chrono::Utc::now().timestamp_millis()).unwrap_or(0)
}

fn say(cx: &mut Cx<'_, '_, Msg>, text: String) {
    if cx.core.reasoning {
        cx.plain(Plain::Text("\x1b[0m\n".into()));
    }
    cx.plain(Plain::Line(text.clone()));
    cx.core(CoreAction::Notice(text));
}

impl Component for Share {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration {
            id: "share",
            interests: &[
                Interest { topic: "share.changed", version: 1, variants: &["ShareChanged"] },
                Interest { topic: "share.status", version: 1, variants: &["ShareStatus"] },
            ],
            slash: &[("/share", "[list|stop [id|all]]", "public web sharing: list live shares, stop one or all (never asks)")],
            bindings: &[],
        }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        if event.topic().is_some() && event.version() != Some(1) {
            return None;
        }
        let topic = event.topic();
        match event.event()["type"].as_str()? {
            "ShareChanged" if matches!(topic, None | Some("share.changed")) => {
                Changed::deserialize(event.event()).ok().map(|c| Msg::Changed(c.share))
            }
            "ShareStatus" if matches!(topic, None | Some("share.status")) => {
                Status::deserialize(event.event()).ok().map(|s| Msg::Domain(s.domain))
            }
            _ => None,
        }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Lifecycle(Lifecycle::EngineChanged) => {
                *self = Self::default();
                send(cx, Request::List { print: false });
            }
            Input::Lifecycle(Lifecycle::Mount | Lifecycle::Resync) => send(cx, Request::List { print: false }),
            Input::Slash { args, .. } => match parse(args) {
                Ok(request) => send(cx, request),
                Err(usage) => cx.error(usage.into()),
            },
            Input::Message(Msg::Changed(share)) => self.put(share),
            Input::Message(Msg::Domain(domain)) => {
                if domain.is_some() {
                    self.domain = domain;
                }
            }
            Input::Message(Msg::Listed { shares: Ok(shares), print }) => {
                if print {
                    say(cx, list_text(&shares, now_ms()));
                }
                self.shares.clear();
                for share in shares {
                    self.put(share);
                }
            }
            // A quiet refresh on an engine without sharing (`NOT_RUNNING`, an old engine) stays quiet.
            Input::Message(Msg::Listed { shares: Err(error), print }) => {
                if print {
                    cx.error(error);
                }
            }
            Input::Message(Msg::Stopped(Ok((stopped, pending)))) => {
                say(cx, stopped_text(stopped, pending.as_deref()));
                send(cx, Request::List { print: false });
            }
            Input::Message(Msg::Stopped(Err(error))) => cx.error(error),
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, _: &ViewCx<'_>) -> Option<View> {
        if !matches!(at, Surface::Status(StatusSlot::AfterCore)) {
            return None;
        }
        use iocraft::prelude::*;
        self.segment().map(|text| View::IntrinsicElement {
            element: element!(Text(content: text, color: crate::ui::CARNELIAN)).into_any(),
            rows: 1,
            replace: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::tests::{core, topic};
    use serde_json::json;

    fn share(id: &str, state: &str) -> serde_json::Value {
        json!({"type": "ShareChanged", "share": {
            "id": id, "engine": "e", "edge": "x", "domain": "tun.zip", "host": format!("{id}.sky.tun.zip"),
            "path": "/", "url": format!("https://{id}.sky.tun.zip/"), "source": {"port": 5173}, "access": "public",
            "interstitial": true, "created_ms": 1, "live_ms": 2, "expires_ms": 3_600_000u64,
            "created_by": {"session": null}, "approved_by": {}, "state": state, "stopped_ms": null, "stopped_by": null,
        }})
    }

    /// Views the registry renders in the status slot (one red element while anything is active).
    fn segment(registry: &Registry) -> usize {
        let cx = ViewCx { core: core(), width: 80, height: 24, frame: 0, commands: None, endpoint: None, session_badge: None };
        registry.views(Surface::Status(StatusSlot::AfterCore), &cx).count()
    }

    fn registry() -> Registry {
        let mut registry = Registry::empty();
        registry.register(Share::default()).unwrap();
        registry
    }

    #[test]
    fn segment_follows_share_changed_and_status() {
        let mut reg = registry();
        assert_eq!(segment(&reg), 0);
        reg.wire(&topic("share.changed", 1, share("a", "live")), &core());
        reg.wire(&topic("share.changed", 1, share("b", "pending")), &core());
        assert_eq!(segment(&reg), 1);
        let mut s = Share::default();
        for (id, state) in [("a", "live"), ("b", "pending")] {
            let Some(Msg::Changed(live)) = s.decode(&topic("share.changed", 1, share(id, state)), &core()) else {
                panic!("decodes")
            };
            s.put(live);
        }
        assert_eq!(s.segment().as_deref(), Some("  ● LIVE 2 · tun.zip"));
        let Some(Msg::Domain(d)) = s.decode(
            &topic("share.status", 1, json!({"type": "ShareStatus", "edge": "x", "domain": "edge.example", "state": {"attached": null}, "clock_off_ms": null})),
            &core(),
        ) else { panic!("decodes") };
        s.domain = d;
        assert_eq!(s.segment().as_deref(), Some("  ● LIVE 2 · edge.example"));
        // Stopping removes; the last stop clears the segment, on the registry too.
        for id in ["a", "b"] {
            let Some(Msg::Changed(live)) = s.decode(&topic("share.changed", 1, share(id, "stopped")), &core()) else {
                panic!("decodes")
            };
            s.put(live);
            reg.wire(&topic("share.changed", 1, share(id, "stopped")), &core());
        }
        assert_eq!(s.segment(), None);
        assert_eq!(segment(&reg), 0);
        // Other versions are not this contract.
        assert!(s.decode(&topic("share.changed", 2, share("c", "live")), &core()).is_none());
    }

    #[test]
    fn slash_stop_calls_share_stop_all_at_once_without_asking() {
        let mut reg = registry();
        let out = reg.slash("/share", "stop", &core()).expect("routed");
        assert_eq!(out.effects.len(), 1);
        assert_eq!(out.effects[0].request::<Request>(), Some(&Request::Stop("all".into())));
        assert!(out.claimed.is_none() && out.actions.is_empty(), "no confirmation prompt");
        let out = reg.slash("/share", "list", &core()).expect("routed");
        assert_eq!(out.effects[0].request::<Request>(), Some(&Request::List { print: true }));
    }

    #[test]
    fn parse_accepts_list_stop_and_ids() {
        assert_eq!(parse(""), Ok(Request::List { print: true }));
        assert_eq!(parse("list"), Ok(Request::List { print: true }));
        assert_eq!(parse("stop"), Ok(Request::Stop("all".into())));
        assert_eq!(parse("stop all"), Ok(Request::Stop("all".into())));
        assert_eq!(parse("stop 0193"), Ok(Request::Stop("0193".into())));
        assert!(parse("start").is_err());
        assert!(parse("stop a b").is_err());
    }

    #[test]
    fn texts() {
        let a = Live { id: "a".into(), url: "https://a.tun.zip/".into(), domain: "tun.zip".into(), state: "live".into(), expires_ms: Some(3 * 3_600_000 + 5 * 60_000) };
        let b = Live { state: "pending".into(), id: "b".into(), expires_ms: None, ..a.clone() };
        let gone = Live { state: "stopped".into(), id: "c".into(), ..a.clone() };
        assert_eq!(list_text(&[gone.clone()], 0), "public sharing: nothing is public");
        assert_eq!(
            list_text(&[a, b, gone], 0),
            "public sharing: 2 live  ·  /share stop stops all\n  https://a.tun.zip/  ·  expires in 3h05m  ·  a\n  https://a.tun.zip/ (pending)  ·  no expiry  ·  b"
        );
        assert_eq!(stopped_text(2, None), "public sharing: stopped 2 shares");
        assert_eq!(
            stopped_text(1, Some("detached")),
            "public sharing: stopped 1 share; the edge stop is pending and will be retried (detached)"
        );
    }
}
