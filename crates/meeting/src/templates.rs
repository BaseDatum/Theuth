//! Note templates (158 §11.1, D14/W-p): markdown files with YAML front matter under
//! `<data_dir>/meetings/templates/<slug>.md`. Five built-ins are embedded in the binary
//! ([`BUILTINS`]) and (re)written only while the file on disk is still a version we shipped
//! (`.shipped.json` keeps the sha256 of the last body written per slug); a user edit is never
//! overwritten. `summary::form_for` compiles the chosen template into the summary form.
//!
//! The data dir reaches the sync form compiler through [`attach`] (set at plugin start by
//! `tools::register_post`); without it the built-ins stand in.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

use crate::calendar::EventRef;
use crate::store::Store;
use crate::{MEETING_ACTOR, Manifest, Meeting, MeetingId, MeetingState};

/// Directory under `<data_dir>/meetings/`.
pub const DIR: &str = "templates";
/// The built-in that reproduces today's five headings; `Manifest.template`'s default.
pub const DEFAULT: &str = "meeting";
/// Shipped-body hashes beside the files: `{ slug: sha256 }` of what we last wrote.
const SHIPPED: &str = ".shipped.json";

/// Slug → shipped body. `meeting` first: it is the fallback for everything.
pub const BUILTINS: &[(&str, &str)] = &[
    (DEFAULT, include_str!("../templates/meeting.md")),
    ("one-on-one", include_str!("../templates/one-on-one.md")),
    ("standup", include_str!("../templates/standup.md")),
    ("interview", include_str!("../templates/interview.md")),
    (
        "customer-call",
        include_str!("../templates/customer-call.md"),
    ),
];

/// One section of the summary form.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "TemplateSection")
)]
pub struct Section {
    /// `## <title>` in the note.
    pub title: String,
    /// Per-section instruction for the model.
    #[serde(default)]
    pub instruction: Option<String>,
    /// Keep the heading even when empty.
    #[serde(default)]
    pub required: bool,
}

/// A template as `meeting_templates` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(
    feature = "ts",
    derive(ts_rs::TS),
    ts(export, rename = "MeetingTemplate")
)]
pub struct Template {
    /// File stem.
    pub slug: String,
    /// Front matter `name`.
    pub name: String,
    /// Front matter `description`.
    #[serde(default)]
    pub description: String,
    /// In order.
    pub sections: Vec<Section>,
    /// Shipped with the binary (`meeting_template_reset` restores the shipped body; delete refused).
    #[serde(default)]
    pub builtin: bool,
    /// The raw markdown file, front matter included — what Settings edits and `put` writes verbatim.
    #[serde(default)]
    pub body: String,
    /// A built-in whose file differs from the shipped body (Settings shows "Reset to built-in").
    #[serde(default)]
    pub edited: bool,
}

/// The front matter as written; unknown fields (`builtin: 1`) are ignored.
#[derive(Deserialize)]
struct FrontMatter {
    name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    sections: Option<Vec<Section>>,
}

/// Whether `slug` is shipped.
pub fn is_builtin(slug: &str) -> bool {
    BUILTINS.iter().any(|(s, _)| *s == slug)
}

/// The shipped body of `slug`.
pub fn builtin(slug: &str) -> Option<&'static str> {
    BUILTINS.iter().find(|(s, _)| *s == slug).map(|(_, b)| *b)
}

/// Parse one template file. Errors are the words Settings shows.
pub fn parse(slug: &str, text: &str) -> Result<Template, String> {
    let parsed = gray_matter::Matter::<gray_matter::engine::YAML>::new()
        .parse::<FrontMatter>(text)
        .map_err(|e| format!("front matter: {e}"))?;
    let fm = parsed
        .data
        .ok_or_else(|| "front matter missing: start the file with `---`".to_owned())?;
    let sections = fm
        .sections
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "`sections` missing: list at least one `- title:`".to_owned())?;
    if fm.name.trim().is_empty() {
        return Err("`name` missing".into());
    }
    if let Some(s) = sections.iter().find(|s| s.title.trim().is_empty()) {
        return Err(format!("a section has no title: {s:?}"));
    }
    Ok(Template {
        slug: slug.to_owned(),
        name: fm.name.trim().to_owned(),
        description: fm.description.trim().to_owned(),
        sections,
        builtin: is_builtin(slug),
        body: text.to_owned(),
        edited: builtin(slug).is_some_and(|b| b != text),
    })
}

