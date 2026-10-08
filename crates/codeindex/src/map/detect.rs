//! Detectors (147 §5.2 bindings/process edges, §7 perspective eligibility):
//! indexed queries over `symbols.name`, `symbols.signature`,
//! `imports.specifier`, `files.path` and `refs.name` — never a file read.
//! Each hit carries a confidence (`import` 0.9, `name-pattern` 0.6) and the
//! perspective's eligibility is the §7 threshold. They are heuristics and
//! say so: stage A's model may still decline an eligible perspective.
//!
//! owner: W1 — design §5.2 (inferred edges), §5.3, §7

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

use super::MapEvidence;
use crate::walk;

/// omm's twelve perspectives (§7), catalogue order; `overall-architecture`
/// is always eligible and always chosen.
pub const PERSPECTIVES: [&str; 12] = [
    "overall-architecture",
    "request-lifecycle",
    "data-flow",
    "dependency-map",
    "external-integrations",
    "state-transitions",
    "route-page-map",
    "command-surface",
    "extension-points",
    "pipeline",
    "orchestration",
    "storage",
];

/// The flow perspectives that get one stage-D call per entry point (§6.2).
pub const FLOW_PERSPECTIVES: [&str; 4] = [
    "request-lifecycle",
    "data-flow",
    "pipeline",
    "orchestration",
];

/// Confidence of an import-specifier hit.
pub const CONFIDENCE_IMPORT: f64 = 0.9;
/// Confidence of a name-pattern hit.
pub const CONFIDENCE_NAME: f64 = 0.6;
/// Samples kept per detector.
pub const SAMPLES: usize = 3;
/// Rows one probe reads at most; eligibility never needs more.
const PROBE_LIMIT: usize = 1000;

/// One perspective's detector summary: the facts stage A receives and
/// `perspectives.eligible` stores (§5.3).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Detection {
    /// A [`PERSPECTIVES`] kind.
    pub kind: String,
    /// The §7 threshold is met.
    pub eligible: bool,
    /// Matching rows.
    pub hits: u32,
    /// `path#L`, ≤ [`SAMPLES`].
    pub samples: Vec<String>,
    /// Best confidence among the hits.
    pub confidence: f64,
}

/// A unit→unit edge a detector inferred (bindings, process/socket); drawn
/// solid with the detector named, unlike a model-inferred edge.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct DetectedEdge {
    /// Source unit path.
    pub from: String,
    /// Target unit path.
    pub to: String,
    /// `MapEvidence::Bindings` or `MapEvidence::Process`.
    pub evidence: MapEvidence,
}

/// The display title of a catalogue kind.
pub fn title(kind: &str) -> String {
    let mut out = String::with_capacity(kind.len());
    for (i, part) in kind.split('-').enumerate() {
        if i > 0 {
            out.push(' ');
        }
        if i == 0 {
            let mut c = part.chars();
            if let Some(f) = c.next() {
                out.extend(f.to_uppercase());
                out.push_str(c.as_str());
            }
        } else {
            out.push_str(part);
        }
    }
    out
}

/// `(path, line)` hits of one probe.
type Hits = Vec<(String, i64)>;

/// Distinct non-test import specifiers, read once per pass: the package
/// probes match them in Rust (one `LIKE` per package per row cost ~1.5 s on
/// theuth's 20k imports).
struct Specs(Vec<String>);

impl Specs {
    async fn load(pool: &SqlitePool) -> anyhow::Result<Self> {
        Ok(Self(
            sqlx::query_scalar(
                "SELECT DISTINCT i.specifier FROM imports i JOIN files f ON f.id = i.file_id WHERE f.is_test = 0",
            )
            .fetch_all(pool)
            .await?,
        ))
    }

    /// Specifiers naming one of `pkgs`: the package itself or a path under
    /// it (Rust `x::y`, npm `x/y`, Python `x.y`).
    fn matching(&self, pkgs: &[&str]) -> Vec<&str> {
        self.0
            .iter()
            .map(String::as_str)
            .filter(|s| pkgs.iter().any(|p| spec_matches(s, p)))
            .collect()
    }
}

