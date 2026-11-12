//! The `routine_draft` tool (108 §8): the `routines` skill proposes a routine
//! as an OpenUI card — the §2.2 sentence, the four parts, the prompt, and
//! three `@ToAssistant` buttons: *Create paused*, *Create and start*, *Open in
//! form*. Same pattern as [`crate::agent_draft`]: the press arrives as the
//! user's message naming the draft id and the choice; the assistant then calls
//! `schedule` (the press is the approval) or, for *Open in form*, nothing — the
//! Routines page already has the draft from this call's `args`. Drafts are
//! never written: a draft id lives in the card only. No *Discard*: a draft
//! that is not pressed is nothing.

use std::collections::BTreeSet;
use std::sync::Arc;

use schedule::file;
use schedule::tools::ScheduleArgs;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::actor::Core;
use crate::draft::lit;
use crate::tool::{AppArgs, Origin, commit_source};

/// App name every draft set is published under (a new version per call).
const APP_NAME: &str = "routine-drafts";

/// Chars of the prompt shown on the card.
const PREVIEW: usize = 600;

/// One proposed routine: the fields of `schedules/<name>.md`, named as
/// [`ScheduleArgs`] names them so the model's next call is a field-for-field copy.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoutineDraft {
    /// `[a-z0-9][a-z0-9-]*`, ≤ 40 chars. Your own routine of this name is replaced (the card says so); a name another machine's routine uses is refused.
    pub name: String,
    /// The one-line sentence from the skill's grammar; shown on the card.
    pub sentence: String,
    /// Standard cron, local time. Exactly one of cron / every / at.
    pub cron: Option<String>,
    /// Interval (`30m`, `60s`; `0s` = again as soon as a run ends).
    pub every: Option<String>,
    /// One instant, local (`2026-09-16T15:00`).
    pub at: Option<String>,
    /// The body: what the run does.
    pub prompt: String,
    /// The reply is not delivered; only `message_user` reaches the user.
    #[serde(default)]
    pub quiet: bool,
    /// Model for the run.
    pub model: Option<String>,
    /// Reasoning effort for that model (`low`/`high`, a level it takes); needs `model`.
    pub effort: Option<String>,
    /// Stop command (exit 0 = done).
    pub until: Option<String>,
    /// Stop after this many runs.
    pub max_runs: Option<u32>,
    /// Time limit for one run (`45m`).
    pub run_timeout: Option<String>,
    /// Directory (`~` allowed). Must exist.
    pub cwd: Option<String>,
    /// false: the run never asks permission.
    pub approvals: Option<bool>,
    /// A roster agent: each fire creates a task owned by it.
    pub agent: Option<String>,
    /// The project label of that task.
    pub project: Option<String>,
    /// Watcher: `<file>.py key=value …`.
    pub when: Option<String>,
    /// `on_change` / `while_true` / `once`.
    pub fire: Option<String>,
    /// Consecutive true checks must span this long before the first fire.
    #[serde(rename = "for")]
    pub for_: Option<String>,
    /// Minimum spacing between fires.
    pub cooldown: Option<String>,
    /// `skip` / `queue` / `restart`.
    pub mode: Option<String>,
    /// `continue` / `clear`.
    pub memory: Option<String>,
    /// TTL (`14d`).
    pub expires: Option<String>,
    /// Sensor wall clock (`20s`).
    pub timeout: Option<String>,
}

