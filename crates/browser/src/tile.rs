//! The second target of the `browser_*` tools: the user's web view tile (209 D8, §4.6, §4.7).
//!
//! Each session has one [`Target`] ([`TARGETS`]; `browser_attach` switches it, the session's end
//! forgets it). On a tile, `Ctx::raw` hands each obscura tool name and its arguments to [`call`],
//! which maps them to one tile op ([`plan`]), runs it through `artifact::webview::call` (the
//! `WebViewCallRequested` / `answer_web_view_call` round trip with the client app holding the
//! tile) and shapes the answer like obscura's MCP result (`{ content, isError }`), so the tools
//! above `raw` stay as they are. Reads and actions run the agent script ([`AGENT_JS`]) in the
//! client's isolated world `theuth-agent`; `browser_eval` runs in the page world. A read after an
//! action that started a navigation waits, bounded, for the next page ([`Leaving`]).

use std::collections::HashMap;
use std::sync::LazyLock;
use std::time::Duration;

use artifact::webview::{self, CallOutcome, WebViewOp};
use parking_lot::Mutex;
use runtime::{Runtime, SessionId};
use serde_json::{Value, json};

/// The agent script (209 §4.7): the body of an async function with `cmd` and `params` in scope.
/// Its `VERSION`, which decides whether a client's world keeps the agent it already built, is
/// the hash of the source itself, so an edited script always replaces the old one.
pub static AGENT_JS: LazyLock<String> = LazyLock::new(|| {
    const SOURCE: &str = include_str!("tile/agent.js");
    const MARK: &str = "const VERSION = 1;";
    assert!(SOURCE.contains(MARK), "agent.js lost its VERSION line");
    // FNV-1a 64: a cache key, not a secret.
    let hash = SOURCE.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    });
    SOURCE.replacen(MARK, &format!("const VERSION = '{hash:016x}';"), 1)
});

/// Where a session's `browser_*` calls go (209 §4.6).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Target {
    /// This session's own obscura isolate (the default).
    #[default]
    Obscura,
    /// The user's tile `web:<view>`.
    Tile {
        /// The bare tile id.
        view: String,
        /// The address its last answer named (for the reply prefix), or empty.
        url: String,
        /// The tile was opened by another session of this session's tree (209 §9).
        delegated: bool,
        /// The last action started a navigation away from this page: the next read waits.
        leaving: Option<Leaving>,
    },
}

/// An action that started leaving a page: the agent script's per-document token and the address
/// before the action (209 §4.7). A read showing the same document at the same address is stale.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Leaving {
    /// The agent's `doc` token of the page the action ran on.
    pub doc: String,
    /// `location.href` before the action.
    pub from: String,
}

/// Every session's target; absent = [`Target::Obscura`]. Process-wide, like the profile's
/// session map: the tools write it, the `Browsers` actor forgets a session when it ends.
static TARGETS: LazyLock<Mutex<HashMap<SessionId, Target>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// `session`'s tile: id, last address, delegated (209 §4.6).
#[must_use]
pub fn tile_of(session: SessionId) -> Option<(String, String, bool)> {
    match TARGETS.lock().get(&session) {
        Some(Target::Tile {
            view,
            url,
            delegated,
            ..
        }) => Some((view.clone(), url.clone(), *delegated)),
        _ => None,
    }
}

/// Point `session` at `target` ([`Target::Obscura`] removes the entry); the previous target.
pub fn set_target(session: SessionId, target: Target) -> Option<Target> {
    let mut all = TARGETS.lock();
    match target {
        Target::Obscura => all.remove(&session),
        t => all.insert(session, t),
    }
}

/// The sessions ended (deleted, or their tree closed): their targets go.
pub fn forget(sessions: &[SessionId]) {
    let mut all = TARGETS.lock();
    for s in sessions {
        all.remove(s);
    }
}