/// `[a-z0-9-]`, 1–40 chars: a file stem that is safe on every file system.
pub fn valid_slug(slug: &str) -> bool {
    !slug.is_empty()
        && slug.len() <= 40
        && slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn sha256(text: &str) -> String {
    platform::sha256::hex(text.as_bytes())
}

/// Write the built-ins into `dir`: a missing file is created; a file equal to the version we
/// last wrote is replaced when the shipped body changed; anything else (a user edit) stays.
/// `shipped` lets tests ship a v1 then a v2.
pub fn install(dir: &Path, shipped: &[(&str, &str)]) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    let record = dir.join(SHIPPED);
    let mut written: BTreeMap<String, String> = std::fs::read(&record)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    let mut changed = false;
    for (slug, body) in shipped {
        let path = dir.join(format!("{slug}.md"));
        let body_sha = sha256(body);
        let write = match std::fs::read_to_string(&path) {
            Err(_) => true,
            Ok(cur) => {
                let cur_sha = sha256(&cur);
                match written.get(*slug) {
                    Some(last) => *last == cur_sha && *last != body_sha,
                    // Pre-record file: ours only when it is byte-equal to what we ship.
                    None => cur_sha == body_sha,
                }
            }
        };
        if write {
            std::fs::write(&path, body)?;
        }
        if written.get(*slug) != Some(&body_sha) && (write || !path.exists()) {
            written.insert((*slug).to_owned(), body_sha);
            changed = true;
        }
    }
    if changed {
        std::fs::write(&record, serde_json::to_vec_pretty(&written)?)?;
    }
    Ok(())
}

/// Every `*.md` in `dir` (built-ins installed first), built-ins in shipped order, then the
/// user's by slug. Unparsable files are skipped with a warning; `put` is where errors surface.
pub fn list(dir: &Path) -> anyhow::Result<Vec<Template>> {
    install(dir, BUILTINS)?;
    let mut user = Vec::new();
    let mut builtins: Vec<Option<Template>> = vec![None; BUILTINS.len()];
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }
        let Some(slug) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        let text = std::fs::read_to_string(&path)?;
        match parse(slug, &text) {
            Ok(t) => match BUILTINS.iter().position(|(s, _)| *s == slug) {
                Some(i) => builtins[i] = Some(t),
                None => user.push(t),
            },
            Err(e) => tracing::warn!("template {}: {e}", path.display()),
        }
    }
    user.sort_by(|a, b| a.slug.cmp(&b.slug));
    Ok(builtins.into_iter().flatten().chain(user).collect())
}

/// The template `slug` compiles to: the file in `dir`, else the shipped body of that slug,
/// else `meeting`. Never fails — the summary turn must always have a form.
pub fn load(dir: Option<&Path>, slug: &str) -> Template {
    let from_file = dir
        .map(|d| d.join(format!("{slug}.md")))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .and_then(|text| parse(slug, &text).ok());
    from_file
        .or_else(|| builtin(slug).and_then(|b| parse(slug, b).ok()))
        .unwrap_or_else(|| parse(DEFAULT, builtin(DEFAULT).expect("shipped")).expect("shipped"))
}

// ---- data dir for the sync form compiler.

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Remember the data dir (plugin start). Idempotent; the first call wins.
pub(crate) fn attach(data_dir: PathBuf) {
    let _ = DATA_DIR.set(data_dir);
}