fn spec_matches(spec: &str, pkg: &str) -> bool {
    spec == pkg
        || spec
            .strip_prefix(pkg)
            .is_some_and(|r| r.starts_with("::") || r.starts_with('/') || r.starts_with('.'))
}

/// Non-test parsed files only: fixture trees name their own frameworks.
const NOT_TEST: &str = "f.is_test = 0";

/// `(path, line)` of the import rows whose specifier names one of `pkgs`.
async fn imports_of(pool: &SqlitePool, specs: &Specs, pkgs: &[&str]) -> anyhow::Result<Hits> {
    let matched = specs.matching(pkgs);
    if matched.is_empty() {
        return Ok(Vec::new());
    }
    let mut q = sqlx::QueryBuilder::new(
        "SELECT f.path, i.line FROM imports i JOIN files f ON f.id = i.file_id WHERE f.is_test = 0 AND i.specifier IN (",
    );
    let mut sep = q.separated(", ");
    for s in &matched {
        sep.push_bind(*s);
    }
    q.push(format!(") LIMIT {PROBE_LIMIT}"));
    Ok(q.build_query_as().fetch_all(pool).await?)
}

/// Symbols whose `signature` contains any of `needles` (attribute macros,
/// decorators, `enum`).
async fn signatures_like(pool: &SqlitePool, needles: &[&str]) -> anyhow::Result<Hits> {
    let conds: Vec<String> = needles
        .iter()
        .map(|n| format!("s.signature LIKE '%{}%'", n.replace('\'', "''")))
        .collect();
    let sql = format!(
        "SELECT f.path, s.line_start FROM symbols s JOIN files f ON f.id = s.file_id WHERE {NOT_TEST} AND ({}) LIMIT {PROBE_LIMIT}",
        conds.join(" OR ")
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?)
}

