//! Predictor state and controls; core approvals and asks stay in the transcript model.
use std::collections::HashMap;
use ::double::{DoublePredicted, DoubleAnswered};
use rpc_compose::api::clients::DreamsApiClient;
use super::*;

#[derive(Default)]
pub(crate) struct Double {
    pub predictions: HashMap<String, DoublePredicted>,
    shown: Option<(uuid::Uuid, String)>,
}
pub(crate) enum Msg {
    Predicted(DoublePredicted),
    Answered(DoubleAnswered),
    Done(Result<String, String>),
}
impl Double {
    fn sync(&mut self, prompt: Option<uuid::Uuid>) {
        if self.shown.as_ref().map(|(id, _)| *id) != prompt {
            self.shown = prompt.map(|id| (id, id.to_string()));
        }
    }
    fn prediction(&self) -> Option<&DoublePredicted> {
        self.shown.as_ref().and_then(|(_, id)| self.predictions.get(id))
    }
    pub(crate) fn chip(&self) -> Option<String> {
        self.prediction().map(|prediction| double_chip(prediction, crate::model::now_ms()))
    }
    fn plain(text: String, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
        cx.plain(Plain::Line(text));
    }
}
impl Component for Double {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "double", bindings: &[],
            slash: &[("/double", "[status|off|pause [off]|train [a|b] [venue]|rollback <version>|cap <class> <rung> [opt-in]]", "the owner's double (126): its numbers, learning on/off, a training run, caps")],
            interests: &[
                Interest { topic: "double.predicted", version: 1, variants: &["DoublePredicted"] },
                Interest { topic: "double.answered", version: 1, variants: &["DoubleAnswered"] },
            ],
        }
    }
    fn decode(&self, event: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> {
        match (event.topic(), event.version(), event.event()["type"].as_str()) {
            (Some("double.predicted"), Some(1), _) | (None, _, Some("DoublePredicted")) => serde::Deserialize::deserialize(event.event()).ok().map(Msg::Predicted),
            (Some("double.answered"), Some(1), _) | (None, _, Some("DoubleAnswered")) => serde::Deserialize::deserialize(event.event()).ok().map(Msg::Answered),
            _ => None,
        }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        self.sync(cx.core.prompt);
        match input {
            Input::Slash { args, .. } => match DoubleCmd::parse(args) {
                None => cx.core(CoreAction::Reject(DoubleCmd::USAGE.into())),
                Some(command) => cx.spawn_request(Lane::Concurrent, command, |api, command| async move { Msg::Done(execute(&api, command).await) }),
            },
            Input::Message(Msg::Predicted(prediction)) => {
                Self::plain(double_chip(&prediction, crate::model::now_ms()), cx);
                self.predictions.insert(prediction.question_id.clone(), prediction);
            }
            Input::Message(Msg::Answered(answer)) => {
                self.predictions.remove(&answer.question_id);
                let text = ::double::chip::answered(&answer);
                if answer.session.is_none_or(|session| cx.core.session == Some(session) || cx.core.members.member(session).is_some()) {
                    cx.core(CoreAction::Notice(text.clone()));
                }
                Self::plain(text, cx);
            }
            Input::Message(Msg::Done(result)) => match result {
                Ok(text) => { Self::plain(text.clone(), cx); cx.core(CoreAction::Notice(text)); }
                Err(error) => cx.error(error),
            },
            Input::Lifecycle(Lifecycle::PromptChanged { resolved: Some(id) }) => { self.predictions.remove(&id.to_string()); }
            Input::Lifecycle(Lifecycle::Prompts(_)) => self.predictions.retain(|id, _| cx.core.prompts.contains(id)),
            Input::Lifecycle(Lifecycle::SessionReset) => *self = Self::default(),
            _ => {}
        }
    }
    fn animating(&self) -> bool { self.prediction().is_some_and(|prediction| prediction.will_act_at.is_some()) }
    fn view(&self, at: Surface<'_>, _: &ViewCx<'_>) -> Option<View> {
        if matches!(at, Surface::PromptBadge) { self.chip().map(View::Text) } else { None }
    }
}
/// One `/double` subcommand (126 §10).
#[derive(Clone, Debug, PartialEq)]
pub enum DoubleCmd {
    /// `double_status`.
    Status,
    /// `enabled: false` in config.yaml via `double_settings_set`; takes effect at restart.
    Off,
    /// `double_pause(on)`.
    Pause(bool),
    /// `double_train(tier, venue)`.
    Train {
        /// `a` | `b`.
        tier: ::double::Tier,
        /// `auto` | `local` | `node:<name>`.
        venue: ::double::Venue,
    },
    /// `double_rollback(version)`.
    Rollback(String),
    /// `double_set_cap(class, rung, mutating_opt_in)`.
    Cap {
        /// The §3 key.
        class: String,
        /// 0..=3.
        rung: u8,
        /// `opt-in` was given.
        opt_in: bool,
    },
}

