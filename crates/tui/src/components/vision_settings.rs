//! Harness-wide image fallback settings and image-attachment notices.
use super::*;
use agent::ModelInfo;
use rpc_compose::api::clients::AgentSettingsApiClient;

/// Fallback cache is harness-wide and survives session resets.
#[derive(Default)]
pub struct VisionSettings {
    /// Last authoritative fallback setting, absent when disabled.
    pub vision_model: Option<ModelInfo>,
    awaiting: bool,
}
/// Typed owner requests retained on effects.
#[derive(Clone, Debug)]
pub enum Request {
    /// Read the fallback setting.
    GetVisionModel,
    /// Change the fallback, or turn it off.
    SetVisionModel(Option<String>),
}
/// Fallback setting readbacks and failures.
pub enum Msg {
    /// Authoritative fallback model.
    VisionModel(Option<ModelInfo>),
    /// Preserve the existing generic-error path.
    Failed(String),
}
impl VisionSettings {
    fn request(request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            let result = match request {
                Request::GetVisionModel => api.vision_model().await,
                Request::SetVisionModel(model) => api.set_vision_model(model).await,
            };
            match result { Ok(model) => Msg::VisionModel(model), Err(error) => Msg::Failed(crate::reason(error)) }
        });
    }
    fn image_attached(&self, cx: &mut Cx<'_, '_, Msg>) {
        if !cx.core.catalog.and_then(|c| c.closest(cx.core.model_id)).is_some_and(|m| !m.image) { return; }
        let tail = match &self.vision_model {
            Some(v) => format!("{} will inspect the image for it and answer its question (/vision to change)", v.id),
            None => "the image will be sent as omitted — /model to switch or /vision <model> to set a fallback".to_owned(),
        };
        cx.core(CoreAction::Notice(format!("{} is text-only; {tail}", cx.core.model_id)));
    }
}
impl Component for VisionSettings {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "vision-settings", interests: &[], slash: &[("/vision", "[model|off]", "model that inspects images when the main model cannot (off = images are omitted)")], bindings: &[] }
    }
    fn decode(&self, _: &TopicEvent, _: &CoreView<'_>) -> Option<Msg> { None }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        match input {
            Input::Lifecycle(Lifecycle::Mount) if cx.core.interactive => Self::request(Request::GetVisionModel, cx),
            Input::Lifecycle(Lifecycle::ImageAttached) => self.image_attached(cx),
            Input::Slash { args, .. } => {
                self.awaiting = true;
                Self::request(if args.is_empty() { Request::GetVisionModel } else {
                    Request::SetVisionModel((args != "off" && args != "clear").then(|| args.to_owned()))
                }, cx);
            }
            Input::Message(Msg::VisionModel(model)) => {
                if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                let line = vision_model_line(model.as_ref());
                cx.plain(Plain::Line(line.clone()));
                if std::mem::take(&mut self.awaiting) { cx.core(CoreAction::Notice(line)); }
                self.vision_model = model;
            }
            Input::Message(Msg::Failed(error)) => cx.error(error),
            Input::Lifecycle(Lifecycle::SessionReset | Lifecycle::CommandFailed(_)) => self.awaiting = false,
            _ => {}
        }
    }
    fn view(&self, _: Surface<'_>, _: &ViewCx<'_>) -> Option<View> { None }
}

/// The `/vision` reply: the model that inspects images for a text-only main
/// model (52 §3), or off.
pub fn vision_model_line(m: Option<&ModelInfo>) -> String {
    match m {
        Some(m) => format!(
            "vision fallback: {} ({}) — used only when the main model cannot view images; it receives the image and a question written by the main model, an extra call to that provider",
            m.id, m.name
        ),
        None => "vision fallback: off — a text-only main model gets images as omitted (/vision <model> to set one)".into(),
    }
}

