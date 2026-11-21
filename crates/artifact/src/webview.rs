//! The `show_web` tool and its [`WebViewOpened`] event (205 §4.1, §4.2): a live web page in a tile
//! beside the chat. The engine only validates the address and announces it; the desktop shell
//! hosts the page in a native child webview (205 §4.3).

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use runtime::{Runtime, SessionId};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use tools::{Risk, Tool, ToolCx, ToolOutput};
use url::{Host, Url};

/// Arguments of `show_web` (205 §4.1).
#[derive(Clone, Debug, Default, Serialize, Deserialize, JsonSchema)]
pub struct ShowWebArgs {
    /// Absolute `http://` or `https://` address. A bare `localhost:3000` or `127.0.0.1:3000/x`
    /// gets `http://` prepended. No `user:password@`.
    pub url: String,
    /// Tile and card title. Default: the address's `host[:port]`.
    pub title: Option<String>,
}

/// Event: a `show_web` call opened (or re-targeted) a web view tile (205 §4.2).
/// Wire shape `{ "type": "WebViewOpened", "session", "call_id", "id", "url", "title" }`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[schemars(transform = wire::serde_type_tag::<Self>)]
#[serde(tag = "type")]
pub struct WebViewOpened {
    /// Calling session.
    pub session: SessionId,
    /// The tool call (joins the card).
    pub call_id: String,
    /// Frame identity: first 16 hex of `sha256(session ‖ "\n" ‖ origin)` (205 D4).
    pub id: String,
    /// The normalised address.
    pub url: String,
    /// Tile and card title.
    pub title: String,
}

impl wire::WireEvent for WebViewOpened {
    const TOPIC: &'static str = "artifact.web-view-opened";
}

/// `show_web`: validate the address, publish [`WebViewOpened`], return the receipt (205 §4.1).
pub struct ShowWebTool {
    /// For publishing the event.
    pub rt: Runtime,
}

#[async_trait::async_trait]
impl Tool for ShowWebTool {
    const NAME: &'static str = "show_web";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const DIRECT: bool = true;
    type Args = ShowWebArgs;

    fn description(&self) -> String {
        "Open a live web page — a dev server on localhost, a dashboard, a docs site — in a tile \
         beside the chat that the user can click, scroll and pop out into its own window. \
         `url` is http(s) only (a bare `localhost:3000` gets `http://`); `title` names the tile \
         (default host[:port]). Showing the same origin again in this session reuses its tile \
         and navigates it. You do not see the page: to read or drive it use browser_* or \
         read_url. Not for files — `show` hands those over."
            .into()
    }