/// References by name (`refs.name`), optionally restricted to a qualifier set.
async fn refs_named(
    pool: &SqlitePool,
    names: &[&str],
    qualifiers: Option<&[&str]>,
) -> anyhow::Result<Hits> {
    let list = |xs: &[&str]| {
        xs.iter()
            .map(|x| format!("'{}'", x.replace('\'', "''")))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let qual = match qualifiers {
        Some(q) => format!(" AND r.qualifier IN ({})", list(q)),
        None => String::new(),
    };
    let sql = format!(
        "SELECT f.path, r.line FROM refs r JOIN files f ON f.id = r.file_id WHERE {NOT_TEST} AND r.name IN ({}){qual} LIMIT {PROBE_LIMIT}",
        list(names)
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?)
}

/// Symbols by name pattern (`LIKE`, case-insensitive) and kind.
async fn symbols_like(
    pool: &SqlitePool,
    patterns: &[&str],
    kinds: Option<&[&str]>,
) -> anyhow::Result<Hits> {
    let conds: Vec<String> = patterns
        .iter()
        .map(|p| format!("s.name LIKE '{p}'"))
        .collect();
    let kinds = match kinds {
        Some(k) => format!(
            " AND s.kind IN ({})",
            k.iter()
                .map(|x| format!("'{x}'"))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        None => String::new(),
    };
    let sql = format!(
        "SELECT f.path, s.line_start FROM symbols s JOIN files f ON f.id = s.file_id WHERE {NOT_TEST} AND ({}){kinds} LIMIT {PROBE_LIMIT}",
        conds.join(" OR ")
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(pool)
        .await?)
}

/// Every parsed non-test path; the route and manifest probes filter in Rust.
async fn paths(pool: &SqlitePool) -> anyhow::Result<Vec<String>> {
    Ok(
        sqlx::query_scalar("SELECT path FROM files WHERE is_test = 0 ORDER BY path")
            .fetch_all(pool)
            .await?,
    )
}

/// UI route files (W4 (c)): `routes/**`, `app/**/page.tsx`, `pages/**`
/// (`.tsx|.jsx|.vue|.svelte`), outside tests and `node_modules`.
pub fn is_route_file(path: &str) -> bool {
    if walk::is_test_path(path) {
        return false;
    }
    let ui = path.ends_with(".tsx")
        || path.ends_with(".jsx")
        || path.ends_with(".vue")
        || path.ends_with(".svelte")
        || path.ends_with(".astro");
    if !ui {
        return false;
    }
    let segs: Vec<&str> = path.split('/').collect();
    let dirs = &segs[..segs.len().saturating_sub(1)];
    let file = segs.last().copied().unwrap_or("");
    dirs.contains(&"routes")
        || dirs.contains(&"pages")
        || (dirs.contains(&"app") && (file.starts_with("page.") || file.starts_with("layout.")))
}

/// Route files of the index, for `route-page-map`'s entry points (W4 (c)).
pub async fn route_files(pool: &SqlitePool) -> anyhow::Result<Vec<String>> {
    Ok(paths(pool)
        .await?
        .into_iter()
        .filter(|p| is_route_file(p))
        .collect())
}

fn detection(kind: &str, eligible: bool, hits: &Hits, confidence: f64) -> Detection {
    Detection {
        kind: kind.to_owned(),
        eligible,
        hits: u32::try_from(hits.len()).unwrap_or(u32::MAX),
        samples: hits
            .iter()
            .take(SAMPLES)
            .map(|(p, l)| format!("{p}#L{l}"))
            .collect(),
        confidence: if hits.is_empty() { 0.0 } else { confidence },
    }
}

/// Two hit lists, the import one first (its confidence wins the summary).
fn merged(imports: Hits, names: Hits) -> (Hits, f64) {
    let confidence = if !imports.is_empty() {
        CONFIDENCE_IMPORT
    } else if !names.is_empty() {
        CONFIDENCE_NAME
    } else {
        0.0
    };
    let mut all = imports;
    all.extend(names);
    (all, confidence)
}

const HTTP_SERVERS: &[&str] = &[
    "axum",
    "actix_web",
    "actix-web",
    "warp",
    "rocket",
    "tide",
    "poem",
    "salvo",
    "jsonrpsee",
    "tonic",
    "express",
    "fastify",
    "koa",
    "hono",
    "@hapi/hapi",
    "next/server",
    "elysia",
    "h3",
    "nitro",
    "fastapi",
    "flask",
    "django",
    "starlette",
    "aiohttp",
    "sanic",
    "tornado",
    "net/http",
    "github.com/gin-gonic/gin",
    "github.com/labstack/echo",
    "github.com/gorilla/mux",
];
const STORAGE_CLIENTS: &[&str] = &[
    "sqlx",
    "rusqlite",
    "diesel",
    "sea_orm",
    "sea-orm",
    "redb",
    "sled",
    "rocksdb",
    "tokio_postgres",
    "redis",
    "mongodb",
    "aws_sdk_s3",
    "aws-sdk-s3",
    "object_store",
    "yrs",
    "better-sqlite3",
    "sqlite3",
    "sqlite",
    "pg",
    "postgres",
    "mysql2",
    "mongoose",
    "prisma",
    "@prisma/client",
    "drizzle-orm",
    "knex",
    "typeorm",
    "sequelize",
    "ioredis",
    "@aws-sdk/client-s3",
    "yjs",
    "@automerge/automerge",
    "level",
    "lowdb",
    "sqlalchemy",
    "psycopg2",
    "psycopg",
    "asyncpg",
    "pymongo",
    "motor",
    "boto3",
    "aiosqlite",
    "peewee",
    "tortoise",
    "database/sql",
    "github.com/jackc/pgx",
    "gorm.io/gorm",
    "go.etcd.io/bbolt",
];
const HTTP_CLIENTS: &[&str] = &[
    "reqwest",
    "ureq",
    "hyper",
    "isahc",
    "surf",
    "octocrab",
    "axios",
    "node-fetch",
    "undici",
    "got",
    "ky",
    "superagent",
    "cross-fetch",
    "requests",
    "httpx",
    "aiohttp",
    "urllib3",
    "urllib.request",
];
const SERIALIZERS: &[&str] = &[
    "serde",
    "serde_json",
    "serde_yaml",
    "toml",
    "bincode",
    "prost",
    "quick_xml",
    "zod",
    "yup",
    "io-ts",
    "valibot",
    "superstruct",
    "ajv",
    "protobufjs",
    "yaml",
    "js-yaml",
    "pydantic",
    "marshmallow",
    "attrs",
    "dataclasses_json",
    "msgspec",
    "encoding/json",
    "google.golang.org/protobuf",
];
const SDKS: &[&str] = &[
    "octocrab",
    "aws_sdk_s3",
    "aws_config",
    "stripe",
    "openai",
    "async_openai",
    "anthropic",
    "google_cloud",
    "@anthropic-ai/sdk",
    "openai",
    "@octokit/rest",
    "@octokit/core",
    "stripe",
    "twilio",
    "@sendgrid/mail",
    "googleapis",
    "@aws-sdk/client-s3",
    "@slack/web-api",
    "discord.js",
    "firebase",
    "firebase-admin",
    "@supabase/supabase-js",
    "anthropic",
    "openai",
    "boto3",
    "google.cloud",
    "slack_sdk",
    "github",
    "stripe",
    "twilio",
    "sentry_sdk",
];
const CLI_PARSERS: &[&str] = &[
    "clap",
    "argh",
    "structopt",
    "pico_args",
    "lexopt",
    "commander",
    "yargs",
    "cac",
    "meow",
    "oclif",
    "@oclif/core",
    "citty",
    "cmd-ts",
    "clipanion",
    "argparse",
    "click",
    "typer",
    "docopt",
    "fire",
    "github.com/spf13/cobra",
    "github.com/urfave/cli",
    "flag",
];
const BUSES: &[&str] = &[
    "rdkafka",
    "lapin",
    "async_nats",
    "nats",
    "redis",
    "tokio::sync::broadcast",
    "kafkajs",
    "amqplib",
    "nats",
    "ioredis",
    "bullmq",
    "bull",
    "eventemitter3",
    "mitt",
    "events",
    "aiokafka",
    "kafka",
    "pika",
    "aio_pika",
    "nats",
    "celery",
    "redis",
    "dramatiq",
    "github.com/segmentio/kafka-go",
    "github.com/nats-io/nats.go",
];

/// Run the twelve perspective detectors over an index pool; one
/// [`Detection`] per [`PERSPECTIVES`] entry, catalogue order.
pub async fn perspectives(pool: &SqlitePool) -> anyhow::Result<Vec<Detection>> {
    let all_paths = paths(pool).await?;
    let specs = Specs::load(pool).await?;
    let mut out = Vec::with_capacity(PERSPECTIVES.len());

    // overall-architecture: always; hits = parsed files.
    let files: Hits = all_paths.iter().map(|p| (p.clone(), 1)).collect();
    out.push(detection("overall-architecture", true, &files, 1.0));

    // request-lifecycle: ≥ 1 route/RPC handler.
    let (hits, c) = merged(imports_of(pool, &specs, HTTP_SERVERS).await?, {
        let mut h = signatures_like(
            pool,
            &[
                "#[method(",
                "#[get(",
                "#[post(",
                "#[put(",
                "#[delete(",
                "#[route(",
                "@app.get",
                "@app.post",
                "@app.route",
                "@router.",
                "@api_view",
                "@bp.route",
            ],
        )
        .await?;
        h.extend(
            refs_named(
                pool,
                &["get", "post", "put", "delete", "patch", "route", "all"],
                Some(&[
                    "app",
                    "router",
                    "Router",
                    "server",
                    "api",
                    "bp",
                    "blueprint",
                ]),
            )
            .await?,
        );
        h
    });
    out.push(detection("request-lifecycle", !hits.is_empty(), &hits, c));

    // data-flow: ≥ 2 of storage client, HTTP client, serializer.
    let storage = imports_of(pool, &specs, STORAGE_CLIENTS).await?;
    let http = imports_of(pool, &specs, HTTP_CLIENTS).await?;
    let ser = imports_of(pool, &specs, SERIALIZERS).await?;
    let categories = [&storage, &http, &ser]
        .iter()
        .filter(|h| !h.is_empty())
        .count();
    let mut hits = storage.clone();
    hits.extend(http.iter().cloned());
    hits.extend(ser.iter().cloned());
    out.push(detection(
        "data-flow",
        categories >= 2,
        &hits,
        CONFIDENCE_IMPORT,
    ));

    // dependency-map: ≥ 3 workspace members or ≥ 10 units — read as indexed
    // non-test manifests below the root (a member each), else ≥ 10 top-level
    // directories holding parsed files.
    let manifests: Hits = all_paths
        .iter()
        .filter(|p| {
            p.contains('/')
                && (p.ends_with("/Cargo.toml")
                    || p.ends_with("/package.json")
                    || p.ends_with("/pyproject.toml")
                    || p.ends_with("/go.mod"))
                && !walk::is_test_path(p)
        })
        .map(|p| (p.clone(), 1))
        .collect();
    let top_dirs: std::collections::BTreeSet<&str> = all_paths
        .iter()
        .filter_map(|p| p.split_once('/').map(|(d, _)| d))
        .collect();
    out.push(detection(
        "dependency-map",
        manifests.len() >= 3 || top_dirs.len() >= 10,
        &manifests,
        1.0,
    ));

    // external-integrations: ≥ 1 HTTP client or SDK import.
    let mut hits = imports_of(pool, &specs, SDKS).await?;
    hits.extend(http.iter().cloned());
    out.push(detection(
        "external-integrations",
        !hits.is_empty(),
        &hits,
        CONFIDENCE_IMPORT,
    ));

    // state-transitions: an enum named *State|*Status|*Phase referenced ≥ 3 times.
    let enums = symbols_like(
        pool,
        &["%State", "%Status", "%Phase", "%Stage"],
        Some(&["type", "class", "interface"]),
    )
    .await?;
    let enum_names: Vec<String> = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT s.name FROM symbols s JOIN files f ON f.id = s.file_id \
         WHERE f.is_test = 0 AND s.kind IN ('type', 'class', 'interface') \
         AND (s.name LIKE '%State' OR s.name LIKE '%Status' OR s.name LIKE '%Phase' OR s.name LIKE '%Stage') \
         AND (s.signature LIKE '%enum %' OR s.signature LIKE '%Enum%' OR s.signature LIKE '%Literal[%' OR s.signature LIKE '%|%') LIMIT 50",
    )
    .fetch_all(pool)
    .await?;
    let mut matched = 0usize;
    if !enum_names.is_empty() {
        // References per enum name (match/switch sites, constructions).
        let mut q = sqlx::QueryBuilder::new(
            "SELECT r.name, count(*) FROM refs r JOIN files f ON f.id = r.file_id WHERE f.is_test = 0 AND r.name IN (",
        );
        let mut sep = q.separated(", ");
        for n in &enum_names {
            sep.push_bind(n);
        }
        q.push(") GROUP BY r.name");
        let rows: Vec<(String, i64)> = q.build_query_as().fetch_all(pool).await?;
        matched = rows.iter().filter(|(_, c)| *c >= 3).count();
    }
    out.push(detection(
        "state-transitions",
        matched >= 1,
        &enums,
        CONFIDENCE_NAME,
    ));

    // route-page-map: ≥ 3 UI route files.
    let routes: Hits = all_paths
        .iter()
        .filter(|p| is_route_file(p))
        .map(|p| (p.clone(), 1))
        .collect();
    out.push(detection(
        "route-page-map",
        routes.len() >= 3,
        &routes,
        CONFIDENCE_NAME,
    ));

    // command-surface: a CLI parser import and ≥ 3 subcommand sites.
    let parser = imports_of(pool, &specs, CLI_PARSERS).await?;
    let mut commands = signatures_like(
        pool,
        &[
            "#[command",
            "#[derive(Parser",
            "#[derive(Subcommand",
            "#[clap(",
            "@click.command",
            "@app.command",
            "@cli.command",
        ],
    )
    .await?;
    commands.extend(
        refs_named(
            pool,
            &[
                "command",
                "subcommand",
                "add_parser",
                "add_command",
                "AddCommand",
            ],
            None,
        )
        .await?,
    );
    let (hits, c) = merged(parser.clone(), commands.clone());
    out.push(detection(
        "command-surface",
        !parser.is_empty() && commands.len() >= 3,
        &hits,
        c,
    ));

    // extension-points: a registry pattern — ≥ 3 registration sites.
    let registry = refs_named(
        pool,
        &[
            "register",
            "register_tool",
            "registerTool",
            "defineComponent",
            "define_component",
            "add_plugin",
            "addPlugin",
            "use_plugin",
            "register_plugin",
            "registerPlugin",
            "define",
            "extend",
        ],
        None,
    )
    .await?;
    let traits = symbols_like(
        pool,
        &[
            "Plugin",
            "%Plugin",
            "Extension",
            "%Extension",
            "Tool",
            "%Provider",
            "Registry",
            "%Registry",
        ],
        Some(&["trait", "interface", "class", "type"]),
    )
    .await?;
    let (hits, c) = merged(Vec::new(), {
        let mut h = registry.clone();
        h.extend(traits);
        h
    });
    out.push(detection("extension-points", registry.len() >= 3, &hits, c));

    // pipeline: ≥ 3 symbols named stage|step|pipeline|phase.
    let stages = symbols_like(
        pool,
        &[
            "%Stage%",
            "%Step%",
            "%Pipeline%",
            "%Phase%",
            "%stage%",
            "%step%",
            "%pipeline%",
        ],
        None,
    )
    .await?;
    out.push(detection(
        "pipeline",
        stages.len() >= 3,
        &stages,
        CONFIDENCE_NAME,
    ));

    // orchestration: publish/subscribe pairs or a bus import.
    let bus = imports_of(pool, &specs, BUSES).await?;
    let publishers = refs_named(
        pool,
        &["publish", "emit", "send_event", "dispatch", "broadcast"],
        None,
    )
    .await?;
    let subscribers = refs_named(
        pool,
        &[
            "subscribe",
            "on",
            "addEventListener",
            "listen",
            "consume",
            "subscribe_events",
        ],
        None,
    )
    .await?;
    let pairs = !publishers.is_empty() && !subscribers.is_empty();
    let (hits, c) = merged(bus.clone(), {
        let mut h = publishers;
        h.extend(subscribers);
        h
    });
    out.push(detection(
        "orchestration",
        pairs || !bus.is_empty(),
        &hits,
        c,
    ));

    // storage: ≥ 2 storage clients (distinct packages: the matched
    // specifiers cut to their first segment — `@scope/name` keeps two).
    let distinct: std::collections::BTreeSet<String> = specs
        .matching(STORAGE_CLIENTS)
        .into_iter()
        .map(|s| {
            s.split("::")
                .next()
                .unwrap_or(s)
                .split('/')
                .take(if s.starts_with('@') { 2 } else { 1 })
                .collect::<Vec<_>>()
                .join("/")
                .split('.')
                .next()
                .unwrap_or("")
                .to_owned()
        })
        .collect();
    out.push(detection(
        "storage",
        distinct.len() >= 2,
        &storage,
        CONFIDENCE_IMPORT,
    ));

    debug_assert_eq!(out.len(), PERSPECTIVES.len());
    Ok(out)
}

/// The deepest of `units` that is `path` itself or a directory prefix of it.
fn unit_of<'a>(units: &'a [String], path: &str) -> Option<&'a str> {
    units
        .iter()
        .filter(|u| {
            path == u.as_str()
                || path
                    .strip_prefix(u.as_str())
                    .is_some_and(|r| r.starts_with('/'))
        })
        .max_by_key(|u| u.len())
        .map(String::as_str)
}

