//! Projects as the meeting series (158 §9, W3): the meeting ↔ project RPC, the
//! `cal_project_memory` lookup order (`uid:` → `title:` → `meeting.project` → none), and the
//! pre-meeting brief — one Private writer turn on `meeting.model`, its bullets cached in
//! `cal_prebrief` and filed as a `briefs.sqlite` row with attendee aliases, surfaced as
//! `MeetingEvent::Upcoming` from `start − 5 min` by the one-minute [`ensure_upcoming`] tick.
//!
//! Cache validity is checked lazily (the `sources` column holds the Done meetings it was
//! built from plus the event's `sequence`/description fingerprint), so a new Done meeting or
//! an edited event regenerates on the next tick without a hook in the summary path.

use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::Context as _;
use chrono::{DateTime, Utc};
use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::Runtime;
use serde::{Deserialize, Serialize};

use super::store as cal;
use super::{CalEvent, EventId, EventRef, actor};
use crate::cache::{Cache, CachedBriefPut};
use crate::detect::Detection;
use crate::{BriefEvent, BriefId, Manifest, Meeting, MeetingEvent, MeetingId, Store};

/// Past meetings a brief draws on (§9.4).
pub const PAST_MEETINGS: u32 = 3;
/// Generation starts this long before the event (§9.4).
pub const GENERATE_MIN: i64 = 15;
/// `Upcoming` goes out from here (§9.4); the island clears it at `start + SHOW_MIN`.
pub const SHOW_MIN: i64 = 5;
/// The writer turn's budget (§9.4).
pub const TIMEOUT: Duration = Duration::from_secs(45);
/// Most bullets a brief has (the cache takes 2–3).
pub const MAX_BULLETS: usize = 3;

// ---- §9.3 project memory

/// `title:` key material: lowercase, letters only (digits and punctuation go), month and
/// weekday words dropped, single spaces.
pub fn norm_title(title: &str) -> String {
    title
        .to_lowercase()
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty() && !crate::oracle::CALENDAR.contains(w))
        .collect::<Vec<_>>()
        .join(" ")
}

/// The memory keys an event / title pair writes and reads, in lookup order.
fn memory_keys(event: Option<&EventRef>, title: Option<&str>) -> Vec<String> {
    let mut keys = Vec::with_capacity(2);
    if let Some(e) = event.filter(|e| !e.uid.trim().is_empty()) {
        keys.push(format!("uid:{}", e.uid.trim()));
    }
    let norm = title
        .or(event.map(|e| e.title.as_str()))
        .map(norm_title)
        .unwrap_or_default();
    if !norm.is_empty() {
        keys.push(format!("title:{norm}"));
    }
    keys
}

/// The remembered project: `uid:` first, then `title:`.
pub(crate) async fn remembered(
    store: &Store,
    event: Option<&EventRef>,
    title: Option<&str>,
) -> anyhow::Result<Option<String>> {
    for key in memory_keys(event, title) {
        let row: Option<(String,)> =
            sqlx::query_as("SELECT project FROM cal_project_memory WHERE key = ?")
                .bind(&key)
                .fetch_optional(store.pool())
                .await
                .context("project memory")?;
        if let Some((p,)) = row {
            return Ok(Some(p));
        }
    }
    Ok(None)
}

/// §9.3 lookup order: `uid:` → `title:` → `default` (`meeting.project`) → `None`.
pub(crate) async fn lookup(
    store: &Store,
    default: Option<&str>,
    event: Option<&EventRef>,
    title: Option<&str>,
) -> anyhow::Result<Option<String>> {
    Ok(remembered(store, event, title)
        .await?
        .or_else(|| default.map(str::to_owned)))
}

/// Write `project` under every key of the pair.
pub(crate) async fn remember(
    store: &Store,
    event: Option<&EventRef>,
    title: Option<&str>,
    project: &str,
) -> anyhow::Result<()> {
    let now = cal::stamp(Utc::now());
    for key in memory_keys(event, title) {
        sqlx::query(
            "INSERT INTO cal_project_memory (key, project, updated_at) VALUES (?, ?, ?) \
             ON CONFLICT(key) DO UPDATE SET project = excluded.project, updated_at = excluded.updated_at",
        )
        .bind(&key)
        .bind(project)
        .bind(&now)
        .execute(store.pool())
        .await
        .context("remember project")?;
    }
    Ok(())
}

/// Delete the pair's keys (§9.3: a cleared project is not sticky).
pub(crate) async fn forget(
    store: &Store,
    event: Option<&EventRef>,
    title: Option<&str>,
) -> anyhow::Result<()> {
    for key in memory_keys(event, title) {
        sqlx::query("DELETE FROM cal_project_memory WHERE key = ?")
            .bind(&key)
            .execute(store.pool())
            .await
            .context("forget project")?;
    }
    Ok(())
}

/// A project label as the wire gives it: trimmed, empty → `None`.
pub(crate) fn clean_project(p: Option<String>) -> Option<String> {
    p.map(|p| p.trim().to_owned()).filter(|p| !p.is_empty())
}

// ---- actor side: what the off-actor code needs, and `meeting_set_project`

