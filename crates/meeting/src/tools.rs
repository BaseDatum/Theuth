//! The two live tools (02 §5, 53 §3.3), the four post tools (04 §5), the four
//! brief-cache tools (05 §4), the prompt block (53 §3.4 + 05 §6) and the
//! compaction hint (53 §4.7). All `ReadOnly` (`docs/plugins.md` §3.5): the live
//! pair's only effect is a row in the meeting's own store and a redraw, the post
//! four return data, the cache four touch a store that is not world state
//! (05 §0.5). The live pair is registered while a meeting records; the other
//! eight at plugin start, for good.

use std::sync::Arc;

use kameo::actor::ActorRef;
use serde::{Deserialize, Serialize};
use tools::{RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry, UnregisterTool};

use crate::cache::{CachedBrief, CachedBriefPut};
use crate::store::Store;
use crate::transcript::mmss;
use crate::{
    AddBrief, AddTopic, BriefDelete, BriefId, BriefList, BriefPut, BriefSearch, LIVE_LOG, Meeting,
    MeetingId, MeetingState, TopicId, Track, Utterance, session,
};

/// Most lines one `meeting_read` returns (53 §3.3).
pub const READ_CAP: usize = 400;

/// 53 §3.4 verbatim plus 05 §6's cache paragraph: one static block.
pub const PROMPT: &str = "**Meetings.** \
A user line beginning `[meeting hh:mm:ss]` is a live transcript batch from a meeting the user is \
in (`me:` the user; `Speaker 1:`, `Speaker 2:`… the other participants by voice, numbered in order \
of first speaking — `them:` when the speaker is not yet known). You are not a participant. Your job: notice \
anything the user might have to ask about — a ticket, a system, a person, a project, a number, a \
term of art, a past decision — look it up with your tools (knowledge first), and call \
`meeting_topic` with a label of at most four words as it was spoken and a brief of two or three \
short factual bullets. Never write what the user should say. Do not repeat a topic already in \
`live:` or `dismissed:`. Call nothing when there is nothing worth a brief; reply with no text — \
text replies are not shown. `[meeting request …]` is the user asking you directly: answer with \
`meeting_brief` at once, tools if needed. `[meeting tapped \"X\"]`: the user wants the brief for X \
now — if you already gave one with the topic, do nothing; otherwise `meeting_brief` with `topic`. \
`[meeting deeper … — parent: <id>]`: a longer follow-up on that brief (up to three bullets, more \
specific; pass that id as `parent`). A `[meeting: no brief was produced …]` line means your last turn \
answered a request in text nobody saw — call `meeting_brief` then. After \
the meeting, the first line of the conversation is its summary; `meeting_read()`, `meeting_search()` \
and `meeting_speakers()` reach the full record — use them instead of guessing.\n\n\
Briefs are cached: a `live:` topic marked `(cached …)` already has a brief on the user's screen — \
do not research or re-register it; if you know it is wrong or out of date, `brief_put()` over the \
same id. For a new subject, call `brief_search()` first; a hit → `meeting_topic` with its `brief` id \
and the label as spoken. Only when nothing is cached, look it up (knowledge first) and \
`meeting_topic` with title and bullets — it is cached for next time. Outside a meeting: when the \
user says a meeting is coming up, research each subject and `brief_put()` one brief per subject with \
`aliases` as people will say them (ticket ids, product names, first names) and a `tag` for the \
meeting; when the user says a brief is stale or wrong, `brief_put()` the fix or `brief_delete()` it. \
In a live meeting session (inputs beginning `[meeting …]`) there is no `python` and every tool in \
your tool list — the meeting's allowlist, by default meeting_topic, meeting_brief, brief_search, \
brief_put, knowledge_search, knowledge_get, meeting_read, meeting_search, search_web, read_url — is \
called directly by name; outside one, `meeting_topic` and `meeting_brief` are direct tools and the \
others named here are runtime functions. Order of lookup: \
`brief_search` (live) / `brief_search()` (otherwise), then knowledge, then one web search and one read, then brief.";

/// 02 §4: appended to the compaction instruction for the meeting session — the summariser's
/// fallback while no plugin head is set (02 §4.1: a supplied head never reaches the summariser).
pub const COMPACTION_HINT: &str = "This session is a live meeting. Keep: who is in the meeting, \
what has been discussed in order, decisions and numbers said aloud, the current live topics and \
every dismissed topic.";