impl RoutineDraft {
    /// The `schedule` call the press leads to, without `enabled`.
    fn schedule_args(&self) -> ScheduleArgs {
        ScheduleArgs {
            name: self.name.clone(),
            cron: self.cron.clone(),
            every: self.every.clone(),
            at: self.at.clone(),
            prompt: self.prompt.clone(),
            quiet: self.quiet,
            model: self.model.clone(),
            effort: self.effort.clone(),
            until: self.until.clone(),
            max_runs: self.max_runs,
            run_timeout: self.run_timeout.clone(),
            cwd: self.cwd.clone(),
            approvals: self.approvals,
            enabled: None,
            agent: self.agent.clone(),
            project: self.project.clone(),
            when: self.when.clone(),
            fire: self.fire.clone(),
            for_: self.for_.clone(),
            cooldown: self.cooldown.clone(),
            mode: self.mode.clone(),
            memory: self.memory.clone(),
            // 131: the draft UI has no runtime switch; the file default (`clear`).
            runtime: None,
            expires: self.expires.clone(),
            timeout: self.timeout.clone(),
        }
    }
}

/// Arguments of `routine_draft`.
#[derive(Clone, Debug, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RoutineDraftArgs {
    /// One or more proposed routines (normally one).
    pub routines: Vec<RoutineDraft>,
}

/// `routine_draft`: proposed routines as approval cards in chat.
pub struct RoutineDraftTool {
    /// Shared with the actor.
    pub core: Arc<Core>,
}

/// What a button press becomes: the `enabled` the `schedule` call lands with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Enabled(pub bool);

/// The approval for `choice` (`create paused` → `Enabled(false)`, `create and
/// start` → `Enabled(true)`; anything else, incl. `open in form`, is `None`:
/// no `schedule` call).
pub fn approval(choice: &str) -> Option<Enabled> {
    match choice.trim() {
        "create paused" => Some(Enabled(false)),
        "create and start" => Some(Enabled(true)),
        _ => None,
    }
}

#[async_trait::async_trait]
impl Tool for RoutineDraftTool {
    const NAME: &'static str = "routine_draft";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    const CONSUMES_SECRETS: bool = false;
    type Args = RoutineDraftArgs;

    fn description(&self) -> String {
        "Propose a routine (a schedules/<name>.md file) for the user's approval before anything is written. \
         After the `routines` skill's questions, call this with 1..n drafts {name, sentence, cron|every|at, \
         prompt, …the schedule keys, `effort` beside `model`}. Each renders as a card (sentence, Trigger · Stop · Run · Concurrency, \
         prompt) with three buttons: Create paused, Create and start, Open in form. Then STOP and wait: the \
         press arrives as the user's next message (it begins `[app routine-drafts …]`) naming the draft id and \
         the choice; the receipt says what to call. The draft is validated as `schedule` would (name, parse, \
         sensor resolves) but not dry-run; nothing exists until approval."
            .into()
    }