/// `ask` → the store, the brief cache and the config bits the brief needs.
pub(crate) struct PrebriefCtx;

/// Reply to [`PrebriefCtx`].
#[derive(Clone)]
pub(crate) struct Ctx {
    pub store: Arc<Store>,
    pub cache: Option<Arc<Cache>>,
    /// `meeting.model`; `None` → `agent.model`.
    pub model: Option<String>,
    /// `meeting.project`.
    pub default_project: Option<String>,
    /// `meeting.calendar.prebrief`.
    pub prebrief: bool,
}

impl Message<PrebriefCtx> for Meeting {
    type Reply = Ctx;

    async fn handle(&mut self, _: PrebriefCtx, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        Ctx {
            store: self.store.clone(),
            cache: self.cache.clone(),
            model: self.cfg.model.clone(),
            default_project: self.cfg.project.clone(),
            prebrief: self.cfg.calendar.prebrief,
        }
    }
}

pub(super) async fn ctx() -> anyhow::Result<Ctx> {
    actor()?
        .ask(PrebriefCtx)
        .await
        .map_err(|e| anyhow::anyhow!("meeting: {e}"))
}

/// `ask` → the manifest after `meeting_set_project` (§9.2).
pub(crate) struct SetProject {
    pub meeting: MeetingId,
    pub project: Option<String>,
}

impl Message<SetProject> for Meeting {
    type Reply = Result<Manifest, String>;

    async fn handle(&mut self, m: SetProject, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
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
        manifest.project = m.project;
        let (event, title) = (manifest.event.as_ref(), manifest.title.as_deref());
        let memory = match &manifest.project {
            Some(p) => remember(&self.store, event, title, p).await,
            None => forget(&self.store, event, title).await,
        };
        if let Err(e) = memory {
            tracing::warn!(meeting = %m.meeting, "project memory: {e:#}");
        }
        if let Some(rec) = self.current.as_mut().filter(|r| r.manifest.id == m.meeting) {
            rec.manifest = manifest.clone();
        }
        self.save(&manifest).await;
        // §9.2 W-c: the sidebar follows — the project, else the `meetings` label.
        if let Ok(Some(agents)) = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR) {
            let project = manifest.project.clone().or_else(|| Some("meetings".into()));
            if let Err(e) = agents
                .ask(agent::SetSessionProject {
                    session: manifest.session,
                    project,
                })
                .await
            {
                tracing::warn!(meeting = %m.meeting, "session project: {e}");
            }
        }
        Ok(manifest)
    }
}

/// `meeting_set_project(id, project) -> Manifest` (§9.2, W3): manifest + index row +
/// `agent::SetSessionProject` + the memory rows (cleared when `None`).
pub async fn rpc_set_project(
    _rt: &Runtime,
    id: MeetingId,
    project: Option<String>,
) -> anyhow::Result<Manifest> {
    actor()?
        .ask(SetProject {
            meeting: id,
            project: clean_project(project),
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

/// §9.3 lookup for an event (or a bare title when `event` is `None`): the remembered project,
/// else `meeting.project`, else `None`.
pub async fn remembered_project(
    _rt: &Runtime,
    event: Option<&EventRef>,
    title: Option<&str>,
) -> anyhow::Result<Option<String>> {
    let cx = ctx().await?;
    lookup(&cx.store, cx.default_project.as_deref(), event, title).await
}

/// §9.4: the cached bullets for `event` in `project`, generating them when missing and
/// `meeting.calendar.prebrief` is on.
pub async fn brief_for(
    rt: &Runtime,
    event: &EventRef,
    project: &str,
) -> anyhow::Result<Option<Vec<String>>> {
    let cx = ctx().await?;
    let Some(row) = cal::event(&cx.store, &event.id).await? else {
        return Ok(cached(&cx.store, &event.id).await?.map(|c| c.bullets));
    };
    if !cx.prebrief {
        return Ok(cached(&cx.store, &event.id).await?.map(|c| c.bullets));
    }
    ensure(rt, &cx, &PrivateTurn, &row, project)
        .await
        .map(|c| Some(c.bullets))
}

/// Fill `Detection.project` / `Detection.brief` (§9.3, §9.4) from memory and the cache; never
/// generates (the scheduler does, from `start − 15 min`).
pub(crate) async fn decorate(store: &Store, default: Option<&str>, d: &mut Detection) {
    let event = d.event.as_ref().map(|m| &m.event);
    d.project = match lookup(store, default, event, None).await {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!("project memory: {e:#}");
            None
        }
    };
    d.brief = match event {
        Some(e) => cached(store, &e.id).await.ok().flatten().map(|c| c.bullets),
        None => None,
    };
}

// ---- §9.4 cache

/// What a cached brief was built from; a mismatch regenerates.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Sources {
    /// The Done meetings, newest first.
    meetings: Vec<MeetingId>,
    /// `sequence:description-hash` of the event.
    fingerprint: String,
}

/// One `cal_prebrief` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Cached {
    pub bullets: Vec<String>,
    pub brief: Option<BriefId>,
    sources: Sources,
}

