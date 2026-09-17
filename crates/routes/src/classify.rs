//! Exchange → Route (44 §3.3): admit, decode, find the record collection,
//! template the URL, split auth header names from values, rank. Pure
//! functions, no I/O.

use std::collections::{BTreeMap, HashMap};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use url::{Position, Url};

/// One captured request/response row as `browser_network_capture` emits it (44 §3.1).
#[derive(Clone, Debug, Deserialize)]
pub struct Exchange {
    /// Request URL.
    pub url: String,
    /// Request method.
    pub method: String,
    /// obscura `ResourceType` Debug name (`Fetch`, `Xhr`, `Document`, `Script`, …); matched case-insensitively.
    pub resource_type: String,
    /// Response status.
    pub status: u16,
    /// Request headers, names lowercased.
    #[serde(default)]
    pub request_headers: HashMap<String, String>,
    /// Response headers, names lowercased.
    #[serde(default)]
    pub response_headers: HashMap<String, String>,
    /// Response content type before `;`, lowercased.
    pub content_type: Option<String>,
    /// Response body when textual, cut at the capture cap.
    pub body: Option<String>,
    /// Uncut body length in bytes.
    #[serde(default)]
    pub body_len: usize,
    /// `body` was cut.
    #[serde(default)]
    pub truncated: bool,
    /// Capture time, epoch millis.
    #[serde(default)]
    pub ts: u64,
}

/// Where a hole sits in the template.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum HoleIn {
    /// A path segment `{name}`.
    Path,
    /// A query value `key={name}`.
    Query,
}

/// A variable part of a route template.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Hole {
    /// Parameter name (`id`, `id2`, … for path; the key for query).
    pub name: String,
    /// Path or query.
    #[serde(rename = "in")]
    pub location: HoleIn,
    /// The value observed at capture.
    pub example: String,
}

/// A learned data route (44 §3.4). Structure only: header names, never values.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema)]
pub struct Route {
    /// `{domain}:{METHOD}:{template path}`.
    pub id: String,
    /// GraphQL `operationName`, else the last non-hole path segment, else the domain.
    pub name: String,
    /// Upper-case method.
    pub method: String,
    /// Absolute URL with `{name}` holes; tracking params and fragment dropped.
    pub template: String,
    /// Holes in `template`.
    pub holes: Vec<Hole>,
    /// JSON pointer to the record array, `""` for root.
    pub collection: String,
    /// Record key → JSON type name.
    pub keys: BTreeMap<String, String>,
    /// Records in the collection at capture.
    pub records_seen: usize,
    /// Lowercase request header names outside the allow-list (§3.3.5).
    pub auth_headers: Vec<String>,
    /// A `cookie` header was sent.
    pub cookies: bool,
    /// Safe method, or a GraphQL POST whose operation is in the URL.
    pub replay: bool,
    /// Learn count.
    pub seen: u32,
    /// Last successful replay, RFC 3339.
    pub last_ok: Option<String>,
    /// Last replay failed the drift check.
    pub stale: bool,
    /// The URL the route was learned from.
    pub example_url: String,
}

/// One learned route plus the credential values that must never be serialised.
pub struct Learned {
    /// The route.
    pub route: Route,
    /// `(lowercase header name, value)` for every `auth_headers` entry.
    pub creds: Vec<(String, String)>,
}

/// The record array found in a decoded body.
#[derive(Clone, Debug)]
pub struct Collection {
    /// JSON pointer to the array, `""` for root.
    pub pointer: String,
    /// Union of record keys → JSON type name.
    pub keys: BTreeMap<String, String>,
    /// Record count.
    pub count: usize,
    /// The records.
    pub records: Vec<Value>,
}

const MAX_DEPTH: usize = 8;
const MAX_NODES: usize = 50_000;
const XSSI_PREFIXES: &[&str] = &[")]}'", "for(;;);", "while(1);"];
const NON_AUTH_HEADERS: &[&str] = &[
    "accept",
    "accept-language",
    "accept-encoding",
    "content-type",
    "content-length",
    "user-agent",
    "referer",
    "origin",
    "x-requested-with",
    "host",
    "connection",
    "pragma",
    "cache-control",
    "priority",
    "te",
    "upgrade-insecure-requests",
];