/// `meeting_topic` arguments. Flat (02 §10 deviation): the python runtime
/// shows the model `meeting_topic(label, title, bullets, brief)`; a nested
/// `brief` object rendered as `brief: Any` and cost three retries. Either
/// `title` + `bullets`, or `brief` (05 §4).
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TopicArgs {
    /// At most four words, as spoken.
    pub label: String,
    /// Title of the precomputed brief shown when the user taps the topic. Omit when passing `brief`.
    #[serde(default)]
    pub title: Option<String>,
    /// Two or three short factual bullets of that brief. Omit when passing `brief`.
    #[serde(default)]
    pub bullets: Option<Vec<String>>,
    /// Instead of `title`/`bullets`: the UUID `id` of a cached brief as returned by `brief_search` (not a knowledge id) to show for this label.
    #[serde(default)]
    pub brief: Option<BriefId>,
}

/// `meeting_brief` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BriefArgs {
    /// Short title.
    pub title: String,
    /// Two or three short factual bullets.
    pub bullets: Vec<String>,
    /// The live topic this answers (a `[meeting tapped]` line), if any.
    #[serde(default)]
    pub topic: Option<TopicId>,
    /// The brief this goes deeper on (a `[meeting deeper]` line), if any.
    #[serde(default)]
    pub parent: Option<BriefId>,
    /// Also cache it under these spoken aliases (each ≤ 4 words) for future meetings; omit for a one-off answer.
    #[serde(default)]
    pub aliases: Option<Vec<String>>,
}

/// Registers a topic button with its precomputed brief.
pub struct MeetingTopicTool(pub ActorRef<Meeting>);

#[async_trait::async_trait]
impl Tool for MeetingTopicTool {
    const NAME: &'static str = "meeting_topic";
    const RISK: Risk = Risk::ReadOnly;
    // 08 §0.4: the reply is an acknowledgement, not data — stay a direct tool.
    const DIRECT: bool = true;
    type Args = TopicArgs;

    fn description(&self) -> String {
        "During a live meeting: register a topic button for something the user may need to ask \
         about, with the brief that appears when they tap it. Label of at most four words as it \
         was spoken. Either `title` + `bullets` (two or three short factual bullets; the brief is \
         cached for next time) or `brief` (a cached brief id from `brief_search`; the label is \
         learnt as its alias). Refused when the label is already live or was dismissed."
            .into()
    }

