//! 159 W7 — the mechanism as data (D13): `protocol.md`, `fire.md`, `advisor.md` and
//! `mechanism.yaml` in the plugin-owned git repo `<root>/mechanism/`, validated against a
//! compiled schema. The gate, scoring, custody, budgets, holdout handling, the toolset, the
//! plugin prompt block and the skill are not in it (159 §12) and `mechanism.yaml` refuses
//! any key it does not know.
//!
//! Versions live in `<root>/mechanism.json`: v1 is the seeded export of the compiled
//! defaults; a finished meta study registers its final mechanism as the next version
//! ([`register`]), and `autoresearch.mechanism {op: adopt}` — a press — makes one active.
//! `active: None` = the compiled defaults, so a 140 study reads exactly today's texts.
//!
//! Readers ([`protocol`], [`fire_prompt`], [`advisor_prompt`]) take the study's pinned sha,
//! or `None` for the active version; a missing or invalid mechanism falls back to the
//! compiled default with a `tracing::warn!` line (the readers are sync and have no store
//! for an incident row).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::AutoresearchConfig;
use crate::error::{Error, Result};
use crate::types::{MechanismInfo, MechanismOp, MechanismVersion};

/// `<root>/mechanism/`: the repo.
pub const DIR: &str = "mechanism";
/// `<root>/mechanism.json`: the version index (outside the repo, so it never dirties it).
pub const INDEX: &str = "mechanism.json";
/// The files of a mechanism, in the order the Study view lists them.
pub const FILES: [&str; 4] = ["protocol.md", "fire.md", "advisor.md", "mechanism.yaml"];
/// Each text file is at most this long.
pub const MAX_TEXT: usize = 8 * 1024;
/// `plateau.window` range.
pub const PLATEAU_RANGE: (u32, u32) = (4, 48);
/// `novelty.threshold` range.
pub const NOVELTY_RANGE: (f64, f64) = (0.3, 0.95);
/// `defaults.crash_retries` upper bound.
pub const MAX_CRASH_RETRIES: u8 = 5;
/// At most this many rotation categories, each at most 32 characters.
pub const MAX_CATEGORIES: usize = 12;

/// The compiled `brief::protocol` text with its one placeholder (brief.rs `fn protocol`).
pub const PROTOCOL: &str = "Protocol:\n\
1. One idea per experiment. Read what you need, make one coherent change in the worktree, test it quickly if you can.\n\
2. Call experiment_run(idea, category, hypothesis). The harness measures; it may take minutes. Do not run the benchmark yourself for scoring.\n\
3. Call experiment_log(description, learned, next) with what you did and what the result teaches. You may ask for decision \"discard\" if you distrust a gain, or \"keep_simpler\" if the change simplifies code at equal performance.\n\
4. After a crash you may fix and call experiment_run again (at most {crash_retries} times), then log.\n\
5. Never edit evaluator files, never run git commands that write, never stop to ask: nobody is watching this session. Ideas for later go to experiment_notes.";

/// The compiled fire prompt (schedule_file.rs `text`), `{slug}` filled by the caller.
pub const FIRE: &str = "Run the next experiment of autoresearch study `{slug}`. Call experiment_begin() first and follow the protocol in the brief it returns. Do not ask questions; nobody is watching this session.\n";

/// The compiled advisor system prompt (review.rs `ADVISOR_SYSTEM`).
pub const ADVISOR: &str = "You advise an automated optimisation loop that has plateaued. Answer only with the requested lines.";

/// A loaded, validated mechanism.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Mechanism {
    /// The pinned git sha of `<root>/mechanism/` (`""` for the compiled defaults).
    pub sha: String,
    /// `mechanism.yaml`'s `version`.
    pub version: u32,
    /// `protocol.md`, with `{crash_retries}` still in it.
    pub protocol: String,
    /// `fire.md`, with `{slug}` still in it.
    pub fire: String,
    /// `advisor.md`.
    pub advisor: String,
    /// `mechanism.yaml`.
    pub params: Params,
}

