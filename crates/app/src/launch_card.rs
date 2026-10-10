//! 159 W6 — the launch card (159 §11 "Launch card and attention row"): an app card like the
//! study draft, posted to the study's origin chat when a world experiment asks for its press:
//! the diff summary, the arms, the exposure, the window, *Approve* and *Refuse*. Both presses
//! run `autoresearch.launch {slug, n, token, approve}` with the press token the `launches` row
//! hashes and the model never sees — "the press, not the model, starts the study" (140 §12.4)
//! holds for every launch too (159 §15 "Must not regress").
//!
//! Owner: W6. The skeleton (C) registers the module (`pub mod launch_card` in lib.rs,
//! `// 159:W6`) and freezes [`lines`], [`source`] and [`post`]. An OpenUI `Mutation` runs only
//! an installed tool, and the plugin (crate `autoresearch`) cannot call into this crate, so two
//! small tools carry the card (W6-c): [`LaunchCardTool`] (`autoresearch_launch_card`), which
//! `world::post_card` invokes through the registry with the origin session and the token, and
//! [`LaunchTool`] (`autoresearch_launch`), the Approve/Refuse press, which forwards to
//! `autoresearch::rpc_impl::launch`. Nothing here runs for a 140 study.

use std::sync::Arc;

use autoresearch::StudySpec;
use kameo::actor::ActorRef;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput, ToolRegistry};

use crate::actor::Core;
use crate::draft::lit;
use crate::tool::{AppArgs, Origin, commit_source};

/// The OpenUI app name the card publishes under.
pub const APP_NAME: &str = "study-launches";

/// What the card shows (W6 fills it from the `launches` row and the experiment).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Launch {
    /// Study.
    pub slug: String,
    /// Experiment.
    pub n: u32,
    /// The diff summary the session logged.
    pub summary: String,
    /// Share of traffic both arms take.
    pub exposure: f64,
    /// The window, humantime.
    pub window: String,
    /// The press token (never shown to the model).
    pub token: String,
}

/// The card's plain lines: "Launch #<n> of `<slug>`: <summary>", the arms and the exposure, the
/// window, the rollback command.
pub fn lines(spec: &StudySpec, launch: &Launch) -> [String; 4] {
    let rollback = spec
        .world
        .as_ref()
        .map(|w| w.rollback.trim())
        .filter(|r| !r.is_empty())
        .map(|r| format!("rollback `{r}` runs on every close"))
        .unwrap_or_else(|| "no rollback command".to_owned());
    [
        format!(
            "Launch #{} of `{}`: {}",
            launch.n,
            spec.slug,
            launch.summary.trim()
        ),
        format!(
            "two arms, {:.0} % of traffic together ({:.0} % each): control = the incumbent, candidate = this tree",
            launch.exposure * 100.0,
            launch.exposure * 50.0
        ),
        format!("windows of {}; {rollback}", launch.window),
        "Approve deploys both arms; Refuse closes the experiment".to_owned(),
    ]
}

/// The OpenUI source of the card with its Approve and Refuse presses wired through the token.
pub fn source(spec: &StudySpec, launch: &Launch) -> String {
    let slug = spec.slug.as_str();
    let [l1, l2, l3, l4] = lines(spec, launch);
    let mut out = String::new();
    out.push_str("root = Card([head, l1, l2, l3, l4, buttons, result])\n");
    out.push_str(&format!(
        "head = CardHeader({}, {})\n",
        lit(&format!("launch #{}", launch.n)),
        lit(&format!("study {slug}"))
    ));
    out.push_str(&format!("l1 = TextContent({})\n", lit(&format!("**{l1}**"))));
    for (name, text) in [("l2", &l2), ("l3", &l3), ("l4", &l4)] {
        out.push_str(&format!("{name} = TextContent({}, \"small\")\n", lit(text)));
    }
    for (var, approve) in [("approve", true), ("refuse", false)] {
        out.push_str(&format!(
            "{var} = Mutation(\"{}\", {{slug: {}, n: {}, token: {}, approve: {approve}}})\n",
            LaunchTool::NAME,
            lit(slug),
            launch.n,
            lit(&launch.token)
        ));
    }
    out.push_str("buttons = Buttons([bApprove, bRefuse])\n");
    out.push_str(&format!(
        "bApprove = Button(\"Approve\", Action([@Run(approve), @ToAssistant({})]))\n",
        lit(&format!("launch #{} of {slug}: approved", launch.n))
    ));
    out.push_str(&format!(
        "bRefuse = Button(\"Refuse\", Action([@Run(refuse), @ToAssistant({})]), \"tertiary\", \"destructive\")\n",
        lit(&format!("launch #{} of {slug}: refused", launch.n))
    ));
    out.push_str(
        "result = TextContent(approve.status == \"error\" ? (\"Could not launch: \" + approve.error) : \
         (refuse.status == \"error\" ? (\"Could not refuse: \" + refuse.error) : \
         (approve.status == \"success\" ? \"Both arms are live; the first window is running.\" : \
         (refuse.status == \"success\" ? \"Refused — nothing went live; the study runs on.\" : \"\"))), \"small\")\n",
    );
    out
}