fn fingerprint(e: &CalEvent) -> String {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    e.description.hash(&mut h);
    e.title.hash(&mut h);
    e.start.hash(&mut h);
    format!("{}:{:016x}", e.sequence, h.finish())
}

pub(crate) async fn cached(store: &Store, event: &EventId) -> anyhow::Result<Option<Cached>> {
    let row: Option<(String, Option<String>, String)> =
        sqlx::query_as("SELECT bullets, brief, sources FROM cal_prebrief WHERE event_id = ?")
            .bind(&event.0)
            .fetch_optional(store.pool())
            .await
            .context("read cal_prebrief")?;
    Ok(row.map(|(bullets, brief, sources)| Cached {
        bullets: serde_json::from_str(&bullets).unwrap_or_default(),
        brief: brief.and_then(|b| b.parse().ok()),
        sources: serde_json::from_str(&sources).unwrap_or_default(),
    }))
}

async fn put_cached(store: &Store, event: &EventId, c: &Cached) -> anyhow::Result<()> {
    sqlx::query(
        "INSERT INTO cal_prebrief (event_id, generated_at, bullets, brief, sources) VALUES (?, ?, ?, ?, ?) \
         ON CONFLICT(event_id) DO UPDATE SET generated_at = excluded.generated_at, bullets = excluded.bullets, \
         brief = excluded.brief, sources = excluded.sources",
    )
    .bind(&event.0)
    .bind(cal::stamp(Utc::now()))
    .bind(serde_json::to_string(&c.bullets)?)
    .bind(c.brief.map(|b| b.to_string()))
    .bind(serde_json::to_string(&c.sources)?)
    .execute(store.pool())
    .await
    .context("write cal_prebrief")?;
    Ok(())
}

// ---- §9.4 inputs

/// The section headings a summary writes that a brief reads (`note.rs`, `summary.rs`).
pub const SECTIONS: [&str; 3] = ["decisions", "action items", "open questions"];

/// A heading line (`## X`, `**X**`, `**X:**`, or a short `X:` line) as lowercase text.
fn heading(line: &str) -> Option<String> {
    let t = line.trim();
    let name = if let Some(h) = t.strip_prefix('#') {
        h.trim_start_matches('#')
    } else if t.starts_with("**") && t.trim_end_matches(':').ends_with("**") {
        t.trim_matches(|c| c == '*' || c == ':')
    } else if t.ends_with(':') && t.split_whitespace().count() <= 4 {
        t.trim_end_matches(':')
    } else {
        return None;
    };
    Some(
        name.trim_matches(|c: char| c == '*' || c == ':' || c.is_whitespace())
            .to_lowercase(),
    )
}

/// §9.4: the Decisions / Action items / Open questions sections of a summary, split by
/// heading — no model call. Each kept section comes out as `Name:` then its lines.
pub fn excerpt(markdown: &str) -> String {
    let mut out = String::new();
    let mut keep = false;
    for line in markdown.lines() {
        if let Some(h) = heading(line) {
            keep = SECTIONS.iter().any(|s| h.starts_with(s));
            if keep {
                let mut name: Vec<char> = h.chars().collect();
                if let Some(c) = name.first_mut() {
                    *c = c.to_ascii_uppercase();
                }
                out.push_str(&name.into_iter().collect::<String>());
                out.push_str(":\n");
            }
            continue;
        }
        if keep && !line.trim().is_empty() {
            out.push_str(line.trim_end());
            out.push('\n');
        }
    }
    out
}

/// Longest excerpt one past meeting contributes (§9.4 cost: ≤ 400 words each).
const EXCERPT_CHARS: usize = 2_500;

/// The writer prompt (§9.4), facts only. The brief row is filed by the engine, not the model.
fn prompt(event: &CalEvent, project: &str, excerpts: &str) -> String {
    let r = event.to_ref();
    let at = cal::parse(&event.start)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_default();
    let with = r
        .attendee_line()
        .unwrap_or_else(|| "no listed attendees".into());
    let mut p = format!(
        "You are preparing the user for \"{}\" at {at} with {with}. Facts only — never what the \
         user should say. From the project's last meetings (excerpts below) and knowledge \
         (`knowledge_search` first, over the attendee names and the title's proper nouns), write \
         at most {MAX_BULLETS} bullets (≤ 20 words each): decisions carried over, open items with \
         owners, numbers and dates that matter. Reply with the bullets only, one per line, each \
         starting with \"- \".\n\n{}\n",
        r.title,
        r.context_block()
    );
    p.push_str(&format!("\nProject \"{project}\" — last meetings:\n"));
    if excerpts.trim().is_empty() {
        p.push_str("(no finished meetings with notes yet)\n");
    } else {
        p.push_str(excerpts);
    }
    p
}

/// Bullet lines of a reply (`-`, `*`, `•`, `1.`), at most [`MAX_BULLETS`]; a reply without
/// markers contributes its non-empty lines.
fn bullets(reply: &str) -> Vec<String> {
    let marked: Vec<String> = reply
        .lines()
        .filter_map(|l| {
            let t = l.trim();
            let body = t
                .strip_prefix("- ")
                .or_else(|| t.strip_prefix("* "))
                .or_else(|| t.strip_prefix("• "))
                .or_else(|| {
                    let n = t.find(". ")?;
                    t[..n].chars().all(|c| c.is_ascii_digit()).then(|| &t[n + 2..])
                })?;
            let body = body.trim();
            (!body.is_empty()).then(|| body.to_owned())
        })
        .collect();
    let lines = if marked.is_empty() {
        reply
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        marked
    };
    lines.into_iter().take(MAX_BULLETS).collect()
}