/// `mechanism.yaml`. Every struct refuses unknown keys: there is no key for the gate,
/// scoring, custody or budgets, so `gate: {z: 1.0}` is refused as unknown (159 §12 Guard).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Params {
    /// Bumped by the outer loop when it changes anything.
    pub version: u32,
    /// The brief's caps (`config.rs` `BriefConfig`), each at most the compiled default.
    pub brief: BriefCaps,
    /// `plateau.window`.
    pub plateau: Plateau,
    /// `novelty.threshold`.
    pub novelty: Novelty,
    /// `defaults.crash_retries`: fills the protocol's `{crash_retries}`.
    pub defaults: Defaults,
    /// Preferred category rotation for wave slots (wave.rs `assign_categories`); empty =
    /// the categories seen in the ledger, as today.
    pub categories: Vec<String>,
}

/// The brief's caps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct BriefCaps {
    /// The brief's byte cap.
    pub max_bytes: u32,
    /// Recent experiments shown.
    pub recent: u32,
    /// Dead ends shown.
    pub dead_ends: u32,
    /// Open ideas shown.
    pub ideas: u32,
    /// The playbook's byte cap.
    pub playbook_bytes: u32,
}

/// `plateau:`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Plateau {
    /// Experiments without a keep before explore mode.
    pub window: u32,
}

/// `novelty:`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Novelty {
    /// Similarity above which a past discarded experiment counts as the same idea.
    pub threshold: f64,
}

/// `defaults:`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Defaults {
    /// Crash retries the protocol text allows.
    pub crash_retries: u8,
}

impl Default for BriefCaps {
    fn default() -> Self {
        let b = crate::config::BriefConfig::default();
        Self {
            max_bytes: b.max_bytes,
            recent: b.recent,
            dead_ends: b.dead_ends,
            ideas: b.ideas,
            playbook_bytes: b.playbook_bytes,
        }
    }
}

impl Default for Plateau {
    fn default() -> Self {
        Self {
            window: crate::config::Plateau::default().window,
        }
    }
}

impl Default for Novelty {
    fn default() -> Self {
        Self {
            threshold: crate::config::Novelty::default().threshold,
        }
    }
}

impl Default for Defaults {
    fn default() -> Self {
        Self {
            crash_retries: crate::config::Defaults::default().crash_retries,
        }
    }
}

impl Default for Params {
    fn default() -> Self {
        Self {
            version: 1,
            brief: BriefCaps::default(),
            plateau: Plateau::default(),
            novelty: Novelty::default(),
            defaults: Defaults::default(),
            categories: Vec::new(),
        }
    }
}

/// The version index, `<root>/mechanism.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
struct Index {
    /// The adopted version; `None` = the compiled defaults.
    active: Option<u32>,
    /// Oldest first on disk.
    versions: Vec<MechanismVersion>,
}

/// The compiled defaults as a mechanism (`sha` empty, version 1).
pub fn defaults() -> Mechanism {
    Mechanism {
        sha: String::new(),
        version: 1,
        protocol: PROTOCOL.to_owned(),
        fire: FIRE.to_owned(),
        advisor: ADVISOR.to_owned(),
        params: Params::default(),
    }
}

