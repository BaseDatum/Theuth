// 144-E2 owns this file.
//! Overviews (design 144 §10, wave-2 contract §2): Briefing, FAQ, Timeline, Mind map and the Audio
//! overview are skill-shaped instructions run as a turn in a notebook-bound session (one session per
//! `(notebook, kind)`, reused so Regenerate makes `show`'s next version) that ends in
//! `show(name=…)`. The engine watches the turn's tool events for that `show`, files the artifact as
//! an `ItemKind::Artifact` of the notebook and records the run in `<data_dir>/notebooks/overviews.json`
//! (the "small table of ours": this wave adds no agent migration). `notebook_overviews` lists the
//! records newest version first per kind, a running or failed run on top of its kind.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use kameo::actor::{ActorRef, Spawn};
use kameo::message::{Context, Message};
use parking_lot::Mutex;
use runtime::{Runtime, SessionId};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::lifecycle::{CreateSession, CreateSessionOptions};
use crate::notebooks::invoke;
use crate::supervisor::Agents;
use crate::{AgentEvent, Delivery, HistoryEntry, PinTitle, ReadSession, RenameSession, SendInput};

/// Which overview.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum OverviewKind {
    /// One page, every claim a chip → `briefing.md`.
    Briefing,
    /// One `Accordion` of Q/A → `faq.md`.
    Faq,
    /// One `Timeline` fence, `scale: time` → `timeline.md`.
    Timeline,
    /// One `Tree` (or `Graph`) fence → `mindmap.md`.
    MindMap,
    /// Two-host script + MP3 → `overview-script.md` + `overview.mp3`.
    Audio,
}

impl OverviewKind {
    /// The artifact `show` writes (the script, for `Audio`).
    pub fn file(self) -> &'static str {
        match self {
            Self::Briefing => "briefing.md",
            Self::Faq => "faq.md",
            Self::Timeline => "timeline.md",
            Self::MindMap => "mindmap.md",
            Self::Audio => "overview-script.md",
        }
    }

    /// The session title (pinned) of this kind's overview chat.
    pub fn title(self) -> &'static str {
        match self {
            Self::Briefing => "Overview · Briefing",
            Self::Faq => "Overview · FAQ",
            Self::Timeline => "Overview · Timeline",
            Self::MindMap => "Overview · Mind map",
            Self::Audio => "Overview · Audio",
        }
    }
}

/// Where a run is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "lowercase")]
pub enum OverviewStatus {
    /// The turn (and, for audio, the synthesis) is in flight.
    Running,
    /// The artifact exists.
    Ready,
    /// The run ended without an artifact; `error` says why.
    Failed,
}

/// One overview version (contract §2).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct Overview {
    /// Which overview.
    pub kind: OverviewKind,
    /// The artifact's `call_id` (the mp3 for `Audio`); `None` while running or failed.
    pub artifact: Option<String>,
    /// `briefing.md`, `overview.mp3`, `overview-script.md`, …
    pub name: String,
    /// `show`'s lineage version on `(session, name)`; a running/failed row carries the next number.
    pub version: u32,
    /// Unix ms.
    pub created_ms: i64,
    /// The overview session of this `(notebook, kind)`.
    pub session: SessionId,
    /// Where it is.
    pub status: OverviewStatus,
    /// `one voice available — both hosts read by peter_yearsley` and the like.
    pub note: Option<String>,
    /// Why it failed.
    pub error: Option<String>,
}

/// `notebook_overview_run` reply.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
pub struct OverviewRun {
    /// The overview session the turn runs in.
    pub session: SessionId,
}

// ---------------------------------------------------------------- records

/// One artifact a run produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shown {
    /// File name.
    pub name: String,
    /// The `show` call.
    pub call_id: String,
    /// `show`'s version.
    pub version: u32,
    /// Unix ms.
    pub created_ms: i64,
    /// The card note.
    #[serde(default)]
    pub note: Option<String>,
}

/// The `(notebook, kind)` row: its session and every artifact shown there, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    /// Notebook id.
    pub notebook: String,
    /// Which overview.
    pub kind: OverviewKind,
    /// The bound session.
    pub session: SessionId,
    /// Artifacts shown, oldest first.
    #[serde(default)]
    pub shown: Vec<Shown>,
}