    async fn call(&self, args: ShowWebArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let url = match check(&args.url, tools::egress::lan()) {
            Ok(u) => u,
            Err(msg) => return Ok(ToolOutput::err(format!("show_web: {msg}"))),
        };
        let title = args
            .title
            .map(|t| clean_title(&t))
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| host_port(&url));
        let id = frame_id(cx.session, &url);
        let url = url.to_string();
        self.rt
            .publish(WebViewOpened {
                session: cx.session,
                call_id: cx.call_id.clone(),
                id: id.clone(),
                url: url.clone(),
                title: title.clone(),
            })
            .await;
        let content = receipt(&title, &url, &id, crate::preview::feed_live());
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "id": id,
                "url": url,
                "title": title,
                "session": cx.session,
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// The text the model reads (205 §4.1).
fn receipt(title: &str, url: &str, id: &str, live: bool) -> String {
    let mut s = format!("web view opened · {title} · {url} · tile web:{id}");
    if !live {
        s.push_str("\nnote: no app is connected; the card in the chat opens it later");
    }
    s.push_str(
        "\nThe user sees and clicks this page; you do not. Read a page with browser_* or read_url.",
    );
    s
}

/// 205 D4: first 16 hex of `sha256(session ‖ "\n" ‖ origin)`.
fn frame_id(session: SessionId, url: &Url) -> String {
    let mut h = Sha256::new();
    h.update(session.to_string().as_bytes());
    h.update(b"\n");
    h.update(url.origin().ascii_serialization().as_bytes());
    let mut id = hex::encode(h.finalize());
    id.truncate(16);
    id
}

/// `host[:port]`, the port only when it is not the scheme's default.
fn host_port(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    match url.port() {
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    }
}

/// Longest title kept, in chars (the `…` included).
const MAX_TITLE: usize = 120;
/// Longest accepted address, in bytes.
const MAX_URL: usize = 2048;

/// One line: control characters and whitespace runs become one space, trimmed, capped at
/// [`MAX_TITLE`] chars with `…` when cut. The receipt's first line must stay one line.
fn clean_title(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for c in raw.chars() {
        if c.is_whitespace() || c.is_control() {
            if !out.is_empty() && !out.ends_with(' ') {
                out.push(' ');
            }
        } else {
            out.push(c);
        }
    }
    let out = out.trim_end();
    if out.chars().count() <= MAX_TITLE {
        return out.to_owned();
    }
    let mut cut: String = out.chars().take(MAX_TITLE - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// Whether `s` starts with `scheme://` (RFC 3986 scheme characters).
fn has_scheme(s: &str) -> bool {
    let Some((scheme, _)) = s.split_once("://") else {
        return false;
    };
    let mut chars = scheme.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

/// Normalise and validate `raw` (205 §4.1, D8). `lan` is the call's local-network grant.
fn check(raw: &str, lan: bool) -> Result<Url, String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err("url is empty".into());
    }
    if raw.len() > MAX_URL {
        return Err(format!(
            "url is {} bytes; at most {MAX_URL} are allowed",
            raw.len()
        ));
    }
    // `localhost:3000` parses as scheme `localhost`; only an explicit `scheme://` prefix is kept.
    let url = if has_scheme(raw) {
        Url::parse(raw)
    } else {
        Url::parse(&format!("http://{raw}"))
    }
    .map_err(|e| format!("not a valid URL ({e}): {raw}"))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(format!("only http and https URLs (got {}:)", url.scheme()));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("credentials in the URL are not allowed".into());
    }
    let local = match url.host() {
        None => return Err("the URL has no host".into()),
        Some(Host::Domain(d)) => local_name(d),
        Some(Host::Ipv4(ip)) => classify(IpAddr::V4(ip))?,
        Some(Host::Ipv6(ip)) => classify(IpAddr::V6(ip))?,
    };
    if local && !lan {
        return Err("local network not allowed for this call".into());
    }
    Ok(url)
}

/// Suffixes of names that only resolve on a local network.
const LOCAL_SUFFIXES: &[&str] = &[
    ".localhost",
    ".local",
    ".lan",
    ".home.arpa",
    ".internal",
    ".localdomain",
    ".intranet",
    ".corp",
    ".home",
];

/// A single-label name (`nas`, `localhost`) or one under a local-only suffix.
fn local_name(d: &str) -> bool {
    let d = d.trim_end_matches('.').to_ascii_lowercase();
    !d.contains('.') || LOCAL_SUFFIXES.iter().any(|s| d.ends_with(s))
}

/// `Ok(true)` for a local-network address, `Err` for a link-local one (always refused, also
/// when embedded in an IPv4-mapped, NAT64 or 6to4 address).
fn classify(ip: IpAddr) -> Result<bool, String> {
    let embedded = match ip {
        IpAddr::V4(v4) => Some(v4),
        IpAddr::V6(v6) => embedded_v4(v6),
    };
    let link_local = match ip {
        IpAddr::V4(v4) => v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_unicast_link_local(),
    } || embedded.is_some_and(|v4| v4.is_link_local());
    if link_local {
        return Err(format!("link-local address {ip} is not allowed"));
    }
    Ok(match (ip, embedded) {
        (_, Some(v4)) => local_v4(v4),
        (IpAddr::V6(v6), None) => local_v6(v6),
        (IpAddr::V4(_), None) => unreachable!("an IPv4 address embeds itself"),
    })
}

/// The IPv4 address carried by an IPv4-mapped (`::ffff:0:0/96`), NAT64 (`64:ff9b::/96`) or
/// 6to4 (`2002::/16`) address.
fn embedded_v4(ip: Ipv6Addr) -> Option<Ipv4Addr> {
    let s = ip.segments();
    let v4 = |hi: u16, lo: u16| Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo));
    if let Some(v4) = ip.to_ipv4_mapped() {
        Some(v4)
    } else if s[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        Some(v4(s[6], s[7]))
    } else if s[0] == 0x2002 {
        Some(v4(s[1], s[2]))
    } else {
        None
    }
}

fn local_v4(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_broadcast()
        || ip.is_multicast() // 224.0.0.0/4
        || o[0] == 0 // 0.0.0.0/8
        || (o[0] == 100 && (64..128).contains(&o[1])) // CGNAT 100.64.0.0/10
        || (o[0] == 198 && (18..20).contains(&o[1])) // 198.18.0.0/15
        || (o[0], o[1], o[2]) == (192, 0, 0) // 192.0.0.0/24
}