    async fn call(&self, args: RoutineDraftArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if args.routines.is_empty() {
            return Ok(ToolOutput::err("routine_draft: at least one routine"));
        }
        let data_dir = &self.core.rt.data_dir;
        let dir = data_dir.join(schedule::DIR);
        let sensors =
            agent::sensor::SensorRunner::new(data_dir.clone(), None, None, Default::default());
        let existing: BTreeSet<String> = args
            .routines
            .iter()
            .filter(|d| dir.join(format!("{}.md", d.name)).is_file())
            .map(|d| d.name.clone())
            .collect();
        let mut seen = BTreeSet::new();
        for d in &args.routines {
            if let Err(e) = validate(d, &sensors) {
                return Ok(ToolOutput::err(format!("routine_draft: {}: {e}", d.name)));
            }
            // 185 C8: the press would be refused by `schedule`; refuse the card.
            if let Some(why) = schedule::tools::name_in_use(&d.name, true).await {
                return Ok(ToolOutput::err(format!("routine_draft: {}: {why}", d.name)));
            }
            if !seen.insert(d.name.as_str()) {
                return Ok(ToolOutput::err(format!(
                    "routine_draft: {} is drafted twice",
                    d.name
                )));
            }
        }
        let alnum: Vec<char> = cx
            .call_id
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        let suffix: String = alnum[alnum.len().saturating_sub(4)..].iter().collect();
        let (source, ids) = source(&args.routines, &existing, &suffix);
        let app = AppArgs {
            name: APP_NAME.into(),
            source,
            title: Some(if ids.len() == 1 {
                "Routine draft".into()
            } else {
                format!("{} routine drafts", ids.len())
            }),
            edit: false,
            base_call_id: None,
        };
        let (loaded, _) = match commit_source(
            &self.core,
            app,
            cx.session,
            cx.call_id.clone(),
            Origin::Tool,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => return Ok(ToolOutput::err(format!("routine_draft: {e}"))),
        };
        let lines: Vec<String> = ids
            .iter()
            .zip(&args.routines)
            .map(|(id, d)| {
                format!(
                    "{id} · {} — {}{}",
                    d.name,
                    d.sentence.trim(),
                    if existing.contains(d.name.as_str()) {
                        " (exists: replaces)"
                    } else {
                        ""
                    }
                )
            })
            .collect();
        let content = format!(
            "drafted {} routine card{} (record_call_id {}):\n{}\nWait for the user. Their press arrives as a message \
             beginning `[app {APP_NAME} …]` with `routine draft <id>: create paused | create and start | open in form`. \
             Then: create paused → `schedule` with that draft's fields and `enabled: false`; create and start → \
             `schedule` with `enabled: true`; open in form → answer one line, call nothing.",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" },
            loaded.artifact.call_id,
            lines.join("\n"),
        );
        Ok(ToolOutput {
            structured: Some(serde_json::json!({
                "record_call_id": loaded.artifact.call_id,
                "drafts": ids.iter().zip(&args.routines).map(|(id, d)| serde_json::json!({ "id": id, "name": d.name, "exists": existing.contains(d.name.as_str()) })).collect::<Vec<_>>(),
            })),
            ..ToolOutput::ok(content)
        })
    }
}