/// Remember the address an answer from tile `view` named and whether the action is leaving the
/// page, while `session` is still on that tile.
fn seen(session: SessionId, view: &str, address: Option<String>, left: Option<Leaving>) {
    if let Some(Target::Tile {
        view: v,
        url,
        leaving,
        ..
    }) = TARGETS.lock().get_mut(&session)
        && v == view
    {
        if let Some(a) = address {
            *url = a;
        }
        if left.is_some() {
            *leaving = left;
        }
    }
}

/// Take the pending [`Leaving`] of `session` on tile `view` (a read consumes it).
fn take_leaving(session: SessionId, view: &str) -> Option<Leaving> {
    match TARGETS.lock().get_mut(&session) {
        Some(Target::Tile {
            view: v, leaving, ..
        }) if v == view => leaving.take(),
        _ => None,
    }
}

/// What `browser_tabs` answers on a tile (209 §4.7).
pub const NO_TABS: &str = "browser_tabs is not available on a tile: a tile is one page, and new \
     windows open in the system browser. web_views lists the user's tiles; browser_attach {} \
     returns to your own browser, which has tabs.";

/// How a session gets back from a tile no client answers for (209 §4.6).
const BACK_HOME: &str = "browser_attach {} returns to your own browser";

/// How long a navigation may take in the client before it answers with the page still loading.
const NAV_MS: u64 = 25_000;
/// How long one agent-script command may run in the client.
const SCRIPT_MS: u64 = 15_000;
/// The most characters one read of a tile returns (the reply crosses the mesh twice).
pub const MAX_READ_CHARS: u64 = 40_000;
/// A read after an action that started a navigation tries this often, [`RETRY`] apart, for the
/// next page (209 §4.7): 5 s at most.
const LEAVE_TRIES: u32 = 10;
/// Between two reads of a page going away.
const RETRY: Duration = Duration::from_millis(500);

/// One tile op of 209 §4.7 for one obscura tool call.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    /// The agent script's command `cmd` with `params`, in the isolated world.
    Agent {
        /// `read`, `elements`, `click`, `fill`, `type`, `press`, `select`, `scroll`, `wait`,
        /// `location`.
        cmd: &'static str,
        /// The command's parameters.
        params: Value,
        /// The client's limit for the script.
        timeout_ms: u64,
    },
    /// `browser_eval`: one expression in the page world.
    Page {
        /// The model's expression.
        expression: String,
    },
    /// `navigate`, `back`, `forward` or `reload`.
    Nav {
        /// The op.
        op: WebViewOp,
        /// Its arguments.
        args: Value,
    },
    /// `snapshot`: the viewport as a PNG.
    Snapshot,
    /// Not available on a tile: the error the model reads.
    Unsupported(String),
}

impl Op {
    /// The op, its arguments as the client receives them, and how long it may run once a client
    /// claimed it (the client's limit plus 5 s; `artifact` makes that at least 30 s).
    #[must_use]
    pub fn request(&self) -> Option<(WebViewOp, Value, Duration)> {
        let wait = |ms: u64| Duration::from_millis(ms + 5_000);
        Some(match self {
            Self::Agent {
                cmd,
                params,
                timeout_ms,
            } => (
                WebViewOp::Script,
                json!({
                    "world": "isolated",
                    "source": AGENT_JS.as_str(),
                    "arguments": { "cmd": cmd, "params": params },
                    "timeout_ms": timeout_ms,
                }),
                wait(*timeout_ms),
            ),
            Self::Page { expression } => {
                // `document.title;` is a statement on obscura; here it would end the wrapper.
                let expression =
                    expression.trim_end_matches(|c: char| c == ';' || c.is_whitespace());
                (
                    WebViewOp::Script,
                    json!({
                        "world": "page",
                        "source": format!("return ({expression}\n);"),
                        "arguments": {},
                        "timeout_ms": SCRIPT_MS,
                    }),
                    wait(SCRIPT_MS),
                )
            }
            Self::Nav { op, args } => (
                *op,
                args.clone(),
                wait(args["timeout_ms"].as_u64().unwrap_or(NAV_MS)),
            ),
            Self::Snapshot => (WebViewOp::Snapshot, json!({}), wait(SCRIPT_MS)),
            Self::Unsupported(_) => return None,
        })
    }

