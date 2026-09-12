//! URL canonicalisation (design doc 16 §7): tracking strip, trailing slash,
//! case, scheme rejection, query order, Bing unwrap.

use web::url::{canonical_key, canonicalize};

#[test]
fn lowercase_host_and_scheme() {
    let n = canonicalize("HTTPS://Example.COM/Path").expect("url");
    assert_eq!(n.key, "https://example.com/Path");
    assert_eq!(n.pretty, "https://example.com/Path");
}

#[test]
fn strips_tracking_params_and_fragment() {
    let n = canonicalize(
        "https://example.com/a?utm_source=newsletter&utm_medium=email&id=9&fbclid=1&gclid=2#top",
    )
    .expect("url");
    assert_eq!(n.key, "https://example.com/a?id=9");
    assert_eq!(n.pretty, "https://example.com/a?id=9");
}

#[test]
fn trailing_slash_equality_key() {
    let with = canonicalize("https://example.com/docs/").unwrap();
    let without = canonicalize("https://example.com/docs").unwrap();
    assert_eq!(with.key, without.key);
    assert_eq!(with.key, "https://example.com/docs");
    assert_eq!(with.pretty, "https://example.com/docs/");
    assert_eq!(without.pretty, "https://example.com/docs");
}

#[test]
fn root_keeps_slash_and_default_ports_drop() {
    assert_eq!(
        canonicalize("https://example.com").unwrap().key,
        "https://example.com/"
    );
    assert_eq!(
        canonicalize("https://example.com:443/x").unwrap().key,
        "https://example.com/x"
    );
    assert_eq!(
        canonicalize("http://example.com:80/x").unwrap().key,
        "http://example.com/x"
    );
    assert_eq!(
        canonicalize("http://example.com:8080/x").unwrap().key,
        "http://example.com:8080/x"
    );
}

#[test]
fn rejects_non_http_schemes() {
    for s in [
        "ftp://files.example.com/a",
        "file:///tmp/x",
        "javascript:alert(1)",
        "",
        "   ",
    ] {
        assert!(canonicalize(s).is_none(), "{s}");
    }
    assert_eq!(canonical_key(" not a url "), "not a url");
}

#[test]
fn never_truncates_urls() {
    let long = format!("https://example.com/{}", "p".repeat(500));
    assert!(canonicalize(&long).unwrap().pretty.len() > 400);
}

#[test]
fn query_param_order_independent_key_but_pretty_keeps_order() {
    let a = canonicalize("https://example.com/x?z=1&a=2").unwrap();
    let b = canonicalize("https://example.com/x?a=2&z=1").unwrap();
    assert_eq!(a.key, b.key);
    assert_eq!(a.pretty, "https://example.com/x?z=1&a=2");
}
