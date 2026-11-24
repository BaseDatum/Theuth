//! The pages the hosting engine itself shows a visitor (210 §7.5): interstitial, password prompt
//! and the error answers of a navigation. One self-contained template: the app's palette
//! (`crates/desktop/ui/src/styles.css`, copied as literal values), light or dark from
//! `prefers-color-scheme`, all CSS inline in `<head>`, no script, no external request.

use http::header::{self, HeaderMap, HeaderValue};
use http::{Response, StatusCode};

use crate::proxy::{Body, full, text};

/// Light tokens on `:root`, dark ones under `prefers-color-scheme: dark`; `html` carries the page
/// background in both so no frame paints the UA's white. Text pairs meet WCAG AA in both schemes
/// (accent-filled button ≥4.5:1, links ≥4.5:1, field border ≥3:1).
const CSS: &str = "\
:root{color-scheme:light dark;\
--bg-page:#e8dcc8;--bg-layer-1:#f3eadf;--fg-primary:#1a1612;--fg-secondary:#6b5e4f;\
--fg-tertiary:#8f7f6a;--accent:#b4531e;--accent-hover:#8a3e16;--on-accent:#fbf6ef;--link:#8a3e16;\
--border-default:#d4c4a8;--border-strong:#c9b48a;--warn:#b4531e;--warn-bg:#b4531e1f;\
--danger:#9b2d1f;--danger-bg:#9b2d1f1f;\
--shadow:0 1px 2px #1a161210,0 18px 40px -18px #1a161240;\
--font-sans:-apple-system-body,ui-sans-serif,-apple-system,system-ui,\"Segoe UI\",Helvetica,Arial,sans-serif;\
--font-mono:ui-monospace,SFMono-Regular,Menlo,Monaco,Consolas,\"Liberation Mono\",\"Courier New\",monospace}\
html{background:#e8dcc8}\
@media (prefers-color-scheme:dark){\
:root{--bg-page:#12100e;--bg-layer-1:#1c1915;--fg-primary:#e8dcc8;--fg-secondary:#c9b89a;\
--fg-tertiary:#9a8b72;--accent:#c47a2c;--accent-hover:#d48c40;--on-accent:#12100e;--link:#c47a2c;\
--border-default:#2a241c;--border-strong:#3a3228;--warn:#c47a2c;--warn-bg:#c47a2c1f;\
--danger:#c45a48;--danger-bg:#c45a4826;\
--shadow:0 1px 2px #00000040,0 18px 40px -18px #000000b0}\
html{background:#12100e}}\
*,*::before,*::after{box-sizing:border-box}\
html{-webkit-text-size-adjust:100%;text-size-adjust:100%}\
body{margin:0;min-height:100vh;min-height:100dvh;display:flex;align-items:center;justify-content:center;\
padding:24px 16px;background:var(--bg-page);color:var(--fg-primary);font-family:var(--font-sans);\
font-size:15px;line-height:1.55;-webkit-font-smoothing:antialiased}\
main{width:100%;max-width:440px;background:var(--bg-layer-1);border:1px solid var(--border-default);\
border-radius:16px;box-shadow:var(--shadow);padding:clamp(24px,7vw,36px)}\
.brand{display:flex;align-items:center;gap:8px;margin:0 0 28px;color:var(--fg-primary);font-weight:600;\
font-size:15px;letter-spacing:-.01em}\
.brand svg{width:22px;height:22px;color:var(--accent);flex:none}\
.eyebrow{margin:0 0 6px;color:var(--fg-secondary);font-size:13px;font-weight:500;letter-spacing:.02em}\
h1{margin:0 0 16px;font-family:var(--font-mono);font-size:19px;font-weight:600;line-height:1.35;\
letter-spacing:-.01em;overflow-wrap:anywhere}\
p{margin:0 0 14px;color:var(--fg-secondary)}\
h1.title{font-family:var(--font-sans);font-size:21px;letter-spacing:-.015em;overflow-wrap:normal}\
.eyebrow.host{font-family:var(--font-mono);overflow-wrap:anywhere}\
.note{margin:0 0 24px;padding:10px 12px;border-left:3px solid var(--warn);border-radius:6px;\
background:var(--warn-bg);color:var(--fg-primary)}\
.note.error{border-left-color:var(--danger);background:var(--danger-bg)}\
.detail{margin:0 0 20px;font-family:var(--font-mono);font-size:12.5px;color:var(--fg-secondary);\
overflow-wrap:anywhere}\
form{margin:0}\
label{display:block;margin:0 0 6px;font-size:13px;font-weight:500;color:var(--fg-secondary)}\
input{display:block;width:100%;margin:0 0 14px;padding:11px 13px;font:inherit;font-size:16px;\
color:var(--fg-primary);background:var(--bg-page);border:1px solid var(--fg-tertiary);border-radius:10px}\
input:focus-visible{outline:2px solid var(--accent);outline-offset:1px;border-color:var(--accent)}\
button{display:block;width:100%;margin:0;padding:12px 16px;font:inherit;font-weight:600;\
line-height:1.35;color:var(--on-accent);background:var(--accent);border:0;border-radius:10px;\
cursor:pointer;overflow-wrap:anywhere;transition:background-color .12s ease}\
button:hover{background:var(--accent-hover)}\
button:focus-visible{outline:2px solid var(--accent);outline-offset:3px}\
footer{margin-top:20px;text-align:center;font-size:13px}\
a{color:var(--link);text-underline-offset:3px;text-decoration-thickness:1px}\
footer a{color:var(--fg-secondary)}\
a:hover{color:var(--link);text-decoration-thickness:2px}\
a:focus-visible{outline:2px solid var(--accent);outline-offset:2px;border-radius:3px}\
@media (prefers-reduced-motion:reduce){button{transition:none}}";