    /// A read of the page (what waits for a page an action is leaving).
    fn reads(&self) -> bool {
        matches!(
            self,
            Self::Agent {
                cmd: "read" | "elements",
                ..
            }
        )
    }
}

/// `{ ref?, selector? }` of an obscura call, with obscura's ref selector
/// (`[data-obscura-ref="e3"]`, what `browser_select` sends) turned back into the ref.
fn target(args: &Value) -> Value {
    let mut out = serde_json::Map::new();
    if let Some(r) = args.get("ref").and_then(Value::as_str) {
        out.insert("ref".into(), r.into());
    }
    if let Some(s) = args.get("selector").and_then(Value::as_str) {
        match s
            .strip_prefix("[data-obscura-ref=\"")
            .and_then(|r| r.strip_suffix("\"]"))
        {
            Some(r) if out.is_empty() => {
                out.insert("ref".into(), r.into());
            }
            _ => {
                out.insert("selector".into(), s.into());
            }
        }
    }
    Value::Object(out)
}

/// `target(args)` plus one more member copied from `args` under `key`.
fn with(args: &Value, key: &str) -> Value {
    let mut v = target(args);
    v[key] = args.get(key).cloned().unwrap_or(Value::Null);
    v
}

/// A read's `max_chars`, at most [`MAX_READ_CHARS`] (default 4000).
fn read_chars(args: &Value) -> u64 {
    args["max_chars"]
        .as_u64()
        .unwrap_or(4_000)
        .min(MAX_READ_CHARS)
}

/// The tile op for obscura tool `name` called with `args` (209 §4.7's table).
#[must_use]
pub fn plan(name: &str, args: &Value) -> Op {
    let agent = |cmd, params| Op::Agent {
        cmd,
        params,
        timeout_ms: SCRIPT_MS,
    };
    match name {
        "browser_navigate" => match args["url"].as_str().map(url::Url::parse) {
            Some(Ok(u)) if matches!(u.scheme(), "http" | "https") => Op::Nav {
                op: WebViewOp::Navigate,
                args: json!({ "url": u.as_str(), "timeout_ms": NAV_MS }),
            },
            _ => Op::Unsupported("a tile opens http(s) addresses only".to_owned()),
        },
        "browser_back" => Op::Nav {
            op: WebViewOp::Back,
            args: json!({ "timeout_ms": NAV_MS }),
        },
        "browser_snapshot" => agent(
            "read",
            json!({ "format": "text", "max_chars": read_chars(args) }),
        ),
        "browser_markdown" => agent(
            "read",
            json!({ "format": "markdown", "max_chars": read_chars(args) }),
        ),
        "browser_interactive_elements" => agent("elements", json!({})),
        "browser_click" => agent("click", target(args)),
        "browser_fill" => agent("fill", with(args, "value")),
        "browser_type" => agent("type", with(args, "text")),
        "browser_press_key" => agent("press", with(args, "key")),
        "browser_select_option" => agent("select", with(args, "value")),
        "browser_scroll" => {
            let mut p = target(args);
            p["direction"] = args.get("direction").cloned().unwrap_or(Value::Null);
            p["amount"] = args.get("amount").cloned().unwrap_or(Value::Null);
            agent("scroll", p)
        }
        "browser_wait_for" | "browser_wait_for_text" => {
            let secs = args["timeout"].as_u64().unwrap_or(30);
            let timeout_ms = secs * 1000;
            let key = if name == "browser_wait_for" {
                "selector"
            } else {
                "text"
            };
            Op::Agent {
                cmd: "wait",
                params: json!({ key: args[key], "timeout_ms": timeout_ms }),
                timeout_ms: timeout_ms + 1_000,
            }
        }
        "browser_evaluate" => Op::Page {
            expression: args["expression"].as_str().unwrap_or_default().to_owned(),
        },
        "browser_screenshot" => Op::Snapshot,
        LOCATE_TOOL => agent("location", json!({})),
        "browser_tab_list" | "browser_tab_new" | "browser_tab_switch" | "browser_tab_close" => {
            Op::Unsupported(NO_TABS.to_owned())
        }
        other => Op::Unsupported(format!("{other} is not available on a tile; {BACK_HOME}")),
    }
}