/// Registrable domain of `url` (`psl`), else the host; lowercase.
#[must_use]
pub fn registrable_domain(url: &Url) -> String {
    let host = url.host_str().unwrap_or_default();
    psl::domain_str(host).unwrap_or(host).to_ascii_lowercase()
}

/// XSSI-strip and parse `body`; for `text/html` collect every
/// `<script type="application/json">` / `application/ld+json` block into an
/// array. `None` when nothing parses.
#[must_use]
pub fn decode(body: &str, content_type: Option<&str>) -> Option<Value> {
    if content_type.is_some_and(|c| c.starts_with("text/html")) {
        let blocks: Vec<Value> = script_json_blocks(body).filter_map(parse_json).collect();
        return (!blocks.is_empty()).then_some(Value::Array(blocks));
    }
    parse_json(body)
}

fn parse_json(s: &str) -> Option<Value> {
    let s = s.trim_start();
    let s = XSSI_PREFIXES
        .iter()
        .find_map(|p| s.strip_prefix(*p))
        .unwrap_or(s);
    serde_json::from_str(s).ok()
}

fn script_json_blocks(html: &str) -> impl Iterator<Item = &str> {
    let mut rest = html;
    std::iter::from_fn(move || {
        loop {
            let start = rest.find("<script")?;
            let after = &rest[start + "<script".len()..];
            let tag_end = after.find('>')?;
            let attrs = &after[..tag_end];
            let body = &after[tag_end + 1..];
            let end = body.find("</script").unwrap_or(body.len());
            rest = &body[end..];
            if attrs.contains("application/json") || attrs.contains("application/ld+json") {
                return Some(&body[..end]);
            }
        }
    })
}

/// Breadth-first, depth ≤ 8, ≤ 50k nodes: the first array of ≥ 2 objects
/// whose key sets overlap ≥ 50 % with the first record's; the largest wins
/// among candidates at the same depth.
#[must_use]
pub fn find_collection(v: &Value) -> Option<Collection> {
    let mut level: Vec<(String, &Value)> = vec![(String::new(), v)];
    let mut visited = 0usize;
    for _ in 0..=MAX_DEPTH {
        let mut next = Vec::new();
        let mut best: Option<Collection> = None;
        for (ptr, node) in level {
            visited += 1;
            if visited > MAX_NODES {
                return best;
            }
            match node {
                Value::Array(items) => {
                    if let Some(c) = as_collection(&ptr, items) {
                        if best.as_ref().is_none_or(|b| c.count > b.count) {
                            best = Some(c);
                        }
                        continue;
                    }
                    for (i, item) in items.iter().enumerate() {
                        if item.is_array() || item.is_object() {
                            next.push((format!("{ptr}/{i}"), item));
                        }
                    }
                }
                Value::Object(m) => {
                    for (k, item) in m {
                        if item.is_array() || item.is_object() {
                            next.push((child_pointer(&ptr, k), item));
                        }
                    }
                }
                _ => {}
            }
        }
        if best.is_some() {
            return best;
        }
        level = next;
    }
    None
}

fn child_pointer(ptr: &str, key: &str) -> String {
    let mut s = String::with_capacity(ptr.len() + 1 + key.len());
    s.push_str(ptr);
    s.push('/');
    for c in key.chars() {
        match c {
            '~' => s.push_str("~0"),
            '/' => s.push_str("~1"),
            c => s.push(c),
        }
    }
    s
}