    async fn call(&self, a: TopicArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let r = self
            .0
            .ask(AddTopic {
                session: cx.session,
                label: a.label,
                title: a.title,
                bullets: a.bullets,
                brief: a.brief,
            })
            .await
            .map_err(|e| e.to_string());
        Ok(match r {
            Ok(text) => ToolOutput::ok(text),
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// Shows a brief now: the answer to a tap, a request, or a deeper.
pub struct MeetingBriefTool(pub ActorRef<Meeting>);

#[async_trait::async_trait]
impl Tool for MeetingBriefTool {
    const NAME: &'static str = "meeting_brief";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = true;
    type Args = BriefArgs;

    fn description(&self) -> String {
        "During a live meeting: show the user a brief right now — the answer to a `[meeting \
         request …]`, to a `[meeting tapped \"X\"]` (set `topic`), or to a `[meeting deeper …]` \
         (set `parent`). Two or three short factual bullets. Pass `aliases` to also cache it for \
         future meetings."
            .into()
    }

    async fn call(&self, a: BriefArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let r = self
            .0
            .ask(AddBrief {
                session: cx.session,
                title: a.title,
                bullets: a.bullets,
                topic: a.topic,
                parent: a.parent,
                aliases: a.aliases,
            })
            .await
            .map_err(|e| e.to_string());
        Ok(match r {
            Ok(text) => ToolOutput::ok(text),
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// The registry, if the tools plugin is up.
pub fn registry() -> Option<ActorRef<ToolRegistry>> {
    ActorRef::<ToolRegistry>::lookup(tools::REGISTRY_ACTOR)
        .ok()
        .flatten()
}

/// Register both live tools (meeting start).
pub async fn register(me: ActorRef<Meeting>) {
    let Some(reg) = registry() else { return };
    let _ = reg
        .ask(RegisterTool(Arc::new(MeetingTopicTool(me.clone()))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(MeetingBriefTool(me)))).await;
}

/// Unregister both live tools (meeting stop).
pub async fn unregister() {
    let Some(reg) = registry() else { return };
    for name in [MeetingTopicTool::NAME, MeetingBriefTool::NAME] {
        let _ = reg.ask(UnregisterTool(name.to_owned())).await;
    }
}

// ---------------------------------------------------------------------------
// Post tools (04 §5): data readers over the store, not `DIRECT` (08 §0.4).

/// The meeting a call means: the given one, else the one owning the calling
/// session, else the most recent.
async fn resolve(
    store: &Store,
    given: Option<MeetingId>,
    cx: &ToolCx,
) -> Result<MeetingId, String> {
    if let Some(id) = given {
        return Ok(id);
    }
    if let Ok(Some(id)) = store.by_session(cx.session).await {
        return Ok(id);
    }
    store
        .list(1)
        .await
        .map_err(|e| format!("{e:#}"))?
        .first()
        .map(|m| m.id)
        .ok_or_else(|| "no meetings recorded yet".to_owned())
}

fn result(r: Result<String, String>) -> anyhow::Result<ToolOutput> {
    Ok(match r {
        Ok(text) => ToolOutput::ok(text),
        Err(e) => ToolOutput::err(e),
    })
}

/// `meeting_read` arguments.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ReadArgs {
    /// Meeting id; default: the meeting of this session, else the most recent.
    #[serde(default)]
    pub meeting: Option<MeetingId>,
    /// Only segments ending at or after this time (ms on the recording clock).
    #[serde(default)]
    pub from_ms: Option<u64>,
    /// Only segments starting at or before this time.
    #[serde(default)]
    pub to_ms: Option<u64>,
    /// Only this speaker: a label id like `s1`, a name, or — while recording — `me`, `them`
    /// (every other-side line) or `Speaker 2`.
    #[serde(default)]
    pub speaker: Option<String>,
}

/// The transcript of record as `[mm:ss] Name: text` lines (live text while recording).
pub struct MeetingReadTool(pub Arc<Store>);

#[async_trait::async_trait]
impl Tool for MeetingReadTool {
    const NAME: &'static str = "meeting_read";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ReadArgs;

    fn description(&self) -> String {
        "Read a meeting's transcript as `[mm:ss] Name: text` lines — the reviewed transcript of \
         record, or the live text (`me`/`them`) while it is still recording. At most 400 lines per \
         call: narrow with `from_ms`/`to_ms` (recording-clock milliseconds) or `speaker`, and read \
         a long meeting in pieces. Defaults to the meeting this session belongs to."
            .into()
    }

    async fn call(&self, a: ReadArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        result(self.read(a, &cx).await)
    }
}

impl MeetingReadTool {
    async fn read(&self, a: ReadArgs, cx: &ToolCx) -> Result<String, String> {
        let id = resolve(&self.0, a.meeting, cx).await?;
        let manifest = self.0.manifest(id).map_err(|e| format!("{e:#}"))?;
        let recording = matches!(
            manifest.state,
            MeetingState::Starting
                | MeetingState::Recording
                | MeetingState::Paused
                | MeetingState::Stopping
        );
        let lines: Vec<String> = match self.0.transcript(id) {
            Ok(t) if !recording => t.lines(a.from_ms, a.to_ms, a.speaker.as_deref()).collect(),
            _ => {
                let path = self.0.meeting_dir(id).join(LIVE_LOG);
                let mut live: Vec<Utterance> = crate::read_jsonl(&path, crate::LIVE_TEXT_CAP)?;
                // 07 §3: held system lines are appended after later mic lines.
                live.sort_by_key(|u| u.t0_ms);
                let who = a.speaker.as_deref().map(str::to_lowercase);
                live.iter()
                    .filter(|u| a.from_ms.is_none_or(|f| u.t1_ms >= f))
                    .filter(|u| a.to_ms.is_none_or(|t| u.t0_ms <= t))
                    .filter(|u| {
                        who.as_deref().is_none_or(|w| {
                            w == u.who().to_lowercase()
                                || (w == "them" && u.track == Track::System)
                                || u.speaker.as_deref().is_some_and(|s| s == w)
                        })
                    })
                    .map(|u| format!("[{}] {}: {}", mmss(u.t0_ms), u.who(), u.text))
                    .collect()
            }
        };
        if lines.is_empty() {
            return Ok(if recording {
                "nothing said yet".to_owned()
            } else {
                "no transcript lines match (the transcript may not be ready yet)".to_owned()
            });
        }
        let mut out = lines[..lines.len().min(READ_CAP)].join("\n");
        if lines.len() > READ_CAP {
            let more = lines.len() - READ_CAP;
            out.push_str(&format!(
                "\n… {more} more lines; call again with `from_ms` past the last line's time."
            ));
        }
        Ok(out)
    }
}

/// `meeting_search` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchArgs {
    /// Words to find (all must match; speaker names count).
    pub query: String,
    /// One meeting, or every meeting when omitted.
    #[serde(default)]
    pub meeting: Option<MeetingId>,
}

/// Full-text search over transcript segments.
pub struct MeetingSearchTool(pub Arc<Store>);

#[async_trait::async_trait]
impl Tool for MeetingSearchTool {
    const NAME: &'static str = "meeting_search";
    const RISK: Risk = Risk::ReadOnly;
    type Args = SearchArgs;

    fn description(&self) -> String {
        "Full-text search over the transcripts of one or all meetings: each hit is the meeting, \
         its title, `[mm:ss] Name:` and a snippet with the matches in brackets. At most 40 hits; \
         follow up with `meeting_read` around the time."
            .into()
    }

    async fn call(&self, a: SearchArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let hits = match self.0.search(&a.query, a.meeting).await {
            Ok(h) => h,
            Err(e) => return result(Err(format!("{e:#}"))),
        };
        if hits.is_empty() {
            return result(Ok("no matches".into()));
        }
        let lines: Vec<String> = hits
            .iter()
            .map(|h| {
                format!(
                    "{} \"{}\" [{}] {}: {}",
                    h.meeting,
                    h.title,
                    mmss(h.t0_ms),
                    h.speaker,
                    h.snippet
                )
            })
            .collect();
        result(Ok(lines.join("\n")))
    }
}

/// `meeting_speakers` arguments.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct SpeakersArgs {
    /// With a meeting: its speakers and talk time; without: every enrolled profile.
    #[serde(default)]
    pub meeting: Option<MeetingId>,
}

/// Speakers of a meeting, or the enrolled profiles.
pub struct MeetingSpeakersTool {
    /// The store.
    pub store: Arc<Store>,
    /// `<data_dir>/meetings/speakers/`.
    pub speakers: std::path::PathBuf,
}

#[async_trait::async_trait]
impl Tool for MeetingSpeakersTool {
    const NAME: &'static str = "meeting_speakers";
    const RISK: Risk = Risk::ReadOnly;
    type Args = SpeakersArgs;

    fn description(&self) -> String {
        "With `meeting`: who spoke in it (label, name, talk time). Without: every enrolled \
         speaker profile (name, knowledge entity, samples)."
            .into()
    }

    async fn call(&self, a: SpeakersArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let Some(id) = a.meeting else {
            let p = match crate::Profiles::load(&self.speakers) {
                Ok(p) => p.list(),
                Err(e) => return result(Err(format!("{e:#}"))),
            };
            if p.is_empty() {
                return result(Ok("no enrolled speakers".into()));
            }
            let lines: Vec<String> = p
                .iter()
                .map(|p| {
                    format!(
                        "{} — entity: {} — samples: {}",
                        p.name,
                        p.entity.as_deref().unwrap_or("none"),
                        p.samples
                    )
                })
                .collect();
            return result(Ok(lines.join("\n")));
        };
        let t = match self.store.transcript(id) {
            Ok(t) => t,
            Err(e) => return result(Err(format!("{e:#}"))),
        };
        let lines: Vec<String> = t
            .speakers
            .iter()
            .map(|s| {
                let ms: u64 = t
                    .segments
                    .iter()
                    .filter(|g| g.speaker == s.id)
                    .map(|g| g.t1_ms.saturating_sub(g.t0_ms))
                    .sum();
                format!("{} — {} — talk time {}", s.id, s.name, mmss(ms))
            })
            .collect();
        result(Ok(lines.join("\n")))
    }
}

/// `meeting_list` arguments.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct ListArgs {
    /// Most meetings to return (default 20).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Recorded meetings, newest first.
pub struct MeetingListTool(pub Arc<Store>);

#[async_trait::async_trait]
impl Tool for MeetingListTool {
    const NAME: &'static str = "meeting_list";
    const RISK: Risk = Risk::ReadOnly;
    type Args = ListArgs;

    fn description(&self) -> String {
        "Recorded meetings, newest first: id, date, title, duration, state and session id. Use \
         the id with `meeting_read`/`meeting_search`/`meeting_speakers`."
            .into()
    }

    async fn call(&self, a: ListArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let rows = match self.0.list(a.limit.unwrap_or(20)).await {
            Ok(r) => r,
            Err(e) => return result(Err(format!("{e:#}"))),
        };
        if rows.is_empty() {
            return result(Ok("no meetings recorded yet".into()));
        }
        let lines: Vec<String> = rows
            .iter()
            .map(|m| {
                format!(
                    "{} — {} — \"{}\" — {} — {:?} — session {}",
                    m.id,
                    m.started_at,
                    m.title.as_deref().unwrap_or(&m.process_name),
                    m.duration_ms
                        .map_or("in progress".to_owned(), |d| format!("{} min", d / 60_000)),
                    m.state,
                    m.session
                )
            })
            .collect();
        result(Ok(lines.join("\n")))
    }
}

/// Register the four post tools (plugin start).
pub async fn register_post(reg: &ActorRef<ToolRegistry>, store: Arc<Store>) {
    let speakers = store.speakers_dir();
    let _ = reg
        .ask(RegisterTool(Arc::new(MeetingReadTool(store.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(MeetingSearchTool(store.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(MeetingSpeakersTool {
            store: store.clone(),
            speakers,
        })))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(MeetingListTool(store))))
        .await;
}

// ---------------------------------------------------------------------------
// Brief cache tools (05 §4): every session, through the actor.

/// Text for the transcript, the same rows as a Python value for the runtime.
fn structured<T: Serialize>(text: String, data: T) -> ToolOutput {
    ToolOutput {
        structured: serde_json::to_value(data).ok(),
        ..ToolOutput::ok(text)
    }
}

/// One row as the tools print it:
/// `` `<id>` — <title> [<aliases>] (<age>, used N×): <bullets · > ``.
pub fn brief_line(b: &CachedBrief, now: chrono::DateTime<chrono::Utc>) -> String {
    let mut s = format!(
        "`{}` — {} [{}] ({}, used {}×): {}",
        b.id,
        b.title,
        b.aliases.join(", "),
        session::age_of(&b.updated_at, now),
        b.uses,
        b.bullets.join(" · ")
    );
    if !b.tags.is_empty() {
        s.push_str(&format!(" #{}", b.tags.join(" #")));
    }
    s
}

fn brief_lines(rows: &[CachedBrief], empty: &str) -> String {
    if rows.is_empty() {
        return empty.to_owned();
    }
    let now = chrono::Utc::now();
    rows.iter()
        .map(|b| brief_line(b, now))
        .collect::<Vec<_>>()
        .join("\n")
}

/// `brief_search` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BriefSearchArgs {
    /// Words to find (any may match; aliases weigh most, then title, tags, bullets).
    pub query: String,
    /// Most rows (default 8, max 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Full-text search over cached briefs.
pub struct BriefSearchTool(pub ActorRef<Meeting>);

#[async_trait::async_trait]
impl Tool for BriefSearchTool {
    const NAME: &'static str = "brief_search";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = BriefSearchArgs;

    fn description(&self) -> String {
        "Search the cached meeting briefs (title, bullets, spoken aliases, tags) by keywords. Each \
         hit is `id` — title [aliases] (age, uses): bullets. During a meeting, check here before \
         researching a subject; a hit goes to `meeting_topic` as `brief`."
            .into()
    }

    async fn call(&self, a: BriefSearchArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let r = self
            .0
            .ask(BriefSearch {
                query: a.query,
                limit: a.limit,
            })
            .await
            .map_err(|e| e.to_string());
        Ok(match r {
            Ok(rows) => structured(brief_lines(&rows, "no matches"), &rows),
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// `brief_list` arguments.
#[derive(Debug, Default, Deserialize, schemars::JsonSchema)]
pub struct BriefListArgs {
    /// Only briefs carrying this tag.
    #[serde(default)]
    pub tag: Option<String>,
    /// Most rows (default 50, max 50).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Cached briefs, newest first.
pub struct BriefListTool(pub ActorRef<Meeting>);

#[async_trait::async_trait]
impl Tool for BriefListTool {
    const NAME: &'static str = "brief_list";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = BriefListArgs;

    fn description(&self) -> String {
        "Cached meeting briefs, most recently updated first, optionally only those with a `tag` \
         (e.g. the meeting they were prepared for). Same line format as `brief_search`."
            .into()
    }

    async fn call(&self, a: BriefListArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let r = self
            .0
            .ask(BriefList {
                tag: a.tag,
                limit: a.limit,
            })
            .await
            .map_err(|e| e.to_string());
        Ok(match r {
            Ok(rows) => structured(brief_lines(&rows, "no cached briefs"), &rows),
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// `brief_put`: create or update a cached brief.
pub struct BriefPutTool(pub ActorRef<Meeting>);

#[async_trait::async_trait]
impl Tool for BriefPutTool {
    const NAME: &'static str = "brief_put";
    // ReadOnly on purpose (05 §0.5, after `knowledge_put`): asking on every brief would stop
    // the prefill conversation and the meeting's own turns. And a cache row is not world
    // state — the meeting's `briefs.jsonl` keeps the record of what was shown, knowledge
    // keeps the facts, and a lost row costs one research turn to recreate.
    const RISK: Risk = Risk::ReadOnly;
    type Args = CachedBriefPut;

    fn description(&self) -> String {
        "Cache a brief for meetings: a short title, two or three factual bullets, `aliases` as \
         people will say the subject aloud (ticket ids, product names, first names; each at most \
         four words) and optional `tags` (one for the meeting it is for). Put with an existing \
         `id` updates that brief — use it when a cached brief is wrong or stale. The cached brief \
         appears on the user's screen the moment an alias is spoken in a meeting."
            .into()
    }

    async fn call(&self, p: CachedBriefPut, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let r = self
            .0
            .ask(BriefPut {
                put: p,
                source: format!("session:{}", cx.session),
            })
            .await
            .map_err(|e| e.to_string());
        Ok(match r {
            Ok((row, created)) => {
                let how = if created { "new" } else { "updated" };
                ToolOutput::ok(format!("put {} ({how})", row.id))
            }
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// `brief_delete` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct BriefDeleteArgs {
    /// The cached brief.
    pub id: BriefId,
}

/// `brief_delete`: remove a cached brief.
pub struct BriefDeleteTool(pub ActorRef<Meeting>);

#[async_trait::async_trait]
impl Tool for BriefDeleteTool {
    const NAME: &'static str = "brief_delete";
    // ReadOnly as `brief_put` (05 §0.5): a stale brief must be evictable mid-meeting without
    // an approval card; the row is expendable, not world state.
    const RISK: Risk = Risk::ReadOnly;
    type Args = BriefDeleteArgs;

    fn description(&self) -> String {
        "Delete a cached meeting brief by id (from `brief_search`/`brief_list`) — when the user \
         says it is wrong, stale, or no longer wanted."
            .into()
    }

    async fn call(&self, a: BriefDeleteArgs, _: ToolCx) -> anyhow::Result<ToolOutput> {
        let r = self
            .0
            .ask(BriefDelete { id: a.id })
            .await
            .map_err(|e| e.to_string());
        Ok(match r {
            Ok(()) => ToolOutput::ok(format!("deleted {}", a.id)),
            Err(e) => ToolOutput::err(e),
        })
    }
}

/// Register the four brief-cache tools (plugin start, beside the post tools).
pub async fn register_cache(reg: &ActorRef<ToolRegistry>, me: ActorRef<Meeting>) {
    let _ = reg
        .ask(RegisterTool(Arc::new(BriefSearchTool(me.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(BriefListTool(me.clone()))))
        .await;
    let _ = reg
        .ask(RegisterTool(Arc::new(BriefPutTool(me.clone()))))
        .await;
    let _ = reg.ask(RegisterTool(Arc::new(BriefDeleteTool(me)))).await;
}