/// An MCP-shaped text result.
fn text(text: impl Into<String>, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": is_error })
}

/// Not an obscura tool: `browser_attach`'s probe, the agent command `location` (`{ url, title }`).
pub(crate) const LOCATE_TOOL: &str = "tile_location";

/// Tiles the user stopped the agent on, until when (209 §4.4: Stop holds for 60 s).
static STOPPED: LazyLock<Mutex<HashMap<String, tokio::time::Instant>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// What the model reads while a tile refuses it.
fn stopped_text(view: &str) -> String {
    format!(
        "the user stopped the agent on this page (tile web:{view}): do not act on it again \
         unless they ask you to; {BACK_HOME}"
    )
}

/// Whether the user stopped the agent on `view` less than [`webview::STOP_HOLD`] ago.
fn is_stopped(view: &str) -> bool {
    let mut all = STOPPED.lock();
    let now = tokio::time::Instant::now();
    all.retain(|_, until| *until > now);
    all.contains_key(view)
}

/// One answered call on a tile, shaped: `out` is an MCP-shaped result (an error the client or
/// the page gave is `isError`) or `Err` for a tile nobody answers (`no client holds tile
/// web:<id>`, or a claimed op that never finished).
#[derive(Debug)]
pub struct Reply {
    /// The result.
    pub out: Result<Value, String>,
    /// The page address the answer named.
    pub url: Option<String>,
    /// The agent script's document token.
    doc: Option<String>,
    /// The action started leaving the page.
    leaving: Option<Leaving>,
}

impl Reply {
    fn of(out: Result<Value, String>) -> Self {
        Self {
            out,
            url: None,
            doc: None,
            leaving: None,
        }
    }
}

/// Run obscura tool `name` with `args` on tile `view` for `session` (`delegated` as in the
/// request). A read after an action that left the page waits for the next one, at most
/// [`LEAVE_TRIES`] reads, and says when the page had not changed; a read that fails (the old
/// document going away) is tried again. The address and a navigation the answer reported are
/// remembered for `session`.
pub async fn call(
    rt: &Runtime,
    session: SessionId,
    view: &str,
    delegated: bool,
    name: &str,
    args: &Value,
) -> Reply {
    let op = plan(name, args);
    let leaving = if op.reads() {
        take_leaving(session, view)
    } else {
        None
    };
    let tries = if leaving.is_some() { LEAVE_TRIES } else { 1 };
    let mut n = 0;
    let mut reply = loop {
        let r = run(rt, session, view, delegated, &op).await;
        let failed = op.reads() && matches!(&r.out, Ok(v) if v["isError"] == true);
        let stale = leaving.as_ref().is_some_and(|l| same_page(&r, l));
        if (failed || stale) && n < tries {
            n += 1;
            tokio::time::sleep(RETRY).await;
            continue;
        }
        break r;
    };
    let still = leaving.as_ref().filter(|l| same_page(&reply, l));
    if let (Some(l), Ok(v)) = (still, &mut reply.out)
        && let Some(t) = v["content"][0]["text"].as_str()
    {
        v["content"][0]["text"] = format!(
            "{t}\n(still on {}: the navigation the last action started had not finished; \
             browser_read again in a moment)",
            l.from
        )
        .into();
    }
    seen(session, view, reply.url.clone(), reply.leaving.clone());
    reply
}

/// Whether `r` read the page `l` was leaving: the same document at the same address.
fn same_page(r: &Reply, l: &Leaving) -> bool {
    r.out.is_ok()
        && r.doc.as_deref() == Some(l.doc.as_str())
        && r.url.as_deref() == Some(l.from.as_str())
}