/// The theuth mark (T0 "Theta moon", `crates/desktop/ui/src/components/Mark.tsx`): ring in `currentColor`, pen bar in ochre.
const MARK: &str = "<svg viewBox=\"14 14 92 92\" aria-hidden=\"true\" focusable=\"false\">\
<path fill=\"currentColor\" fill-rule=\"evenodd\" d=\"M18 60 a42 42 0 1 0 84 0 a42 42 0 1 0 -84 0 Z M27 60 a37 37 0 1 0 74 0 a37 37 0 1 0 -74 0 Z\"/>\
<path fill=\"#b4531e\" d=\"M24 56.5 H86 L101 60 L86 63.5 H24 Z\"/></svg>";

/// HTML-escape text or a double-quoted attribute value.
pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The whole document around `body` (already HTML). `title` is escaped here.
fn document(title: &str, body: &str) -> String {
    let mut s = String::with_capacity(CSS.len() + MARK.len() + body.len() + 512);
    s.push_str("<!doctype html><html lang=en><head><meta charset=utf-8>");
    s.push_str("<meta name=viewport content=\"width=device-width,initial-scale=1\">");
    s.push_str("<meta name=color-scheme content=\"light dark\">");
    s.push_str("<meta name=robots content=\"noindex,nofollow\">");
    s.push_str("<meta name=referrer content=no-referrer>");
    s.push_str("<title>");
    s.push_str(&escape(title));
    s.push_str("</title><style>");
    s.push_str(CSS);
    s.push_str("</style></head><body><main><div class=brand>");
    s.push_str(MARK);
    s.push_str("<span>theuth</span></div>");
    s.push_str(body);
    s.push_str("</main></body></html>");
    s
}

/// The interstitial. `host` is the shared address (host and prefix), `action` the form target,
/// `operator` and `abuse_url` from the edge's `welcome` (empty: unnamed / no link).
pub fn interstitial(host: &str, action: &str, operator: &str, abuse_url: &str) -> String {
    let operator = if operator.is_empty() {
        "the edge's operator".into()
    } else {
        escape(operator)
    };
    let host = escape(host);
    let report = if abuse_url.is_empty() {
        String::new()
    } else {
        format!(
            "<footer><a href=\"{}\" rel=\"noopener\">Report abuse ↗</a></footer>",
            escape(abuse_url)
        )
    };
    document(
        "Shared with theuth",
        &format!(
            "<p class=eyebrow>A shared app</p><h1>{host}</h1>\
             <p>This is an app a theuth user is sharing from their own computer. Neither theuth \
             nor {operator} checked it.</p>\
             <p class=note>Don't enter passwords you use elsewhere.</p>\
             <form method=post action=\"{}\"><button>Continue to {host}</button></form>{report}",
            escape(action)
        ),
    )
}