/// Read a JSON array file; missing or unreadable = empty.
pub(crate) fn read_json<T: DeserializeOwned>(path: &Path) -> Vec<T> {
    std::fs::read(path)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default()
}

/// Write a JSON array file (parent created); a failure is logged, never fatal.
pub(crate) fn write_json<T: Serialize>(path: &Path, rows: &[T]) {
    let r = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, serde_json::to_vec_pretty(rows).unwrap_or_default()));
    if let Err(e) = r {
        tracing::warn!(path = %path.display(), "notebooks records: {e}");
    }
}

fn records_path(rt: &Runtime) -> PathBuf {
    rt.data_dir.join("notebooks").join("overviews.json")
}

/// Live run states, keyed by `(notebook, kind)`; cleared on success.
static RUNS: LazyLock<Mutex<HashMap<(String, OverviewKind), Result<(), String>>>> =
    LazyLock::new(Mutex::default);

/// The rows of `notebook_overviews(notebook)` from `records` and `runs` (the live states): per kind the
/// running/failed row first, then every shown version newest first. Pure; tested.
pub fn list(
    records: &[Record],
    runs: &HashMap<(String, OverviewKind), Result<(), String>>,
    notebook: &str,
) -> Vec<Overview> {
    let mut out = Vec::new();
    for r in records.iter().filter(|r| r.notebook == notebook) {
        let latest = r
            .shown
            .iter()
            .filter(|s| s.name == r.kind.file())
            .map(|s| s.version)
            .max()
            .unwrap_or(0);
        if let Some(state) = runs.get(&(r.notebook.clone(), r.kind)) {
            let (status, error) = match state {
                Ok(()) => (OverviewStatus::Running, None),
                Err(e) => (OverviewStatus::Failed, Some(e.clone())),
            };
            out.push(Overview {
                kind: r.kind,
                artifact: None,
                name: match r.kind {
                    OverviewKind::Audio => "overview.mp3".to_owned(),
                    k => k.file().to_owned(),
                },
                version: latest + 1,
                created_ms: chrono::Utc::now().timestamp_millis(),
                session: r.session,
                status,
                note: None,
                error,
            });
        }
        let mut rows: Vec<Overview> = r
            .shown
            .iter()
            .map(|s| Overview {
                kind: r.kind,
                artifact: Some(s.call_id.clone()),
                name: s.name.clone(),
                version: s.version,
                created_ms: s.created_ms,
                session: r.session,
                status: OverviewStatus::Ready,
                note: s.note.clone(),
                error: None,
            })
            .collect();
        rows.sort_by(|a, b| b.version.cmp(&a.version).then_with(|| a.name.cmp(&b.name)));
        out.extend(rows);
    }
    out
}

// ---------------------------------------------------------------- the turn watcher

/// One actor on the bus for every overview run: routes a session's `AgentEvent`s to its run.
#[derive(kameo::Actor, Default)]
struct Watch {
    follows: HashMap<SessionId, mpsc::UnboundedSender<AgentEvent>>,
}

impl Message<AgentEvent> for Watch {
    type Reply = ();
    async fn handle(&mut self, e: AgentEvent, _: &mut Context<Self, Self::Reply>) {
        if let Some(s) = e.session()
            && let Some(tx) = self.follows.get(&s)
            && tx.send(e).is_err()
        {
            self.follows.remove(&s);
        }
    }
}

/// `ask` → the session's event stream until the receiver drops.
struct Follow(SessionId);

impl Message<Follow> for Watch {
    type Reply = mpsc::UnboundedReceiver<AgentEvent>;
    async fn handle(&mut self, m: Follow, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let (tx, rx) = mpsc::unbounded_channel();
        self.follows.insert(m.0, tx);
        rx
    }
}

static WATCH: tokio::sync::OnceCell<ActorRef<Watch>> = tokio::sync::OnceCell::const_new();

async fn watch(rt: &Runtime) -> ActorRef<Watch> {
    WATCH
        .get_or_init(|| async {
            let actor = Watch::spawn_with_mailbox(Watch::default(), runtime::subscriber_mailbox());
            rt.subscribe::<Watch, AgentEvent>(&actor).await;
            actor
        })
        .await
        .clone()
}