/// One op on the tile, shaped (see [`call`]).
async fn run(rt: &Runtime, session: SessionId, view: &str, delegated: bool, op: &Op) -> Reply {
    let Some((name, args, work)) = op.request() else {
        let Op::Unsupported(why) = op else {
            unreachable!("only Unsupported has no request")
        };
        return Reply::of(Ok(text(why.clone(), true)));
    };
    if is_stopped(view) {
        return Reply::of(Ok(text(stopped_text(view), true)));
    }
    match webview::call(rt, session, view, delegated, name, args, work).await {
        CallOutcome::NoClient => Reply::of(Err(format!(
            "no client holds tile web:{view} (the user may have closed it); {BACK_HOME}"
        ))),
        CallOutcome::TimedOut => Reply::of(Err(format!(
            "tile web:{view} did not finish `{}` within {} s",
            name.as_str(),
            work.max(webview::OP_WAIT).as_secs()
        ))),
        CallOutcome::Failed(reason) if reason == webview::STOPPED => {
            STOPPED.lock().insert(
                view.to_owned(),
                tokio::time::Instant::now() + webview::STOP_HOLD,
            );
            Reply::of(Ok(text(stopped_text(view), true)))
        }
        CallOutcome::Failed(reason) => Reply::of(Ok(text(reason, true))),
        CallOutcome::Done { result, png } => {
            let str_of = |k: &str| {
                result
                    .get(k)
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            };
            let doc = str_of("doc");
            let leaving = match (result["navigating"] == true, &doc, str_of("from")) {
                (true, Some(doc), Some(from)) => Some(Leaving {
                    doc: doc.clone(),
                    from,
                }),
                _ => None,
            };
            Reply {
                url: str_of("url"),
                doc,
                leaving,
                out: Ok(shape(op, &result, png)),
            }
        }
    }
}

/// A done op's result as the text obscura would have answered (209 §4.7: our format, close to
/// obscura's).
#[must_use]
pub fn shape(op: &Op, result: &Value, png: Option<String>) -> Value {
    let s = |k: &str| result.get(k).and_then(Value::as_str).unwrap_or_default();
    let n = |k: &str| result.get(k).and_then(Value::as_i64).unwrap_or_default();
    match op {
        Op::Agent { cmd, params, .. } => {
            if let Some(e) = result.get("error").and_then(Value::as_str) {
                return text(e, true);
            }
            let mut out = match *cmd {
                "read" => format!("URL: {}\nTitle: {}\n\n{}", s("url"), s("title"), s("text")),
                "elements" => {
                    let lines: Vec<&str> = result["lines"]
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_str).collect())
                        .unwrap_or_default();
                    if lines.is_empty() {
                        "No interactive elements found.".to_owned()
                    } else {
                        lines.join("\n")
                    }
                }
                "click" => {
                    let mut t = format!("Clicked {}", s("clicked"));
                    if !s("covered").is_empty() {
                        t.push_str(&format!(
                            " (at its centre the page shows {} on top; a real click would hit that)",
                            s("covered")
                        ));
                    }
                    t
                }
                "fill" => format!("Filled {}", s("filled")),
                "type" => format!("Typed {} characters into {}", n("typed"), s("into")),
                "press" => {
                    let mut t = format!("Pressed {} on {}", s("pressed"), s("on"));
                    if result["submitted"] == true {
                        t.push_str(
                            "; Enter submitted the form on this tile: do not also click its submit button",
                        );
                    }
                    t
                }
                "select" => format!("Selected \"{}\" in {}", s("selected"), s("in")),
                "scroll" => format!(
                    "Scrolled to x={}, y={} (page height {} px, viewport {} px)",
                    n("x"),
                    n("y"),
                    n("height"),
                    n("viewport")
                ),
                "wait" => {
                    let what = if s("found") == "selector" {
                        format!("selector {}", params["selector"])
                    } else {
                        format!("text {}", params["text"])
                    };
                    format!("Found {what} after {} ms", n("waited_ms"))
                }
                "location" => format!("\"{}\" ({})", s("title"), s("url")),
                _ => s("url").to_owned(),
            };
            if result["navigating"] == true {
                out.push_str("; the page is navigating");
            }
            text(out, false)
        }
        Op::Page { .. } => text(
            match result {
                Value::String(v) => v.clone(),
                Value::Null => "null".to_owned(),
                other => serde_json::to_string_pretty(other).unwrap_or_default(),
            },
            false,
        ),
        Op::Nav { op, .. } => {
            let verb = match op {
                WebViewOp::Navigate => "Navigated to",
                WebViewOp::Back => "Went back to",
                WebViewOp::Forward => "Went forward to",
                _ => "Reloaded",
            };
            let mut t = format!("{verb} {} — \"{}\"", s("url"), s("title"));
            if result["loading"] == true {
                t.push_str(" (still loading)");
            }
            text(t, false)
        }
        Op::Snapshot => match png {
            Some(data) => json!({
                "content": [
                    { "type": "image", "data": data, "mimeType": "image/png" },
                    { "type": "text", "text": "tile viewport" },
                ],
                "isError": false,
            }),
            None => text("the tile's snapshot carried no image", true),
        },
        Op::Unsupported(why) => text(why.clone(), true),
    }
}