// ponytail: 50 % Jaccard against the first record; heterogeneous unions
// (`[{type:"a",…},{type:"b",…}]`) are rejected — loosen per-type if a real
// site needs it.
fn as_collection(pointer: &str, items: &[Value]) -> Option<Collection> {
    if items.len() < 2 {
        return None;
    }
    let first = items[0].as_object()?;
    if first.is_empty() {
        return None;
    }
    let mut keys: BTreeMap<String, String> = BTreeMap::new();
    for item in items {
        let m = item.as_object()?;
        let inter = m.keys().filter(|k| first.contains_key(k.as_str())).count();
        if inter * 2 < first.len() + m.len() - inter {
            return None;
        }
        for (k, v) in m {
            let t = type_name(v);
            match keys.get_mut(k.as_str()) {
                Some(e) if e.as_str() == "null" && t != "null" => *e = t.to_owned(),
                Some(_) => {}
                None => {
                    keys.insert(k.clone(), t.to_owned());
                }
            }
        }
    }
    Some(Collection {
        pointer: pointer.to_owned(),
        keys,
        count: items.len(),
        records: items.to_vec(),
    })
}

fn type_name(v: &Value) -> &'static str {
    match v {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

// ponytail: a slug ≥ 16 chars of `[A-Za-z0-9_-]` is a hole too
// (`filter-search-proto` → `{id}`); the sibling rule is the real signal,
// tighten this when a false positive bites.
fn looks_like_id(seg: &str) -> bool {
    let b = seg.as_bytes();
    !b.is_empty()
        && (b.iter().all(u8::is_ascii_digit)
            || is_uuid(b)
            || (b.len() >= 16
                && b.iter()
                    .all(|c| c.is_ascii_alphanumeric() || *c == b'_' || *c == b'-')))
}

fn is_uuid(b: &[u8]) -> bool {
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                *c == b'-'
            } else {
                c.is_ascii_hexdigit()
            }
        })
}

fn is_auth_header(name: &str) -> bool {
    !(name.starts_with("sec-") || NON_AUTH_HEADERS.contains(&name))
}

fn admit(ex: &Exchange) -> Option<(Url, Collection)> {
    if !(200..300).contains(&ex.status)
        || !["fetch", "xhr", "document"]
            .iter()
            .any(|t| ex.resource_type.eq_ignore_ascii_case(t))
    {
        return None;
    }
    let url = Url::parse(&ex.url).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let body = ex.body.as_deref()?;
    let value = decode(body, ex.content_type.as_deref())?;
    let coll = find_collection(&value)?;
    Some((url, coll))
}

/// Admit, decode, find the collection, template (with sibling merge across
/// exchanges), split auth, rank. Deduped by `(method, template)` keeping the
/// larger `records_seen`; sorted by rank then `records_seen`, descending.
#[must_use]
pub fn classify_all(exs: &[Exchange]) -> Vec<Learned> {
    let cands: Vec<(&Exchange, Url, Collection)> = exs
        .iter()
        .filter_map(|ex| admit(ex).map(|(u, c)| (ex, u, c)))
        .collect();
    let segs: Vec<Vec<&str>> = cands
        .iter()
        .map(|(_, u, _)| u.path_segments().map_or_else(Vec::new, |s| s.collect()))
        .collect();
    let mut holes: Vec<Vec<bool>> = segs
        .iter()
        .map(|s| s.iter().map(|seg| looks_like_id(seg)).collect())
        .collect();
    // ponytail: O(n²) sibling scan over one capture (≤ a few hundred rows).
    // Two guards keep `/users` vs `/posts` (and `/graphql` vs `/gql`) apart:
    // the differing segment is never the first, and both replies must carry
    // the same record shape — same resource, different id.
    for i in 0..cands.len() {
        for j in i + 1..cands.len() {
            let (a, b) = (&cands[i], &cands[j]);
            if segs[i].len() != segs[j].len()
                || a.1[..Position::BeforePath] != b.1[..Position::BeforePath]
                || !a.0.method.eq_ignore_ascii_case(&b.0.method)
                || !same_shape(&a.2.keys, &b.2.keys)
            {
                continue;
            }
            let mut diff = segs[i]
                .iter()
                .zip(&segs[j])
                .enumerate()
                .filter(|(_, (x, y))| x != y)
                .map(|(k, _)| k);
            if let (Some(k), None) = (diff.next(), diff.next())
                && k > 0
            {
                holes[i][k] = true;
                holes[j][k] = true;
            }
        }
    }
    // The page's own URL is the strongest single-visit signal: a `resource/id`
    // pair that appears in both the navigated path (`/crates/tokio`) and an
    // API path (`/api/v1/crates/tokio/downloads`) makes the id a hole.
    // ponytail: pairs only; `/users/alice/posts` also holes `posts` — the
    // route stays callable (two path holes), just less pretty.
    let page_segs: std::collections::HashSet<String> = exs
        .iter()
        .filter(|e| e.resource_type.eq_ignore_ascii_case("document"))
        .filter_map(|e| Url::parse(&e.url).ok())
        .flat_map(|u| {
            u.path_segments().map_or_else(Vec::new, |s| {
                s.filter(|s| !s.is_empty()).map(str::to_owned).collect()
            })
        })
        .collect();
    for (row, segs) in holes.iter_mut().zip(&segs) {
        for k in 1..segs.len() {
            if !row[k] && page_segs.contains(segs[k]) && page_segs.contains(segs[k - 1]) {
                row[k] = true;
            }
        }
    }
    let mut built: Vec<(f64, Learned)> = cands
        .iter()
        .zip(&segs)
        .zip(&holes)
        .map(|(((ex, url, coll), segs), holes)| build(ex, url, coll, segs, holes))
        .collect();
    built.sort_by(|a, b| {
        (&a.1.route.method, &a.1.route.template).cmp(&(&b.1.route.method, &b.1.route.template))
    });
    built.dedup_by(|later, kept| {
        let same = later.1.route.method == kept.1.route.method
            && later.1.route.template == kept.1.route.template;
        if same && later.1.route.records_seen > kept.1.route.records_seen {
            std::mem::swap(later, kept);
        }
        same
    });
    built.sort_by(|a, b| {
        b.0.total_cmp(&a.0)
            .then(b.1.route.records_seen.cmp(&a.1.route.records_seen))
    });
    built.into_iter().map(|(_, l)| l).collect()
}