/// The generated-bindings and process/socket edges of §5.2 between the given
/// unit paths (root-relative, no trailing slash).
///
/// - **Bindings**: a unit holding a ts-rs exporter (`export.rs`, a `ts_rs`
///   import or an `export_bindings` symbol) → every unit importing from a
///   `bindings/` path (the output is gitignored, so the import specifier is
///   the only trace of it in the index).
/// - **Process**: a unit that dials (`connect`, `WebSocket`, `dial`) → every
///   unit that listens (`listen`, `bind`, `serve`); a unit that spawns
///   (`Command`, `spawn`) → units owning a binary entry point (`main.rs`,
///   `main.py`, `cli.*`, `bin/`), when there are at most three such units.
pub async fn inferred_edges(
    pool: &SqlitePool,
    units: &[String],
) -> anyhow::Result<Vec<DetectedEdge>> {
    let mut out: Vec<DetectedEdge> = Vec::new();
    if units.is_empty() {
        return Ok(out);
    }
    let mut seen = std::collections::HashSet::new();
    let mut push = |from: &str, to: &str, ev: MapEvidence| {
        if from != to && seen.insert((from.to_owned(), to.to_owned(), std::mem::discriminant(&ev)))
        {
            out.push(DetectedEdge {
                from: from.to_owned(),
                to: to.to_owned(),
                evidence: ev,
            });
        }
    };

    // Bindings.
    let exporters: Hits = {
        let mut h = imports_of(pool, &Specs::load(pool).await?, &["ts_rs"]).await?;
        h.extend(symbols_like(pool, &["export_bindings", "export_bindings_%"], None).await?);
        h.extend(
            sqlx::query_as::<_, (String, i64)>(
                "SELECT path, 1 FROM files WHERE is_test = 0 AND (path LIKE '%/export.rs' OR path LIKE '%/bindings.rs')",
            )
            .fetch_all(pool)
            .await?,
        );
        h
    };
    let consumers: Vec<(String, String)> = sqlx::query_as(
        "SELECT f.path, i.specifier FROM imports i JOIN files f ON f.id = i.file_id \
         WHERE i.specifier LIKE '%bindings/%' OR i.specifier LIKE '%/bindings' LIMIT 2000",
    )
    .fetch_all(pool)
    .await?;
    for (ep, _) in &exporters {
        let Some(from) = unit_of(units, ep) else {
            continue;
        };
        for (cp, spec) in &consumers {
            let Some(to) = unit_of(units, cp) else {
                continue;
            };
            // `../rpc/bindings/Foo` → `<consumer dir>/../rpc/bindings`, normalised.
            let dir = spec.rsplit_once('/').map_or(spec.as_str(), |(d, _)| d);
            let output_dir = normalise(&format!(
                "{}/{dir}",
                cp.rsplit_once('/').map_or("", |(d, _)| d)
            ));
            push(
                from,
                to,
                MapEvidence::Bindings {
                    generator: "ts-rs".to_owned(),
                    output_dir,
                },
            );
        }
    }

    // Sockets: dial → listen. Names a storage client also uses (`connect`,
    // `bind`) are left out: sqlx's `.bind()` would make every store a server.
    let dials = refs_named(
        pool,
        &[
            "WebSocket",
            "dial",
            "Dial",
            "connect_async",
            "TcpStream",
            "UnixStream",
        ],
        None,
    )
    .await?;
    let listens = refs_named(
        pool,
        &[
            "listen",
            "serve",
            "TcpListener",
            "UnixListener",
            "Listen",
            "run_server",
        ],
        None,
    )
    .await?;
    let mut listen_units: BTreeMap<&str, (String, i64)> = BTreeMap::new();
    for (p, l) in &listens {
        if let Some(u) = unit_of(units, p) {
            listen_units.entry(u).or_insert_with(|| (p.clone(), *l));
        }
    }
    let mut dial_units: BTreeMap<&str, (String, i64)> = BTreeMap::new();
    for (p, l) in &dials {
        if let Some(u) = unit_of(units, p) {
            dial_units.entry(u).or_insert_with(|| (p.clone(), *l));
        }
    }
    for (from, (site, line)) in &dial_units {
        for to in listen_units.keys() {
            push(
                from,
                to,
                MapEvidence::Process {
                    site: format!("{site}#L{line}"),
                    verb: "dial".to_owned(),
                },
            );
        }
    }

    // Processes: spawn → binary units (≤ 3 candidates, else too ambiguous).
    let spawns = refs_named(
        pool,
        &[
            "Command",
            "spawn",
            "spawn_blocking_process",
            "execa",
            "subprocess",
            "Popen",
            "exec",
            "execFile",
        ],
        Some(&[
            "Command",
            "process",
            "child_process",
            "subprocess",
            "std",
            "tokio",
            "os",
        ]),
    )
    .await?;
    let bin_units: Vec<&str> = {
        let bins: Vec<String> = sqlx::query_scalar(
            "SELECT path FROM files WHERE is_test = 0 AND (path LIKE '%/main.rs' OR path LIKE '%/main.py' OR path = 'main.py' \
             OR path LIKE '%/cli.ts' OR path LIKE '%/cli.js' OR path LIKE '%/cli.py' OR path LIKE '%/bin/%' OR path LIKE '%/__main__.py')",
        )
        .fetch_all(pool)
        .await?;
        let mut us: Vec<&str> = bins.iter().filter_map(|p| unit_of(units, p)).collect();
        us.sort_unstable();
        us.dedup();
        us
    };
    if !bin_units.is_empty() && bin_units.len() <= 3 {
        let mut spawn_units: BTreeMap<&str, (String, i64)> = BTreeMap::new();
        for (p, l) in &spawns {
            if let Some(u) = unit_of(units, p) {
                spawn_units.entry(u).or_insert_with(|| (p.clone(), *l));
            }
        }
        for (from, (site, line)) in &spawn_units {
            for to in &bin_units {
                push(
                    from,
                    to,
                    MapEvidence::Process {
                        site: format!("{site}#L{line}"),
                        verb: "spawn".to_owned(),
                    },
                );
            }
        }
    }
    Ok(out)
}