/// The password prompt; `wrong` after a failed attempt.
pub fn password(host: &str, action: &str, wrong: bool) -> String {
    let message = if wrong {
        "<p class=\"note error\" role=alert>Wrong password.</p>"
    } else {
        "<p>This share needs a password.</p>"
    };
    document(
        "Password required · theuth",
        &format!(
            "<p class=eyebrow>Password required</p><h1>{}</h1>{message}\
             <form method=post action=\"{}\"><label for=password>Password</label>\
             <input type=password id=password name=password autofocus autocomplete=off>\
             <button>Open</button></form>",
            escape(host),
            escape(action)
        ),
    )
}

/// Heading and explanation of an error a visitor's browser shows.
fn error_copy(status: StatusCode) -> (&'static str, &'static str) {
    match status {
        StatusCode::MISDIRECTED_REQUEST => (
            "Nothing is shared here",
            "This computer isn't sharing anything at this address right now. The share may have \
             been stopped, or the link may be wrong.",
        ),
        StatusCode::NOT_FOUND => ("Not found", "There's nothing at this address."),
        StatusCode::BAD_GATEWAY => (
            "The app isn't answering",
            "The shared app isn't reachable on its computer right now. It may have been closed; \
             try again in a moment.",
        ),
        StatusCode::GATEWAY_TIMEOUT => (
            "The app did not answer in time",
            "The shared app took too long to respond. Try again in a moment.",
        ),
        StatusCode::FORBIDDEN => (
            "Request refused",
            "This form was sent from another site, so it was refused. Go back and try again.",
        ),
        StatusCode::TOO_MANY_REQUESTS => (
            "Slow down",
            "Too many attempts from your network. Wait a minute, then try again.",
        ),
        StatusCode::UNAUTHORIZED => ("Password required", "This share needs a password."),
        _ => (
            "Something went wrong",
            "This request could not be answered.",
        ),
    }
}

/// An error page. `host` (if known) is shown; `detail` is the plain-text reason, shown small.
pub fn error(status: StatusCode, host: Option<&str>, detail: &str) -> String {
    let (heading, copy) = error_copy(status);
    let eyebrow = match host {
        Some(h) if !h.is_empty() => format!("<p class=\"eyebrow host\">{}</p>", escape(h)),
        _ => String::new(),
    };
    document(
        &format!("{heading} · theuth"),
        &format!(
            "{eyebrow}<h1 class=title>{heading}</h1><p>{copy}</p><p class=detail>{} · {}</p>",
            status.as_u16(),
            escape(detail)
        ),
    )
}

/// A top-level navigation that renders HTML (`Sec-Fetch-Mode: navigate`, `Accept` has
/// `text/html`): such a visitor gets a page; scripts and API clients keep the plain-text answer.
pub fn wants_html(h: &HeaderMap) -> bool {
    h.get("sec-fetch-mode").and_then(|v| v.to_str().ok()) == Some("navigate")
        && h.get(header::ACCEPT)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|a| a.contains("text/html"))
}

/// An HTML answer: no caching, never framed.
pub fn html(status: StatusCode, body: String) -> Response<Body> {
    let mut r = Response::new(full(body));
    *r.status_mut() = status;
    let h = r.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert("x-frame-options", HeaderValue::from_static("DENY"));
    r
}

