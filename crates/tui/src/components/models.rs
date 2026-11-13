//! Standard model catalog picker, effort chips, favorites and title settings.
use super::*;
use agent::{AgentEvent, ModelInfo};
use crate::components::search_attach::state as search;
use crate::model::k;
use rpc_compose::api::clients::AgentSettingsApiClient;

/// A completed picker choice; the requesting owner supplies its destination.
pub enum Pick {
    /// A model row, carrying the prior effort only when the chosen model accepts it.
    Model(Option<String>, Option<String>),
    /// Current model and selected effort; None effort restores the default.
    Effort(Option<String>, Option<String>),
}
/// Caller-owned completion; its captured session and reply endpoint never retarget.
pub type Completion = Box<dyn Fn(Pick) -> Option<CoreAction> + Send + Sync>;
/// Presentation and completion for a picker requested by another owner.
pub struct Selection {
    /// Picker title noun, e.g. the requesting setting or harness name.
    pub noun: String,
    /// Current model, absent when the setting is off.
    pub model: Option<String>,
    /// Current explicitly stored effort.
    pub effort: Option<String>,
    /// Whether the setting offers an Off row.
    pub off: bool,
    /// Whether an effort level is mandatory (no Default chip).
    pub required: bool,
    /// Whether this destination supports starring models.
    pub favorites: bool,
    /// Owner-typed completion, never a central feature target enum.
    pub complete: Completion,
    pub(crate) native: Option<Option<SessionId>>,
}
impl std::fmt::Debug for Selection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("Selection { .. }") }
}
/// One mounted catalog menu.
#[derive(Debug)]
pub struct Picker {
    /// Models in catalog order.
    pub items: Vec<ModelInfo>,
    /// Inline search query.
    pub filter: String,
    /// Highlighted row.
    pub cursor: usize,
    /// Highlighted effort chip.
    pub chip: usize,
    /// Browse level; None lists providers.
    pub provider: Option<String>,
    /// Owner-local destination and immutable completion endpoint.
    pub selection: Selection,
}
/// Catalog state persists across session resets.
#[derive(Debug, Default)]
pub struct Models {
    /// Starred model ids in starring order.
    pub favorite_models: Vec<String>,
    /// Exclusive modal, absent when closed.
    pub menu: Option<Picker>,
    pub(crate) serial: u64,
    pub(crate) pending: Option<(Selection, bool)>,
}
/// Owner-local requests retained on effects for assertion rebinding.
#[derive(Clone, Debug)]
pub enum Request {
    /// Read the model catalog.
    ListModels,
    /// Mutate only the original session, not the engine default.
    SetModel {
        /// Original session.
        session: SessionId,
        /// Selected catalog id.
        model: String,
    },
    /// Mutate only the original session's effort.
    SetEffort {
        /// Original session.
        session: SessionId,
        /// Selected level, or restore the default.
        effort: Option<String>,
    },
    /// Star or unstar one model.
    SetModelFavorite {
        /// Catalog id.
        model: String,
        /// Desired star state.
        favorite: bool,
    },
    /// Read title-model configuration.
    GetTitleModel,
    /// Read the selected harness options.
    HarnessOptions(SessionId),
    /// Mutate one harness select option.
    SetHarnessOption {
        /// Original session.
        session: SessionId,
        /// Harness option id.
        config_id: String,
        /// Selected option value.
        value: String,
    },
    /// Set or clear title-model configuration.
    SetTitleModel(Option<String>),
}
/// Catalog events, addressed picker requests and asynchronous replies.
pub enum Msg {
    /// Read a catalog for an external requester with its own completion.
    Open {
        /// Requester's presentation and completion endpoint.
        selection: Selection,
        /// Start on the effort row.
        chips: bool,
    },
    /// Update the current setting while its external picker is visible.
    Current {
        /// Setting whose picker is visible.
        noun: String,
        /// Applied model.
        model: Option<String>,
        /// Applied effort.
        effort: Option<String>,
    },
    /// Catalog response bound to the request that issued it.
    Loaded {
        /// Original catalog request identity.
        serial: u64,
        /// Catalog read outcome.
        result: Result<Vec<ModelInfo>, String>,
    },
    /// A completed native picker choice, bound to its original session or draft.
    Picked {
        /// Original session, absent for a draft pick.
        session: Option<SessionId>,
        /// Completed model or effort choice.
        pick: Pick,
    },
    /// Harness option reply and original picker mode.
    HarnessOptions {
        /// Original session.
        session: SessionId,
        /// Original effort-picker mode.
        chips: bool,
        /// Harness select options.
        options: Vec<::acp::ConfigOption>,
    },
    /// Harness mutation readback.
    HarnessOptionSet(String, String, ::acp::SetOptionResult),
    /// Completed harness picker.
    HarnessPick {
        /// Original session.
        session: SessionId,
        /// Harness model option id.
        model_option: Option<String>,
        /// Harness effort option id.
        effort_option: Option<String>,
        /// Completed model or effort choice.
        pick: Pick,
    },
    /// A successful model mutation.
    ModelSet(ModelInfo),
    /// A successful effort mutation.
    EffortSet(Option<String>),
    /// Authoritative starring order.
    FavoriteModels(Vec<String>),
    /// Title-model setting.
    TitleModel(Option<ModelInfo>),
    /// Existing generic command failure.
    Failed(String),
}
impl Models {
    /// Seed persistent favorites from the single boot info read, with config fallback.
    pub fn configured(info: Option<&rpc_compose::api::Info>, runtime: &runtime::Runtime) -> Self {
        Self {
            favorite_models: info.map(|i| i.favorite_models.clone()).unwrap_or_else(||
                runtime.section::<agent::AgentConfig>("agent").unwrap_or_default().favorite_models),
            ..Self::default()
        }
    }
    fn native(cx: &Cx<'_, '_, Msg>) -> Selection {
        let session = cx.core.session;
        let (to, engine_epoch) = (cx.token, cx.epoch);
        Selection {
            noun: "model".into(), model: Some(cx.core.model_id.into()),
            effort: if session.is_some() { cx.core.effort.map(str::to_owned) } else { cx.core.draft_options.and_then(|d| d.effort.clone()) },
            off: false, required: false, favorites: true, native: Some(session),
            complete: Box::new(move |pick| Some(CoreAction::Post(LocalReply {
                to, engine_epoch, body: Box::new(Msg::Picked { session, pick }),
            }))),
        }
    }
}
impl Selection {
    /// An external setting uses the same standard picker without a central target enum.
    pub fn external(noun: String, model: Option<String>, effort: Option<String>, off: bool, required: bool, favorites: bool, complete: Completion) -> Self {
        Self { noun, model, effort, off, required, favorites, complete, native: None }
    }
}
impl Models {
    /// Model marked current by this picker, using live applied session metadata.
    pub fn pick_current<'a>(&'a self, core: &'a CoreView<'_>) -> Option<&'a str> {
        let selection = &self.menu.as_ref()?.selection;
        if selection.native.is_some() { Some(core.model_id) } else { selection.model.as_deref() }
    }
    /// Standard provider/search rows with effort chips under the current model.
    pub fn rows(&self, core: &CoreView<'_>) -> Vec<ModelRow<'_>> {
        let Some(Picker { items, filter, provider, selection, .. }) = &self.menu else { return Vec::new() };
        let picked = self.pick_current(core);
        let current = items.iter().find(|m| Some(m.id.as_str()) == picked);
        let favorites: &[String] = if selection.favorites { &self.favorite_models } else { &[] };
        let mut rows = Vec::new();
        if filter.is_empty() && selection.off { rows.push(ModelRow::Off { current: picked.is_none() }); }
        if !filter.is_empty() {
            rows.extend(current.map(ModelRow::Current));
            rows.extend(rank_models(items, filter).into_iter().filter(|m| Some(m.id.as_str()) != picked).map(ModelRow::Model));
        } else if let Some(p) = provider {
            rows.extend(provider_models(items, p).into_iter().map(ModelRow::Model));
        } else {
            rows.extend(favorites.iter().filter_map(|f| items.iter().find(|m| m.id == *f)).map(ModelRow::Favorite));
            if !picked.is_some_and(|p| favorites.iter().any(|f| f == p)) { rows.extend(current.map(ModelRow::Current)); }
            let now = current.map(|c| c.provider.as_str());
            rows.extend(providers(items).into_iter().map(|(id, count)| {
                let current = now == Some(id.as_str());
                ModelRow::Provider { id, count, current }
            }));
        }
        if let Some(current) = current.filter(|c| !c.efforts.is_empty()) {
            let effort = if selection.native.is_some() {
                if core.session.is_some() { core.effort } else { core.draft_options.and_then(|d| d.effort.as_deref()) }
            } else { selection.effort.as_deref() };
            let levels: Vec<Option<String>> = (!selection.required).then_some(None).into_iter().chain(current.efforts.iter().cloned().map(Some)).collect();
            let active = levels.iter().position(|l| l.as_deref() == effort);
            if let Some(at) = rows.iter().position(|r| r.model().is_some_and(|m| Some(m.id.as_str()) == picked)) {
                rows.insert(at + 1, ModelRow::Efforts(EffortChips { levels, active }));
            }
        }
        rows
    }
    fn home(&mut self, core: &CoreView<'_>) {
        let rows = self.rows(core);
        let lead = rows.iter().take_while(|r| matches!(r, ModelRow::Off { .. } | ModelRow::Current(_) | ModelRow::Efforts(_))).count();
        let home = if rows.len() > lead { lead } else { 0 };
        if let Some(menu) = &mut self.menu { menu.cursor = home; }
    }
    /// Open already-read catalog rows at their normal browse level.
    pub fn show(&mut self, items: Vec<ModelInfo>, selection: Selection, chips: bool, core: &CoreView<'_>) {
        let provider = match providers(&items).as_slice() { [(p, _)] => Some(p.clone()), _ => None };
        self.menu = Some(Picker { items, filter: String::new(), cursor: 0, chip: 0, provider, selection });
        self.home(core);
        let at = self.rows(core).iter().enumerate().find_map(|(i, r)| match r {
            ModelRow::Efforts(c) if chips => Some((i, c.active.unwrap_or(0))), _ => None,
        });
        if let (Some((row, active)), Some(menu)) = (at, &mut self.menu) { menu.cursor = row; menu.chip = active; }
    }
    fn favorites(&mut self, models: Vec<String>, core: &CoreView<'_>) {
        self.favorite_models = models;
        let len = self.rows(core).len();
        if let Some(menu) = &mut self.menu { menu.cursor = menu.cursor.min(len.saturating_sub(1)); }
    }
    fn list(&mut self, selection: Selection, chips: bool, cx: &mut Cx<'_, '_, Msg>) {
        self.serial = self.serial.wrapping_add(1);
        let serial = self.serial;
        self.pending = Some((selection, chips));
        cx.spawn_request(Lane::Concurrent, Request::ListModels, move |api, _| async move {
            Msg::Loaded { serial, result: api.list_models(None).await.map_err(crate::reason) }
        });
    }
    fn request(request: Request, cx: &mut Cx<'_, '_, Msg>) {
        cx.spawn_request(Lane::Concurrent, request, |api, request| async move {
            let result = async {
                Ok(match request {
                    Request::SetModel { session, model } => Msg::ModelSet(api.set_model(Some(session), model, None).await.map_err(crate::reason)?),
                    Request::SetEffort { session, effort } => {
                        api.set_effort(Some(session), effort.clone(), None).await.map_err(crate::reason)?;
                        Msg::EffortSet(effort)
                    }
                    Request::SetModelFavorite { model, favorite } => Msg::FavoriteModels(api.set_model_favorite(model, favorite).await.map_err(crate::reason)?),
                    Request::HarnessOptions(session) => Msg::HarnessOptions { session, chips: false, options: api.harness_options(session).await.map_err(crate::reason)? },
                    Request::SetHarnessOption { session, config_id, value } => {
                        let result = api.set_harness_option(session, config_id.clone(), value.clone()).await.map_err(crate::reason)?;
                        Msg::HarnessOptionSet(config_id, value, result)
                    }
                    Request::GetTitleModel => Msg::TitleModel(api.title_model().await.map_err(crate::reason)?),
                    Request::SetTitleModel(model) => Msg::TitleModel(api.set_title_model(model).await.map_err(crate::reason)?),
                    Request::ListModels => unreachable!("catalog requests carry their destination"),
                })
            }.await;
            result.unwrap_or_else(Msg::Failed)
        });
    }
    fn picked(session: Option<SessionId>, pick: Pick, cx: &mut Cx<'_, '_, Msg>) {
        match (session, pick) {
            (Some(session), Pick::Model(Some(model), _)) => Self::request(Request::SetModel { session, model }, cx),
            (Some(session), Pick::Effort(_, effort)) => {
                cx.core(CoreAction::AwaitingEffort(true));
                Self::request(Request::SetEffort { session, effort }, cx);
            }
            (None, Pick::Model(Some(model), _)) if cx.core.session.is_none() => cx.core(CoreAction::PickModel(model)),
            (None, Pick::Effort(_, effort)) if cx.core.session.is_none() => cx.core(CoreAction::SetEffort(effort)),
            _ => {}
        }
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        enum Hit { Provider(String), Model(String, Vec<String>), Off, Chips(Vec<Option<String>>) }
        let Some(menu) = &self.menu else { return };
        let many = providers(&menu.items).len() > 1;
        let favorites = menu.selection.favorites;
        let current_model = self.pick_current(cx.core).map(str::to_owned);
        let current_effort = if menu.selection.native.is_some() {
            if cx.core.session.is_some() { cx.core.effort.map(str::to_owned) } else { cx.core.draft_options.and_then(|d| d.effort.clone()) }
        } else { menu.selection.effort.clone() };
        let rows = self.rows(cx.core);
        let len = rows.len();
        let at = menu.cursor.min(len.saturating_sub(1));
        let hit = match rows.get(at) {
            Some(ModelRow::Current(m) | ModelRow::Favorite(m) | ModelRow::Model(m)) => Some(Hit::Model(m.id.clone(), m.efforts.clone())),
            Some(ModelRow::Provider { id, .. }) => Some(Hit::Provider(id.clone())),
            Some(ModelRow::Off { .. }) => Some(Hit::Off),
            Some(ModelRow::Efforts(c)) => Some(Hit::Chips(c.levels.clone())), None => None,
        };
        let chips_at = rows.iter().enumerate().find_map(|(i, r)| match r { ModelRow::Efforts(c) => Some((i, c.active.unwrap_or(0))), _ => None });
        let Some(menu) = &mut self.menu else { return };
        menu.cursor = at;
        let mut home = false;
        let mut pick = None;
        match code {
            KeyCode::Up | KeyCode::Down => {
                menu.cursor = if code == KeyCode::Up { menu.cursor.saturating_sub(1) } else { (menu.cursor + 1).min(len.saturating_sub(1)) };
                if let Some((row, active)) = chips_at && row == menu.cursor { menu.chip = active; }
            }
            KeyCode::Char(c) => { menu.filter.push(c); home = true; }
            KeyCode::Backspace => {
                if menu.filter.pop().is_some() { home = true; }
                else if many && menu.provider.is_some() { menu.provider = None; home = true; }
            }
            KeyCode::Left => match hit {
                Some(Hit::Chips(_)) => menu.chip = menu.chip.saturating_sub(1),
                _ if menu.filter.is_empty() && many && menu.provider.is_some() => { menu.provider = None; home = true; }
                _ => {}
            },
            KeyCode::Right => match hit {
                Some(Hit::Chips(levels)) => menu.chip = (menu.chip + 1).min(levels.len().saturating_sub(1)),
                Some(Hit::Provider(p)) => { menu.provider = Some(p); home = true; } _ => {}
            },
            KeyCode::Enter => match hit {
                Some(Hit::Provider(p)) => { menu.provider = Some(p); home = true; }
                Some(Hit::Chips(levels)) => pick = levels.get(menu.chip).cloned().map(|effort| Pick::Effort(current_model, effort)),
                Some(Hit::Model(model, efforts)) => pick = Some(Pick::Model(Some(model), current_effort.filter(|e| efforts.contains(e)))),
                Some(Hit::Off) => pick = Some(Pick::Model(None, None)), None => {}
            },
            KeyCode::Esc => { self.menu = None; cx.close(); }
            KeyCode::Tab => if let Some(Hit::Model(model, _)) = hit && favorites {
                let favorite = !self.favorite_models.contains(&model);
                Self::request(Request::SetModelFavorite { model, favorite }, cx);
            },
            _ => {}
        }
        if let Some(pick) = pick && let Some(menu) = self.menu.take() {
            cx.close();
            if let Some(action) = (menu.selection.complete)(pick) { cx.core(action); }
        }
        if home { self.home(cx.core); }
    }
}
impl Component for Models {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "models", interests: &[Interest { topic: "agent.events", version: 1, variants: &["FavoriteModelsChanged"] }],
            slash: &[
                ("/model", "[id]", "switch model (picker without id)"),
                ("/title", "[model|off]", "model that names sessions in the history list (off = the session's model)"),
                ("/effort", "[level|clear]", "reasoning effort (picker without arg)"),
            ], bindings: &[] }
    }
    fn decode(&self, event: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        if !((event.topic() == Some("agent.events") && event.version() == Some(1)) || event.topic().is_none()) { return None; }
        if let Some(AgentEvent::FavoriteModelsChanged { models }) = core.agent { return Some(Msg::FavoriteModels(models.clone())); }
        if event.event()["type"] != "FavoriteModelsChanged" { return None; }
        use serde::Deserialize;
        match AgentEvent::deserialize(event.event()).ok()? { AgentEvent::FavoriteModelsChanged { models } => Some(Msg::FavoriteModels(models)), _ => None }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning && matches!(&input, Input::Message(Msg::ModelSet(_) | Msg::EffortSet(_) | Msg::TitleModel(_) | Msg::HarnessOptions { .. } | Msg::HarnessOptionSet(..))) {
            cx.plain(Plain::Text("\x1b[0m\n".into()));
        }
        match input {
            Input::Slash { name: "/title", args } => Self::request(if args.is_empty() { Request::GetTitleModel } else { Request::SetTitleModel((args != "off" && args != "clear").then(|| args.to_owned())) }, cx),
            Input::Slash { name, args } if name == "/model" || name == "/effort" => {
                let chips = name == "/effort";
                if cx.core.harness.is_some() {
                    if let Some(session) = cx.core.session {
                        if args.is_empty() {
                            self.serial = self.serial.wrapping_add(1);
                            cx.spawn_request(Lane::Concurrent, Request::HarnessOptions(session), move |api, _| async move {
                                match api.harness_options(session).await { Ok(options) => Msg::HarnessOptions { session, chips, options }, Err(e) => Msg::Failed(crate::reason(e)) }
                            });
                        } else { Self::request(Request::SetHarnessOption { session, config_id: name[1..].to_owned(), value: args.to_owned() }, cx); }
                    }
                } else if !args.is_empty() {
                    let pick = if chips { Pick::Effort(Some(cx.core.model_id.into()), (args != "clear").then(|| args.to_owned())) } else { Pick::Model(Some(args.to_owned()), None) };
                    Self::picked(cx.core.session, pick, cx);
                } else if chips && !cx.core.catalog.and_then(|c| c.closest(cx.core.model_id)).is_some_and(|m| !m.efforts().is_empty()) {
                    cx.core(CoreAction::Notice(format!("{} has no reasoning effort levels", cx.core.model_id)));
                } else { self.list(Self::native(cx), chips, cx); }
            }
            Input::Message(Msg::Open { selection, chips }) => self.list(selection, chips, cx),
            Input::Message(Msg::Current { noun, model, effort }) => {
                if let Some((selection, _)) = &mut self.pending && selection.noun == noun {
                    selection.model = model.clone(); selection.effort = effort.clone();
                }
                if let Some(menu) = &mut self.menu && menu.selection.noun == noun {
                    menu.selection.model = model; menu.selection.effort = effort;
                }
            }
            Input::Message(Msg::Loaded { serial, result }) => {
                if serial != self.serial { return; }
                let Some((selection, chips)) = self.pending.take() else { return };
                if result.is_ok() {
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    cx.plain(Plain::TtyRequired("menus need a TTY".into()));
                }
                match result {
                    Ok(items) if items.is_empty() => cx.core(CoreAction::Reject("no models: no provider is configured".into())),
                    Ok(items) => { self.show(items, selection, chips, cx.core); cx.focus(FocusClass::Modal); }
                    Err(error) => cx.error(error),
                }
            }
            Input::Message(Msg::Picked { session, pick }) => Self::picked(session, pick, cx),
            Input::Message(Msg::ModelSet(info)) => { cx.plain(Plain::Line(format!("model → {}", info.id))); cx.core(CoreAction::ModelApplied(info.id)); }
            Input::Message(Msg::EffortSet(effort)) => {
                cx.plain(Plain::Line(match &effort { Some(e) => format!("effort: {e}"), None => "effort cleared".into() }));
                cx.core(CoreAction::EffortApplied(effort));
            }
            Input::Message(Msg::FavoriteModels(models)) => {
                if cx.core.agent.is_none() { if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); } cx.plain(Plain::Line(format!("favorites → {} models", models.len()))); }
                self.favorites(models, cx.core);
            }
            Input::Message(Msg::TitleModel(model)) => { let text = title_model_line(model.as_ref()); cx.plain(Plain::Line(text.clone())); cx.core(CoreAction::Notice(text)); }
            Input::Message(Msg::HarnessOptions { session, chips, options }) => {
                for line in harness_options_lines(&options) { cx.plain(Plain::Line(line)); }
                let harness = cx.core.harness.unwrap_or("harness");
                match harness_groups(&options) {
                    (_, None) if chips => cx.core(CoreAction::Notice(format!("{harness} has no reasoning effort levels"))),
                    (None, None) => for line in harness_options_lines(&options) { cx.core(CoreAction::Notice(line)); },
                    (model, effort) => {
                        let current = model.map(|o| o.current.clone()).or_else(|| Some(cx.core.harness_model.unwrap_or(cx.core.model_id).to_owned()));
                        let selected_effort = effort.map(|o| o.current.clone());
                        let model_option = model.map(|o| o.id.clone());
                        let effort_option = effort.map(|o| o.id.clone());
                        let (to, engine_epoch) = (cx.token, cx.epoch);
                        let complete = Box::new(move |pick| Some(CoreAction::Post(LocalReply { to, engine_epoch, body: Box::new(Msg::HarnessPick { session, model_option: model_option.clone(), effort_option: effort_option.clone(), pick }) })));
                        let selection = Selection::external(harness.into(), current, selected_effort, false, true, false, complete);
                        let items = harness_models(&options, harness, cx.core.harness_model.unwrap_or(cx.core.model_id));
                        self.show(items, selection, chips, cx.core); cx.focus(FocusClass::Modal);
                    }
                }
            }
            Input::Message(Msg::HarnessPick { session, model_option, effort_option, pick }) => {
                let pair = match pick { Pick::Model(value, _) => model_option.zip(value), Pick::Effort(_, value) => effort_option.zip(value) };
                if let Some((config_id, value)) = pair { Self::request(Request::SetHarnessOption { session, config_id, value }, cx); }
            }
            Input::Message(Msg::HarnessOptionSet(key, value, result)) => { let line = harness_option_set_line(&key, &value, &result); cx.plain(Plain::Line(line.clone())); cx.core(CoreAction::Notice(line)); }
            Input::Message(Msg::Failed(error)) => cx.error(error),
            Input::Key(key) if self.menu.is_some() => { self.key(key.code, cx); cx.consume_key(); }
            Input::Lifecycle(Lifecycle::FocusLost | Lifecycle::SessionReset | Lifecycle::EngineChanged) => { self.menu = None; self.pending = None; self.serial = self.serial.wrapping_add(1); }
            Input::Lifecycle(Lifecycle::CommandFailed(_)) => self.pending = None,
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        if !matches!(at, Surface::Modal) { return None; }
        self.menu.as_ref()?;
        let (title, rows, cursor) = self.presentation(cx.core, cx.width as u16);
        Some(View::Element { element: crate::ui::menu_view(title, rows, cursor, cx.width as u16, cx.height as u16, false), rows: 0, replace: true })
    }
}
/// A `/model` picker row (124 §3).
#[derive(Debug)]
pub enum ModelRow<'a> {
    /// 125 §7.2: the advisor picker's lead row; `current` while no advisor is set.
    Off {
        /// Whether the advisor is off now.
        current: bool,
    },
    /// The session's current model: always first, never filtered out.
    Current(&'a ModelInfo),
    /// A provider on the providers level; `current` marks the current model's.
    Provider {
        /// Provider id as it appears in `provider/model`.
        id: String,
        /// Models listed under it.
        count: usize,
        /// Whether the current model belongs to it.
        current: bool,
    },
    /// A starred model on the providers level (127 §3), in favorites order.
    Favorite(&'a ModelInfo),
    /// A model on a provider's level or in the search results.
    Model(&'a ModelInfo),
    /// 194 D6: the picked model's effort chips, directly under its row.
    Efforts(EffortChips),
}

impl ModelRow<'_> {
    /// The model a Current, Favorite or Model row stands for.
    pub fn model(&self) -> Option<&ModelInfo> {
        match self {
            Self::Current(m) | Self::Favorite(m) | Self::Model(m) => Some(m),
            _ => None,
        }
    }
}

/// The chip row under the picked model (194 D6).
#[derive(Debug, PartialEq, Eq)]
pub struct EffortChips {
    /// The chips in order: `None` is Default (absent when a level is
    /// required), then the levels.
    pub levels: Vec<Option<String>>,
    /// The active chip; `None` when the stored effort is not one of them.
    pub active: Option<usize>,
}

/// Search terms (124 §2.1): lowercased words of `query`; none means browse mode.
pub(crate) fn search_terms(query: &str) -> Vec<String> {
    query.split_whitespace().map(str::to_lowercase).collect()
}

/// Whether `t` occurs in `s` at the start of a word (124 §2.2): at index 0 or
/// after a character outside `[a-z0-9]`.
fn boundary_match(s: &str, t: &str) -> bool {
    s.match_indices(t)
        .any(|(i, _)| !s[..i].ends_with(|c: char| c.is_ascii_alphanumeric()))
}

/// Tier of one search term for one model (124 §2.2 table), before gating.
fn term_tier(name: &str, id: &str, provider: &str, t: &str) -> u32 {
    let tail = id.split_once('/').map_or(id, |(_, tail)| tail);
    if name == t || tail == t {
        100
    } else if name.starts_with(t) {
        90
    } else if boundary_match(name, t) {
        80
    } else if provider == t {
        70
    } else if provider.starts_with(t) {
        65
    } else if boundary_match(tail, t) {
        60
    } else if name.contains(t) {
        40
    } else if id.contains(t) {
        30
    } else {
        0
    }
}

/// Word search over `items` (124 §2): every term must match; per term, plain
/// substring hits (40/30) only count when nothing matches at a word boundary;
/// score is the sum of tiers, ties keep catalog order. Browse mode (no terms)
/// returns everything.
pub fn rank_models<'a>(items: &'a [ModelInfo], query: &str) -> Vec<&'a ModelInfo> {
    let lower: Vec<(String, String, String)> = items
        .iter()
        .map(|m| {
            (
                m.name.to_lowercase(),
                m.id.to_lowercase(),
                m.provider.to_lowercase(),
            )
        })
        .collect();
    let mut scores = vec![Some(0u32); items.len()];
    for t in search_terms(query) {
        let tiers: Vec<u32> = lower
            .iter()
            .map(|(name, id, provider)| term_tier(name, id, provider, &t))
            .collect();
        let gate = tiers.iter().any(|&tier| tier >= 60);
        for (score, &tier) in scores.iter_mut().zip(&tiers) {
            match tier {
                0 => *score = None,
                1..60 if gate => *score = None,
                _ => {
                    if let Some(s) = score {
                        *s += tier;
                    }
                }
            }
        }
    }
    let mut hits: Vec<(u32, &ModelInfo)> = scores
        .iter()
        .zip(items)
        .filter_map(|(score, m)| score.map(|s| (s, m)))
        .collect();
    hits.sort_by_key(|h| std::cmp::Reverse(h.0));
    hits.into_iter().map(|(_, m)| m).collect()
}