/// `brief_put` aliases (§9.4): the event's short label (≤ 4 words) and the attendees' first
/// names (not the user's).
fn aliases(event: &CalEvent) -> Vec<String> {
    let mut out = vec![
        event
            .title
            .split_whitespace()
            .take(4)
            .collect::<Vec<_>>()
            .join(" "),
    ];
    out.extend(
        event
            .attendees
            .iter()
            .filter(|a| !a.is_me)
            .filter_map(|a| a.name.split_whitespace().next())
            .map(str::to_owned),
    );
    out
}

// ---- §9.4 the writer turn

/// One model turn over the brief prompt; the production writer is [`PrivateTurn`], tests pass
/// a canned one.
#[async_trait::async_trait]
pub(crate) trait Writer: Send + Sync {
    async fn write(&self, cx: &Ctx, project: &str, prompt: String) -> Result<String, String>;
}

/// Synthetic owner peer of the brief's Private root (a plugin has no RPC connection).
const PEER: u64 = u64::MAX - 158;

/// §9.4: a Private root on `meeting.model`, the read tools only, one turn, closed after the
/// reply — nothing survives but the brief row the engine files.
pub(crate) struct PrivateTurn;

#[async_trait::async_trait]
impl Writer for PrivateTurn {
    async fn write(&self, cx: &Ctx, project: &str, prompt: String) -> Result<String, String> {
        let agents = ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
            .ok()
            .flatten()
            .ok_or("agent is not running")?;
        let closed = tools::CancellationToken::new();
        let open = agents
            .ask(agent::CreateSession {
                options: agent::CreateSessionOptions {
                    retention: tools::Retention::Private,
                    project: Some(project.to_owned()),
                    view: Some("prebrief".into()),
                    model: cx.model.clone(),
                    memory: Some(agent::MemoryMode::Epochs),
                    ..Default::default()
                },
                peer: Some((PEER, closed.clone())),
            })
            .await
            .map_err(|e| e.to_string())?;
        let session = open.session;
        let turn = async {
            agents
                .ask(agent::SetToolset {
                    session,
                    toolset: Some(agent::Toolset {
                        tools: [
                            "knowledge_search",
                            "knowledge_get",
                            "meeting_read",
                            "meeting_search",
                            "meeting_list",
                            "brief_search",
                        ]
                        .map(str::to_owned)
                        .to_vec(),
                        prompts: vec!["meeting".into(), "knowledge".into()],
                        runtime: false,
                        hint_limit: Some(0),
                    }),
                })
                .await
                .map_err(|e| e.to_string())?;
            agents
                .ask(agent::SendInput {
                    session: Some(session),
                    text: prompt,
                    files: Vec::new(),
                    delivery: agent::Delivery::default(),
                    id: None,
                })
                .await
                .map_err(|e| e.to_string())?;
            let reply = async {
                loop {
                    tokio::time::sleep(Duration::from_secs(1)).await;
                    let loaded = agents
                        .ask(agent::LoadSession {
                            session,
                            before: None,
                            limit: Some(12),
                            settled_only: false,
                        })
                        .await
                        .map_err(|e| e.to_string())?;
                    if loaded.running {
                        continue;
                    }
                    let text = loaded.history.iter().rev().find_map(|h| match h {
                        agent::HistoryEntry::Assistant { text, .. } if !text.trim().is_empty() => {
                            Some(text.trim().to_owned())
                        }
                        _ => None,
                    });
                    if let Some(t) = text {
                        return Ok::<_, String>(t);
                    }
                }
            };
            tokio::time::timeout(TIMEOUT, reply)
                .await
                .map_err(|_| format!("no reply within {} s", TIMEOUT.as_secs()))?
        }
        .await;
        if let Err(e) = agents
            .ask(agent::LeaveSession {
                session,
                lease_secret: open.lease_secret,
                peer: PEER,
            })
            .await
        {
            tracing::warn!(%session, "prebrief: close private root: {e}");
        }
        closed.cancel();
        turn
    }
}

