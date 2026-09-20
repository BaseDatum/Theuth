//! The two live tools (02 §5, 53 §3.3), the prompt block (53 §3.4) and the
//! compaction hint (53 §4.7). Both tools are `ReadOnly` (`docs/plugins.md`
//! §3.5): their only effect is a row in the meeting's own store and a redraw —
//! no world state. Registered while a meeting records, unregistered at stop.

use std::sync::Arc;

use kameo::actor::ActorRef;
use serde::Deserialize;
use tools::{RegisterTool, Risk, Tool, ToolCx, ToolOutput, ToolRegistry, UnregisterTool};

use crate::{AddBrief, AddTopic, BriefId, Meeting, TopicId};

/// 53 §3.4, verbatim: one static paragraph.
pub const PROMPT: &str = "**Meetings.** \
A user line beginning `[meeting hh:mm:ss]` is a live transcript batch from a meeting the user is \
in (`them:` other participants, `me:` the user). You are not a participant. Your job: notice \
anything the user might have to ask about — a ticket, a system, a person, a project, a number, a \
term of art, a past decision — look it up with your tools (knowledge first), and call \
`meeting_topic` with a label of at most four words as it was spoken and a brief of two or three \
short factual bullets. Never write what the user should say. Do not repeat a topic already in \
`live:` or `dismissed:`. Call nothing when there is nothing worth a brief; reply with no text — \
text replies are not shown. `[meeting request …]` is the user asking you directly: answer with \
`meeting_brief` at once, tools if needed. `[meeting tapped \"X\"]`: the user wants the brief for X \
now — if you already gave one with the topic, do nothing; otherwise `meeting_brief` with `topic`. \
`[meeting deeper …]`: a longer follow-up on that brief (up to three bullets, more specific). After \
the meeting, the first line of the conversation is its summary; `meeting_read`, `meeting_search` \
and `meeting_speakers` reach the full record — use them instead of guessing.";

/// 02 §4: appended to the compaction instruction for the meeting session.
pub const COMPACTION_HINT: &str = "This session is a live meeting. Keep: who is in the meeting, \
what has been discussed in order, decisions and numbers said aloud, the current live topics and \
every dismissed topic.";

/// `meeting_topic` arguments. Flat (02 §10 deviation): the python runtime
/// shows the model `meeting_topic(label: str, title: str, bullets: list[str])`;
/// a nested `brief` object rendered as `brief: Any` and cost three retries.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TopicArgs {
    /// At most four words, as spoken.
    pub label: String,
    /// Title of the precomputed brief shown when the user taps the topic.
    pub title: String,
    /// Two or three short factual bullets of that brief.
    pub bullets: Vec<String>,
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
         was spoken; two or three short factual bullets. Refused when the label is already live \
         or was dismissed."
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
         (set `parent`). Two or three short factual bullets."
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