/// What the watched turn produced.
#[derive(Debug, Default)]
pub(crate) struct Turn {
    /// `show` calls that landed: `(name, call_id, version, content)`.
    pub shown: Vec<(String, String, u32, Option<String>)>,
    /// The turn's stop reason.
    pub stop: String,
    /// An `Error` event's text.
    pub error: Option<String>,
}

/// `(v<N> · …)` of a `show` receipt.
pub(crate) fn receipt_version(output: &str) -> u32 {
    output
        .split_once("(v")
        .and_then(|(_, rest)| {
            rest.split(|c: char| !c.is_ascii_digit())
                .next()?
                .parse()
                .ok()
        })
        .unwrap_or(1)
}

/// Run the session's events until the turn opened by `marker` (a prefix of its user text) finishes — or
/// `limit` — collecting the `show`s. Events of another turn in flight before ours are skipped.
async fn follow_turn(
    mut rx: mpsc::UnboundedReceiver<AgentEvent>,
    limit: Duration,
    marker: &str,
) -> Turn {
    let mut turn = Turn::default();
    let mut pending: HashMap<String, (String, Option<String>)> = HashMap::new();
    let deadline = tokio::time::Instant::now() + limit;
    let mut ours = false;
    loop {
        let e = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Some(e)) => e,
            Ok(None) => break,
            Err(_) => {
                turn.stop = "timeout".into();
                break;
            }
        };
        if !ours {
            if let AgentEvent::TurnStarted { text, .. } = &e
                && text.starts_with(marker)
            {
                ours = true;
            }
            continue;
        }
        match e {
            AgentEvent::ToolCallStarted {
                call_id,
                name,
                args,
                ..
            } if name == "show" => {
                let name = args["name"]
                    .as_str()
                    .or_else(|| {
                        args["path"]
                            .as_str()
                            .and_then(|p| Path::new(p).file_name()?.to_str())
                    })
                    .unwrap_or_default()
                    .to_owned();
                let content = args["content"].as_str().map(str::to_owned);
                pending.insert(call_id, (name, content));
            }
            AgentEvent::ToolCallFinished {
                call_id,
                output,
                is_error,
                ..
            } => {
                if let Some((name, content)) = pending.remove(&call_id)
                    && !is_error
                {
                    turn.shown
                        .push((name, call_id, receipt_version(&output), content));
                }
            }
            AgentEvent::Error { message, .. } => turn.error = Some(message),
            AgentEvent::TurnFinished { stop_reason, .. } => {
                turn.stop = stop_reason;
                break;
            }
            _ => {}
        }
    }
    turn
}

// ---------------------------------------------------------------- instructions

const COMMON: &str = "Work only from this notebook's context and sources (read more with `doc_read` when a mention is \
not enough). Every factual claim carries a chip: a markdown link to the `theuth://` id of the passage it rests on, \
cited at the block. Call things by title, never by id. Do not ask questions; do not explain what you are doing.";

/// The skill-shaped instruction of `kind`.
pub fn instruction(kind: OverviewKind) -> String {
    let body = match kind {
        OverviewKind::Briefing => {
            "Write a one-page briefing of this notebook: a title, a two-sentence summary, then \
            4–7 short sections (`##`) covering what matters most across the sources — facts, decisions, numbers, open \
            points. Plain markdown, no fences."
        }
        OverviewKind::Faq => {
            "Write the FAQ of this notebook: a `# FAQ` heading, one sentence of scope, then exactly one \
            ```json-render fence holding an `Accordion` whose `titles` are the 6–10 questions a reader would ask and \
            whose children are the answers (Markdown components, each answer 1–3 sentences with chips)."
        }
        OverviewKind::Timeline => {
            "Write the timeline of this notebook: a `# Timeline` heading, one sentence of scope, \
            then exactly one ```json-render fence holding a `Timeline` with `scale: \"time\"`: one item per dated event \
            found in the sources (ISO `when`, `until` for spans, a short `label`, `detail` with the chip, `state` where \
            known). Skip events without a date."
        }
        OverviewKind::MindMap => {
            "Write the mind map of this notebook: a `# Mind map` heading, one sentence of scope, then \
            exactly one ```json-render fence: a `Tree` whose root is the notebook's topic, children the 4–8 themes, \
            grandchildren the key points (each point's label ends with its chip). Use a `Graph` instead only when \
            cross-links between themes matter more than the hierarchy — then say so in the sentence above the fence."
        }
        OverviewKind::Audio => {
            "Write the script of a 6–8 minute audio overview of this notebook (1 000–1 300 words). \
            Step 1, silently: outline 5–7 segments, each with a title, 2–4 points and the ids they cite. Step 2: write \
            the script as a conversation between two named hosts, Ada and Ben, who take turns; every line is one \
            speaker turn on its own paragraph in the form `**Ada:** …` / `**Ben:** …`, never two speakers in one \
            paragraph, no stage directions, no headings inside the dialogue. Open by naming the notebook, close with \
            one takeaway each. Every factual sentence carries a chip (the chip text is not read aloud)."
        }
    };
    format!(
        "{COMMON}\n\n{body}\n\nWhen the document is complete, call `show` once with `content` = the whole document \
         and `name` = \"{}\" (and a short `title`); then reply with one line saying it is shown.",
        kind.file()
    )
}