fn build(
    ex: &Exchange,
    url: &Url,
    coll: &Collection,
    segs: &[&str],
    holes: &[bool],
) -> (f64, Learned) {
    let mut path = String::new();
    let mut route_holes = Vec::new();
    let mut last_literal = "";
    for (&seg, &hole) in segs.iter().zip(holes) {
        path.push('/');
        if hole {
            let n = route_holes.len() + 1;
            let name = if n == 1 {
                "id".to_owned()
            } else {
                format!("id{n}")
            };
            path.push('{');
            path.push_str(&name);
            path.push('}');
            route_holes.push(Hole {
                name,
                location: HoleIn::Path,
                example: seg.to_owned(),
            });
        } else {
            path.push_str(seg);
            if !seg.is_empty() {
                last_literal = seg;
            }
        }
    }
    let mut query = String::new();
    let mut operation = None;
    let mut graphql = false;
    for (k, v) in url.query_pairs() {
        if k == "operationName" {
            operation = Some(v.to_string());
        }
        graphql |= k == "operationName" || k == "extensions";
        if web::url::is_tracking_param(&k) || route_holes.iter().any(|h| h.name == k) {
            continue;
        }
        query.push(if query.is_empty() { '?' } else { '&' });
        query.push_str(&k);
        query.push_str("={");
        query.push_str(&k);
        query.push('}');
        route_holes.push(Hole {
            name: k.into_owned(),
            location: HoleIn::Query,
            example: v.into_owned(),
        });
    }
    let domain = registrable_domain(url);
    let method = ex.method.to_ascii_uppercase();
    let name = operation.unwrap_or_else(|| {
        if last_literal.is_empty() {
            domain.clone()
        } else {
            last_literal.to_owned()
        }
    });
    let mut creds: Vec<(String, String)> = ex
        .request_headers
        .iter()
        .filter(|(k, _)| is_auth_header(&k.to_ascii_lowercase()))
        .map(|(k, v)| (k.to_ascii_lowercase(), v.clone()))
        .collect();
    creds.sort_unstable();
    let auth_headers: Vec<String> = creds.iter().map(|(k, _)| k.clone()).collect();
    let header_len: usize = ex
        .request_headers
        .iter()
        .map(|(k, v)| k.len() + v.len())
        .sum();
    let rank = ex.body_len as f64 / (ex.url.len() + header_len).max(1) as f64;
    let route = Route {
        id: format!("{domain}:{method}:{path}"),
        name,
        replay: matches!(method.as_str(), "GET" | "HEAD" | "OPTIONS")
            || (method == "POST" && graphql),
        template: format!("{}{path}{query}", &url[..Position::BeforePath]),
        method,
        holes: route_holes,
        collection: coll.pointer.clone(),
        keys: coll.keys.clone(),
        records_seen: coll.count,
        cookies: auth_headers.iter().any(|h| h == "cookie"),
        auth_headers,
        seen: 1,
        last_ok: None,
        stale: false,
        example_url: ex.url.clone(),
    };
    (rank, Learned { route, creds })
}

