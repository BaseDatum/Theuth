//! Direct study text controls; goals use the existing skill invocation path.
use rpc_compose::api::clients::ResearchApiClient;
use super::*;

/// Direct study controls and goal handoff.
#[derive(Default)]
pub struct Autoresearch;
/// Parsed owner operation or existing skill fallback.
#[derive(Clone)]
pub enum Request {
    /// List studies or inspect one slug.
    Status(Option<String>),
    /// Apply an explicit control to a slug.
    Control(String, ::autoresearch::ControlOp),
    /// Deliver steering text to a slug.
    Steer(String, String),
    /// Hand unrecognized arguments to the autoresearch skill.
    Goal(String),
}
/// Study request completion.
pub enum Msg {
    /// Existing owner display text or failure.
    Loaded(Result<String, String>),
}
fn request(args: &str) -> Request {
    match args.split_once(' ').map_or((args, ""), |(a, b)| (a, b.trim())) {
        ("", _) | ("list", "") | ("status", "") => Request::Status(None),
        ("status", slug) if !slug.is_empty() => Request::Status(Some(slug.to_owned())),
        (op @ ("pause" | "resume" | "stop" | "stop_now"), slug) if !slug.is_empty() => {
            let op = match op {
                "pause" => ::autoresearch::ControlOp::Pause,
                "resume" => ::autoresearch::ControlOp::Resume,
                "stop" => ::autoresearch::ControlOp::StopAfter,
                _ => ::autoresearch::ControlOp::StopNow,
            };
            Request::Control(slug.to_owned(), op)
        }
        ("steer", rest) if rest.contains(' ') => {
            let (slug, text) = rest.split_once(' ').unwrap_or((rest, ""));
            Request::Steer(slug.to_owned(), text.trim().to_owned())
        }
        (slug, "") if slug.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') => {
            Request::Status(Some(slug.to_owned()))
        }
        _ => Request::Goal(args.to_owned()),
    }
}
impl Component for Autoresearch {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "autoresearch", interests: &[], slash: &[("/autoresearch", "[<goal> | <slug> | list | pause|resume|stop|stop_now <slug> | steer <slug> <text>]", "autoresearch studies (140): list, open, pause/resume/stop/steer one directly; a goal or anything else hands the `autoresearch` skill to the model")], bindings: &[] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { args, .. } => match request(args) {
                Request::Goal(args) => cx.core(CoreAction::UseSkill { name: "autoresearch".into(), args }),
                request => cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
                    Msg::Loaded(run(&api, request).await)
                }),
            },
            Input::Message(Msg::Loaded(result)) => match result {
                Ok(text) => {
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    cx.plain(Plain::Line(text.clone()));
                    cx.core(CoreAction::Notice(text));
                }
                Err(error) => cx.error(error),
            },
            _ => {}
        }
    }
    fn view(&self, _: Surface<'_>, _: &ViewCx<'_>) -> Option<View> { None }
}
async fn run(api: &Engine, request: Request) -> Result<String, String> {
    Ok(match request {
        Request::Status(slug) => {
            let studies = api.autoresearch_list().await.map_err(crate::reason)?;
            let mut text = match &slug {
                Some(s) => {
                    let Some(info) = studies.iter().find(|i| &i.slug == s) else {
                        return Err(format!("no autoresearch study `{s}`"));
                    };
                    ::autoresearch::tools::render_info(info)
                }
                None if studies.is_empty() => "no autoresearch studies (start one: /autoresearch <goal>)".to_owned(),
                None => studies.iter().map(::autoresearch::tools::render_info).collect::<Vec<_>>().join("\n"),
            };
            if let Some(s) = slug {
                let rows = api.autoresearch_experiments(s, None, 10).await.map_err(crate::reason)?;
                if !rows.is_empty() {
                    text.push_str("\nLast experiments (newest first):");
                    for e in &rows {
                        text.push('\n');
                        text.push_str(&::autoresearch::tools::render_row(e));
                    }
                }
            }
            text
        }
        Request::Control(slug, op) => {
            let info = api.autoresearch_control(slug, op).await.map_err(crate::reason)?;
            ::autoresearch::tools::render_info(&info)
        }
        Request::Steer(slug, text) => {
            let id = api.autoresearch_steer(slug.clone(), text).await.map_err(crate::reason)?;
            format!("steer #{id} queued: the next experiment of `{slug}` reads it; the loop does not pause")
        }
        Request::Goal(_) => unreachable!("goals use the skill port before spawning"),
    })
}

#[cfg(test)]
#[test]
fn autoresearch_control_vs_goal_uses_existing_skill_path() {
    use crate::model::{Command, Model};
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut model = Model::new(String::new(), false);
    model.mount_components(&tx);
    model.submit("/autoresearch pause study-a", &tx);
    let Command::Component(effect) = rx.try_recv().unwrap() else { panic!("control effect") };
    assert!(matches!(effect.request::<Request>(), Some(Request::Control(slug, ::autoresearch::ControlOp::Pause)) if slug == "study-a"));
    model.submit("/autoresearch Improve the search ranking", &tx);
    assert!(matches!(rx.try_recv(), Ok(Command::UseSkill { name, args, session: None, .. }) if name == "autoresearch" && args == "Improve the search ranking"));
}