impl DoubleCmd {
    /// The usage line.
    pub const USAGE: &'static str = "usage: /double [status|off|pause [off]|train [a|b] [auto|local|node:<name>]|rollback <version>|cap <class> <rung> [opt-in]]";

    /// Parse the text after `/double`; `None` is a usage error. A class key may
    /// contain spaces (`approval:shell:mutating:git push:root`), so `cap` reads
    /// its trailing `<rung> [opt-in]` first and the rest is the key.
    pub fn parse(arg: &str) -> Option<Self> {
        let mut w = arg.split_whitespace();
        let head = w.next().unwrap_or("status");
        if head == "cap" {
            let mut rest: Vec<&str> = w.collect();
            let opt_in = rest.last() == Some(&"opt-in");
            if opt_in {
                rest.pop();
            }
            let rung: u8 = rest.pop()?.parse().ok().filter(|r| *r <= 3)?;
            if rest.is_empty() {
                return None;
            }
            return Some(Self::Cap {
                class: rest.join(" "),
                rung,
                opt_in,
            });
        }
        Some(match (head, w.next(), w.next(), w.next()) {
            ("status", None, ..) => Self::Status,
            ("off", None, ..) => Self::Off,
            ("pause", None, ..) => Self::Pause(true),
            ("pause", Some("off"), None, _) => Self::Pause(false),
            ("train", tier, venue, None) => Self::Train {
                tier: match tier.unwrap_or("a") {
                    "a" => ::double::Tier::A,
                    "b" => ::double::Tier::B,
                    _ => return None,
                },
                venue: match ::double::Venue::parse(venue.unwrap_or("auto"))? {
                    ::double::Venue::Off => return None,
                    v => v,
                },
            },
            ("rollback", Some(v), None, _) => Self::Rollback(v.to_owned()),
            _ => return None,
        })
    }
}

/// The chip text for one card's prediction (126 §10): `double: Allow 91 % · L1`, or at L2
/// the countdown `double: Allow 91 % · L2 · answers in 18 s (answer to take over)`.
/// `none` is the double saying it is unsure.
pub fn double_chip(p: &DoublePredicted, now_ms: u64) -> String {
    let mut s = format!("double: {} {:.0} % · L{}", p.choice, p.p * 100.0, p.rung);
    if let Some(left) = ::double::chip::grace_left_s(p, now_ms) {
        s.push_str(&format!(" · answers in {left} s (answer to take over)"));
    }
    s
}

