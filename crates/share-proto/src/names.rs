//! Name, label, host and path rules (§5.2, §5.4, §5.1), shared so the client refuses a bad name before asking.
//!
//! A mesh's **name** is one DNS label under `D`; a share's host is `<name>.D` or `<label>.<name>.D` — exactly one
//! label below a name, never deeper (the wildcard certificate covers one level).

use serde::{Deserialize, Serialize};

/// Reserved names: at least this long.
pub const RESERVED_MIN_LEN: usize = 3;
/// Reserved names: at most this long.
pub const RESERVED_MAX_LEN: usize = 32;
/// Random names: exactly this long (§12 Q15).
pub const RANDOM_LEN: usize = 6;
/// Random names draw from lowercase base32: `[a-z2-7]`.
pub const RANDOM_ALPHABET: &[u8; 32] = b"abcdefghijklmnopqrstuvwxyz234567";
/// Sub-labels: `[a-z0-9-]{1,63}`.
pub const LABEL_MAX_LEN: usize = 63;
/// Share paths: at most this long (§5.1).
pub const PATH_MAX_LEN: usize = 64;
/// Built-in reserved words; the operator's `reserved_words` add to them (§5.2).
pub const BUILTIN_RESERVED_WORDS: &[&str] = &[
    "www", "api", "admin", "login", "mail", "abuse", "status", "support", "secure", "account",
    "theuth", "edge", "ns1", "ns2",
];

/// How a mesh came to hold a name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NameKind {
    /// 6 × `[a-z2-7]`, drawn by the client.
    Random,
    /// `[a-z0-9-]{3,32}`, first claim wins.
    Reserved,
}

/// Why a name, label or path is refused. [`NameError::reason`] is the `reason` of `400 {code: invalid, reason}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum NameError {
    /// Outside the length bounds.
    #[error("wrong length")]
    Length,
    /// A character outside the allowed set.
    #[error("characters outside the allowed set")]
    Charset,
    /// Leading or trailing `-`.
    #[error("leading or trailing hyphen")]
    Hyphen,
    /// `xn--` (IDNA) labels are refused.
    #[error("punycode (xn--) is not allowed")]
    Idna,
    /// A built-in or operator reserved word.
    #[error("reserved word")]
    Reserved,
    /// Matches the operator's brand blocklist (§9.4).
    #[error("too close to a protected brand")]
    Brand,
    /// A path with an empty segment or a trailing slash.
    #[error("malformed path")]
    Path,
}

impl NameError {
    /// The wire `reason`.
    pub fn reason(self) -> &'static str {
        match self {
            NameError::Length => "length",
            NameError::Charset => "charset",
            NameError::Hyphen => "hyphen",
            NameError::Idna => "idna",
            NameError::Reserved => "reserved",
            NameError::Brand => "brand",
            NameError::Path => "path",
        }
    }
}

fn is_ldh(b: u8) -> bool {
    b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'
}

/// A DNS label as this design uses it: `[a-z0-9-]{1,63}`, no leading or trailing `-`, no `xn--`. Sub-labels and
/// the name part of a host are checked with this.
pub fn check_label(label: &str) -> Result<(), NameError> {
    if label.is_empty() || label.len() > LABEL_MAX_LEN {
        return Err(NameError::Length);
    }
    if !label.bytes().all(is_ldh) {
        return Err(NameError::Charset);
    }
    if label.starts_with('-') || label.ends_with('-') {
        return Err(NameError::Hyphen);
    }
    if label.starts_with("xn--") {
        return Err(NameError::Idna);
    }
    Ok(())
}

/// A reserved-name claim (§5.2): `[a-z0-9-]{3,32}`, no leading or trailing `-`, no `xn--`, not a built-in or
/// operator reserved word. The brand blocklist is [`check_brand`], applied by the edge.
pub fn check_reserved<S: AsRef<str>>(name: &str, operator_words: &[S]) -> Result<(), NameError> {
    if name.len() < RESERVED_MIN_LEN || name.len() > RESERVED_MAX_LEN {
        return Err(NameError::Length);
    }
    check_label(name)?;
    if BUILTIN_RESERVED_WORDS.contains(&name) || operator_words.iter().any(|w| w.as_ref() == name) {
        return Err(NameError::Reserved);
    }
    Ok(())
}

/// A random candidate: exactly 6 of `[a-z2-7]`, and not a built-in reserved word (`status`, `secure` and
/// `theuth` are drawable).
pub fn check_random(name: &str) -> Result<(), NameError> {
    if name.len() != RANDOM_LEN {
        return Err(NameError::Length);
    }
    if !name.bytes().all(|b| RANDOM_ALPHABET.contains(&b)) {
        return Err(NameError::Charset);
    }
    if BUILTIN_RESERVED_WORDS.contains(&name) {
        return Err(NameError::Reserved);
    }
    Ok(())
}