/// Distinct providers of `items` with their model counts, A→Z (124 §3.1).
pub fn providers(items: &[ModelInfo]) -> Vec<(String, usize)> {
    let mut counts = std::collections::BTreeMap::new();
    for m in items {
        *counts.entry(m.provider.as_str()).or_insert(0) += 1;
    }
    counts.into_iter().map(|(p, n)| (p.to_owned(), n)).collect()
}

/// The models of `provider`, by name (case-insensitive) then id (124 §3.2).
pub fn provider_models<'a>(items: &'a [ModelInfo], provider: &str) -> Vec<&'a ModelInfo> {
    let mut out: Vec<&ModelInfo> = items.iter().filter(|m| m.provider == provider).collect();
    out.sort_by_cached_key(|m| (m.name.to_lowercase(), m.id.as_str()));
    out
}

/// The `/title` reply: the model naming sessions, or the fallback.
pub fn title_model_line(m: Option<&ModelInfo>) -> String {
    match m {
        Some(m) => format!("title model: {} ({})", m.id, m.name),
        None => "title model: off — each session names itself with its own model (/title <model> to set a cheap one)".into(),
    }
}
impl Models {
    /// Existing title and model/chip row formatting, before shared modal geometry.
    pub fn presentation(&self, core: CoreView<'_>, width: u16) -> (String, Vec<String>, usize) {
        let Some(Picker { items, filter, cursor, chip, provider, selection }) = &self.menu else { return (String::new(), Vec::new(), 0) };
        let noun = selection.noun.as_str();
            let rows = self.rows(&core);
            let star = selection.favorites.then(|| "⇥ ★ favorite".to_owned());
            let on_chips = matches!(rows.get(*cursor), Some(ModelRow::Efforts(_)));
            let mut title: Vec<String> = Vec::new();
            if on_chips {
                title.extend([format!("{noun} › effort"), "←→ choose".into(), "⏎ set".into()]);
            } else if !filter.is_empty() {
                title.extend([format!("{noun}  search: {filter}▏"), "⏎ set".into()]);
                title.extend(star);
            } else if let Some(p) = provider {
                let models = rows.iter().filter(|r| matches!(r, ModelRow::Model(_))).count();
                title.extend([format!("{noun} › {p} ({models})"), "⏎ set".into()]);
                title.extend(star);
                if providers(items).len() > 1 {
                    title.push("←/⌫ back".into());
                }
            } else {
                title.extend([noun.to_owned(), "⏎ open provider".into()]);
                title.extend(star);
                title.push("type to search all models".into());
            }
            title.push("esc close".into());
            let none = rows
                .iter()
                .all(|r| matches!(r, ModelRow::Current(_) | ModelRow::Efforts(_)));
            if !on_chips && !filter.is_empty() && none {
                title.push("no match".into());
            }
            // 194 D6: the title never wraps; hints that do not fit drop whole.
            let title = fit_title(&title, usize::from(width).saturating_sub(5));
            // Two-char marker column (127 §3): `●` current, `★` favorite.
            let marks = |on: bool, fav: bool| {
                format!(
                    "{}{}",
                    if on { "●" } else { " " },
                    if fav { "★" } else { " " }
                )
            };
            let model_row = |on: bool, i: &agent::ModelInfo| {
                format!(
                    "{} {:<44} {:<32} {}{}{}",
                    // The harness picker stars nothing (⇥ is off there).
                    marks(
                        on,
                        selection.favorites && self.favorite_models.contains(&i.id)
                    ),
                    i.id,
                    i.name,
                    // A harness row has no catalog facts.
                    if i.context > 0 { k(i.context) } else { String::new() },
                    if i.image { " · vision" } else { "" },
                    if i.local { " · local" } else { "" }
                )
            };
            let picked = self.pick_current(&core);
            let rows = rows
                .iter()
                .enumerate()
                .map(|(n, r)| match r {
                    ModelRow::Off { current } => {
                        format!("{} {:<44} no advisor", marks(*current, false), "Off")
                    }
                    ModelRow::Current(i) => model_row(true, i),
                    ModelRow::Favorite(i) | ModelRow::Model(i) => {
                        model_row(Some(i.id.as_str()) == picked, i)
                    }
                    ModelRow::Provider { id, count, current } => {
                        format!("{} {id:<24} {count:>4} models", marks(*current, false))
                    }
                    ModelRow::Efforts(c) => effort_chips_row(
                        c,
                        (n == *cursor).then_some(*chip),
                        usize::from(width).saturating_sub(7),
                    ),
                })
                .collect();
            (title, rows, *cursor)
    }
}
/// `parts` joined by two spaces within `room` cells: the first always
/// (clipped), each later one only whole, so the title row never wraps.
fn fit_title(parts: &[String], room: usize) -> String {
    let mut out = String::new();
    for p in parts {
        if out.is_empty() {
            out = search::fit(p, room);
        } else if search::cells(&out) + 2 + search::cells(p) <= room {
            out.push_str("  ");
            out.push_str(p);
        }
    }
    out
}