/// The attached data dir, if the plugin started.
pub(crate) fn data_dir() -> Option<&'static Path> {
    DATA_DIR.get().map(PathBuf::as_path)
}

fn dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join(crate::DIR).join(DIR)
}

/// `<data_dir>/meetings/templates/` once attached.
pub(crate) fn dir() -> Option<PathBuf> {
    data_dir().map(dir_in)
}

// ---- selection (§11.1): start pick → remembered per event → per project → `meeting.template` → `meeting`.

/// `title:<norm>` key part: lowercase alphanumerics, single spaces.
fn norm_title(title: &str) -> String {
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// Memory keys for a meeting, most specific first.
fn memory_keys(event: Option<&EventRef>, project: Option<&str>) -> Vec<String> {
    let mut keys = Vec::new();
    if let Some(e) = event {
        keys.push(format!("tpl:uid:{}", e.uid));
        let t = norm_title(&e.title);
        if !t.is_empty() {
            keys.push(format!("tpl:title:{t}"));
        }
    }
    if let Some(p) = project.map(str::trim).filter(|p| !p.is_empty()) {
        keys.push(format!("tpl:project:{p}"));
    }
    keys
}

/// The slug a new meeting gets: an explicit `pick`, else the one remembered for the event's
/// uid, its normalised title or the project (`cal_project_memory`), else `default`
/// (`meeting.template`), else [`DEFAULT`]. The start handler calls this (hand-off, lib.rs).
pub async fn select(
    store: &Store,
    pick: Option<&str>,
    event: Option<&EventRef>,
    project: Option<&str>,
    default: &str,
) -> String {
    if let Some(p) = pick.map(str::trim).filter(|p| !p.is_empty()) {
        return p.to_owned();
    }
    for key in memory_keys(event, project) {
        let hit: Option<String> =
            sqlx::query_scalar("SELECT project FROM cal_project_memory WHERE key = ?")
                .bind(&key)
                .fetch_optional(store.pool())
                .await
                .ok()
                .flatten();
        if let Some(slug) = hit {
            return slug;
        }
    }
    let d = default.trim();
    if d.is_empty() { DEFAULT } else { d }.to_owned()
}

/// Remember a user's pick for the meeting's event and project.
pub async fn remember(
    store: &Store,
    event: Option<&EventRef>,
    project: Option<&str>,
    slug: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    for key in memory_keys(event, project) {
        sqlx::query(
            "INSERT INTO cal_project_memory (key, project, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT(key) DO UPDATE SET project = excluded.project, updated_at = excluded.updated_at",
        )
        .bind(&key)
        .bind(slug)
        .bind(&now)
        .execute(store.pool())
        .await?;
    }
    Ok(())
}

// ---- actor message: the manifest changes on the actor so the live copy stays coherent.

/// `ask` → the manifest after `meeting_set_template`.
pub(crate) struct SetTemplate {
    pub meeting: MeetingId,
    pub template: String,
}

impl Message<SetTemplate> for Meeting {
    type Reply = Result<Manifest, String>;

    async fn handle(&mut self, m: SetTemplate, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let live = self
            .current
            .as_ref()
            .is_some_and(|r| r.manifest.id == m.meeting);
        let mut manifest = match self.current.as_ref().filter(|_| live) {
            Some(r) => r.manifest.clone(),
            None => self
                .store
                .manifest(m.meeting)
                .map_err(|e| format!("{e:#}"))?,
        };
        if matches!(
            manifest.state,
            MeetingState::Summarizing | MeetingState::Done
        ) {
            return Err(format!(
                "{:?}: the template can change until Summarize",
                manifest.state
            ));
        }
        manifest.template = m.template;
        if let Some(rec) = self.current.as_mut().filter(|r| r.manifest.id == m.meeting) {
            rec.manifest = manifest.clone();
        }
        self.save(&manifest).await;
        Ok(manifest)
    }
}

// ---- RPC delegates (`crates/rpc/src/api.rs` W4 rows).

fn dir_of(rt: &Runtime) -> PathBuf {
    attach(rt.data_dir.clone());
    dir_in(&rt.data_dir)
}

/// `meeting_templates() -> Vec<Template>` (§15.2, W4).
pub async fn rpc_templates(rt: &Runtime) -> anyhow::Result<Vec<Template>> {
    list(&dir_of(rt))
}

/// `meeting_template_put(slug, body) -> Template` (W4): parse first (the error is Settings'
/// message), then write the file.
pub async fn rpc_template_put(
    rt: &Runtime,
    slug: String,
    body: String,
) -> anyhow::Result<Template> {
    put(&dir_of(rt), &slug, &body)
}

/// [`rpc_template_put`]'s body: validate by parsing, then write `body` exactly as given.
pub fn put(dir: &Path, slug: &str, body: &str) -> anyhow::Result<Template> {
    if !valid_slug(slug) {
        anyhow::bail!("slug must be 1–40 chars of a-z, 0-9 and '-'");
    }
    let t = parse(slug, body).map_err(anyhow::Error::msg)?;
    install(dir, BUILTINS)?;
    std::fs::write(dir.join(format!("{slug}.md")), body)?;
    Ok(t)
}

/// `meeting_template_delete(slug)` (W4): a user template is removed; refused for a built-in
/// (§11.1 — `meeting_template_reset` is the way back to the shipped body).
pub async fn rpc_template_delete(rt: &Runtime, slug: String) -> anyhow::Result<()> {
    delete(&dir_of(rt), &slug)
}

/// [`rpc_template_delete`]'s body.
pub fn delete(dir: &Path, slug: &str) -> anyhow::Result<()> {
    if !valid_slug(slug) {
        anyhow::bail!("unknown template {slug:?}");
    }
    if is_builtin(slug) {
        anyhow::bail!("{slug} is built in: reset it instead of deleting it");
    }
    std::fs::remove_file(dir.join(format!("{slug}.md")))
        .map_err(|e| anyhow::anyhow!("remove template {slug}: {e}"))
}

/// `meeting_template_reset(slug) -> Template` (W4, W4U request): drop the user's edits to a
/// built-in so the embedded body applies again; refused for a user template.
pub async fn rpc_template_reset(rt: &Runtime, slug: String) -> anyhow::Result<Template> {
    reset(&dir_of(rt), &slug)
}

/// [`rpc_template_reset`]'s body: the shipped body is written back and recorded as ours, so the
/// next upgrade sees an unedited file again.
pub fn reset(dir: &Path, slug: &str) -> anyhow::Result<Template> {
    let Some(body) = builtin(slug) else {
        anyhow::bail!("{slug:?} is not a built-in template");
    };
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join(format!("{slug}.md")), body)?;
    install(dir, BUILTINS)?;
    parse(slug, body).map_err(anyhow::Error::msg)
}