/// Maps 6 random bytes (the caller's RNG) onto [`RANDOM_ALPHABET`]; uniform because 32 divides 256. The client
/// redraws when [`check_random`] refuses the result.
pub fn random_name(bytes: [u8; RANDOM_LEN]) -> String {
    bytes
        .iter()
        .map(|b| RANDOM_ALPHABET[(b & 31) as usize] as char)
        .collect()
}

/// The brand blocklist rule (§9.4): with `-` and digits removed, the name contains a listed brand or is within
/// edit distance 1 of one. `brands` are lowercase. Random names are not checked.
pub fn check_brand<S: AsRef<str>>(name: &str, brands: &[S]) -> Result<(), NameError> {
    let core: String = name.chars().filter(|c| c.is_ascii_lowercase()).collect();
    let hit = brands
        .iter()
        .map(AsRef::as_ref)
        .filter(|b| !b.is_empty())
        .any(|b| core.contains(b) || within_one_edit(&core, b));
    if hit { Err(NameError::Brand) } else { Ok(()) }
}

/// Levenshtein distance ≤ 1 over bytes.
fn within_one_edit(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    let (short, long) = if a.len() <= b.len() { (a, b) } else { (b, a) };
    if long.len() - short.len() > 1 {
        return false;
    }
    let prefix = short.iter().zip(long).take_while(|(x, y)| x == y).count();
    if prefix == short.len() {
        return true;
    }
    if short.len() == long.len() {
        short[prefix + 1..] == long[prefix + 1..]
    } else {
        short[prefix..] == long[prefix + 1..]
    }
}

/// A share path (§5.1): `/` or `/seg[/seg…]` of `[a-z0-9-]`, ≤ 64 bytes, no trailing slash, no empty segment.
/// Dots are outside the set, so no dot-segment can appear.
pub fn check_path(path: &str) -> Result<(), NameError> {
    if path == "/" {
        return Ok(());
    }
    if path.len() > PATH_MAX_LEN {
        return Err(NameError::Length);
    }
    let Some(rest) = path.strip_prefix('/') else {
        return Err(NameError::Path);
    };
    if !rest.bytes().all(|b| is_ldh(b) || b == b'/') {
        return Err(NameError::Charset);
    }
    if rest.split('/').any(str::is_empty) {
        return Err(NameError::Path);
    }
    Ok(())
}

/// A host split into its mesh name and optional sub-label.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostParts {
    /// The mesh's name (one label under `D`).
    pub name: String,
    /// The sub-label, for `<label>.<name>.D`.
    pub label: Option<String>,
}

/// Why a host is not routable under `D`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum HostError {
    /// Not below `D`.
    #[error("host is not under the edge's domain")]
    NotUnderDomain,
    /// `D` itself.
    #[error("host is the edge's domain itself")]
    Apex,
    /// More than one label below a name (§4.2: reset).
    #[error("more than one label below a name")]
    TooDeep,
    /// A label breaks [`check_label`].
    #[error("bad label: {0}")]
    Label(NameError),
}

/// Parses an SNI or `Host` (case-insensitive, optional trailing dot, no port) under `domain` into
/// `(name, label?)`, enforcing the one-label-deep rule. Name policy (reserved words, holding) is the caller's.
pub fn parse_host(host: &str, domain: &str) -> Result<HostParts, HostError> {
    let host = host.strip_suffix('.').unwrap_or(host).to_ascii_lowercase();
    let domain = domain
        .strip_suffix('.')
        .unwrap_or(domain)
        .to_ascii_lowercase();
    if host == domain {
        return Err(HostError::Apex);
    }
    let below = host
        .strip_suffix(domain.as_str())
        .and_then(|h| h.strip_suffix('.'))
        .ok_or(HostError::NotUnderDomain)?;
    let mut labels = below.rsplit('.');
    let name = labels.next().ok_or(HostError::NotUnderDomain)?;
    let label = labels.next();
    if labels.next().is_some() {
        return Err(HostError::TooDeep);
    }
    check_label(name).map_err(HostError::Label)?;
    if let Some(l) = label {
        check_label(l).map_err(HostError::Label)?;
    }
    Ok(HostParts {
        name: name.to_owned(),
        label: label.map(str::to_owned),
    })
}

/// `<name>.D` or `<label>.<name>.D`.
pub fn host(name: &str, label: Option<&str>, domain: &str) -> String {
    match label {
        Some(l) => format!("{l}.{name}.{domain}"),
        None => format!("{name}.{domain}"),
    }
}