/// The four files of `m`, file name → text (the export; `parse(&export(&m))` is `m`).
pub fn export(m: &Mechanism) -> BTreeMap<String, String> {
    let yaml = serde_yaml::to_string(&m.params).unwrap_or_default();
    [
        ("protocol.md", m.protocol.clone()),
        ("fire.md", m.fire.clone()),
        ("advisor.md", m.advisor.clone()),
        ("mechanism.yaml", yaml),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect()
}

/// Validate four files into a mechanism (`sha` left empty). Unknown keys, out-of-range
/// numbers, over-long or empty texts are refused with the reason.
pub fn parse(files: &BTreeMap<String, String>) -> std::result::Result<Mechanism, String> {
    let text = |name: &str| -> std::result::Result<String, String> {
        let t = files.get(name).ok_or_else(|| format!("{name}: missing"))?;
        if t.len() > MAX_TEXT {
            return Err(format!("{name}: {} bytes, at most {MAX_TEXT}", t.len()));
        }
        if t.trim().is_empty() {
            return Err(format!("{name}: empty"));
        }
        Ok(t.clone())
    };
    let protocol = text("protocol.md")?;
    let fire = text("fire.md")?;
    let advisor = text("advisor.md")?;
    let yaml = text("mechanism.yaml")?;
    let params: Params = serde_yaml::from_str(&yaml).map_err(|e| format!("mechanism.yaml: {e}"))?;
    check_params(&params)?;
    Ok(Mechanism {
        sha: String::new(),
        version: params.version,
        protocol,
        fire,
        advisor,
        params,
    })
}

fn check_params(p: &Params) -> std::result::Result<(), String> {
    if p.version == 0 {
        return Err("mechanism.yaml: version must be at least 1".into());
    }
    let caps = BriefCaps::default();
    for (name, v, max) in [
        ("brief.max_bytes", p.brief.max_bytes, caps.max_bytes),
        ("brief.recent", p.brief.recent, caps.recent),
        ("brief.dead_ends", p.brief.dead_ends, caps.dead_ends),
        ("brief.ideas", p.brief.ideas, caps.ideas),
        (
            "brief.playbook_bytes",
            p.brief.playbook_bytes,
            caps.playbook_bytes,
        ),
    ] {
        if v == 0 || v > max {
            return Err(format!("mechanism.yaml: {name} {v} is outside 1..={max}"));
        }
    }
    let (lo, hi) = PLATEAU_RANGE;
    if !(lo..=hi).contains(&p.plateau.window) {
        return Err(format!(
            "mechanism.yaml: plateau.window {} is outside {lo}..={hi}",
            p.plateau.window
        ));
    }
    let (lo, hi) = NOVELTY_RANGE;
    if !(p.novelty.threshold >= lo && p.novelty.threshold <= hi) {
        return Err(format!(
            "mechanism.yaml: novelty.threshold {} is outside {lo}..={hi}",
            p.novelty.threshold
        ));
    }
    if p.defaults.crash_retries > MAX_CRASH_RETRIES {
        return Err(format!(
            "mechanism.yaml: defaults.crash_retries {} is above {MAX_CRASH_RETRIES}",
            p.defaults.crash_retries
        ));
    }
    if p.categories.len() > MAX_CATEGORIES {
        return Err(format!(
            "mechanism.yaml: {} categories, at most {MAX_CATEGORIES}",
            p.categories.len()
        ));
    }
    if let Some(c) = p
        .categories
        .iter()
        .find(|c| c.trim().is_empty() || c.len() > 32)
    {
        return Err(format!(
            "mechanism.yaml: category `{c}` is empty or over 32 chars"
        ));
    }
    Ok(())
}

/// `<root>/mechanism/`.
pub fn repo(root: &Path) -> PathBuf {
    root.join(DIR)
}

fn index_path(root: &Path) -> PathBuf {
    root.join(INDEX)
}

fn read_index(root: &Path) -> Index {
    std::fs::read(index_path(root))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

fn write_index(root: &Path, ix: &Index) -> Result<()> {
    std::fs::create_dir_all(root)?;
    platform::files::write_atomic(&index_path(root), &serde_json::to_vec_pretty(ix)?)?;
    Ok(())
}

/// Create `<root>/mechanism/` on first use: the compiled defaults exported, committed, and
/// registered as v1 ("compiled defaults"). Returns the repo path. Never called by a reader,
/// so a 140 study creates nothing.
pub fn ensure(root: &Path) -> Result<PathBuf> {
    let repo = repo(root);
    if repo.join(".git").exists() {
        return Ok(repo);
    }
    std::fs::create_dir_all(&repo)?;
    for (name, text) in export(&defaults()) {
        std::fs::write(repo.join(name), text)?;
    }
    crate::git::plumbing(&repo, &["init", "-q"])?;
    crate::git::plumbing(&repo, &["add", "-A"])?;
    crate::git::plumbing(
        &repo,
        &["commit", "-q", "-m", "mechanism v1: compiled defaults"],
    )?;
    let sha = crate::git::head(&repo)?;
    let mut ix = read_index(root);
    if ix.versions.is_empty() {
        ix.versions.push(MechanismVersion {
            version: 1,
            sha,
            at: crate::store::now(),
            summary: "compiled defaults".into(),
        });
        write_index(root, &ix)?;
    }
    Ok(repo)
}

/// The four files at `sha` in the repo (`git show`), from any commit in its object store —
/// the worktree snapshots of [`snapshot`] included.
pub fn files_at(root: &Path, sha: &str) -> Result<BTreeMap<String, String>> {
    let repo = repo(root);
    if !repo.join(".git").exists() {
        return Err(Error::refused(format!(
            "no mechanism repo at {}",
            repo.display()
        )));
    }
    let mut out = BTreeMap::new();
    for name in FILES {
        let text = crate::git::read(&repo, &["show", &format!("{sha}:{name}")])?;
        out.insert(name.to_owned(), text);
    }
    Ok(out)
}

/// The active version `(version, sha)`; `None` = the compiled defaults.
pub fn active(root: &Path) -> Option<(u32, String)> {
    let ix = read_index(root);
    let v = ix.active?;
    ix.versions
        .iter()
        .find(|m| m.version == v)
        .map(|m| (m.version, m.sha.clone()))
}

/// Load the mechanism pinned at `sha` (or the active one when `None`), validated.
/// `None` with nothing adopted is refused ("compiled defaults"), which the readers treat as
/// the silent fallback.
pub fn load(root: &Path, sha: Option<&str>) -> Result<Mechanism> {
    let sha = match sha {
        Some(s) => s.to_owned(),
        None => {
            active(root)
                .ok_or_else(|| Error::refused("no mechanism adopted: compiled defaults"))?
                .1
        }
    };
    let files = files_at(root, &sha)?;
    let mut m = parse(&files).map_err(Error::refused)?;
    m.sha = sha;
    Ok(m)
}

/// The readers' shared body: `None` = nothing pinned or adopted (silent); an invalid or
/// missing mechanism warns once per call and falls back too.
fn resolve(root: &Path, sha: Option<&str>) -> Option<Mechanism> {
    if sha.is_none() && active(root).is_none() {
        return None;
    }
    match load(root, sha) {
        Ok(m) => Some(m),
        Err(e) => {
            tracing::warn!(
                sha = sha.unwrap_or("active"),
                "mechanism not loaded, compiled default in force: {e}"
            );
            None
        }
    }
}

/// The protocol text for the brief, `{crash_retries}` filled; `None` = the compiled
/// `brief::protocol`.
pub fn protocol(root: &Path, sha: Option<&str>) -> Option<String> {
    let m = resolve(root, sha)?;
    Some(m.protocol.trim_end().replace(
        "{crash_retries}",
        &m.params.defaults.crash_retries.to_string(),
    ))
}

/// The fire prompt for the schedule file (`{slug}` left for the caller, one trailing newline);
/// `None` = the compiled text in `schedule_file::text`.
pub fn fire_prompt(root: &Path, sha: Option<&str>) -> Option<String> {
    let m = resolve(root, sha)?;
    Some(format!("{}\n", m.fire.trim_end()))
}

/// The advisor system prompt; `None` = the compiled `review::ADVISOR_SYSTEM`.
pub fn advisor_prompt(root: &Path, sha: Option<&str>) -> Option<String> {
    let m = resolve(root, sha)?;
    Some(m.advisor.trim_end().to_owned())
}

/// `cfg` with the mechanism's parameters laid over it (brief caps, plateau window, novelty
/// threshold, crash retries); `cfg` unchanged when nothing is pinned or adopted. The readers
/// of those parameters in actor.rs, stop.rs and brief.rs are frozen for W7, so this is the
/// one call Z wires at each (as-built W7, Hand-offs).
pub fn overlay(root: &Path, sha: Option<&str>, cfg: &AutoresearchConfig) -> AutoresearchConfig {
    let mut out = cfg.clone();
    if let Some(m) = resolve(root, sha) {
        let p = &m.params;
        out.brief.max_bytes = p.brief.max_bytes;
        out.brief.recent = p.brief.recent;
        out.brief.dead_ends = p.brief.dead_ends;
        out.brief.ideas = p.brief.ideas;
        out.brief.playbook_bytes = p.brief.playbook_bytes;
        out.plateau.window = p.plateau.window;
        out.novelty.threshold = p.novelty.threshold;
        out.defaults.crash_retries = p.defaults.crash_retries;
    }
    out
}

/// A candidate mechanism: the outer worktree's current files as a commit that touches no
/// branch (`git stash create`; HEAD when clean), kept alive under
/// `refs/autoresearch/<slug>/mechanism/<n>` and validated. An invalid candidate (an unknown
/// key, an out-of-range number) is refused here, before any inner study starts.
pub fn snapshot(worktree: &Path, slug: &str, n: u32) -> Result<String> {
    let created = crate::git::plumbing(worktree, &["stash", "create"])?;
    let sha = match created.trim() {
        "" => crate::git::head(worktree)?,
        s => s.to_owned(),
    };
    let r = format!("refs/autoresearch/{slug}/mechanism/{n}");
    crate::git::plumbing(worktree, &["update-ref", &r, &sha])?;
    let mut files = BTreeMap::new();
    for name in FILES {
        files.insert(
            name.to_owned(),
            crate::git::read(worktree, &["show", &format!("{sha}:{name}")])?,
        );
    }
    parse(&files).map_err(|e| Error::refused(format!("mechanism refused: {e}")))?;
    Ok(sha)
}

/// Register `sha` as the next version (idempotent per sha) with `summary` (the adopting
/// study's headline). Returns the version number. Nothing becomes active here.
pub fn register(root: &Path, sha: &str, summary: &str) -> Result<u32> {
    ensure(root)?;
    let mut ix = read_index(root);
    if let Some(v) = ix.versions.iter().find(|m| m.sha == sha) {
        return Ok(v.version);
    }
    let version = ix.versions.iter().map(|m| m.version).max().unwrap_or(0) + 1;
    ix.versions.push(MechanismVersion {
        version,
        sha: sha.to_owned(),
        at: crate::store::now(),
        summary: summary.to_owned(),
    });
    write_index(root, &ix)?;
    Ok(version)
}

fn info(root: &Path, files: BTreeMap<String, String>) -> MechanismInfo {
    let ix = read_index(root);
    let mut versions = ix.versions;
    versions.reverse();
    MechanismInfo {
        active: ix.active,
        versions,
        files,
    }
}

/// `autoresearch.mechanism {op}`: list, show or adopt (a press) a version. `adopt` with
/// `version: 0` reverts to the compiled defaults.
pub async fn rpc(root: &Path, op: MechanismOp) -> Result<MechanismInfo> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || rpc_blocking(&root, op))
        .await
        .map_err(|e| Error::refused(e.to_string()))?
}

