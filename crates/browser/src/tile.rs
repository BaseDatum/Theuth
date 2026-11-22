//! The second target of the `browser_*` tools: the user's web view tile (209 D8, §4.6, §4.7).
//!
//! [`crate::tools::Ctx`] keeps one [`Target`] per session (`browser_attach` switches it).
//! On a tile, `Ctx::raw` hands each obscura tool name and its arguments to [`call`], which maps
//! them to one tile op ([`plan`]), runs it through `artifact::webview::call` (the
//! `WebViewCallRequested` / `answer_web_view_call` round trip with the client app holding the
//! tile) and shapes the answer like obscura's MCP result (`{ content, isError }`), so the tools
//! above `raw` stay as they are. Reads and actions run the agent script ([`AGENT_JS`]) in the
//! client's isolated world `theuth-agent`; `browser_eval` runs in the page world.

use std::time::Duration;

use artifact::webview::{self, CallOutcome};
use runtime::{Runtime, SessionId};
use serde_json::{Value, json};

/// The agent script (209 §4.7): the body of an async function with `cmd` and `params` in scope.
pub const AGENT_JS: &str = include_str!("tile/agent.js");

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
    },
}

/// What `browser_tabs` answers on a tile (209 §4.7).
pub const NO_TABS: &str = "browser_tabs is not available on a tile: a tile is one page, and new \
     windows open in the system browser. web_views lists the user's tiles; browser_attach {} \
     returns to your own browser, which has tabs.";

/// How long a navigation may take in the client before it answers with the page still loading.
const NAV_MS: u64 = 25_000;
/// How long one agent-script command may run in the client.
const SCRIPT_MS: u64 = 15_000;

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
        op: &'static str,
        /// Its arguments.
        args: Value,
    },
    /// `snapshot`: the viewport as a PNG.
    Snapshot,
    /// Not available on a tile: the error the model reads.
    Unsupported(String),
}

impl Op {
    /// The op name, its arguments as the client receives them, and how long it may run once a
    /// client claimed it (the client's limit plus 5 s; `artifact` makes that at least 30 s).
    #[must_use]
    pub fn request(&self) -> Option<(&'static str, Value, Duration)> {
        let wait = |ms: u64| Duration::from_millis(ms + 5_000);
        Some(match self {
            Self::Agent {
                cmd,
                params,
                timeout_ms,
            } => (
                "script",
                json!({
                    "world": "isolated",
                    "source": AGENT_JS,
                    "arguments": { "cmd": cmd, "params": params },
                    "timeout_ms": timeout_ms,
                }),
                wait(*timeout_ms),
            ),
            Self::Page { expression } => (
                "script",
                json!({
                    "world": "page",
                    "source": format!("return ({expression}\n);"),
                    "arguments": {},
                    "timeout_ms": SCRIPT_MS,
                }),
                wait(SCRIPT_MS),
            ),
            Self::Nav { op, args } => (
                *op,
                args.clone(),
                wait(args["timeout_ms"].as_u64().unwrap_or(NAV_MS)),
            ),
            Self::Snapshot => ("snapshot", json!({}), wait(SCRIPT_MS)),
            Self::Unsupported(_) => return None,
        })
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
                op: "navigate",
                args: json!({ "url": u.as_str(), "timeout_ms": NAV_MS }),
            },
            _ => Op::Unsupported("a tile opens http(s) addresses only".to_owned()),
        },
        "browser_back" => Op::Nav {
            op: "back",
            args: json!({ "timeout_ms": NAV_MS }),
        },
        "browser_snapshot" => agent(
            "read",
            json!({ "format": "text", "max_chars": args["max_chars"] }),
        ),
        "browser_markdown" => agent(
            "read",
            json!({ "format": "markdown", "max_chars": args["max_chars"] }),
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
        other => Op::Unsupported(format!(
            "{other} is not available on a tile; browser_attach {{}} returns to your own browser"
        )),
    }
}

/// An MCP-shaped text result.
fn text(text: impl Into<String>, is_error: bool) -> Value {
    json!({ "content": [{ "type": "text", "text": text.into() }], "isError": is_error })
}