/// `https://{host}{path}/` exactly as the approval card shows it (§5.1); `path` is `/` or `/app1`.
pub fn share_url(host: &str, path: &str) -> String {
    if path == "/" {
        format!("https://{host}/")
    } else {
        format!("https://{host}{path}/")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: &[&str] = &[];

    #[test]
    fn reserved_rules() {
        assert_eq!(check_reserved("sky", NONE), Ok(()));
        assert_eq!(check_reserved("my-app-2", NONE), Ok(()));
        assert_eq!(check_reserved("ab", NONE), Err(NameError::Length));
        assert_eq!(
            check_reserved(&"a".repeat(33), NONE),
            Err(NameError::Length)
        );
        assert_eq!(check_reserved(&"a".repeat(32), NONE), Ok(()));
        assert_eq!(check_reserved("Sky", NONE), Err(NameError::Charset));
        assert_eq!(check_reserved("sky_1", NONE), Err(NameError::Charset));
        assert_eq!(check_reserved("-sky", NONE), Err(NameError::Hyphen));
        assert_eq!(check_reserved("sky-", NONE), Err(NameError::Hyphen));
        assert_eq!(check_reserved("xn--sky", NONE), Err(NameError::Idna));
        assert_eq!(check_reserved("admin", NONE), Err(NameError::Reserved));
        assert_eq!(check_reserved("edge", NONE), Err(NameError::Reserved));
        assert_eq!(check_reserved("acme", &["acme"]), Err(NameError::Reserved));
        assert_eq!(NameError::Reserved.reason(), "reserved");
    }

    #[test]
    fn random_rules() {
        assert_eq!(check_random("k3f7q2"), Ok(()));
        assert_eq!(check_random("k3f7q"), Err(NameError::Length));
        assert_eq!(check_random("k3f7q1"), Err(NameError::Charset));
        assert_eq!(check_random("k3f7q8"), Err(NameError::Charset));
        assert_eq!(check_random("status"), Err(NameError::Reserved));
        let n = random_name([0, 25, 26, 31, 32, 255]);
        assert_eq!(n, "az27a7");
        assert_eq!(check_random(&n), Ok(()));
    }

    #[test]
    fn brand_rules() {
        let brands = ["paypal", "google"];
        assert_eq!(check_brand("paypal-login", &brands), Err(NameError::Brand));
        assert_eq!(check_brand("pay-pal", &brands), Err(NameError::Brand));
        assert_eq!(check_brand("paypa1", &brands), Err(NameError::Brand)); // digit dropped → "paypa", one edit
        assert_eq!(check_brand("gooogle", &brands), Err(NameError::Brand));
        assert_eq!(check_brand("goggle", &brands), Err(NameError::Brand));
        assert_eq!(check_brand("sky", &brands), Ok(()));
        assert_eq!(check_brand("gogle-maps", &brands), Ok(())); // containment is on the whole core
    }

    #[test]
    fn labels_and_paths() {
        assert_eq!(check_label("a"), Ok(()));
        assert_eq!(check_label(&"a".repeat(64)), Err(NameError::Length));
        assert_eq!(check_path("/"), Ok(()));
        assert_eq!(check_path("/app1"), Ok(()));
        assert_eq!(check_path("/docs/v-2"), Ok(()));
        assert_eq!(check_path("/app1/"), Err(NameError::Path));
        assert_eq!(check_path("app1"), Err(NameError::Path));
        assert_eq!(check_path("/a//b"), Err(NameError::Path));
        assert_eq!(check_path("/../etc"), Err(NameError::Charset));
        assert_eq!(check_path("/App"), Err(NameError::Charset));
        assert_eq!(
            check_path(&format!("/{}", "a".repeat(64))),
            Err(NameError::Length)
        );
    }

    #[test]
    fn host_parsing() {
        let d = "tun.zip";
        assert_eq!(
            parse_host("sky.tun.zip", d),
            Ok(HostParts {
                name: "sky".into(),
                label: None
            })
        );
        assert_eq!(
            parse_host("App.Sky.TUN.zip.", d),
            Ok(HostParts {
                name: "sky".into(),
                label: Some("app".into())
            })
        );
        assert_eq!(parse_host("a.b.sky.tun.zip", d), Err(HostError::TooDeep));
        assert_eq!(parse_host("tun.zip", d), Err(HostError::Apex));
        assert_eq!(
            parse_host("sky.example.org", d),
            Err(HostError::NotUnderDomain)
        );
        assert_eq!(parse_host("skytun.zip", d), Err(HostError::NotUnderDomain));
        assert_eq!(
            parse_host(".tun.zip", d),
            Err(HostError::Label(NameError::Length))
        );
        assert_eq!(
            parse_host("-x.sky.tun.zip", d),
            Err(HostError::Label(NameError::Hyphen))
        );
        assert_eq!(host("sky", Some("app"), d), "app.sky.tun.zip");
        assert_eq!(host("sky", None, d), "sky.tun.zip");
        assert_eq!(share_url("sky.tun.zip", "/"), "https://sky.tun.zip/");
        assert_eq!(
            share_url("sky.tun.zip", "/docs"),
            "https://sky.tun.zip/docs/"
        );
    }
}