/// Post the card to the study's origin chat (called by `world::launch` plugin-side).
pub async fn post(core: Arc<Core>, spec: &StudySpec, launch: &Launch) -> Result<(), String> {
    let detail = autoresearch::rpc_impl::get(launch.slug.clone()).await?;
    let origin = detail
        .info
        .origin
        .ok_or_else(|| format!("study {} has no origin chat", launch.slug))?;
    post_to(&core, origin, spec, launch).await
}

/// Commit the card as an app artifact of `session` (a synthetic `ui-` call id: no model turn,
/// the session's artifact list shows it).
async fn post_to(
    core: &Core,
    session: runtime::SessionId,
    spec: &StudySpec,
    launch: &Launch,
) -> Result<(), String> {
    let app = AppArgs {
        name: APP_NAME.into(),
        source: source(spec, launch),
        title: Some(format!("Launch #{} of {}", launch.n, spec.slug)),
        edit: false,
        base_call_id: None,
    };
    let call_id = format!("ui-{}", uuid::Uuid::now_v7());
    commit_source(core, app, session, call_id, Origin::Ui)
        .await
        .map(drop)
}

/// `autoresearch_launch_card` arguments: what `world::post_card` sends.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct LaunchCardArgs {
    /// Study.
    pub slug: String,
    /// Experiment.
    pub n: u32,
    /// The idea and hypothesis the session gave.
    #[serde(default)]
    pub summary: String,
    /// Share of traffic both arms take.
    #[serde(default)]
    pub exposure: f64,
    /// The window, humantime.
    #[serde(default)]
    pub window: String,
    /// The press token.
    pub token: String,
}

/// The plugin's way to post a launch card: invoked through the registry with the study's
/// origin session (quiet, so the model sees neither the call nor the token). Useless to the
/// model: a card with a made-up token is refused at the press.
pub struct LaunchCardTool {
    /// Shared with the actor.
    pub core: Arc<Core>,
}

#[async_trait::async_trait]
impl Tool for LaunchCardTool {
    const NAME: &'static str = "autoresearch_launch_card";
    const RISK: Risk = Risk::ReadOnly;
    type Args = LaunchCardArgs;

    fn description(&self) -> String {
        "Internal: posts a world study's launch card (Approve / Refuse) into this chat. The autoresearch \
         harness calls it when an experiment waits for the user's press; it needs the press token, which you \
         do not have."
            .into()
    }

    async fn call(&self, a: LaunchCardArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let detail = match autoresearch::rpc_impl::get(a.slug.clone()).await {
            Ok(d) => d,
            Err(e) => return Ok(ToolOutput::err(format!("autoresearch_launch_card: {e}"))),
        };
        let launch = Launch {
            slug: a.slug,
            n: a.n,
            summary: a.summary,
            exposure: a.exposure,
            window: a.window,
            token: a.token,
        };
        Ok(match post_to(&self.core, cx.session, &detail.spec, &launch).await {
            Ok(()) => ToolOutput::ok(format!(
                "launch card #{} of {} posted",
                launch.n, launch.slug
            )),
            Err(e) => ToolOutput::err(format!("autoresearch_launch_card: {e}")),
        })
    }
}