/// The error answer: the styled page for a navigation, else the plain-text `msg`.
pub fn fail(status: StatusCode, msg: &str, nav: bool, host: Option<&str>) -> Response<Body> {
    if nav {
        html(status, error(status, host, msg))
    } else {
        text(status, msg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EVIL: &str = "x\"><script>alert(1)</script>.sky.tun.zip";

    fn all_pages(host: &str) -> Vec<(&'static str, String)> {
        vec![
            (
                "interstitial",
                interstitial(
                    host,
                    "/.theuth-share/continue",
                    "tun.zip",
                    "https://tun.zip/abuse",
                ),
            ),
            ("password", password(host, "/.theuth-share/login", false)),
            (
                "password-wrong",
                password(host, "/.theuth-share/login", true),
            ),
            (
                "not-shared",
                error(
                    StatusCode::MISDIRECTED_REQUEST,
                    Some(host),
                    "misdirected request",
                ),
            ),
            (
                "not-found",
                error(StatusCode::NOT_FOUND, Some(host), "not found"),
            ),
            (
                "offline",
                error(
                    StatusCode::BAD_GATEWAY,
                    Some(host),
                    "port 5173 is not listening",
                ),
            ),
            (
                "timeout",
                error(
                    StatusCode::GATEWAY_TIMEOUT,
                    Some(host),
                    "the app did not answer in time",
                ),
            ),
            (
                "refused",
                error(
                    StatusCode::FORBIDDEN,
                    Some(host),
                    "cross-site request refused",
                ),
            ),
            (
                "rate-limited",
                error(
                    StatusCode::TOO_MANY_REQUESTS,
                    Some(host),
                    "too many attempts, wait a minute",
                ),
            ),
        ]
    }

    #[test]
    fn pages_are_themed_and_self_contained() {
        for (name, p) in all_pages("h3renow.sky.tun.zip") {
            assert!(
                p.contains("<meta name=color-scheme content=\"light dark\">"),
                "{name}"
            );
            assert!(p.contains("@media (prefers-color-scheme:dark)"), "{name}");
            // Theming lives in <head>, before any content, and needs no script.
            assert!(
                p.find("<style>").unwrap() < p.find("<body>").unwrap(),
                "{name}"
            );
            assert!(!p.contains("<script"), "{name}");
            // No external loads: the only absolute URL is the abuse link's href.
            for attr in ["src=", "@import", "url(http", "<link"] {
                assert!(!p.contains(attr), "{name}: {attr}");
            }
            let urls = p.matches("http://").count() + p.matches("https://").count();
            let allowed = usize::from(name == "interstitial");
            assert_eq!(urls, allowed, "{name}");
        }
        assert!(
            all_pages("h3renow.sky.tun.zip")[0]
                .1
                .contains("<button>Continue to h3renow.sky.tun.zip</button>")
        );
    }

    #[test]
    fn host_and_operator_are_escaped() {
        for (name, p) in all_pages(EVIL) {
            assert!(!p.contains("<script>"), "{name}");
            assert!(p.contains("x&quot;&gt;&lt;script&gt;"), "{name}");
        }
        let p = interstitial("a.tun.zip", "/c\"x", "<b>op</b>", "https://e/\"><x");
        assert!(p.contains("&lt;b&gt;op&lt;/b&gt;"));
        assert!(p.contains("action=\"/c&quot;x\""));
        assert!(p.contains("href=\"https://e/&quot;&gt;&lt;x\""));
        let e = error(StatusCode::BAD_GATEWAY, None, "<img src=x>");
        assert!(e.contains("&lt;img src=x&gt;") && !e.contains("<img"));
    }

    /// Writes every page, light and dark, to `/tmp/gate-preview/` (run with `--ignored`).
    #[test]
    #[ignore = "writes preview files"]
    fn write_previews() {
        let dir = std::path::Path::new("/tmp/gate-preview");
        std::fs::create_dir_all(dir).unwrap();
        for (name, p) in all_pages("h3renow.sky.tun.zip") {
            let light = p
                .replace("(prefers-color-scheme:dark)", "not all")
                .replace("content=\"light dark\"", "content=\"light\"");
            let dark = p
                .replace("(prefers-color-scheme:dark)", "all")
                .replace("content=\"light dark\"", "content=\"dark\"");
            std::fs::write(dir.join(format!("{name}.html")), &p).unwrap();
            std::fs::write(dir.join(format!("{name}-light.html")), light).unwrap();
            std::fs::write(dir.join(format!("{name}-dark.html")), dark).unwrap();
        }
    }
}