/// Not an obscura tool: `browser_attach`'s probe, the agent command `location` (`{ url, title }`).
pub(crate) const LOCATE_TOOL: &str = "tile_location";

/// Run obscura tool `name` with `args` on tile `view` for `session`. `Ok` is an MCP-shaped
/// result (an error the client or the page gave is `isError`), `Err` a tile nobody answers
/// (`no client holds tile web:<id>`, or a claimed op that never finished). The second value is
/// the page address the answer named, when it named one.
pub async fn call(
    rt: &Runtime,
    session: SessionId,
    view: &str,
    name: &str,
    args: &Value,
) -> (Result<Value, String>, Option<String>) {
    let op = plan(name, args);
    let mut out = run(rt, session, view, &op).await;
    // A read right after a click that navigated can meet the old document going away: once more.
    if matches!(
        &op,
        Op::Agent {
            cmd: "read" | "elements",
            ..
        }
    ) && matches!(&out.0, Ok(v) if v["isError"] == true)
    {
        tokio::time::sleep(Duration::from_millis(500)).await;
        out = run(rt, session, view, &op).await;
    }
    out
}

/// One op on the tile, shaped (see [`call`]).
async fn run(
    rt: &Runtime,
    session: SessionId,
    view: &str,
    op: &Op,
) -> (Result<Value, String>, Option<String>) {
    let Some((name, args, work)) = op.request() else {
        let Op::Unsupported(why) = op else {
            unreachable!("only Unsupported has no request")
        };
        return (Ok(text(why.clone(), true)), None);
    };
    match webview::call(rt, session, view, name, args, work).await {
        CallOutcome::NoClient => (Err(format!("no client holds tile web:{view}")), None),
        CallOutcome::TimedOut => (
            Err(format!(
                "tile web:{view} did not finish `{name}` within {} s",
                work.max(webview::OP_WAIT).as_secs()
            )),
            None,
        ),
        CallOutcome::Failed(reason) => (Ok(text(reason, true)), None),
        CallOutcome::Done { result, png } => (Ok(shape(op, &result, png)), url_of(&result)),
    }
}

/// The address an answer names (`url`), when it is a string.
fn url_of(result: &Value) -> Option<String> {
    result
        .get("url")
        .and_then(Value::as_str)
        .filter(|u| !u.is_empty())
        .map(str::to_owned)
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
            let out = match *cmd {
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
                "click" => format!("Clicked {}", s("clicked")),
                "fill" => format!("Filled {}", s("filled")),
                "type" => format!("Typed {} characters into {}", n("typed"), s("into")),
                "press" => {
                    let mut t = format!("Pressed {} on {}", s("pressed"), s("on"));
                    if result["submitted"] == true {
                        t.push_str("; the form was submitted");
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
            let verb = match *op {
                "navigate" => "Navigated to",
                "back" => "Went back to",
                "forward" => "Went forward to",
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
                op: "navigate",
                args: json!({ "url": "https://a.test/x", "timeout_ms": NAV_MS })
            }
        );
        assert!(matches!(
            plan("browser_back", &json!({})),
            Op::Nav { op: "back", .. }
        ));
        let read = plan("browser_snapshot", &json!({ "max_chars": 900 }));
        assert_eq!(
            agent(&read),
            ("read", &json!({ "format": "text", "max_chars": 900 }))
        );
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
        assert_eq!(op, "script");
        assert_eq!(args["world"], "isolated");
        assert_eq!(args["source"], AGENT_JS);
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
        assert_eq!(op, "script");
        assert_eq!(args["world"], "page");
        assert_eq!(args["source"], "return (document.title // x\n);");
        let (_, _, work) = plan(
            "browser_wait_for",
            &json!({ "selector": "#x", "timeout": 60 }),
        )
        .request()
        .unwrap();
        assert_eq!(work, Duration::from_secs(66));
        let (op, args, _) = plan("browser_screenshot", &json!({})).request().unwrap();
        assert_eq!((op, args), ("snapshot", json!({})));
        assert!(Op::Unsupported("x".into()).request().is_none());
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
}