/// Generate for `event` in `project`: inputs → one writer turn → bullets → the brief row
/// (new, or over the previous one). `sources` is what the result is valid for.
async fn generate(
    rt: &Runtime,
    cx: &Ctx,
    writer: &dyn Writer,
    event: &CalEvent,
    project: &str,
    sources: Sources,
    previous: Option<BriefId>,
) -> anyhow::Result<Cached> {
    let mut excerpts = String::new();
    for id in &sources.meetings {
        let Ok(md) = cx.store.summary(*id) else {
            continue;
        };
        let x = excerpt(&md);
        if x.trim().is_empty() {
            continue;
        }
        let m = cx.store.manifest(*id).ok();
        let title = m
            .as_ref()
            .and_then(|m| m.title.clone())
            .unwrap_or_else(|| "Meeting".into());
        let date = m
            .as_ref()
            .map(|m| m.started_at.chars().take(10).collect::<String>())
            .unwrap_or_default();
        excerpts.push_str(&format!("\n## {title} ({date})\n"));
        let mut x = x;
        if let Some((i, _)) = x.char_indices().nth(EXCERPT_CHARS) {
            x.truncate(i);
            x.push('…');
        }
        excerpts.push_str(&x);
    }
    let reply = writer
        .write(cx, project, prompt(event, project, &excerpts))
        .await
        .map_err(anyhow::Error::msg)?;
    let bullets = bullets(&reply);
    anyhow::ensure!(!bullets.is_empty(), "the brief turn produced no bullets");
    // The cache takes 2–3 bullets; a one-line brief still shows on the island, unfiled.
    let mut brief = previous;
    if let Some(cache) = cx.cache.as_ref().filter(|_| bullets.len() >= 2) {
        let put = CachedBriefPut {
            id: previous,
            title: event.title.clone(),
            bullets: bullets.clone(),
            aliases: aliases(event),
            tags: vec!["pre-meeting".into(), project.to_owned()],
        };
        match cache.put(put, &format!("event:{}", event.id)).await {
            Ok((row, _)) => {
                rt.publish(BriefEvent::Changed {
                    id: row.id,
                    removed: false,
                })
                .await;
                brief = Some(row.id);
            }
            Err(e) => tracing::warn!(event = %event.id, "prebrief: brief row: {e}"),
        }
    }
    Ok(Cached {
        bullets,
        brief,
        sources,
    })
}

/// The valid cached brief for `event`, generating when there is none (§9.4 caching: the Done
/// meetings and the event fingerprint must still match).
pub(crate) async fn ensure(
    rt: &Runtime,
    cx: &Ctx,
    writer: &dyn Writer,
    event: &CalEvent,
    project: &str,
) -> anyhow::Result<Cached> {
    let want = Sources {
        meetings: cx
            .store
            .done_in_project(project, PAST_MEETINGS)
            .await?
            .into_iter()
            .map(|s| s.id)
            .collect(),
        fingerprint: fingerprint(event),
    };
    let have = cached(&cx.store, &event.id).await?;
    if let Some(c) = have.as_ref().filter(|c| c.sources == want) {
        return Ok(c.clone());
    }
    let c = generate(
        rt,
        cx,
        writer,
        event,
        project,
        want,
        have.and_then(|c| c.brief),
    )
    .await?;
    put_cached(&cx.store, &event.id, &c).await?;
    Ok(c)
}

// ---- §9.4 the Upcoming tick

/// Per-process state of the one-minute tick: which instances were announced, which failed.
#[derive(Default)]
pub(crate) struct Scheduler {
    emitted: HashSet<EventId>,
    // ponytail: one generation attempt per instance per process; a restart retries.
    failed: HashSet<EventId>,
}

impl Scheduler {
    /// One tick at `now`: generate for matched-or-remembered events inside `start − 15 min`,
    /// return `Upcoming` once per instance from `start − 5 min` to `start + 5 min`.
    pub(crate) async fn tick(
        &mut self,
        rt: &Runtime,
        cx: &Ctx,
        writer: &dyn Writer,
        now: DateTime<Utc>,
    ) -> Vec<MeetingEvent> {
        let mut out = Vec::new();
        if !cx.prebrief {
            return out;
        }
        let from = cal::stamp(now - chrono::Duration::minutes(SHOW_MIN));
        let to = cal::stamp(now + chrono::Duration::minutes(GENERATE_MIN));
        let rows = match cal::events(&cx.store, &from, &to).await {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!("prebrief: events: {e:#}");
                return out;
            }
        };
        let mut seen = HashSet::new();
        for e in rows
            .iter()
            .filter(|e| !e.cancelled && !e.all_day && e.start >= from)
        {
            seen.insert(e.id.clone());
            let r = e.to_ref();
            let project = match lookup(&cx.store, cx.default_project.as_deref(), Some(&r), None).await
            {
                Ok(Some(p)) => p,
                Ok(None) => continue,
                Err(err) => {
                    tracing::warn!(event = %e.id, "prebrief: project: {err:#}");
                    continue;
                }
            };
            let brief = if self.failed.contains(&e.id) {
                None
            } else {
                match ensure(rt, cx, writer, e, &project).await {
                    Ok(c) => Some(c.bullets),
                    Err(err) => {
                        tracing::warn!(event = %e.id, "prebrief: {err:#}");
                        self.failed.insert(e.id.clone());
                        None
                    }
                }
            };
            let starts_in = cal::parse(&e.start)
                .map(|s| s - now)
                .unwrap_or_else(|_| chrono::Duration::max_value());
            if starts_in <= chrono::Duration::minutes(SHOW_MIN) && !self.emitted.contains(&e.id) {
                self.emitted.insert(e.id.clone());
                out.push(MeetingEvent::Upcoming {
                    event: r,
                    project: Some(project),
                    brief,
                });
            }
        }
        self.emitted.retain(|id| seen.contains(id));
        self.failed.retain(|id| seen.contains(id));
        out
    }
}

