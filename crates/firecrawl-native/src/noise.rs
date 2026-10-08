//! Noise safety valves (design doc 122 D8): whether a strip-list match may be
//! removed and whether a node is noise for candidate scoring. The 42 strip
//! selectors in `html.rs` stay CSS selectors; this module only decides what
//! a match (or a scoring candidate) is allowed to do.

use kuchikiki::NodeRef;

use crate::html::EXCLUDE_NON_MAIN_TAGS;

/// A strip-list match with more text than this is content and is kept.
pub const TEXT_VALVE_CHARS: usize = 5_000;
/// A `<form>` with less text than this is noise.
pub const FORM_NOISE_CHARS: usize = 500;
/// A `<form>` whose class contains one of these is noise whatever its size.
const FORM_NOISE_CLASS: [&str; 6] = [
    "login",
    "search",
    "subscribe",
    "signup",
    "newsletter",
    "contact",
];
/// An id ending in one of these is structural: never stripped by an id/class rule.
const STRUCTURAL_ID_SUFFIXES: [&str; 6] =
    ["portal", "root", "container", "wrapper", "mount", "app"];
/// Consent-platform id/class prefixes that `only_main_content` strips. Consulted by
/// `transform_html` (not appended to the selector list); prefix match, ASCII case-insensitive.
pub const CONSENT_PREFIXES: [&str; 14] = [
    "onetrust",
    "optanon",
    "ot-sdk",
    "cookiebot",
    "CybotCookiebot",
    "cc-",
    "cookie-law",
    "gdpr",
    "consent-",
    "cmp-",
    "sp_message",
    "qc-cmp",
    "trustarc",
    "evidon",
];
const NOISE_TAGS: [&str; 4] = ["header", "footer", "nav", "aside"];
const NOISE_ROLES: [&str; 7] = [
    "navigation",
    "banner",
    "contentinfo",
    "complementary",
    "dialog",
    "alertdialog",
    "search",
];

/// Chars of `node`'s text with whitespace runs collapsed to one space.
pub(crate) fn text_len(node: &NodeRef) -> usize {
    collapsed_len(&node.text_contents())
}

/// Chars of `s` with whitespace runs collapsed to one space and the ends trimmed.
pub(crate) fn collapsed_len(s: &str) -> usize {
    let mut words = 0usize;
    let mut chars = 0usize;
    for w in s.split_whitespace() {
        words += 1;
        chars += w.chars().count();
    }
    chars + words.saturating_sub(1)
}

fn has_structural_id(id: Option<&str>) -> bool {
    id.is_some_and(|id| {
        let id = id.trim();
        STRUCTURAL_ID_SUFFIXES
            .iter()
            .any(|s| id.len() >= s.len() && id[id.len() - s.len()..].eq_ignore_ascii_case(s))
    })
}

fn form_is_noise(class: Option<&str>, text_len: usize) -> bool {
    text_len < FORM_NOISE_CHARS
        || class.is_some_and(|c| {
            let c = c.to_ascii_lowercase();
            FORM_NOISE_CLASS.iter().any(|k| c.contains(k))
        })
}

fn starts_with_ci(s: &str, prefix: &str) -> bool {
    s.len() >= prefix.len()
        && s.is_char_boundary(prefix.len())
        && s[..prefix.len()].eq_ignore_ascii_case(prefix)
}

/// Whether `token` (an id or one class token) starts with a consent-platform prefix.
fn consent_token(token: &str) -> bool {
    CONSENT_PREFIXES.iter().any(|p| starts_with_ci(token, p))
}

/// Whether `node`'s id or any class token starts with one of [`CONSENT_PREFIXES`].
#[must_use]
pub fn has_consent_prefix(node: &NodeRef) -> bool {
    let Some(el) = node.as_element() else {
        return false;
    };
    let attrs = el.attributes.borrow();
    attrs.get("id").is_some_and(|id| consent_token(id.trim()))
        || attrs
            .get("class")
            .is_some_and(|c| c.split_whitespace().any(consent_token))
}

/// Whether `token` is the id/class part of one of the 42 strip selectors.
fn strip_token(token: &str) -> bool {
    EXCLUDE_NON_MAIN_TAGS.iter().any(|sel| {
        sel.strip_prefix('.')
            .or_else(|| sel.strip_prefix('#'))
            .is_some_and(|t| t == token)
    })
}

/// Whether a strip-list match may be removed.
///
/// Valves, in order: text valve (> [`TEXT_VALVE_CHARS`] chars of text is content), the `<form>`
/// rule (noise only when its text < [`FORM_NOISE_CHARS`] or its class names a login/search/
/// subscribe/signup/newsletter/contact form), structural ids (`…portal|root|container|wrapper|
/// mount|app` are never stripped by an id/class rule; a tag rule — `header`/`footer`/`nav`/
/// `aside` — still applies).
#[must_use]
pub fn may_strip(node: &NodeRef) -> bool {
    let Some(el) = node.as_element() else {
        return true;
    };
    let len = text_len(node);
    if len > TEXT_VALVE_CHARS {
        return false;
    }
    let tag: &str = &el.name.local;
    let attrs = el.attributes.borrow();
    if tag == "form" {
        return form_is_noise(attrs.get("class"), len);
    }
    !has_structural_id(attrs.get("id")) || NOISE_TAGS.contains(&tag)
}