/// `[tile web:<id> · <host>]`, the prefix of every reply on a tile (209 §4.6). `url` is the
/// address last seen; without one, what the client last reported for the tile.
#[must_use]
pub fn prefix(view: &str, url: &str) -> String {
    let known = (!url.is_empty())
        .then(|| url.to_owned())
        .or_else(|| webview::tile_url(view));
    let host = known
        .as_deref()
        .and_then(|u| url::Url::parse(u).ok())
        .and_then(|u| {
            let host = u.host_str()?.to_owned();
            Some(match u.port() {
                Some(p) => format!("{host}:{p}"),
                None => host,
            })
        })
        .unwrap_or_else(|| "page not reported".to_owned());
    format!("[tile web:{view} \u{b7} {host}]")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(op: &Op) -> (&'static str, &Value) {
        match op {
            Op::Agent { cmd, params, .. } => (cmd, params),
            other => panic!("not an agent op: {other:?}"),
        }
    }

    /// 209 §4.7's table: every obscura tool the `browser_*` tools send maps to its tile op.
    #[test]
    fn every_tool_maps_to_its_tile_op() {
        let nav = plan("browser_navigate", &json!({ "url": "https://a.test/x" }));
        assert_eq!(
            nav,
            Op::Nav {
                op: WebViewOp::Navigate,
                args: json!({ "url": "https://a.test/x", "timeout_ms": NAV_MS })
            }
        );
        assert!(matches!(
            plan("browser_back", &json!({})),
            Op::Nav {
                op: WebViewOp::Back,
                ..
            }
        ));
        let read = plan("browser_snapshot", &json!({ "max_chars": 900 }));
        assert_eq!(
            agent(&read),
            ("read", &json!({ "format": "text", "max_chars": 900 }))
        );
        // A read of the user's page is capped whatever the model asks.
        let huge = plan("browser_snapshot", &json!({ "max_chars": 10_000_000 }));
        assert_eq!(agent(&huge).1["max_chars"], MAX_READ_CHARS);
        let md = plan("browser_markdown", &json!({ "max_chars": 900 }));
        assert_eq!(agent(&md).1["format"], "markdown");
        assert_eq!(
            agent(&plan("browser_interactive_elements", &json!({}))).0,
            "elements"
        );
        assert_eq!(
            agent(&plan("browser_click", &json!({ "ref": "e3" }))),
            ("click", &json!({ "ref": "e3" }))
        );
        assert_eq!(
            agent(&plan(
                "browser_fill",
                &json!({ "selector": "#q", "value": "v" })
            )),
            ("fill", &json!({ "selector": "#q", "value": "v" }))
        );
        assert_eq!(
            agent(&plan("browser_type", &json!({ "ref": "e1", "text": "ab" }))),
            ("type", &json!({ "ref": "e1", "text": "ab" }))
        );
        assert_eq!(
            agent(&plan("browser_press_key", &json!({ "key": "Enter" }))),
            ("press", &json!({ "key": "Enter" }))
        );
        // `browser_select` sends obscura's ref selector; the tile gets the ref back.
        assert_eq!(
            agent(&plan(
                "browser_select_option",
                &json!({ "selector": "[data-obscura-ref=\"e7\"]", "value": "b" })
            )),
            ("select", &json!({ "ref": "e7", "value": "b" }))
        );
        assert_eq!(
            agent(&plan(
                "browser_scroll",
                &json!({ "direction": "bottom", "amount": null })
            )),
            ("scroll", &json!({ "direction": "bottom", "amount": null }))
        );
        let wait = plan(
            "browser_wait_for",
            &json!({ "selector": "#x", "timeout": 12 }),
        );
        assert_eq!(
            wait,
            Op::Agent {
                cmd: "wait",
                params: json!({ "selector": "#x", "timeout_ms": 12_000 }),
                timeout_ms: 13_000,
            }
        );
        let text_wait = plan(
            "browser_wait_for_text",
            &json!({ "text": "done", "timeout": 30 }),
        );
        assert_eq!(
            agent(&text_wait).1,
            &json!({ "text": "done", "timeout_ms": 30_000 })
        );
        assert_eq!(
            plan("browser_evaluate", &json!({ "expression": "1+1" })),
            Op::Page {
                expression: "1+1".into()
            }
        );
        assert_eq!(plan("browser_screenshot", &json!({})), Op::Snapshot);
        for tab in [
            "browser_tab_list",
            "browser_tab_new",
            "browser_tab_switch",
            "browser_tab_close",
        ] {
            assert_eq!(plan(tab, &json!({})), Op::Unsupported(NO_TABS.into()));
        }
        assert!(matches!(
            plan("browser_set_storage_state", &json!({})),
            Op::Unsupported(_)
        ));
    }

    /// The requests the client executes (the handoff's shapes): agent commands in the isolated
    /// world with the agent script, `browser_eval` in the page world, budgets of wait + 5 s.
    #[test]
    fn requests_carry_world_source_and_budget() {
        let (op, args, work) = plan("browser_click", &json!({ "ref": "e1" }))
            .request()
            .unwrap();
        assert_eq!(op, WebViewOp::Script);
        assert_eq!(args["world"], "isolated");
        assert_eq!(args["source"], *AGENT_JS);
        assert_eq!(
            args["arguments"],
            json!({ "cmd": "click", "params": { "ref": "e1" } })
        );
        assert_eq!(work, Duration::from_millis(SCRIPT_MS + 5_000));
        let (op, args, _) = plan(
            "browser_evaluate",
            &json!({ "expression": "document.title // x" }),
        )
        .request()
        .unwrap();
        assert_eq!(op, WebViewOp::Script);
        assert_eq!(args["world"], "page");
        assert_eq!(args["source"], "return (document.title // x\n);");
        // A trailing semicolon, fine on obscura, does not end the wrapper here.
        let (_, args, _) = plan(
            "browser_evaluate",
            &json!({ "expression": "document.title; \n" }),
        )
        .request()
        .unwrap();
        assert_eq!(args["source"], "return (document.title\n);");
        let (_, _, work) = plan(
            "browser_wait_for",
            &json!({ "selector": "#x", "timeout": 60 }),
        )
        .request()
        .unwrap();
        assert_eq!(work, Duration::from_secs(66));
        let (op, args, _) = plan("browser_screenshot", &json!({})).request().unwrap();
        assert_eq!((op, args), (WebViewOp::Snapshot, json!({})));
        assert!(Op::Unsupported("x".into()).request().is_none());
    }

    /// The script's version is its content hash, so a changed script replaces the agent a
    /// client's world already holds.
    #[test]
    fn the_agent_version_is_the_source_hash() {
        let line = AGENT_JS
            .lines()
            .find(|l| l.starts_with("const VERSION = "))
            .unwrap();
        let hex = line
            .strip_prefix("const VERSION = '")
            .and_then(|l| l.strip_suffix("';"))
            .unwrap();
        assert_eq!(hex.len(), 16);
        assert!(hex.bytes().all(|b| b.is_ascii_hexdigit()));
        assert!(!AGENT_JS.contains("const VERSION = 1;"));
    }

    #[test]
    fn answers_take_obscuras_shapes() {
        let read = plan("browser_snapshot", &json!({ "max_chars": 100 }));
        let v = shape(
            &read,
            &json!({ "url": "https://a.test/", "title": "A", "text": "ref=e1 button \"Go\" role=button\nhi" }),
            None,
        );
        assert_eq!(
            crate::tools::text_of(&v),
            (
                "URL: https://a.test/\nTitle: A\n\nref=e1 button \"Go\" role=button\nhi".to_owned(),
                false
            )
        );
        let stale = shape(
            &plan("browser_click", &json!({ "ref": "e9" })),
            &json!({ "error": "stale ref e9: that element is gone from the page; re-run browser_elements" }),
            None,
        );
        assert!(crate::tools::text_of(&stale).1);
        let left = shape(
            &plan("browser_click", &json!({ "ref": "e2" })),
            &json!({ "clicked": "a \"next\"", "navigating": true, "covered": "div \"Cookies\"" }),
            None,
        );
        assert_eq!(
            crate::tools::text_of(&left).0,
            "Clicked a \"next\" (at its centre the page shows div \"Cookies\" on top; a real click would hit that); the page is navigating"
        );
        let enter = shape(
            &plan("browser_press_key", &json!({ "key": "Enter" })),
            &json!({ "pressed": "Enter", "on": "input \"Query\"", "submitted": true }),
            None,
        );
        assert!(
            crate::tools::text_of(&enter)
                .0
                .ends_with("do not also click its submit button")
        );
        let nav = shape(
            &plan("browser_navigate", &json!({ "url": "https://a.test/" })),
            &json!({ "url": "https://a.test/", "title": "A", "loading": false }),
            None,
        );
        let (t, _) = crate::tools::text_of(&nav);
        assert_eq!(
            crate::render::parse_navigated(&t),
            Some(("https://a.test/", Some("A")))
        );
        let shot = shape(&Op::Snapshot, &json!({}), Some("iVBOR".into()));
        assert_eq!(crate::image_data(&shot), Some("iVBOR"));
        let eval = shape(
            &Op::Page {
                expression: "x".into(),
            },
            &json!({ "a": 1 }),
            None,
        );
        assert_eq!(crate::tools::text_of(&eval).0, "{\n  \"a\": 1\n}");
    }

    #[test]
    fn prefix_names_the_tile_and_host() {
        assert_eq!(
            prefix("0123456789abcdef", "https://mail.example.com/inbox"),
            "[tile web:0123456789abcdef \u{b7} mail.example.com]"
        );
        assert_eq!(
            prefix("0123456789abcdef", "http://localhost:3000/"),
            "[tile web:0123456789abcdef \u{b7} localhost:3000]"
        );
        assert_eq!(
            prefix("ffffffffffffffff", ""),
            "[tile web:ffffffffffffffff \u{b7} page not reported]"
        );
    }

    /// A session's target goes with the session, and a read consumes the page an action left.
    #[test]
    fn targets_are_forgotten_and_leaving_is_read_once() {
        let s = SessionId::new();
        set_target(
            s,
            Target::Tile {
                view: "aa".into(),
                url: String::new(),
                delegated: true,
                leaving: None,
            },
        );
        assert_eq!(tile_of(s), Some(("aa".into(), String::new(), true)));
        let l = Leaving {
            doc: "d1".into(),
            from: "https://a.test/".into(),
        };
        seen(s, "aa", Some("https://a.test/".into()), Some(l.clone()));
        assert_eq!(take_leaving(s, "bb"), None, "another tile");
        assert_eq!(take_leaving(s, "aa"), Some(l));
        assert_eq!(take_leaving(s, "aa"), None);
        forget(&[s]);
        assert_eq!(tile_of(s), None);
    }
}