/// `meeting_set_template(id, template) -> Manifest` (W4): changeable until Summarize; the pick
/// is remembered for the meeting's event and project.
pub async fn rpc_set_template(
    rt: &Runtime,
    id: MeetingId,
    template: String,
) -> anyhow::Result<Manifest> {
    let slug = template.trim().to_owned();
    if !valid_slug(&slug) {
        anyhow::bail!("unknown template {template:?}");
    }
    let dir = dir_of(rt);
    if !is_builtin(&slug) && !dir.join(format!("{slug}.md")).is_file() {
        anyhow::bail!("unknown template {slug:?}");
    }
    let actor = ActorRef::<Meeting>::lookup(MEETING_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| anyhow::anyhow!("meeting is not running"))?;
    let manifest = actor
        .ask(SetTemplate {
            meeting: id,
            template: slug.clone(),
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let (store, _) = crate::calendar::handles().await?;
    remember(
        &store,
        manifest.event.as_ref(),
        manifest.project.as_deref(),
        &slug,
    )
    .await?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: &str = "---\nname: Standup\nsections:\n  - title: Done\n---\n";
    const V2: &str = "---\nname: Standup\nsections:\n  - title: Done\n  - title: Next\n---\n";

    #[test]
    fn parse_reads_sections_and_refuses_what_settings_must_show() {
        let t = parse("one-on-one", builtin("one-on-one").unwrap()).unwrap();
        assert_eq!(
            (t.name.as_str(), t.builtin, t.sections.len()),
            ("One-on-one", true, 5)
        );
        assert_eq!(
            t.sections[0].instruction.as_deref(),
            Some("at most 120 words")
        );
        assert_eq!(t.sections[3].instruction, None);
        assert!(!t.sections[0].required);
        let with_unknown = "---\nname: X\nbuiltin: 1\ncolour: red\nsections:\n  - title: A\n    required: true\n---\nbody text\n";
        let t = parse("x", with_unknown).unwrap();
        assert!(t.sections[0].required && !t.builtin);
        assert_eq!(
            parse("x", "---\nname: X\n---\n").unwrap_err(),
            "`sections` missing: list at least one `- title:`"
        );
        assert!(
            parse("x", "no front matter")
                .unwrap_err()
                .starts_with("front matter")
        );
        assert!(
            parse("x", "---\nsections:\n  - title: A\n---\n").is_err(),
            "name missing"
        );
        assert!(valid_slug("customer-call") && !valid_slug("Bad Slug") && !valid_slug(""));
        for (slug, body) in BUILTINS {
            parse(slug, body).unwrap_or_else(|e| panic!("{slug}: {e}"));
        }
    }

    #[test]
    fn install_upgrades_only_unedited_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let path = dir.join("standup.md");
        install(dir, &[("standup", V1)]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), V1);
        // Unedited → v2 replaces it.
        install(dir, &[("standup", V2)]).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), V2);
        // Edited → v3 leaves it alone; the record keeps the user's file safe across restarts.
        std::fs::write(&path, format!("{V2}my notes\n")).unwrap();
        install(dir, &[("standup", V1)]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{V2}my notes\n")
        );
        install(dir, &[("standup", V1)]).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("{V2}my notes\n")
        );
        // A file equal to the shipped body but written before the record exists is ours.
        let other = dir.join("meeting.md");
        std::fs::write(&other, V1).unwrap();
        install(dir, &[("meeting", V1)]).unwrap();
        install(dir, &[("meeting", V2)]).unwrap();
        assert_eq!(std::fs::read_to_string(&other).unwrap(), V2);
    }

    #[test]
    fn list_and_load_fall_back_to_the_shipped_bodies() {
        let tmp = tempfile::tempdir().unwrap();
        let all = list(tmp.path()).unwrap();
        assert_eq!(
            all.iter().map(|t| t.slug.as_str()).collect::<Vec<_>>(),
            [
                "meeting",
                "one-on-one",
                "standup",
                "interview",
                "customer-call"
            ]
        );
        std::fs::write(tmp.path().join("retro.md"), V2).unwrap();
        std::fs::write(tmp.path().join("broken.md"), "---\nname: B\n---\n").unwrap();
        let all = list(tmp.path()).unwrap();
        assert_eq!(all.len(), 6, "broken skipped, retro listed");
        assert!(all.last().is_some_and(|t| t.slug == "retro" && !t.builtin));
        assert_eq!(load(Some(tmp.path()), "retro").sections.len(), 2);
        assert_eq!(load(Some(tmp.path()), "nope").slug, DEFAULT);
        assert_eq!(load(None, "standup").name, "Standup");
        assert_eq!(load(None, "broken").slug, DEFAULT);
    }

    /// W4U: `body` is the file verbatim; `put` validates then writes exactly what was sent;
    /// `delete` refuses built-ins; `reset` restores a built-in's shipped body (and the upgrade
    /// record), refuses user templates.
    #[test]
    fn put_delete_and_reset_round_trip_the_raw_body() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path();
        let shipped = builtin("standup").unwrap();
        assert_eq!(
            list(dir).unwrap()[2].body,
            shipped,
            "body is the file as shipped"
        );
        let edited = format!("{V2}\nmy notes  \n");
        let t = put(dir, "standup", &edited).unwrap();
        assert_eq!(
            (t.body.as_str(), t.builtin, t.edited, t.sections.len()),
            (edited.as_str(), true, true, 2)
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("standup.md")).unwrap(),
            edited
        );
        assert!(
            put(dir, "standup", "---\nname: X\n---\n").is_err(),
            "invalid body not written"
        );
        assert_eq!(
            std::fs::read_to_string(dir.join("standup.md")).unwrap(),
            edited
        );
        assert!(put(dir, "Bad Slug", V1).is_err());
        // The edited built-in survives an install (user edit), then reset brings the shipped body back.
        install(dir, BUILTINS).unwrap();
        assert_eq!(load(Some(dir), "standup").body, edited);
        assert!(
            delete(dir, "standup")
                .unwrap_err()
                .to_string()
                .contains("built in")
        );
        let t = reset(dir, "standup").unwrap();
        assert_eq!((t.body.as_str(), t.edited), (shipped, false));
        assert_eq!(
            std::fs::read_to_string(dir.join("standup.md")).unwrap(),
            shipped
        );
        // User template: put, reset refused, delete removes, second delete errs.
        put(dir, "retro", V1).unwrap();
        assert!(
            reset(dir, "retro")
                .unwrap_err()
                .to_string()
                .contains("not a built-in")
        );
        delete(dir, "retro").unwrap();
        assert!(!dir.join("retro.md").exists());
        assert!(delete(dir, "retro").is_err());
        assert!(reset(dir, "nope").is_err());
    }

    fn event(uid: &str, title: &str) -> EventRef {
        EventRef {
            id: crate::calendar::EventId(format!("ics:{uid}:")),
            uid: uid.into(),
            title: title.into(),
            start: "2026-10-10T10:00:00Z".into(),
            end: "2026-10-10T10:30:00Z".into(),
            join_url: None,
            join_host: None,
            organizer: None,
            attendees: Vec::new(),
            description: None,
        }
    }

    #[tokio::test]
    async fn selection_order_is_pick_then_event_memory_then_project_then_default() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Store::open(tmp.path()).await.unwrap();
        let weekly = event("abc", "Weekly Sync");
        let other = event("zzz", "weekly   sync!");
        assert_eq!(select(&store, None, Some(&weekly), None, "").await, DEFAULT);
        assert_eq!(select(&store, None, None, None, "standup").await, "standup");
        assert_eq!(
            select(&store, Some(" interview "), None, None, "standup").await,
            "interview"
        );
        remember(&store, Some(&weekly), Some("acme"), "one-on-one")
            .await
            .unwrap();
        assert_eq!(
            select(&store, None, Some(&weekly), None, "standup").await,
            "one-on-one"
        );
        // Same normalised title, other uid → the title row; a project alone → the project row.
        assert_eq!(
            select(&store, None, Some(&other), None, "standup").await,
            "one-on-one"
        );
        assert_eq!(
            select(&store, None, None, Some("acme"), "standup").await,
            "one-on-one"
        );
        assert_eq!(
            select(&store, None, None, Some("other"), "standup").await,
            "standup"
        );
        // uid wins over title, pick wins over everything.
        remember(&store, Some(&other), None, "customer-call")
            .await
            .unwrap();
        assert_eq!(
            select(&store, None, Some(&other), None, "").await,
            "customer-call"
        );
        assert_eq!(
            select(&store, None, Some(&weekly), None, "").await,
            "one-on-one"
        );
        assert_eq!(
            select(&store, Some("standup"), Some(&weekly), None, "").await,
            "standup"
        );
    }
}