/// Drift check (44 §3.5.5): the collection must sit at `route.collection`
/// with ≥ 1 record and ≥ 50 % key overlap with `route.keys`. `Ok(records)`.
pub fn drift(route: &Route, body: &Value) -> Result<Vec<Value>, String> {
    let ptr = &route.collection;
    let Some(Value::Array(items)) = body.pointer(ptr) else {
        return Err(format!("collection not at {ptr:?}"));
    };
    let records: Vec<Value> = items.iter().filter(|v| v.is_object()).cloned().collect();
    if records.is_empty() {
        return Err(format!("no records at {ptr:?}"));
    }
    let keys: BTreeMap<String, String> = records
        .iter()
        .filter_map(Value::as_object)
        .flat_map(|m| m.keys().map(|k| (k.clone(), String::new())))
        .collect();
    if !same_shape(&keys, &route.keys) {
        return Err(format!("record keys drifted at {ptr:?}"));
    }
    Ok(records)
}

/// Jaccard over key names ≥ 50 %: the one "same records" rule, used by the
/// sibling merge and the drift check.
fn same_shape(a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) -> bool {
    let inter = a.keys().filter(|k| b.contains_key(k.as_str())).count();
    inter * 2 >= a.len() + b.len() - inter
}

/// Fill `route.template` from `params`: every path hole required, query
/// holes default to their `example`, extra params become extra query keys.
pub fn fill(route: &Route, params: &Map<String, Value>) -> Result<Url, String> {
    let (base, query) = route
        .template
        .split_once('?')
        .unwrap_or((route.template.as_str(), ""));
    let scheme_end = base.find("://").map_or(0, |i| i + 3);
    let path_start = base[scheme_end..]
        .find('/')
        .map_or(base.len(), |i| scheme_end + i);
    let mut out = String::with_capacity(route.template.len());
    out.push_str(&base[..path_start]);
    for seg in base[path_start..].split('/').skip(1) {
        out.push('/');
        match hole_name(seg) {
            Some(name) => {
                let v = params
                    .get(name)
                    .ok_or_else(|| format!("missing path parameter {name}"))?;
                encode_segment(&display(v), &mut out);
            }
            None => out.push_str(seg),
        }
    }
    let mut url = Url::parse(&out).map_err(|e| format!("bad template {out}: {e}"))?;
    {
        let mut pairs = url.query_pairs_mut();
        for kv in query.split('&').filter(|s| !s.is_empty()) {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            match hole_name(v) {
                Some(name) => {
                    let value = params.get(name).map(display).or_else(|| {
                        route
                            .holes
                            .iter()
                            .find(|h| h.name == name)
                            .map(|h| h.example.clone())
                    });
                    if let Some(value) = value {
                        pairs.append_pair(k, &value);
                    }
                }
                None => {
                    pairs.append_pair(k, v);
                }
            }
        }
        for (k, v) in params {
            if !route.holes.iter().any(|h| h.name == *k) {
                pairs.append_pair(k, &display(v));
            }
        }
    }
    if url.query() == Some("") {
        url.set_query(None);
    }
    Ok(url)
}

fn hole_name(s: &str) -> Option<&str> {
    s.strip_prefix('{').and_then(|s| s.strip_suffix('}'))
}