fn local_v6(ip: Ipv6Addr) -> bool {
    ip.is_loopback() || ip.is_unspecified() || ip.is_unique_local() || ip.is_multicast()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_host_gets_http() {
        let u = check("localhost:3000", true).unwrap();
        assert_eq!(u.as_str(), "http://localhost:3000/");
        let u = check(" 127.0.0.1:3000/x ", true).unwrap();
        assert_eq!(u.as_str(), "http://127.0.0.1:3000/x");
        let u = check("https://example.com/docs?q=1", false).unwrap();
        assert_eq!(u.as_str(), "https://example.com/docs?q=1");
        // `://` later in the address is not a scheme.
        let u = check("localhost:3000/?u=http://x", true).unwrap();
        assert_eq!(u.as_str(), "http://localhost:3000/?u=http://x");
        let u = check("example.com/p?next=https://x", false).unwrap();
        assert_eq!(u.as_str(), "http://example.com/p?next=https://x");
    }

    #[test]
    fn refusals() {
        for bad in [
            "ftp://example.com/x",
            "file:///etc/passwd",
            "javascript:alert(1)",
            "https://user:pw@example.com/",
            "https://user@example.com/",
            "http://",
            "",
            "http://169.254.169.254/latest/meta-data",
            "http://[fe80::1]/",
            "http://[::ffff:169.254.169.254]/",
        ] {
            assert!(check(bad, true).is_err(), "{bad} accepted");
        }
        assert!(
            check("ftp://example.com/", true)
                .unwrap_err()
                .contains("http")
        );
        assert!(
            check("http://a:b@example.com/", true)
                .unwrap_err()
                .contains("credentials")
        );
        assert!(
            check("169.254.169.254", true)
                .unwrap_err()
                .contains("link-local")
        );
        let long = format!("https://example.com/{}", "a".repeat(MAX_URL));
        assert!(check(&long, false).unwrap_err().contains("at most 2048"));
        let edge = format!("https://example.com/{}", "a".repeat(MAX_URL - 20));
        assert_eq!(edge.len(), MAX_URL);
        assert!(check(&edge, false).is_ok());
    }

    #[test]
    fn titles_stay_one_line() {
        assert_eq!(clean_title("  Dev\nserver \r\n\t 3000 "), "Dev server 3000");
        assert_eq!(clean_title("bell\u{7}here"), "bell here");
        assert_eq!(clean_title("\u{7}\n"), "");
        let long = clean_title(&"é".repeat(500));
        assert_eq!(long.chars().count(), MAX_TITLE);
        assert!(long.ends_with('…'));
        assert_eq!(clean_title(&"x".repeat(MAX_TITLE)), "x".repeat(MAX_TITLE));
        let r = receipt(
            &clean_title("a\nb · c"),
            "http://x/",
            "0123456789abcdef",
            true,
        );
        let first = r.lines().next().unwrap();
        assert_eq!(
            first,
            "web view opened · a b · c · http://x/ · tile web:0123456789abcdef"
        );
    }

    #[test]
    fn lan_classification() {
        #[derive(Clone, Copy, Debug, PartialEq)]
        enum K {
            Public,
            Local,
            LinkLocal,
        }
        use K::*;
        let table = [
            ("https://example.com/", Public),
            ("http://8.8.8.8/", Public),
            ("http://198.20.0.1/", Public),
            ("http://192.0.1.1/", Public),
            ("http://[2606:4700::1111]/", Public),
            ("http://[64:ff9b::808:808]/", Public),
            ("http://[2002:808:808::1]/", Public),
            ("localhost:3000", Local),
            ("http://app.localhost/", Local),
            ("http://printer.local/", Local),
            ("http://nas/", Local),
            ("router:8080", Local),
            ("http://nas.lan/", Local),
            ("http://hub.home.arpa/", Local),
            ("http://svc.internal/", Local),
            ("http://box.localdomain/", Local),
            ("http://wiki.intranet/", Local),
            ("http://git.corp/", Local),
            ("http://tv.home/", Local),
            ("http://127.0.0.1:8080/", Local),
            ("http://10.0.0.2/", Local),
            ("http://172.16.0.1/", Local),
            ("http://192.168.1.1/", Local),
            ("http://100.64.1.1/", Local),
            ("http://0.0.0.0/", Local),
            ("http://0.1.2.3/", Local),
            ("http://198.18.0.1/", Local),
            ("http://198.19.255.1/", Local),
            ("http://192.0.0.8/", Local),
            ("http://224.0.0.1/", Local),
            ("http://239.1.1.1/", Local),
            ("http://255.255.255.255/", Local),
            ("http://[::1]:3000/", Local),
            ("http://[::]/", Local),
            ("http://[fd00::1]/", Local),
            ("http://[ff02::1]/", Local),
            ("http://[::ffff:10.0.0.1]/", Local),
            ("http://[64:ff9b::a00:1]/", Local),
            ("http://[2002:c0a8:101::1]/", Local),
            ("http://169.254.169.254/", LinkLocal),
            ("http://169.254.0.1/", LinkLocal),
            ("http://[fe80::1]/", LinkLocal),
            ("http://[::ffff:169.254.169.254]/", LinkLocal),
            ("http://[64:ff9b::a9fe:a9fe]/", LinkLocal),
            ("http://[2002:a9fe:a9fe::1]/", LinkLocal),
        ];
        for (url, kind) in table {
            let (with, without) = (check(url, true), check(url, false));
            match kind {
                Public => assert!(with.is_ok() && without.is_ok(), "{url}: {without:?}"),
                Local => {
                    assert!(with.is_ok(), "{url} refused with lan: {with:?}");
                    let e = without.unwrap_err();
                    assert!(e.contains("local network"), "{url} without lan: {e}");
                }
                LinkLocal => {
                    for r in [with, without] {
                        let e = r.unwrap_err();
                        assert!(e.contains("link-local"), "{url}: {e}");
                    }
                }
            }
        }
    }

    #[tokio::test]
    async fn lan_grant_comes_from_the_egress_scope() {
        let refused = tools::egress::scope(false, async {
            check("localhost:3000", tools::egress::lan())
        })
        .await;
        assert!(refused.is_err());
        let allowed = tools::egress::scope(true, async {
            check("localhost:3000", tools::egress::lan())
        })
        .await;
        assert_eq!(allowed.unwrap().as_str(), "http://localhost:3000/");
    }

    #[test]
    fn id_per_origin_and_session() {
        let a = SessionId::new();
        let b = SessionId::new();
        let u = |s: &str| check(s, true).unwrap();
        let id = frame_id(a, &u("http://localhost:3000/"));
        assert_eq!(id.len(), 16);
        assert!(
            id.bytes()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        assert_eq!(id, frame_id(a, &u("localhost:3000/other?x=1")));
        assert_ne!(id, frame_id(a, &u("http://localhost:3001/")));
        assert_ne!(id, frame_id(b, &u("http://localhost:3000/")));
        let mut h = Sha256::new();
        h.update(format!("{a}\nhttp://localhost:3000").as_bytes());
        assert_eq!(id, hex::encode(h.finalize())[..16]);
    }

    #[test]
    fn default_title_is_host_port() {
        assert_eq!(
            host_port(&check("localhost:3000", true).unwrap()),
            "localhost:3000"
        );
        assert_eq!(
            host_port(&check("https://example.com:443/", true).unwrap()),
            "example.com"
        );
        assert_eq!(
            host_port(&check("http://[::1]:8080/", true).unwrap()),
            "[::1]:8080"
        );
    }

    #[test]
    fn receipt_text() {
        let live = receipt("Docs", "http://localhost:3000/", "0123456789abcdef", true);
        assert_eq!(
            live,
            "web view opened · Docs · http://localhost:3000/ · tile web:0123456789abcdef\n\
             The user sees and clicks this page; you do not. Read a page with browser_* or read_url."
        );
        let offline = receipt("Docs", "http://localhost:3000/", "0123456789abcdef", false);
        assert_eq!(
            offline,
            "web view opened · Docs · http://localhost:3000/ · tile web:0123456789abcdef\n\
             note: no app is connected; the card in the chat opens it later\n\
             The user sees and clicks this page; you do not. Read a page with browser_* or read_url."
        );
    }

    #[test]
    fn event_wire_shape() {
        let session = SessionId::new();
        let v = serde_json::to_value(WebViewOpened {
            session,
            call_id: "c".into(),
            id: "0123456789abcdef".into(),
            url: "http://localhost:3000/".into(),
            title: "localhost:3000".into(),
        })
        .unwrap();
        assert_eq!(v["type"], "WebViewOpened");
        assert_eq!(v["session"], serde_json::json!(session));
        assert_eq!(v.as_object().unwrap().len(), 6);
    }
}