static UPCOMING: OnceLock<()> = OnceLock::new();

/// Start the one-minute tick once per process (§9.4).
pub fn ensure_upcoming(rt: &Runtime) {
    let rt = rt.clone();
    UPCOMING.get_or_init(move || {
        tokio::spawn(async move {
            let mut s = Scheduler::default();
            loop {
                tokio::select! {
                    () = rt.shutdown.cancelled() => return,
                    () = tokio::time::sleep(Duration::from_secs(60)) => {}
                }
                let Ok(cx) = ctx().await else { continue };
                for ev in s.tick(&rt, &cx, &PrivateTurn, Utc::now()).await {
                    rt.publish(ev).await;
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use parking_lot::Mutex;

    use super::*;
    use crate::calendar::{Attendee, CalSource};
    use crate::{MeetingState, ProcessInfo, store::SUMMARY};

    const FIXTURE: &str = include_str!("../../tests/fixtures/prebrief/summary.md");

    /// A canned model: records every prompt, answers the same bullets.
    struct Canned {
        reply: &'static str,
        prompts: Mutex<Vec<String>>,
        calls: AtomicUsize,
    }

    impl Canned {
        fn new(reply: &'static str) -> Self {
            Self {
                reply,
                prompts: Mutex::new(Vec::new()),
                calls: AtomicUsize::new(0),
            }
        }
        fn calls(&self) -> usize {
            self.calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl Writer for Canned {
        async fn write(&self, _: &Ctx, _: &str, prompt: String) -> Result<String, String> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            self.prompts.lock().push(prompt);
            Ok(self.reply.to_owned())
        }
    }

    struct Rig {
        _dir: tempfile::TempDir,
        rt: Runtime,
        cx: Ctx,
    }

    async fn rig(default_project: Option<&str>) -> Rig {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_owned());
        let store = Arc::new(Store::open(dir.path()).await.unwrap());
        let cache = Arc::new(Cache::open(dir.path()).await.unwrap());
        cal::insert_source(
            &store,
            &CalSource {
                id: "ics1".into(),
                kind: "ics".into(),
                name: "Work".into(),
                enabled: true,
                last_fetch: None,
                last_error: None,
                state: None,
            },
            None,
        )
        .await
        .unwrap();
        Rig {
            _dir: dir,
            rt,
            cx: Ctx {
                store,
                cache: Some(cache),
                model: None,
                default_project: default_project.map(str::to_owned),
                prebrief: true,
            },
        }
    }

    fn att(name: &str, me: bool) -> Attendee {
        Attendee {
            name: name.into(),
            email: None,
            status: None,
            is_me: me,
        }
    }

    /// A weekly-sync instance starting at `start`.
    fn event(start: DateTime<Utc>, sequence: u32) -> CalEvent {
        let end = start + chrono::Duration::minutes(30);
        CalEvent {
            id: EventId(format!("ics1:weekly:{}", cal::stamp(start))),
            source: "ics1".into(),
            uid: "weekly".into(),
            title: "Weekly sync".into(),
            start: cal::stamp(start),
            end: cal::stamp(end),
            join_url: None,
            join_host: Some("zoom.us".into()),
            organizer: Some(att("Dana Park", false)),
            attendees: vec![att("Dana Park", false), att("Lee Wu", false), att("Sky", true)],
            description: Some("Agenda: cutover, runbook".into()),
            all_day: false,
            tz: None,
            cancelled: false,
            sequence,
            location: None,
            recurrence_id: None,
        }
    }

    async fn put_event(store: &Store, e: &CalEvent) {
        let from = cal::stamp(cal::parse(&e.start).unwrap() - chrono::Duration::days(1));
        let to = cal::stamp(cal::parse(&e.start).unwrap() + chrono::Duration::days(1));
        cal::replace_window(store, "ics1", (&from, &to), std::slice::from_ref(e))
            .await
            .unwrap();
    }

    /// A Done meeting of `project` with the fixture summary on disk.
    async fn done(store: &Store, project: &str, started_at: &str, title: &str) -> Manifest {
        let mut m = Manifest::new(
            MeetingId::new(),
            ProcessInfo {
                pid: 1,
                name: "Zoom".into(),
                bundle_id: None,
                active: true,
            },
            started_at.into(),
        );
        m.session = runtime::SessionId::new();
        m.title = Some(title.into());
        m.project = Some(project.into());
        m.state = MeetingState::Done;
        let dir = store.create(&m).await.unwrap();
        std::fs::write(dir.join(SUMMARY), FIXTURE.replace("DR runbook", title)).unwrap();
        m
    }

    #[test]
    fn title_normalises_like_the_memory_key() {
        assert_eq!(norm_title("Weekly Sync — Oct 14 (Tuesday)"), "weekly sync");
        assert_eq!(norm_title("1:1 Dana / Sky"), "dana sky");
        assert_eq!(norm_title("2026-10-10"), "");
        let e = EventRef {
            id: EventId("x".into()),
            uid: "abc".into(),
            title: "Design review, Friday".into(),
            start: String::new(),
            end: String::new(),
            join_url: None,
            join_host: None,
            organizer: None,
            attendees: vec![],
            description: None,
        };
        assert_eq!(
            memory_keys(Some(&e), None),
            ["uid:abc", "title:design review"]
        );
        assert_eq!(memory_keys(None, Some("Design review")), ["title:design review"]);
        assert!(memory_keys(None, Some("10/10")).is_empty());
    }

    /// §9.3 order: `uid:` → `title:` → `meeting.project` → `None`; a cleared project forgets.
    #[tokio::test]
    async fn memory_lookup_order_and_forget() {
        let r = rig(Some("default")).await;
        let store = &r.cx.store;
        let e = event(Utc::now(), 0).to_ref();
        let bare = EventRef {
            uid: "other".into(),
            ..e.clone()
        };
        assert_eq!(
            lookup(store, Some("default"), Some(&e), None).await.unwrap().as_deref(),
            Some("default"),
            "nothing remembered → the default"
        );
        assert_eq!(lookup(store, None, Some(&e), None).await.unwrap(), None);
        // Title memory from a meeting with no event.
        remember(store, None, Some("Weekly sync"), "by-title").await.unwrap();
        assert_eq!(
            lookup(store, Some("default"), Some(&bare), None).await.unwrap().as_deref(),
            Some("by-title")
        );
        // The uid wins over the title.
        remember(store, Some(&e), None, "by-uid").await.unwrap();
        assert_eq!(
            lookup(store, None, Some(&e), None).await.unwrap().as_deref(),
            Some("by-uid")
        );
        assert_eq!(
            lookup(store, None, Some(&bare), None).await.unwrap().as_deref(),
            Some("by-uid"),
            "remembering the event also wrote its title key"
        );
        forget(store, Some(&e), None).await.unwrap();
        assert_eq!(lookup(store, None, Some(&e), None).await.unwrap(), None);
        assert_eq!(lookup(store, None, None, Some("weekly SYNC")).await.unwrap(), None);
        assert_eq!(clean_project(Some("  ".into())), None);
        assert_eq!(clean_project(Some(" x ".into())).as_deref(), Some("x"));
    }

    /// §9.4 excerpt: the three sections in any heading style, nothing else.
    #[test]
    fn excerpt_splits_the_fixture_by_heading() {
        let x = excerpt(FIXTURE);
        assert_eq!(
            x,
            "Decisions:\n\
             - Cutover moved to Nov 3 (decided by Dana)\n\
             - The DR runbook is signed by the on-call lead\n\
             Action items:\n\
             - Lee — draft the DR runbook — due Oct 17\n\
             - Dana — confirm SRE-2386 owner — due Oct 14\n\
             Open questions:\n\
             - Who signs the DR runbook when Lee is out?\n"
        );
        assert!(!x.contains("40 000"), "numbers section is not an input");
        assert!(!x.contains("Budget"), "topics section is not an input");
        assert_eq!(excerpt("# Title\n\nJust prose.\n"), "");
    }

    #[test]
    fn bullets_parse_markers_and_cap() {
        assert_eq!(
            bullets("Here you go:\n- one\n* two\n• three\n- four\n"),
            ["one", "two", "three"]
        );
        assert_eq!(bullets("1. a\n2. b\n"), ["a", "b"]);
        assert_eq!(bullets("plain\n\nlines\n"), ["plain", "lines"]);
        assert!(bullets("   \n").is_empty());
        let e = event(Utc::now(), 0);
        assert_eq!(aliases(&e), ["Weekly sync", "Dana", "Lee"]);
    }

    /// §9.4 caching: one generation per instance; a sequence bump or a new Done meeting of the
    /// project regenerates (over the same brief row); another project's meeting does not.
    #[tokio::test]
    async fn prebrief_caches_and_invalidates() {
        let r = rig(None).await;
        let store = r.cx.store.clone();
        done(&store, "theuth", "2026-10-01T10:00:00Z", "cutover").await;
        let w = Canned::new("- a\n- b\n");
        let start = Utc::now() + chrono::Duration::minutes(10);
        let e = event(start, 0);
        put_event(&store, &e).await;
        let c1 = ensure(&r.rt, &r.cx, &w, &e, "theuth").await.unwrap();
        let c2 = ensure(&r.rt, &r.cx, &w, &e, "theuth").await.unwrap();
        assert_eq!(w.calls(), 1, "second call served from cal_prebrief");
        assert_eq!(c1, c2);
        assert!(c1.brief.is_some());
        // Sequence bump → regenerate, same brief row updated.
        let e2 = event(start, 1);
        put_event(&store, &e2).await;
        let c3 = ensure(&r.rt, &r.cx, &w, &e2, "theuth").await.unwrap();
        assert_eq!(w.calls(), 2);
        assert_eq!(c3.brief, c1.brief);
        // Another project's Done meeting is not an input → still cached.
        done(&store, "other", "2026-10-02T10:00:00Z", "unrelated").await;
        ensure(&r.rt, &r.cx, &w, &e2, "theuth").await.unwrap();
        assert_eq!(w.calls(), 2);
        // A new Done meeting of the project → regenerate.
        done(&store, "theuth", "2026-10-03T10:00:00Z", "runbook").await;
        ensure(&r.rt, &r.cx, &w, &e2, "theuth").await.unwrap();
        assert_eq!(w.calls(), 3);
        let rows = r.cx.cache.as_ref().unwrap().list(Some("pre-meeting"), 10).await.unwrap();
        assert_eq!(rows.len(), 1, "one row, updated in place");
        assert_eq!(cached(&store, &e2.id).await.unwrap().unwrap().bullets, ["a", "b"]);
    }

    /// §16 W3 integration: three Done fixture meetings → one writer turn whose prompt carries
    /// the excerpts and attendees → a brief row with attendee aliases → `Upcoming` once at
    /// T−5 with the bullets; nothing without a resolved project; off with `prebrief = false`.
    #[tokio::test]
    async fn three_done_meetings_one_turn_brief_row_upcoming_at_t_minus_5() {
        let r = rig(None).await;
        let store = r.cx.store.clone();
        for (day, title) in [("01", "cutover"), ("02", "runbook"), ("03", "owner")] {
            done(&store, "theuth", &format!("2026-10-{day}T10:00:00Z"), title).await;
        }
        done(&store, "theuth", "2026-09-01T10:00:00Z", "ancient").await;
        let now = Utc::now();
        let e = event(now + chrono::Duration::minutes(12), 0);
        put_event(&store, &e).await;
        let w = Canned::new("- Last time: cutover moved to Nov 3\n- Open: SRE-2386 owner\n- Who signs the runbook\n");
        let mut s = Scheduler::default();

        // No project remembered and no default → no brief, no event.
        assert!(s.tick(&r.rt, &r.cx, &w, now).await.is_empty());
        assert_eq!(w.calls(), 0);

        // The series is remembered (a past start / set_project did it).
        remember(&store, Some(&e.to_ref()), None, "theuth").await.unwrap();
        // T−12: inside the generation window → one turn, nothing shown yet.
        assert!(s.tick(&r.rt, &r.cx, &w, now).await.is_empty());
        assert_eq!(w.calls(), 1);
        let prompt = w.prompts.lock()[0].clone();
        assert!(prompt.contains("\"Weekly sync\""), "{prompt}");
        assert!(prompt.contains("Dana Park, Lee Wu, Sky"), "attendee names: {prompt}");
        assert!(prompt.contains("Cutover moved to Nov 3"), "excerpt: {prompt}");
        assert!(prompt.contains("## owner (2026-10-03)"), "newest first: {prompt}");
        assert!(!prompt.contains("ancient"), "only the last 3: {prompt}");
        assert!(!prompt.contains("40 000"), "numbers section excluded: {prompt}");
        let rows = r.cx.cache.as_ref().unwrap().list(Some("theuth"), 10).await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].aliases, ["Weekly sync", "Dana", "Lee"]);
        assert_eq!(rows[0].tags, ["pre-meeting", "theuth"]);
        assert_eq!(rows[0].title, "Weekly sync");

        // T−5: `Upcoming` once, from the cache (no second turn); T−4 repeats nothing.
        let t5 = now + chrono::Duration::minutes(7);
        let out = s.tick(&r.rt, &r.cx, &w, t5).await;
        assert_eq!(w.calls(), 1);
        match out.as_slice() {
            [MeetingEvent::Upcoming { event, project, brief }] => {
                assert_eq!(event.id, e.id);
                assert_eq!(project.as_deref(), Some("theuth"));
                assert_eq!(
                    brief.as_deref(),
                    Some(&["Last time: cutover moved to Nov 3".to_owned(), "Open: SRE-2386 owner".into(), "Who signs the runbook".into()][..])
                );
            }
            other => panic!("expected one Upcoming, got {other:?}"),
        }
        assert!(s.tick(&r.rt, &r.cx, &w, t5 + chrono::Duration::minutes(1)).await.is_empty());
        // T+6: out of the window; the instance is forgotten so a later tick never repeats it.
        assert!(s.tick(&r.rt, &r.cx, &w, now + chrono::Duration::minutes(18)).await.is_empty());
        assert!(s.emitted.is_empty());

        // The detection side reads the same memory and cache.
        let mut d = Detection {
            app: ProcessInfo {
                pid: 1,
                name: "zoom".into(),
                bundle_id: None,
                active: true,
            },
            app_id: "us.zoom.xos".into(),
            since: String::new(),
            also: vec![],
            event: Some(crate::calendar::EventMatch {
                event: e.to_ref(),
                score: 1.0,
                alternatives: vec![],
            }),
            project: None,
            brief: None,
        };
        decorate(&store, None, &mut d).await;
        assert_eq!(d.project.as_deref(), Some("theuth"));
        assert_eq!(d.brief.as_ref().map(Vec::len), Some(3));

        // `meeting.calendar.prebrief = false` → the tick is inert.
        let off = Ctx {
            prebrief: false,
            ..r.cx.clone()
        };
        let e2 = event(now + chrono::Duration::minutes(3), 0);
        put_event(&store, &e2).await;
        assert!(Scheduler::default().tick(&r.rt, &off, &w, now).await.is_empty());
        assert_eq!(w.calls(), 1);
    }
}