/// Whether `node` is noise for candidate scoring: by tag (`header`/`footer`/`nav`/`aside`), by
/// `role`, by an id/class token of the strip list or a consent prefix (both subject to the
/// structural-id exemption), or by the `<form>` rule. The text valve applies to every rule.
#[must_use]
pub fn is_noise(node: &NodeRef) -> bool {
    let Some(el) = node.as_element() else {
        return false;
    };
    let tag: &str = &el.name.local;
    let attrs = el.attributes.borrow();
    let by_tag = NOISE_TAGS.contains(&tag);
    let by_role = attrs
        .get("role")
        .is_some_and(|r| NOISE_ROLES.iter().any(|k| r.trim().eq_ignore_ascii_case(k)));
    let by_token = !has_structural_id(attrs.get("id")) && {
        let id = attrs.get("id").map(str::trim);
        id.is_some_and(|id| strip_token(id) || consent_token(id))
            || attrs.get("class").is_some_and(|c| {
                c.split_whitespace()
                    .any(|t| strip_token(t) || consent_token(t))
            })
    };
    let is_form = tag == "form";
    if !(by_tag || by_role || by_token || is_form) {
        return false;
    }
    let len = text_len(node);
    if len > TEXT_VALVE_CHARS {
        return false;
    }
    if by_tag || by_role || by_token {
        return true;
    }
    form_is_noise(attrs.get("class"), len)
}

#[cfg(test)]
mod tests {
    use kuchikiki::{parse_html, traits::TendrilSink};

    use super::*;

    fn node(html: &str, sel: &str) -> NodeRef {
        let doc = parse_html().one(html);
        doc.select_first(sel).expect("selector").as_node().clone()
    }

    fn filler(chars: usize) -> String {
        "word ".repeat(chars.div_ceil(5))
    }

    #[test]
    fn text_valve_keeps_large_matches() {
        let big = format!(r#"<div class="header">{}</div>"#, filler(5_100));
        assert!(!may_strip(&node(&big, ".header")));
        assert!(!is_noise(&node(&big, ".header")));
        let small = format!(r#"<div class="header">{}</div>"#, filler(400));
        assert!(may_strip(&node(&small, ".header")));
        assert!(is_noise(&node(&small, ".header")));
    }

    #[test]
    fn form_rule() {
        let content = format!("<form id=\"aspnetForm\">{}</form>", filler(600));
        assert!(!may_strip(&node(&content, "form")));
        assert!(!is_noise(&node(&content, "form")));
        let login = format!("<form class=\"login-form\">{}</form>", filler(600));
        assert!(may_strip(&node(&login, "form")));
        assert!(is_noise(&node(&login, "form")));
        let short = format!("<form>{}</form>", filler(100));
        assert!(may_strip(&node(&short, "form")));
        assert!(is_noise(&node(&short, "form")));
        let short_exact = format!("<form>{}</form>", filler(499));
        assert_eq!(text_len(&node(&short_exact, "form")), 499);
        assert!(is_noise(&node(&short_exact, "form")));
    }

    #[test]
    fn structural_id_exemption() {
        let h = r#"<div id="page-wrapper" class="menu">menu text</div>"#;
        assert!(!may_strip(&node(h, ".menu")));
        assert!(!is_noise(&node(h, ".menu")));
        let h = r#"<div id="App" class="menu">menu text</div>"#;
        assert!(!may_strip(&node(h, ".menu")));
        // A tag rule still strips a structural id.
        let h = r#"<nav id="root">links</nav>"#;
        assert!(may_strip(&node(h, "nav")));
        assert!(is_noise(&node(h, "nav")));
        let h = r#"<div id="rooting" class="menu">x</div>"#;
        assert!(may_strip(&node(h, ".menu")));
    }

    #[test]
    fn consent_prefixes() {
        assert!(has_consent_prefix(&node(
            r#"<div id="onetrust-consent-sdk">x</div>"#,
            "div"
        )));
        assert!(has_consent_prefix(&node(
            r#"<div class="dialog CybotCookiebotDialog">x</div>"#,
            "div"
        )));
        assert!(has_consent_prefix(&node(
            r#"<div class="cc-window cc-banner">x</div>"#,
            "div"
        )));
        assert!(!has_consent_prefix(&node(
            r#"<div class="cookie recipe">x</div>"#,
            "div"
        )));
        assert!(!has_consent_prefix(&node(
            r#"<div id="ccc">x</div>"#,
            "div"
        )));
        assert!(is_noise(&node(
            r#"<div id="onetrust-banner-sdk">x</div>"#,
            "div"
        )));
        assert_eq!(CONSENT_PREFIXES.len(), 14);
    }

    #[test]
    fn noise_by_tag_role_and_token() {
        assert!(is_noise(&node("<nav>x</nav>", "nav")));
        assert!(is_noise(&node("<aside>x</aside>", "aside")));
        assert!(is_noise(&node(r#"<div role="Navigation">x</div>"#, "div")));
        assert!(is_noise(&node(
            r#"<div class="x sidebar y">x</div>"#,
            "div"
        )));
        assert!(!is_noise(&node(
            r#"<div class="sidebar-wide">x</div>"#,
            "div"
        )));
        assert!(is_noise(&node(r#"<div id="footer">x</div>"#, "div")));
        assert!(!is_noise(&node(r#"<div class="post-body">x</div>"#, "div")));
        assert!(!is_noise(&node("<p>x</p>", "p")));
        assert_eq!(EXCLUDE_NON_MAIN_TAGS.len(), 42);
    }

    #[test]
    fn collapsed_len_counts_single_spaces() {
        assert_eq!(collapsed_len("  a   b\n\tc  "), 5);
        assert_eq!(collapsed_len(""), 0);
        assert_eq!(collapsed_len("héllo"), 5);
    }
}