async fn execute(api: &rpc::client::Engine, cmd: DoubleCmd) -> Result<String, String> {
    Ok(match cmd {
            DoubleCmd::Status => double_status_line(&api.double_status().await.map_err(crate::reason)?),
            DoubleCmd::Off => {
                let mut cfg = api.double_settings_get().await.map_err(crate::reason)?;
                cfg.enabled = false;
                api.double_settings_set(cfg).await.map_err(crate::reason)?;
                "double: disabled in config.yaml — takes effect at the next engine start".into()
            }
            DoubleCmd::Pause(on) => {
                let s = api.double_pause(on).await.map_err(crate::reason)?;
                format!(
                    "double: learning {}; {}",
                    if s.paused { "paused" } else { "resumed" },
                    double_status_line(&s)
                )
            }
            DoubleCmd::Train { tier, venue } => format!(
                "double: training run {} started (tier {}, venue {})",
                api.double_train(tier, venue.clone()).await.map_err(crate::reason)?.0,
                tier.as_str(),
                venue.as_config()
            ),
            DoubleCmd::Rollback(v) => double_status_line(&api.double_rollback(v).await.map_err(crate::reason)?),
            DoubleCmd::Cap {
                class,
                rung,
                opt_in,
            } => {
                let c = api.double_set_cap(class, rung, opt_in).await.map_err(crate::reason)?;
                format!(
                    "double: {} cap L{}{}{}",
                    c.key,
                    c.cap,
                    c.cap_reason
                        .as_deref()
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default(),
                    if c.mutating_opt_in {
                        " · mutating opt-in"
                    } else {
                        ""
                    }
                )
            }
    })
}

/// `/double status`: one line from `double_status` (126 §13).
fn double_status_line(s: &::double::DoubleStatus) -> String {
    format!(
        "double: {}{} · version {} · venue {} · 30 d: n={} acc {:.0} % (baseline {:.0} %) ece {:.3} · asks avoided {} · {:.0} min saved{}",
        if s.enabled { "on" } else { "off" },
        if s.paused { " (learning paused)" } else { "" },
        s.active_version.as_deref().unwrap_or("baseline"),
        s.venue,
        s.kpis.n,
        s.kpis.accuracy * 100.0,
        s.baseline.accuracy * 100.0,
        s.kpis.ece,
        s.kpis.asks_avoided,
        s.kpis.minutes_saved,
        s.next_window
            .as_deref()
            .map(|w| format!(" · next window {w}"))
            .unwrap_or_default()
    )
}

#[cfg(test)]
#[test]
fn empty_pending_approvals_removes_its_prediction_without_resolving_the_ask() {
    use crate::model::{Model, UiEvent};
    use serde_json::json;
    let session = SessionId::new();
    let approval = tools::ApprovalId(uuid::Uuid::now_v7());
    let ask = tools::AskId(uuid::Uuid::now_v7());
    let mut model = Model::new("model".into(), true);
    model.session = Some(session);
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    model.components = Registry::empty();
    model.components.register(Double::default()).unwrap();
    model.mount_components(&tx);
    model.apply(UiEvent::Approval(tools::ApprovalRequested { id: approval, session, tool: "shell".into(), args: json!({"command":"echo hi"}), risk: tools::Risk::ReadOnly, human_only: false, always_ask: false, always_ask_why: None }));
    model.apply(UiEvent::Ask(tools::AskRequested { id: ask, session, questions: vec![tools::Question { id: "q".into(), question: "Which?".into(), options: vec![], recommended: None }] }));
    for (id, kind, choice) in [(approval.0, ::double::CardKind::Approval, "Allow"), (ask.0, ::double::CardKind::Ask, "Wait")] {
        let prediction = DoublePredicted { question_id: id.to_string(), session: Some(session), kind, class: "test".into(), choice: choice.into(), index: None, p: 0.9, probs: vec![], rung: 1, will_act_at: None, version: "baseline".into(), explain: Default::default() };
        model.apply(UiEvent::Wire(super::tests::topic("double.predicted", 1, serde_json::to_value(prediction).unwrap())));
    }
    assert_eq!(model.component_text(Surface::PromptBadge), "double: Allow 90 % · L1");
    model.apply(UiEvent::Wire(TopicEvent::control(json!({"type":"PendingPrompts", "approvals":[]}))));
    let owner = model.components.get::<Double>().unwrap();
    assert!(!owner.predictions.contains_key(&approval.0.to_string()));
    assert!(owner.predictions.contains_key(&ask.0.to_string()));
    assert_eq!(model.component_text(Surface::PromptBadge), "double: Wait 90 % · L1");
    assert_eq!(model.asks.front().unwrap().req.id, ask);
    assert_eq!(model.asks.front().unwrap().current, 0);
    assert!(model.approvals.is_empty());
    assert!(rx.try_recv().is_err(), "snapshot reconciliation must not answer another prompt");
}