/// The chip row under the picked model (194 D6): `effort  Default  ●high  low`,
/// `●` on the active chip, `[…]` around `chip` while the cursor is on the row.
/// Within `room` cells the chips shown are a window around the focused one
/// (`chip`, else the active one), `…` where chips are hidden.
fn effort_chips_row(c: &EffortChips, chip: Option<usize>, room: usize) -> String {
    let chips: Vec<String> = c
        .levels
        .iter()
        .enumerate()
        .map(|(i, level)| {
            let on = if c.active == Some(i) { "●" } else { "" };
            let name = level.as_deref().unwrap_or("Default");
            if chip == Some(i) {
                format!("[{on}{name}]")
            } else {
                format!(" {on}{name} ")
            }
        })
        .collect();
    let head = "   effort ";
    let n = chips.len();
    if n == 0 {
        return head.to_owned();
    }
    let cells = |lo: usize, hi: usize| {
        let marks = 2 * (usize::from(lo > 0) + usize::from(hi < n));
        let body: usize = chips[lo..hi].iter().map(|s| search::cells(s) + 1).sum();
        search::cells(head) + body - 1 + marks
    };
    let focus = chip.or(c.active).unwrap_or(0).min(n - 1);
    let (mut lo, mut hi) = (focus, focus + 1);
    loop {
        if hi < n && cells(lo, hi + 1) <= room {
            hi += 1;
        } else if lo > 0 && cells(lo - 1, hi) <= room {
            lo -= 1;
        } else {
            break;
        }
    }
    format!(
        "{head}{}{}{}",
        if lo > 0 { "… " } else { "" },
        chips[lo..hi].join(" "),
        if hi < n { " …" } else { "" }
    )
}
/// `/model` / `/effort` on an ACP session (75 §8): one line per harness
/// option — `model: anthropic/claude-opus-4-8  (has: a, b, c)` — the current
/// value first, then its values verbatim; the `thought_level` option is
/// labelled `effort` so `/effort <value>` reads back.
pub fn harness_options_lines(options: &[::acp::ConfigOption]) -> Vec<String> {
    if options.is_empty() {
        return vec!["harness offers no options".into()];
    }
    options
        .iter()
        .map(|o| {
            let label = match o.category.as_deref() {
                Some("thought_level") => "effort",
                _ => o.id.as_str(),
            };
            let values: Vec<&str> = o.options.iter().map(|v| v.value.as_str()).collect();
            format!("{label}: {}  (has: {})", o.current, values.join(", "))
        })
        .collect()
}