fn display(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn encode_segment(v: &str, out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for b in v.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(b as char);
            }
            _ => {
                out.push('%');
                out.push(HEX[usize::from(b >> 4)] as char);
                out.push(HEX[usize::from(b & 15)] as char);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ex(method: &str, url: &str, body: &str) -> Exchange {
        Exchange {
            url: url.to_owned(),
            method: method.to_owned(),
            resource_type: "Fetch".to_owned(),
            status: 200,
            request_headers: HashMap::from([("accept".to_owned(), "application/json".to_owned())]),
            response_headers: HashMap::new(),
            content_type: Some("application/json".to_owned()),
            body: Some(body.to_owned()),
            body_len: body.len(),
            truncated: false,
            ts: 0,
        }
    }

    const ITEMS: &str = r#"{"data":{"items":[{"id":1,"name":"a"},{"id":2,"name":"b"}]}}"#;

    #[test]
    fn nested_collection_admitted() {
        let out = classify_all(&[ex("get", "https://api.example.com/v1/things", ITEMS)]);
        assert_eq!(out.len(), 1);
        let r = &out[0].route;
        assert_eq!(r.collection, "/data/items");
        assert_eq!(r.records_seen, 2);
        assert_eq!(r.keys["id"], "number");
        assert_eq!(r.keys["name"], "string");
        assert_eq!(r.method, "GET");
        assert_eq!(r.name, "things");
        assert_eq!(r.id, "example.com:GET:/v1/things");
        assert!(r.replay);
    }

    #[test]
    fn xssi_prefix_admitted_from_xhr() {
        let mut e = ex("GET", "https://x.io/a", ")]}'\n[{\"a\":1},{\"a\":2}]");
        e.resource_type = "Xhr".to_owned();
        let out = classify_all(&[e]);
        assert_eq!(out[0].route.collection, "");
    }

    #[test]
    fn html_data_island_admitted() {
        let mut e = ex(
            "GET",
            "https://shop.example/p",
            r#"<html><script>var x=1</script><script type="application/json">{"props":{"list":[{"id":1},{"id":2}]}}</script></html>"#,
        );
        e.resource_type = "Document".to_owned();
        e.content_type = Some("text/html".to_owned());
        let out = classify_all(&[e]);
        assert_eq!(out[0].route.collection, "/0/props/list");
    }

    #[test]
    fn non_collections_rejected() {
        let mut none = ex("GET", "https://x.io/n", "");
        none.status = 204;
        none.body = None;
        let out = classify_all(&[
            ex(
                "GET",
                "https://x.io/api/main.js.map",
                r#"{"sources":["a.js","b.js"]}"#,
            ),
            ex("GET", "https://x.io/beacon", r#"{"ok":true}"#),
            none,
        ]);
        assert!(out.is_empty());
    }

    #[test]
    fn siblings_merge_into_one_template() {
        let out = classify_all(&[
            ex("GET", "https://x.io/users/123/posts", ITEMS),
            ex("GET", "https://x.io/users/456/posts", ITEMS),
            ex("GET", "https://x.io/repos/alpha/issues", ITEMS),
            ex("GET", "https://x.io/repos/beta/issues", ITEMS),
        ]);
        assert_eq!(out.len(), 2);
        let users = out
            .iter()
            .find(|l| l.route.name == "posts")
            .expect("users route");
        assert_eq!(users.route.template, "https://x.io/users/{id}/posts");
        assert_eq!(users.route.holes.len(), 1);
        assert_eq!(users.route.holes[0].example, "123");
        assert_eq!(users.route.holes[0].location, HoleIn::Path);
        let repos = out
            .iter()
            .find(|l| l.route.name == "issues")
            .expect("repos route");
        assert_eq!(repos.route.template, "https://x.io/repos/{id}/issues");
    }

    #[test]
    fn page_path_resource_id_pair_becomes_hole() {
        let mut page = ex("GET", "https://crates.io/crates/tokio", "<html></html>");
        page.resource_type = "Document".to_owned();
        page.content_type = Some("text/html".to_owned());
        let out = classify_all(&[
            page,
            ex(
                "GET",
                "https://crates.io/api/v1/crates/tokio/downloads",
                ITEMS,
            ),
        ]);
        let r = &out[0].route;
        assert_eq!(r.template, "https://crates.io/api/v1/crates/{id}/downloads");
        assert_eq!(r.holes[0].example, "tokio");
        // `crates` matches the page path too, but its predecessor `v1` does not.
        assert_eq!(r.holes.len(), 1);
    }

    #[test]
    fn query_holes_and_tracking_dropped() {
        let out = classify_all(&[ex("GET", "https://x.io/s?utm_source=x&page=1#frag", ITEMS)]);
        let r = &out[0].route;
        assert_eq!(r.template, "https://x.io/s?page={page}");
        assert_eq!(r.holes.len(), 1);
        assert_eq!(r.holes[0].name, "page");
        assert_eq!(r.holes[0].location, HoleIn::Query);
        assert_eq!(r.holes[0].example, "1");
    }

    #[test]
    fn auth_names_recorded_values_kept_out() {
        let mut e = ex("GET", "https://x.io/me", ITEMS);
        e.request_headers
            .insert("authorization".to_owned(), "Bearer s3cr3t".to_owned());
        e.request_headers
            .insert("cookie".to_owned(), "sid=c00kie".to_owned());
        e.request_headers
            .insert("sec-fetch-mode".to_owned(), "cors".to_owned());
        let out = classify_all(&[e]);
        let l = &out[0];
        assert_eq!(l.route.auth_headers, ["authorization", "cookie"]);
        assert!(l.route.cookies);
        assert!(
            l.creds
                .contains(&("authorization".to_owned(), "Bearer s3cr3t".to_owned()))
        );
        assert!(
            l.creds
                .contains(&("cookie".to_owned(), "sid=c00kie".to_owned()))
        );
        let json = serde_json::to_string(&l.route).expect("route serialises");
        assert!(!json.contains("s3cr3t"));
        assert!(!json.contains("c00kie"));
    }

    #[test]
    fn post_replayable_only_with_operation_in_url() {
        let out = classify_all(&[
            ex("POST", "https://x.io/graphql", ITEMS),
            ex("POST", "https://x.io/gql?operationName=Foo", ITEMS),
        ]);
        let plain = out
            .iter()
            .find(|l| l.route.name == "graphql")
            .expect("plain post");
        assert!(!plain.route.replay);
        let gql = out
            .iter()
            .find(|l| l.route.name == "Foo")
            .expect("graphql post");
        assert!(gql.route.replay);
    }

    #[test]
    fn registrable_domain_strips_subdomain() {
        let u = Url::parse("https://api.crates.io/x").expect("url");
        assert_eq!(registrable_domain(&u), "crates.io");
    }

    fn learned() -> Route {
        classify_all(&[ex("GET", "https://x.io/users/123/posts?page=1", ITEMS)])
            .remove(0)
            .route
    }

    #[test]
    fn drift_checks_pointer_and_keys() {
        let r = learned();
        let ok = json!({"data":{"items":[{"id":9,"name":"z"}]}});
        assert_eq!(drift(&r, &ok).expect("matching body").len(), 1);
        let moved = json!({"data":{"rows":[{"id":9,"name":"z"}]}});
        assert!(drift(&r, &moved).is_err());
        let disjoint = json!({"data":{"items":[{"error":"nope","code":1}]}});
        assert!(drift(&r, &disjoint).is_err());
    }

    #[test]
    fn fill_requires_path_defaults_query() {
        let r = learned();
        assert!(fill(&r, &Map::new()).is_err());
        let mut p = Map::new();
        p.insert("id".to_owned(), json!(7));
        assert_eq!(
            fill(&r, &p).expect("filled").as_str(),
            "https://x.io/users/7/posts?page=1"
        );
        p.insert("page".to_owned(), json!("2"));
        p.insert("q".to_owned(), json!("a b"));
        assert_eq!(
            fill(&r, &p).expect("filled").as_str(),
            "https://x.io/users/7/posts?page=2&q=a+b"
        );
    }
}
