//! Harness-wide AWS profile selection and status.
use super::*;
use agent::AwsProfiles;
use rpc_compose::api::clients::AgentSettingsApiClient;

/// Mounted AWS profile picker; row zero is the default credential chain.
#[derive(Default)]
pub struct Aws {
    /// Discovered profile rows and highlighted index.
    pub picker: Option<(AwsProfiles, usize)>,
}
/// Typed owner requests retained on effects.
#[derive(Clone, Debug)]
pub enum Request {
    /// Read SDK-discovered profiles and the active harness-wide selection.
    ListAwsProfiles,
    /// Select a profile, or restore the SDK default credential chain.
    SetAwsProfile(Option<String>),
}
/// AWS setting readbacks and failures.
pub enum Msg {
    /// Current harness-wide profile configuration.
    Profiles {
        /// SDK profiles and the selection in force.
        info: AwsProfiles,
        /// Open the picker when there is a choice.
        pick: bool,
    },
    /// Preserve the existing generic-error path.
    Failed(String),
}
impl Aws {
    fn request(request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            let pick = matches!(request, Request::ListAwsProfiles);
            let result = match request {
                Request::ListAwsProfiles => api.list_aws_profiles().await,
                Request::SetAwsProfile(profile) => api.set_aws_profile(profile).await,
            };
            match result { Ok(info) => Msg::Profiles { info, pick }, Err(error) => Msg::Failed(crate::reason(error)) }
        });
    }
    /// Default credential chain followed by SDK profile names; mark the active row.
    pub fn rows(&self) -> Vec<String> {
        let Some((info, _)) = &self.picker else { return Vec::new() };
        let mark = |on: bool| if on { "●" } else { " " };
        std::iter::once(format!(
            "{} default credential chain ({})", mark(info.selected.is_none()), info.default_profile
        )).chain(info.profiles.iter().map(|p| format!("{} {p}", mark(info.selected.as_deref() == Some(p.as_str()))))).collect()
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        let Some((info, cursor)) = &mut self.picker else { return };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(info.profiles.len()),
            KeyCode::Esc => { self.picker = None; cx.close(); }
            KeyCode::Enter => {
                let picked = cursor.checked_sub(1).map(|i| info.profiles.get(i).cloned());
                self.picker = None;
                cx.close();
                match picked {
                    None => Self::request(Request::SetAwsProfile(None), cx),
                    Some(Some(profile)) => Self::request(Request::SetAwsProfile(Some(profile)), cx),
                    Some(None) => {}
                }
            }
            _ => {}
        }
    }
}
impl Component for Aws {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "aws", interests: &[], slash: &[("/aws", "[profile|clear]", "AWS profile for Bedrock, harness-wide (picker without arg)")], bindings: &[] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Slash { args, .. } => Self::request(if args.is_empty() { Request::ListAwsProfiles } else { Request::SetAwsProfile((args != "clear").then(|| args.to_owned())) }, cx),
            Input::Message(Msg::Profiles { info, pick }) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                let line = aws_profile_line(&info);
                cx.plain(Plain::Line(line.clone()));
                cx.core(CoreAction::Notice(line));
                if pick && (info.profiles.len() > 1 || info.selected.is_some()) {
                    self.picker = Some((info, 0)); cx.focus(FocusClass::Modal);
                }
            }
            Input::Message(Msg::Failed(error)) => cx.error(error),
            Input::Key(key) if self.picker.is_some() => { self.key(key.code, cx); cx.consume_key(); }
            Input::Lifecycle(Lifecycle::FocusLost | Lifecycle::SessionReset | Lifecycle::EngineChanged) => self.picker = None,
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        if !matches!(at, Surface::Modal) { return None }
        let (_, cursor) = self.picker.as_ref()?;
        Some(View::IntrinsicElement { element: crate::ui::menu_view(
            "aws profile  ⏎ use for every session's Bedrock calls from the next request  esc close".into(),
            self.rows(), *cursor, cx.width as u16, cx.height as u16, false,
        ), rows: 0, replace: true })
    }
}

/// The `/aws` reply: which AWS profile Bedrock calls use, harness-wide.
pub fn aws_profile_line(p: &AwsProfiles) -> String {
    let now = match &p.selected {
        Some(s) if p.profiles.contains(s) => s.clone(),
        Some(s) => {
            format!("{s} (not found by the AWS SDK; `/aws clear` restores the default chain)")
        }
        None => format!(
            "default credential chain ({}), unless a Bedrock API key is configured",
            p.default_profile
        ),
    };
    let found = if p.profiles.is_empty() {
        "none found".to_owned()
    } else {
        p.profiles.join(", ")
    };
    format!(
        "aws profile: {now} — applies to every session's Bedrock calls from the next request\n  profiles: {found}"
    )
}