/// `autoresearch_launch` arguments: the card's press.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct LaunchArgs {
    /// Study.
    pub slug: String,
    /// Experiment.
    pub n: u32,
    /// The press token the card holds.
    pub token: String,
    /// Approve (deploy both arms) or refuse (close the experiment).
    pub approve: bool,
}

/// The launch card's Approve/Refuse press: `autoresearch.launch` with the card's token.
/// Registered like any tool (an OpenUI `Mutation` can only run installed tools), useless to the
/// model: it never sees the token.
pub struct LaunchTool;

#[async_trait::async_trait]
impl Tool for LaunchTool {
    const NAME: &'static str = "autoresearch_launch";
    const RISK: Risk = Risk::Mutating;
    const RETURNS: &'static str = "dict";
    type Args = LaunchArgs;

    fn description(&self) -> String {
        "Internal: the Approve/Refuse press of a world study's launch card. It needs the press token the card \
         holds, which you do not have — a launch goes live only from the user's press."
            .into()
    }

    async fn call(&self, a: LaunchArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let info = autoresearch::rpc_impl::launch(a.slug, a.n, a.token, a.approve)
            .await
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        let text = format!(
            "launch #{} of {}: {}; study {}",
            a.n,
            info.slug,
            if a.approve { "approved, both arms live" } else { "refused" },
            serde_json::to_value(&info.state)
                .ok()
                .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_owned))
                .unwrap_or_default()
        );
        Ok(ToolOutput {
            structured: serde_json::to_value(&info).ok(),
            ..ToolOutput::ok(text)
        })
    }
}

/// Register both tools (the one `// 159:W6` line in lib.rs).
pub async fn register(reg: &ActorRef<ToolRegistry>, core: Arc<Core>) {
    let _ = reg
        .ask(tools::RegisterTool(Arc::new(LaunchCardTool { core })))
        .await;
    let _ = reg.ask(tools::RegisterTool(Arc::new(LaunchTool))).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use autoresearch::WorldSpec;

    fn spec() -> StudySpec {
        StudySpec {
            slug: "landing".into(),
            world: Some(WorldSpec {
                rollback: "bash rollback.sh".into(),
                ..WorldSpec::default()
            }),
            ..StudySpec::default()
        }
    }

    fn launch() -> Launch {
        Launch {
            slug: "landing".into(),
            n: 3,
            summary: "shorter headline — urgency lifts sign-ups".into(),
            exposure: 0.1,
            window: "24h".into(),
            token: "t0k".into(),
        }
    }

    #[test]
    fn lines_name_the_launch_the_arms_and_the_rollback() {
        let [l1, l2, l3, l4] = lines(&spec(), &launch());
        assert_eq!(l1, "Launch #3 of `landing`: shorter headline — urgency lifts sign-ups");
        assert!(l2.starts_with("two arms, 10 % of traffic together (5 % each)"), "{l2}");
        assert_eq!(l3, "windows of 24h; rollback `bash rollback.sh` runs on every close");
        assert!(l4.starts_with("Approve deploys"));
    }

    /// Both presses run `autoresearch_launch` with the token; the model-facing texts carry no token.
    #[test]
    fn source_wires_both_presses_through_the_token() {
        let s = source(&spec(), &launch());
        assert!(s.contains("approve = Mutation(\"autoresearch_launch\", {slug: \"landing\", n: 3, token: \"t0k\", approve: true})"), "{s}");
        assert!(s.contains("refuse = Mutation(\"autoresearch_launch\", {slug: \"landing\", n: 3, token: \"t0k\", approve: false})"), "{s}");
        assert!(s.contains("bApprove = Button(\"Approve\", Action([@Run(approve), @ToAssistant(\"launch #3 of landing: approved\")]))"));
        assert!(s.contains("\"tertiary\", \"destructive\""));
        assert!(s.contains("Buttons([bApprove, bRefuse])"));
        for l in lines(&spec(), &launch()) {
            assert!(!l.contains("t0k"));
        }
    }
}