/// Collapse `.` and `..` segments of a forward-slash path.
fn normalise(p: &str) -> String {
    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }
    out.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_routes_and_paths() {
        assert_eq!(title("overall-architecture"), "Overall architecture");
        assert_eq!(title("route-page-map"), "Route page map");
        assert!(is_route_file("crates/desktop/ui/src/routes/codemap.tsx"));
        assert!(is_route_file("app/dashboard/page.tsx"));
        assert!(is_route_file("src/pages/index.vue"));
        assert!(!is_route_file("src/routes/index.test.tsx"));
        assert!(!is_route_file("src/lib/routes.ts"));
        assert_eq!(
            normalise("crates/desktop/ui/src/lib/../rpc/bindings"),
            "crates/desktop/ui/src/rpc/bindings"
        );
        let units = [
            "crates/rpc".to_owned(),
            "crates/desktop".to_owned(),
            "crates/desktop/ui".to_owned(),
        ];
        assert_eq!(
            unit_of(&units, "crates/desktop/ui/src/a.ts"),
            Some("crates/desktop/ui")
        );
        assert_eq!(
            unit_of(&units, "crates/desktop/src/main.rs"),
            Some("crates/desktop")
        );
        assert_eq!(unit_of(&units, "crates/rpcx/src/a.rs"), None);
    }
}