fn rpc_blocking(root: &Path, op: MechanismOp) -> Result<MechanismInfo> {
    ensure(root)?;
    match op {
        MechanismOp::List => Ok(info(root, BTreeMap::new())),
        MechanismOp::Show { version } => {
            let ix = read_index(root);
            let want = version.or(ix.active);
            let files = match want {
                None => export(&defaults()),
                Some(v) => {
                    let m = ix
                        .versions
                        .iter()
                        .find(|m| m.version == v)
                        .ok_or_else(|| Error::refused(format!("no mechanism version {v}")))?;
                    files_at(root, &m.sha)?
                }
            };
            Ok(info(root, files))
        }
        MechanismOp::Adopt { version } => {
            let mut ix = read_index(root);
            if version == 0 {
                ix.active = None;
            } else {
                let m = ix
                    .versions
                    .iter()
                    .find(|m| m.version == version)
                    .ok_or_else(|| Error::refused(format!("no mechanism version {version}")))?;
                // The press adopts only what loads: a version whose files no longer parse
                // (a hand edit of the repo) is refused rather than silently falling back.
                load(root, Some(&m.sha.clone()))?;
                ix.active = Some(version);
            }
            write_index(root, &ix)?;
            Ok(info(root, BTreeMap::new()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    #[test]
    fn export_then_parse_is_the_identity() {
        let d = defaults();
        let back = parse(&export(&d)).unwrap();
        assert_eq!(back, d);
    }

    #[test]
    fn forbidden_and_unknown_keys_are_refused() {
        let mut files = export(&defaults());
        files.insert(
            "mechanism.yaml".into(),
            "version: 1\ngate:\n  z: 1.0\n".into(),
        );
        let e = parse(&files).unwrap_err();
        assert!(e.contains("unknown field `gate`"), "{e}");
        files.insert(
            "mechanism.yaml".into(),
            "version: 1\nbudget: {usd: 1}\n".into(),
        );
        assert!(parse(&files).is_err());
        files.insert(
            "mechanism.yaml".into(),
            "version: 1\nnoise: {max_cv: 1}\n".into(),
        );
        assert!(parse(&files).is_err());
        files.insert(
            "mechanism.yaml".into(),
            "version: 1\nbrief: {bogus: 1}\n".into(),
        );
        assert!(parse(&files).unwrap_err().contains("bogus"));
    }

    #[test]
    fn ranges_and_sizes_are_enforced() {
        let bad = |yaml: &str| {
            let mut files = export(&defaults());
            files.insert("mechanism.yaml".into(), yaml.to_owned());
            parse(&files).unwrap_err()
        };
        assert!(bad("plateau: {window: 2}").contains("plateau.window"));
        assert!(bad("plateau: {window: 49}").contains("plateau.window"));
        assert!(bad("novelty: {threshold: 0.1}").contains("novelty.threshold"));
        assert!(bad("brief: {ideas: 13}").contains("brief.ideas"));
        assert!(bad("brief: {max_bytes: 0}").contains("brief.max_bytes"));
        assert!(bad("defaults: {crash_retries: 9}").contains("crash_retries"));
        assert!(bad("version: 0").contains("version"));
        let mut files = export(&defaults());
        files.insert("protocol.md".into(), "x".repeat(MAX_TEXT + 1));
        assert!(parse(&files).unwrap_err().contains("protocol.md"));
        files.insert("protocol.md".into(), "  \n".into());
        assert!(parse(&files).unwrap_err().contains("empty"));
        // In range, with every key present, is fine.
        let mut files = export(&defaults());
        files.insert(
            "mechanism.yaml".into(),
            "version: 3\nplateau: {window: 8}\nnovelty: {threshold: 0.5}\ncategories: [algorithm, io]\n"
                .into(),
        );
        let m = parse(&files).unwrap();
        assert_eq!((m.version, m.params.plateau.window), (3, 8));
    }

    /// Through the frozen call sites: with v1 adopted, the schedule file and the brief are
    /// byte for byte what the compiled texts give.
    #[tokio::test]
    async fn adopted_defaults_change_no_byte_of_the_schedule_file_or_the_brief() {
        let r = root();
        let cfg = AutoresearchConfig::default();
        let spec = crate::types::StudySpec {
            slug: "t1".into(),
            goal: "faster".into(),
            repo: r.path().display().to_string(),
            ..crate::types::StudySpec::default()
        };
        let worktree = r.path().join("t1").join("worktree");
        let before =
            crate::schedule_file::text(&spec, &cfg, &worktree, "m", &crate::types::StartWhen::Now);
        let store = crate::store::Store::open(r.path()).await.unwrap();
        let study = crate::store::StudyRecord::new(spec.clone(), None);
        store.create_study(&study).await.unwrap();
        let cx = crate::brief::Context {
            n: 1,
            gate: cfg.gate.clone(),
            crash_retries: cfg.defaults.crash_retries,
            ..crate::brief::Context::default()
        };
        let brief_before = crate::brief::render(&store, &study, &cfg.brief, &cx)
            .await
            .unwrap()
            .text;

        ensure(r.path()).unwrap();
        rpc(r.path(), MechanismOp::Adopt { version: 1 })
            .await
            .unwrap();
        let after =
            crate::schedule_file::text(&spec, &cfg, &worktree, "m", &crate::types::StartWhen::Now);
        assert_eq!(after, before, "the fire prompt is byte-identical");
        let cx = crate::brief::Context {
            protocol: protocol(r.path(), None),
            ..cx
        };
        assert!(cx.protocol.is_some());
        let brief_after = crate::brief::render(&store, &study, &cfg.brief, &cx)
            .await
            .unwrap()
            .text;
        assert_eq!(brief_after, brief_before, "the protocol is byte-identical");
    }

    #[test]
    fn readers_are_silent_until_a_version_is_adopted() {
        let r = root();
        assert!(protocol(r.path(), None).is_none());
        assert!(fire_prompt(r.path(), None).is_none());
        assert!(advisor_prompt(r.path(), None).is_none());
        assert!(!repo(r.path()).exists(), "a reader never creates the repo");
        ensure(r.path()).unwrap();
        assert!(
            protocol(r.path(), None).is_none(),
            "v1 exists but nothing is adopted"
        );
        let (v, sha) = {
            let ix = read_index(r.path());
            (ix.versions[0].version, ix.versions[0].sha.clone())
        };
        assert_eq!(v, 1);
        // Pinned explicitly, the compiled texts come back byte for byte.
        assert_eq!(
            protocol(r.path(), Some(&sha)).unwrap(),
            PROTOCOL.replace("{crash_retries}", "2")
        );
        assert_eq!(fire_prompt(r.path(), Some(&sha)).unwrap(), FIRE);
        assert_eq!(advisor_prompt(r.path(), Some(&sha)).unwrap(), ADVISOR);
        let cfg = AutoresearchConfig::default();
        assert_eq!(overlay(r.path(), Some(&sha), &cfg), cfg);
    }

    #[tokio::test]
    async fn adopt_switches_the_active_version_and_running_studies_keep_their_pin() {
        let r = root();
        let repo = ensure(r.path()).unwrap();
        let v1 = read_index(r.path()).versions[0].sha.clone();
        // A second version: an edited protocol and a tighter plateau, committed to the repo.
        std::fs::write(repo.join("protocol.md"), "Protocol:\n1. Be bold.").unwrap();
        std::fs::write(
            repo.join("mechanism.yaml"),
            "version: 2\nplateau: {window: 6}\n",
        )
        .unwrap();
        crate::git::plumbing(&repo, &["commit", "-q", "-am", "v2"]).unwrap();
        let v2 = crate::git::head(&repo).unwrap();
        let n = register(r.path(), &v2, "suite +0.1").unwrap();
        assert_eq!(n, 2);
        assert_eq!(
            register(r.path(), &v2, "again").unwrap(),
            2,
            "idempotent per sha"
        );

        let info = rpc(r.path(), MechanismOp::List).await.unwrap();
        assert_eq!(info.active, None);
        assert_eq!(info.versions.len(), 2);
        assert_eq!(info.versions[0].version, 2, "newest first");

        let info = rpc(r.path(), MechanismOp::Adopt { version: 2 })
            .await
            .unwrap();
        assert_eq!(info.active, Some(2));
        assert_eq!(protocol(r.path(), None).unwrap(), "Protocol:\n1. Be bold.");
        let cfg = overlay(r.path(), None, &AutoresearchConfig::default());
        assert_eq!(cfg.plateau.window, 6);
        // A study pinned at v1 ignores the adoption.
        assert_eq!(
            protocol(r.path(), Some(&v1)).unwrap(),
            PROTOCOL.replace("{crash_retries}", "2")
        );
        let shown = rpc(r.path(), MechanismOp::Show { version: Some(1) })
            .await
            .unwrap();
        assert_eq!(shown.files["protocol.md"], PROTOCOL);
        // Revert to the compiled defaults.
        let info = rpc(r.path(), MechanismOp::Adopt { version: 0 })
            .await
            .unwrap();
        assert_eq!(info.active, None);
        assert!(protocol(r.path(), None).is_none());
        assert!(
            rpc(r.path(), MechanismOp::Adopt { version: 9 })
                .await
                .is_err()
        );
    }

    #[test]
    fn snapshot_captures_the_worktree_and_refuses_a_bad_candidate() {
        let r = root();
        let repo = ensure(r.path()).unwrap();
        let wt = r.path().join("wt");
        crate::git::worktree_add_detached(&repo, &wt, &crate::git::head(&repo).unwrap()).unwrap();
        // Clean worktree: the snapshot is HEAD.
        assert_eq!(
            snapshot(&wt, "m", 1).unwrap(),
            crate::git::head(&repo).unwrap()
        );
        std::fs::write(wt.join("advisor.md"), "Advise briefly.").unwrap();
        let sha = snapshot(&wt, "m", 2).unwrap();
        assert_ne!(sha, crate::git::head(&repo).unwrap());
        assert_eq!(
            advisor_prompt(r.path(), Some(&sha)).unwrap(),
            "Advise briefly."
        );
        assert!(
            crate::git::read(&wt, &["status", "--porcelain"])
                .unwrap()
                .contains("advisor.md"),
            "the worktree's edit is still uncommitted"
        );
        std::fs::write(wt.join("mechanism.yaml"), "version: 2\ngate: {z: 1.0}\n").unwrap();
        let e = snapshot(&wt, "m", 3).unwrap_err().to_string();
        assert!(e.contains("mechanism refused") && e.contains("gate"), "{e}");
    }
}