/// The draft as `schedule` would refuse it, before any card is drawn: the name
/// rule, the rendered file's parse, a non-empty sentence, and the sensor
/// resolving (not dry-run — that is `schedule`'s job after the press).
fn validate(d: &RoutineDraft, sensors: &agent::sensor::SensorRunner) -> Result<(), String> {
    if !file::valid_name(&d.name) {
        return Err("name: [a-z0-9][a-z0-9-]*, at most 40 chars".into());
    }
    if d.sentence.trim().is_empty() {
        return Err("sentence: empty".into());
    }
    let spec = file::parse(&d.name, &schedule::tools::render_args(&d.schedule_args()))?;
    if let Some(s) = &spec.sensor {
        sensors.resolve(&s.file).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// `Trigger … · Stop … · Run … · Concurrency …` from the fields with clause
/// labels only (the sentence grammar lives in the skill and the UI, not here).
fn parts(d: &RoutineDraft) -> String {
    let trigger = match (&d.cron, &d.every, &d.at, &d.when) {
        (Some(c), _, _, _) => format!("cron {c}"),
        (_, Some(e), _, Some(w)) => format!(
            "every {e} when {}",
            w.split_whitespace().next().unwrap_or_default()
        ),
        (_, Some(e), _, None) => format!("every {e}"),
        (_, _, Some(a), _) => format!("at {a}"),
        _ => "—".to_owned(),
    };
    let mut stop = Vec::new();
    if let Some(u) = &d.until {
        stop.push(format!("until {u}"));
    }
    if let Some(n) = d.max_runs {
        stop.push(format!("max_runs {n}"));
    }
    if let Some(e) = &d.expires {
        stop.push(format!("expires {e}"));
    }
    let mut run = Vec::new();
    if d.quiet {
        run.push("quiet".to_owned());
    }
    if let Some(a) = &d.agent {
        run.push(match &d.project {
            Some(p) => format!("agent {a} project {p}"),
            None => format!("agent {a}"),
        });
    }
    if d.approvals == Some(false) {
        run.push("approvals false".to_owned());
    }
    if let Some(m) = &d.model {
        run.push(format!("model {m}"));
    }
    if let Some(e) = &d.effort {
        run.push(format!("effort {e}"));
    }
    if let Some(c) = &d.cwd {
        run.push(format!("cwd {c}"));
    }
    if let Some(t) = &d.run_timeout {
        run.push(format!("run_timeout {t}"));
    }
    let join = |v: Vec<String>, empty: &str| {
        if v.is_empty() {
            empty.to_owned()
        } else {
            v.join(", ")
        }
    };
    let concurrency = match (&d.when, &d.mode) {
        (Some(_), Some(m)) => m.clone(),
        (Some(_), None) => "skip".to_owned(),
        (None, _) => "deferred".to_owned(),
    };
    format!(
        "Trigger `{trigger}` · Stop `{}` · Run `{}` · Concurrency `{concurrency}`",
        join(stop, "never"),
        join(run, "defaults"),
    )
}

/// The card set's OpenUI source and the draft ids, one per routine (`r1-<suffix>`, …).
/// Every button's `@ToAssistant` message is `routine draft <id>: <choice>`; a name in
/// `existing` adds a note that *Create…* replaces the file.
pub fn source(
    routines: &[RoutineDraft],
    existing: &BTreeSet<String>,
    suffix: &str,
) -> (String, Vec<String>) {
    let ids: Vec<String> = (1..=routines.len())
        .map(|i| {
            if suffix.is_empty() {
                format!("r{i}")
            } else {
                format!("r{i}-{suffix}")
            }
        })
        .collect();
    let cards: Vec<String> = (1..=routines.len()).map(|i| format!("c{i}")).collect();
    let mut out = format!("root = Stack([intro, {}])\n", cards.join(", "));
    let n = routines.len();
    let s = if n == 1 { "" } else { "s" };
    let intro = format!(
        "**{n} routine draft{s}** — *Create paused* writes the file disabled, *Create and start* writes it live, \
         *Open in form* puts it in the editor beside this chat. Nothing is written until you press."
    );
    out.push_str(&format!(
        "intro = TextContent({}, \"small\")\n",
        lit(&intro)
    ));
    for (i, (d, id)) in routines.iter().zip(&ids).enumerate() {
        let c = &cards[i];
        let exists = existing.contains(d.name.as_str());
        let prompt = d.prompt.trim();
        let preview: String = if prompt.chars().count() > PREVIEW {
            format!("{}…", prompt.chars().take(PREVIEW).collect::<String>())
        } else {
            prompt.to_owned()
        };
        let mut members = vec![
            format!("{c}_head"),
            format!("{c}_sentence"),
            format!("{c}_parts"),
            format!("{c}_prompt"),
        ];
        if exists {
            members.push(format!("{c}_note"));
        }
        members.push(format!("{c}_buttons"));
        out.push_str(&format!("{c} = Card([{}])\n", members.join(", ")));
        out.push_str(&format!(
            "{c}_head = CardHeader({}, {})\n",
            lit(&d.name),
            lit(&format!("routine draft {id}"))
        ));
        out.push_str(&format!(
            "{c}_sentence = TextContent({})\n",
            lit(&format!("**{}**", d.sentence.trim()))
        ));
        out.push_str(&format!(
            "{c}_parts = TextContent({}, \"small\")\n",
            lit(&parts(d))
        ));
        out.push_str(&format!("{c}_prompt = TextContent({})\n", lit(&preview)));
        if exists {
            out.push_str(&format!(
                "{c}_note = TextContent({}, \"small\")\n",
                lit(&format!(
                    "`{0}` exists — *Create…* replaces schedules/{0}.md.",
                    d.name
                ))
            ));
        }
        out.push_str(&format!(
            "{c}_buttons = Buttons([{c}_paused, {c}_start, {c}_form])\n"
        ));
        out.push_str(&format!(
            "{c}_paused = Button(\"Create paused\", Action([@ToAssistant({})]))\n",
            lit(&format!("routine draft {id}: create paused"))
        ));
        out.push_str(&format!(
            "{c}_start = Button(\"Create and start\", Action([@ToAssistant({})]), \"secondary\")\n",
            lit(&format!("routine draft {id}: create and start"))
        ));
        out.push_str(&format!(
            "{c}_form = Button(\"Open in form\", Action([@ToAssistant({})]), \"tertiary\")\n",
            lit(&format!("routine draft {id}: open in form"))
        ));
    }
    (out, ids)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft(name: &str) -> RoutineDraft {
        RoutineDraft {
            name: name.into(),
            sentence: "Weekdays at 09:00".into(),
            cron: Some("0 9 * * 1-5".into()),
            every: None,
            at: None,
            prompt: "Brief me on \"today\".\nKeep it short.".into(),
            quiet: false,
            model: None,
            effort: None,
            until: None,
            max_runs: None,
            run_timeout: None,
            cwd: None,
            approvals: None,
            agent: None,
            project: None,
            when: None,
            fire: None,
            for_: None,
            cooldown: None,
            mode: None,
            memory: None,
            expires: None,
            timeout: None,
        }
    }

    fn watcher(name: &str) -> RoutineDraft {
        RoutineDraft {
            sentence:
                "Every 60s checks pr_watch.py on repo=o/r pr=7 · fires on change · at most 40 times"
                    .into(),
            cron: None,
            every: Some("60s".into()),
            when: Some("pr_watch.py repo=o/r pr=7 token=gh_token".into()),
            max_runs: Some(40),
            cooldown: Some("5m".into()),
            quiet: true,
            approvals: Some(false),
            cwd: Some("~".into()),
            mode: Some("queue".into()),
            ..draft(name)
        }
    }

    #[test]
    fn card_buttons_carry_the_draft_id_and_choice_and_note_an_existing_name() {
        let existing: BTreeSet<String> = ["brief".to_owned()].into();
        let drafts = vec![watcher("argo"), draft("brief")];
        let (src, ids) = source(&drafts, &existing, "ab12");
        assert_eq!(ids, ["r1-ab12", "r2-ab12"]);
        assert!(src.starts_with("root = Stack([intro, c1, c2])\n"));
        for (id, c) in ids.iter().zip(["c1", "c2"]) {
            assert!(src.contains(&format!("{c}_paused = Button(\"Create paused\", Action([@ToAssistant(\"routine draft {id}: create paused\")]))")), "{src}");
            assert!(src.contains(&format!("{c}_start = Button(\"Create and start\", Action([@ToAssistant(\"routine draft {id}: create and start\")]), \"secondary\")")), "{src}");
            assert!(src.contains(&format!("{c}_form = Button(\"Open in form\", Action([@ToAssistant(\"routine draft {id}: open in form\")]), \"tertiary\")")), "{src}");
            assert!(
                src.contains(&format!("\"routine draft {id}\"")),
                "the header names the draft"
            );
        }
        assert!(!src.contains("Discard"), "no discard button: {src}");
        assert!(src.contains("c1 = Card([c1_head, c1_sentence, c1_parts, c1_prompt, c1_buttons])"));
        assert!(src.contains(
            "c2 = Card([c2_head, c2_sentence, c2_parts, c2_prompt, c2_note, c2_buttons])"
        ));
        assert!(src.contains("`brief` exists — *Create…* replaces schedules/brief.md."));
        assert!(src.contains(r#"c1_head = CardHeader("argo", "routine draft r1-ab12")"#));
        assert!(src.contains(r#"c2_sentence = TextContent("**Weekdays at 09:00**")"#));
        assert!(
            src.contains(r#"c2_prompt = TextContent("Brief me on \"today\".\nKeep it short.")"#)
        );
        assert!(src.contains("Trigger `every 60s when pr_watch.py` · Stop `max_runs 40` · Run `quiet, approvals false, cwd ~` · Concurrency `queue`"), "{src}");
        assert!(src.contains("Trigger `cron 0 9 * * 1-5` · Stop `never` · Run `defaults` · Concurrency `deferred`"), "{src}");
        assert!(src.contains("**2 routine drafts**"));
        assert!(src.lines().all(|l| l.contains(" = ")));
        // One draft, no suffix.
        let (src, ids) = source(&drafts[1..], &existing, "");
        assert_eq!(ids, ["r1"]);
        assert!(src.contains("**1 routine draft**"));
        // A long prompt is cut.
        let long = RoutineDraft {
            prompt: "x".repeat(PREVIEW + 1),
            ..draft("brief")
        };
        assert!(
            source(&[long], &existing, "")
                .0
                .contains(&format!("{}…", "x".repeat(PREVIEW)))
        );
    }

    #[test]
    fn approval_maps_choice() {
        assert_eq!(approval("create paused"), Some(Enabled(false)));
        assert_eq!(approval(" create and start "), Some(Enabled(true)));
        assert_eq!(approval("open in form"), None);
        assert_eq!(approval("discard"), None);
    }

    #[test]
    fn validate_refuses_what_schedule_would() {
        let tmp = tempfile::tempdir().unwrap();
        let sensors = agent::sensor::SensorRunner::new(
            tmp.path().to_path_buf(),
            None,
            None,
            Default::default(),
        );
        sensors.install_builtins().unwrap();
        assert!(validate(&draft("brief"), &sensors).is_ok());
        assert!(
            validate(&watcher("argo"), &sensors).is_ok(),
            "resolves, not dry-run"
        );
        assert!(
            validate(&draft("Brief"), &sensors)
                .unwrap_err()
                .contains("name:")
        );
        assert_eq!(
            validate(
                &RoutineDraft {
                    sentence: " ".into(),
                    ..draft("brief")
                },
                &sensors
            )
            .unwrap_err(),
            "sentence: empty"
        );
        assert_eq!(
            validate(
                &RoutineDraft {
                    cron: None,
                    every: Some("5s".into()),
                    ..draft("fast")
                },
                &sensors
            )
            .unwrap_err(),
            "every: below 10s"
        );
        assert_eq!(
            validate(
                &RoutineDraft {
                    max_runs: None,
                    ..watcher("argo")
                },
                &sensors
            )
            .unwrap_err(),
            "when: needs a stop (until, max_runs or expires)"
        );
        assert_eq!(
            validate(
                &RoutineDraft {
                    when: Some("missing.py".into()),
                    ..watcher("argo")
                },
                &sensors
            )
            .unwrap_err(),
            "when: no such sensor: missing.py"
        );
        assert!(
            validate(
                &RoutineDraft {
                    prompt: String::new(),
                    ..draft("brief")
                },
                &sensors
            )
            .is_err()
        );
    }

    /// 194 row 18: a draft's `effort` is on the card's Run line beside `model`, reaches the
    /// `schedule` call the press leads to, and is validated as `schedule` would.
    #[test]
    fn effort_rides_with_the_model() {
        let d = RoutineDraft {
            model: Some("a/b".into()),
            effort: Some("high".into()),
            ..draft("brief")
        };
        let (src, _) = source(std::slice::from_ref(&d), &BTreeSet::new(), "");
        assert!(src.contains("Run `model a/b, effort high`"), "{src}");
        let text = schedule::tools::render_args(&d.schedule_args());
        assert!(text.contains("model: a/b\neffort: high\n"), "{text}");
        let tmp = tempfile::tempdir().unwrap();
        let sensors = agent::sensor::SensorRunner::new(
            tmp.path().to_path_buf(),
            None,
            None,
            Default::default(),
        );
        assert!(validate(&d, &sensors).is_ok());
        let bare = RoutineDraft { model: None, ..d };
        assert_eq!(
            validate(&bare, &sensors).unwrap_err(),
            "effort: needs model:"
        );
    }
}