// ---------------------------------------------------------------- the run

/// The `Watch`ed turn: one turn of `instruction` in `session`, returning what it showed.
async fn run_turn(
    agents: &ActorRef<Agents>,
    rt: &Runtime,
    session: SessionId,
    text: String,
) -> anyhow::Result<Turn> {
    let rx = watch(rt)
        .await
        .ask(Follow(session))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let marker: String = text.chars().take(60).collect();
    agents
        .ask(SendInput {
            session: Some(session),
            text,
            files: Vec::new(),
            delivery: Delivery::NextTurn,
            id: None,
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(follow_turn(rx, Duration::from_secs(20 * 60), &marker).await)
}

/// The last non-empty assistant text of `session`.
async fn last_answer(agents: &ActorRef<Agents>, session: SessionId) -> Option<String> {
    let read = agents
        .ask(ReadSession {
            session,
            caller: session,
        })
        .await
        .ok()?;
    read.history.iter().rev().find_map(|e| match e {
        HistoryEntry::Assistant { text, .. } if !text.trim().is_empty() => {
            Some(text.trim().to_owned())
        }
        _ => None,
    })
}

/// `show{content, name}` as `session` → `(call_id, version, created_ms)`.
pub(crate) async fn show_content(
    session: SessionId,
    name: &str,
    content: &str,
    title: &str,
) -> anyhow::Result<(String, u32, i64)> {
    let out = invoke(
        session,
        "show",
        serde_json::json!({ "content": content, "name": name, "title": title }),
    )
    .await?;
    artifact_of(out.structured)
}

/// `show{path, name}` as `session` → `(call_id, version, created_ms)`.
pub(crate) async fn show_path(
    session: SessionId,
    path: &Path,
    name: &str,
    title: &str,
) -> anyhow::Result<(String, u32, i64)> {
    let out = invoke(
        session,
        "show",
        serde_json::json!({ "path": path, "name": name, "title": title }),
    )
    .await?;
    artifact_of(out.structured)
}

fn artifact_of(structured: Option<serde_json::Value>) -> anyhow::Result<(String, u32, i64)> {
    let a = structured.ok_or_else(|| anyhow::anyhow!("show returned no artifact"))?;
    let call_id = a["call_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("show returned no call_id"))?
        .to_owned();
    let version = a["version"].as_u64().unwrap_or(1) as u32;
    let created_ms = a["created_at"]
        .as_i64()
        .map_or_else(|| chrono::Utc::now().timestamp_millis(), |s| s * 1000);
    Ok((call_id, version, created_ms))
}

/// `notebook_add(notebook, artifact, call_id)` as `session`.
pub(crate) async fn file_artifact(
    session: SessionId,
    notebook: &str,
    call_id: &str,
) -> anyhow::Result<()> {
    invoke(
        session,
        "notebook_add",
        serde_json::json!({ "notebook": notebook, "kind": "artifact", "item": call_id }),
    )
    .await?;
    Ok(())
}

/// The whole run; `Err` becomes the kind's `Failed` row.
async fn run(
    agents: &ActorRef<Agents>,
    rt: &Runtime,
    notebook: &str,
    kind: OverviewKind,
    session: SessionId,
) -> anyhow::Result<Vec<Shown>> {
    let turn = run_turn(agents, rt, session, instruction(kind)).await?;
    let want = kind.file();
    let (call_id, version, content) = match turn
        .shown
        .iter()
        .rev()
        .find(|(name, ..)| name == want)
        .cloned()
    {
        Some((_, call_id, version, content)) => (call_id, version, content),
        None => {
            // The model wrote the document in its reply instead: file it ourselves.
            let text = last_answer(agents, session).await.filter(|t| t.len() > 80);
            let Some(text) = text else {
                anyhow::bail!(
                    "the turn ended ({}) without showing {want}{}",
                    turn.stop,
                    turn.error.map_or(String::new(), |e| format!(": {e}"))
                );
            };
            let (call_id, version, _) = show_content(session, want, &text, kind.title()).await?;
            (call_id, version, Some(text))
        }
    };
    let now = chrono::Utc::now().timestamp_millis();
    let mut shown = vec![Shown {
        name: want.to_owned(),
        call_id: call_id.clone(),
        version,
        created_ms: now,
        note: None,
    }];
    file_artifact(session, notebook, &call_id).await?;
    if kind == OverviewKind::Audio {
        let script = match content {
            Some(c) => c,
            None => read_artifact(session, &call_id).await?,
        };
        let (mp3, mp3_version, note) = crate::overview_audio::render(rt, session, &script).await?;
        file_artifact(session, notebook, &mp3).await?;
        shown.push(Shown {
            name: "overview.mp3".to_owned(),
            call_id: mp3,
            version: mp3_version,
            created_ms: chrono::Utc::now().timestamp_millis(),
            note,
        });
    }
    Ok(shown)
}

/// `artifact_read` of a markdown artifact, header line dropped.
async fn read_artifact(session: SessionId, call_id: &str) -> anyhow::Result<String> {
    let out = invoke(
        session,
        "artifact_read",
        serde_json::json!({ "artifact": call_id, "limit": 10_000 }),
    )
    .await?;
    let body = out
        .content
        .split_once('\n')
        .map_or("", |(_, rest)| rest)
        .lines()
        // `read_file` numbering: `   12→text`.
        .map(|l| l.split_once('→').map_or(l, |(_, t)| t))
        .collect::<Vec<_>>()
        .join("\n");
    Ok(body)
}

/// `ask` → `Result<OverviewRun, String>`: start (or refuse while running) the `(notebook, kind)` run.
#[derive(Clone, Debug)]
pub struct StartOverview {
    /// Notebook id.
    pub notebook: String,
    /// Which overview.
    pub kind: OverviewKind,
}

impl Message<StartOverview> for Agents {
    type Reply = Result<OverviewRun, String>;

    async fn handle(
        &mut self,
        m: StartOverview,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let key = (m.notebook.clone(), m.kind);
        if RUNS.lock().get(&key).is_some_and(Result::is_ok) {
            return Err(format!("{} is already running", m.kind.title()));
        }
        let rt = self.rt.clone();
        let agents = ctx.actor_ref().clone();
        let path = records_path(&rt);
        let session = match read_json::<Record>(&path)
            .iter()
            .find(|r| r.notebook == m.notebook && r.kind == m.kind)
        {
            Some(r) => r.session,
            // No `ask` on ourselves from inside a handler: `notebook_overview_run` creates the
            // bound root through the public `CreateSession`, binds it, then asks again.
            None => return Err(NEW_SESSION.to_owned()),
        };
        RUNS.lock().insert(key.clone(), Ok(()));
        tokio::spawn(async move {
            let r = run(&agents, &rt, &key.0, key.1, session).await;
            let mut records = read_json::<Record>(&path);
            match r {
                Ok(shown) => {
                    if let Some(rec) = records
                        .iter_mut()
                        .find(|r| r.notebook == key.0 && r.kind == key.1)
                    {
                        rec.shown.extend(shown);
                    }
                    write_json(&path, &records);
                    RUNS.lock().remove(&key);
                }
                Err(e) => {
                    tracing::warn!(notebook = %key.0, kind = ?key.1, "overview: {e:#}");
                    RUNS.lock().insert(key, Err(format!("{e:#}")));
                }
            }
        });
        Ok(OverviewRun { session })
    }
}

/// [`StartOverview`]'s "create the session first" answer; [`notebook_overview_run`] handles it.
const NEW_SESSION: &str = "overview: no session yet";

/// `ask` → `Result<(), String>`: record a freshly created session as the `(notebook, kind)` overview chat.
#[derive(Clone, Debug)]
pub struct BindOverview {
    /// Notebook id.
    pub notebook: String,
    /// Which overview.
    pub kind: OverviewKind,
    /// The bound root just created.
    pub session: SessionId,
}

impl Message<BindOverview> for Agents {
    type Reply = Result<(), String>;

    async fn handle(&mut self, m: BindOverview, _: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let path = records_path(&self.rt);
        let mut records = read_json::<Record>(&path);
        if !records
            .iter()
            .any(|r| r.notebook == m.notebook && r.kind == m.kind)
        {
            records.push(Record {
                notebook: m.notebook,
                kind: m.kind,
                session: m.session,
                shown: Vec::new(),
            });
            write_json(&path, &records);
        }
        Ok(())
    }
}

/// `ask` → the overview rows of a notebook.
#[derive(Clone, Debug)]
pub struct ListOverviews(pub String);

impl Message<ListOverviews> for Agents {
    type Reply = Vec<Overview>;

    async fn handle(
        &mut self,
        m: ListOverviews,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let records = read_json::<Record>(&records_path(&self.rt));
        list(&records, &RUNS.lock(), &m.0)
    }
}

/// `notebook_overview_run(notebook, kind)`: run (or re-run → next version) the overview in its session.
/// Owner only (the RPC layer guards).
pub async fn notebook_overview_run(
    agents: &ActorRef<Agents>,
    notebook: String,
    kind: OverviewKind,
) -> anyhow::Result<OverviewRun> {
    let start = StartOverview {
        notebook: notebook.clone(),
        kind,
    };
    match agents.ask(start.clone()).await {
        Ok(run) => return Ok(run),
        Err(kameo::error::SendError::HandlerError(e)) if e == NEW_SESSION => {}
        Err(e) => anyhow::bail!("{e}"),
    }
    let open = agents
        .ask(CreateSession {
            options: CreateSessionOptions {
                notebook: Some(notebook.clone()),
                ..Default::default()
            },
            peer: None,
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let session = open.session;
    let _ = agents
        .ask(RenameSession {
            session,
            title: kind.title().to_owned(),
        })
        .await;
    let _ = agents
        .ask(PinTitle {
            session,
            pinned: true,
        })
        .await;
    agents
        .ask(BindOverview {
            notebook,
            kind,
            session,
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    agents.ask(start).await.map_err(|e| anyhow::anyhow!("{e}"))
}

/// `notebook_overviews(notebook)`: newest version first per kind.
pub async fn notebook_overviews(
    agents: &ActorRef<Agents>,
    notebook: String,
) -> anyhow::Result<Vec<Overview>> {
    agents
        .ask(ListOverviews(notebook))
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))
}

// ---------------------------------------------------------------- the tools

/// Arguments of [`NotebookOverviewRunTool`].
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NotebookOverviewRunArgs {
    /// Notebook id.
    pub notebook: String,
    /// `briefing | faq | timeline | mind_map | audio`.
    pub kind: OverviewKind,
}

/// `notebook_overview_run` (Mutating): generate (or regenerate → next version) one overview.
pub struct NotebookOverviewRunTool;

#[async_trait::async_trait]
impl tools::Tool for NotebookOverviewRunTool {
    const NAME: &'static str = "notebook_overview_run";
    const RISK: tools::Risk = tools::Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = NotebookOverviewRunArgs;

    fn description(&self) -> String {
        "Generate an overview of a notebook as an artifact filed in it — `briefing` (one page), `faq` \
         (Accordion), `timeline` (dated events), `mind_map` (Tree) or `audio` (two-host MP3 with its script). \
         Runs in the notebook's overview chat and returns at once with that session; running it again makes the \
         next version. `notebook_overviews` lists them."
            .into()
    }

    async fn call(
        &self,
        a: NotebookOverviewRunArgs,
        _: tools::ToolCx,
    ) -> anyhow::Result<tools::ToolOutput> {
        let agents = match crate::peers::supervisor() {
            Ok(a) => a,
            Err(out) => return Ok(tools::ToolOutput::err(out)),
        };
        match notebook_overview_run(&agents, a.notebook, a.kind).await {
            Ok(run) => Ok(tools::ToolOutput {
                structured: serde_json::to_value(run).ok(),
                ..tools::ToolOutput::ok(format!(
                    "{} is generating in session:{}; `notebook_overviews` shows it when ready.",
                    a.kind.title(),
                    run.session
                ))
            }),
            Err(e) => Ok(tools::ToolOutput::err(format!(
                "notebook_overview_run: {e:#}"
            ))),
        }
    }
}

/// Arguments of [`NotebookOverviewsTool`].
#[derive(Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct NotebookOverviewsArgs {
    /// Notebook id.
    pub notebook: String,
}

/// `notebook_overviews` (ReadOnly): the overview versions of a notebook, newest first per kind.
pub struct NotebookOverviewsTool;

#[async_trait::async_trait]
impl tools::Tool for NotebookOverviewsTool {
    const NAME: &'static str = "notebook_overviews";
    const RISK: tools::Risk = tools::Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = NotebookOverviewsArgs;

    fn description(&self) -> String {
        "List a notebook's overviews (briefing, FAQ, timeline, mind map, audio): per kind the running or failed \
         run first, then every version newest first with its artifact `call_id` (read it with `artifact_read`)."
            .into()
    }

    async fn call(
        &self,
        a: NotebookOverviewsArgs,
        _: tools::ToolCx,
    ) -> anyhow::Result<tools::ToolOutput> {
        let agents = match crate::peers::supervisor() {
            Ok(a) => a,
            Err(out) => return Ok(tools::ToolOutput::err(out)),
        };
        let rows = notebook_overviews(&agents, a.notebook).await?;
        let mut text = format!("{} overviews\n", rows.len());
        for r in &rows {
            text.push_str(&format!(
                "- {:?} {} v{} {:?}{}{}\n",
                r.kind,
                r.name,
                r.version,
                r.status,
                r.artifact
                    .as_deref()
                    .map_or(String::new(), |c| format!(" artifact={c}")),
                r.error
                    .as_deref()
                    .map_or(String::new(), |e| format!(" error={e}")),
            ));
        }
        Ok(tools::ToolOutput {
            structured: serde_json::to_value(&rows).ok(),
            ..tools::ToolOutput::ok(text)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(name: &str, v: u32, ms: i64) -> Shown {
        Shown {
            name: name.into(),
            call_id: format!("{name}-{v}"),
            version: v,
            created_ms: ms,
            note: None,
        }
    }

    #[test]
    fn list_orders_versions_newest_first_with_the_live_run_on_top() {
        let s = SessionId::new();
        let records = vec![
            Record {
                notebook: "NB".into(),
                kind: OverviewKind::Briefing,
                session: s,
                shown: vec![shown("briefing.md", 1, 10), shown("briefing.md", 2, 20)],
            },
            Record {
                notebook: "NB".into(),
                kind: OverviewKind::Audio,
                session: s,
                shown: vec![
                    shown("overview-script.md", 1, 30),
                    shown("overview.mp3", 1, 31),
                ],
            },
            Record {
                notebook: "OTHER".into(),
                kind: OverviewKind::Faq,
                session: s,
                shown: vec![shown("faq.md", 1, 5)],
            },
        ];
        let mut runs = HashMap::new();
        let rows = list(&records, &runs, "NB");
        let briefing: Vec<(u32, &str)> = rows
            .iter()
            .filter(|r| r.kind == OverviewKind::Briefing)
            .map(|r| (r.version, r.artifact.as_deref().unwrap()))
            .collect();
        assert_eq!(briefing, [(2, "briefing.md-2"), (1, "briefing.md-1")]);
        assert!(rows.iter().all(|r| r.status == OverviewStatus::Ready));
        assert!(
            !rows.iter().any(|r| r.kind == OverviewKind::Faq),
            "another notebook's row"
        );
        let audio: Vec<&str> = rows
            .iter()
            .filter(|r| r.kind == OverviewKind::Audio)
            .map(|r| r.name.as_str())
            .collect();
        assert_eq!(
            audio,
            ["overview-script.md", "overview.mp3"],
            "same version, name order"
        );

        runs.insert(("NB".to_owned(), OverviewKind::Briefing), Ok(()));
        runs.insert(
            ("NB".to_owned(), OverviewKind::Audio),
            Err("tts down".to_owned()),
        );
        let rows = list(&records, &runs, "NB");
        let top = &rows[0];
        assert_eq!(
            (top.kind, top.status, top.version),
            (OverviewKind::Briefing, OverviewStatus::Running, 3)
        );
        assert!(top.artifact.is_none());
        let failed = rows
            .iter()
            .find(|r| r.status == OverviewStatus::Failed)
            .unwrap();
        assert_eq!(
            (failed.kind, failed.version, failed.name.as_str()),
            (OverviewKind::Audio, 2, "overview.mp3")
        );
        assert_eq!(failed.error.as_deref(), Some("tts down"));
    }

    #[test]
    fn receipt_version_parses_show_output() {
        assert_eq!(
            receipt_version("shown briefing.md (v3 · 40 lines · 2 KB)"),
            3
        );
        assert_eq!(
            receipt_version("shown x.mp3 (v12 · 480 KB)\nnote: unknown kind"),
            12
        );
        assert_eq!(receipt_version("garbage"), 1);
    }

    #[tokio::test]
    async fn follow_turn_collects_the_show_that_landed() {
        let (tx, rx) = mpsc::unbounded_channel();
        let s = SessionId::new();
        let turn = crate::TurnId::default();
        let started =
            |call_id: &str, name: &str, args: serde_json::Value| AgentEvent::ToolCallStarted {
                session: s,
                turn,
                call_id: call_id.into(),
                name: name.into(),
                args,
            };
        let finished = |call_id: &str, output: &str, is_error: bool| AgentEvent::ToolCallFinished {
            session: s,
            turn,
            call_id: call_id.into(),
            output: output.into(),
            is_error,
            files: vec![],
        };
        let start = |text: &str| AgentEvent::TurnStarted {
            session: s,
            turn,
            text: text.into(),
            model: "m".into(),
            turn_seq: 0,
        };
        let done = AgentEvent::TurnFinished {
            session: s,
            turn,
            stop_reason: "stop".into(),
            usage: Default::default(),
            context: 0,
        };
        // A user's turn still in flight ends first: its show is not ours.
        tx.send(started(
            "c0",
            "show",
            serde_json::json!({ "name": "faq.md", "content": "theirs" }),
        ))
        .unwrap();
        tx.send(finished("c0", "shown faq.md (v1 · 1 lines · 1 KB)", false))
            .unwrap();
        tx.send(done.clone()).unwrap();
        tx.send(start("Work only from this notebook …")).unwrap();
        tx.send(started("c1", "doc_read", serde_json::json!({ "doc": "x" })))
            .unwrap();
        tx.send(finished("c1", "text", false)).unwrap();
        tx.send(started(
            "c2",
            "show",
            serde_json::json!({ "name": "faq.md", "content": "# FAQ" }),
        ))
        .unwrap();
        tx.send(finished("c2", "show: name needs an extension", true))
            .unwrap();
        tx.send(started(
            "c3",
            "show",
            serde_json::json!({ "name": "faq.md", "content": "# FAQ v2" }),
        ))
        .unwrap();
        tx.send(finished("c3", "shown faq.md (v2 · 3 lines · 1 KB)", false))
            .unwrap();
        tx.send(done).unwrap();
        let t = follow_turn(rx, Duration::from_secs(1), "Work only from this notebook").await;
        assert_eq!(t.stop, "stop");
        assert_eq!(
            t.shown,
            vec![(
                "faq.md".to_owned(),
                "c3".to_owned(),
                2,
                Some("# FAQ v2".to_owned())
            )]
        );
    }

    #[test]
    fn instructions_name_their_file() {
        for k in [
            OverviewKind::Briefing,
            OverviewKind::Faq,
            OverviewKind::Timeline,
            OverviewKind::MindMap,
            OverviewKind::Audio,
        ] {
            assert!(instruction(k).contains(&format!("\"{}\"", k.file())));
        }
        assert!(instruction(OverviewKind::Audio).contains("**Ada:**"));
    }
}