/// The harness's `model` select and `thought_level` option, when offered
/// (the desktop's `harnessGroups`).
pub(crate) fn harness_groups(
    options: &[::acp::ConfigOption],
) -> (Option<&::acp::ConfigOption>, Option<&::acp::ConfigOption>) {
    let named = |s: &str| s.to_lowercase().contains("model");
    let model = options
        .iter()
        .find(|o| o.category.as_deref() == Some("model"))
        .or_else(|| options.iter().find(|o| named(&o.id) || named(&o.name)));
    let effort = options
        .iter()
        .find(|o| o.category.as_deref() == Some("thought_level"));
    (model, effort)
}

/// The harness picker's rows (the desktop's `HarnessPicker`): one per
/// `model` value, its provider the value's `provider/` prefix else `agent`;
/// without a model select one row for what it runs (`running`, else
/// `agent`). Every row carries the `thought_level` values as its efforts.
pub(crate) fn harness_models(
    options: &[::acp::ConfigOption],
    agent: &str,
    running: &str,
) -> Vec<ModelInfo> {
    let (model, effort) = harness_groups(options);
    let efforts: Vec<String> = effort
        .map(|o| o.options.iter().map(|v| v.value.clone()).collect())
        .unwrap_or_default();
    let row = |id: &str, name: &str| ModelInfo {
        id: id.to_owned(),
        name: name.to_owned(),
        provider: id.split_once('/').map_or(agent, |(p, _)| p).to_owned(),
        context: 0,
        reasoning: !efforts.is_empty(),
        image: false,
        efforts: efforts.clone(),
        budget_min: None,
        budget_max: None,
        local: false,
    };
    match model {
        Some(o) => o.options.iter().map(|v| row(&v.value, &v.name)).collect(),
        None => {
            let id = if running.is_empty() { agent } else { running };
            vec![row(id, id)]
        }
    }
}

/// The announcement after `set_harness_option` (75 §8): `model → x`, or
/// `model → x after this turn` when the change waits for the turn in flight.
pub fn harness_option_set_line(key: &str, value: &str, r: &::acp::SetOptionResult) -> String {
    if r.pending {
        format!("{key} → {value} after this turn")
    } else {
        format!("{key} → {value}")
    }
}
