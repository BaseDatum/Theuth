//! Skill inventory, invocation, loaded-frame badge and assembled prompt display.
use super::*;
use agent::{SkillBody, SkillMeta};
use rpc_compose::api::clients::*;
use tools::git::SyncResult;

#[derive(Default)]
pub(crate) struct SkillsPrompt {
    pub(crate) skills: Vec<SkillMeta>,
    pub(crate) loaded_skills: Vec<String>,
    pub(crate) menu: Option<(Vec<SkillMeta>, usize)>,
    picker_pending: bool,
    generation: u64,
}
#[derive(Clone, Debug)]
pub(crate) enum Request {
    ShowPrompt(Option<SessionId>), ListSkills, LoadSkill(String),
    AddSkillRepo(String), RemoveSkill(String), SyncSkills,
    UseSkill { session: Option<SessionId>, name: String, args: String, draft: Option<agent::CreateSessionOptions> },
}
pub(crate) enum Msg {
    Prompt(String), Skills(Vec<SkillMeta>), Skill(SkillBody), SkillsAdded(Vec<SkillMeta>),
    Synced(Vec<SyncResult>), Info(String), Failed(String),
    Use { name: String, args: String },
    Created { session: SessionId, request: Request, generation: u64 },
    Continue { request: Request, generation: u64 },
    Used { session: SessionId, generation: u64 },
    Loaded { session: SessionId, skills: Vec<String> },
}
impl SkillsPrompt {
    fn request(&self, request: Request, cx: &mut Cx<'_, '_, Msg>) {
        let generation = self.generation;
        cx.spawn_request(Lane::Concurrent, request, move |api, request| async move {
            let result: Result<Msg, jsonrpsee::core::client::Error> = async {
                Ok(match request {
                    Request::ShowPrompt(session) => Msg::Prompt(api.get_prompt(session).await?),
                    Request::ListSkills => Msg::Skills(api.list_skills().await?),
                    Request::LoadSkill(name) => Msg::Skill(api.load_skill(name).await?),
                    Request::AddSkillRepo(url) => Msg::SkillsAdded(api.add_skill_repo(url, None).await?),
                    Request::RemoveSkill(name) => Msg::Info(if api.remove_skill_repo(name.clone()).await.is_ok() {
                        format!("removed repo {name}")
                    } else { api.remove_skill(name.clone()).await?; format!("removed skill {name}") }),
                    Request::SyncSkills => Msg::Synced(api.sync_skills().await?),
                    Request::UseSkill { name, args, draft: Some(draft), .. } => {
                        let session = api.create_session(draft).await?.session;
                        Msg::Created { session, request: Request::UseSkill { session: Some(session), name, args, draft: None }, generation }
                    }
                    Request::UseSkill { session, name, args, draft: None } => Msg::Used { session: api.use_skill(session, name, args).await?, generation },
                })
            }.await;
            result.unwrap_or_else(|error| Msg::Failed(crate::reason(error)))
        });
    }
    fn use_skill(&self, name: String, args: String, cx: &mut Cx<'_, '_, Msg>) {
        cx.core(CoreAction::AwaitingSession(cx.core.session.is_none()));
        self.request(Request::UseSkill { session: cx.core.session, name, args, draft: cx.core.draft_options.cloned() }, cx);
    }
    fn skills_cmd(&mut self, arg: &str, cx: &mut Cx<'_, '_, Msg>) {
        let (sub, rest) = arg.split_once(' ').map_or((arg, ""), |(s, r)| (s, r.trim()));
        let request = match (sub, rest) {
            ("", _) => { self.picker_pending = true; Request::ListSkills }
            ("sync", _) => Request::SyncSkills,
            ("add", url) if !url.is_empty() => Request::AddSkillRepo(url.to_owned()),
            ("rm", name) if !name.is_empty() => Request::RemoveSkill(name.to_owned()),
            _ => { cx.core(CoreAction::Reject("usage: /skills [add <url> | rm <name> | sync]".into())); return; }
        };
        self.request(request, cx);
    }
    fn card(glyph: &'static str, color: iocraft::Color, title: String, text: &str, cx: &mut Cx<'_, '_, Msg>) {
        cx.core(CoreAction::AppendCard(TranscriptCard { entry_key: None, group: "skills-prompt", key: uuid::Uuid::new_v4(), glyph, color, title, body: markdown::Text::complete(text), expanded: false, plain: false }));
    }
    fn key(&mut self, code: KeyCode, cx: &mut Cx<'_, '_, Msg>) {
        let Some((items, cursor)) = &mut self.menu else { return; };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => { self.menu = None; cx.close(); }
            KeyCode::Char('s') | KeyCode::Enter => {
                let name = items.get(*cursor).map(|s| s.name.clone());
                self.menu = None; cx.close();
                if let Some(name) = name {
                    if code == KeyCode::Enter { self.use_skill(name, String::new(), cx); }
                    else { self.request(Request::LoadSkill(name), cx); }
                }
            }
            _ => {}
        }
    }
}
impl Component for SkillsPrompt {
    type Msg = Msg;
    fn declaration(&self) -> Declaration {
        Declaration { id: "skills-prompt", interests: &[Interest { topic: "agent.events", version: 1, variants: &["SkillsChanged"] }], slash: &[
            ("/skills", "[add <url>|rm <name>|sync]", "skills: picker (⏎ hands one to the model, s shows it); `/<skill> [words]` does the same"),
            ("/prompt", "", "show the assembled system prompt"),
        ], bindings: &[] }
    }
    fn decode(&self, _: &TopicEvent, core: &CoreView<'_>) -> Option<Msg> {
        match core.agent? {
            agent::AgentEvent::SkillsChanged { session, skills } => Some(Msg::Loaded { session: *session, skills: skills.clone() }),
            _ => None,
        }
    }
    fn slash_fallback(&self, name: &str) -> bool {
        let Some(name) = name.strip_prefix('/') else { return false; };
        if self.skills.is_empty() { !name.is_empty() && !name.contains(['/', '.']) }
        else { self.skills.iter().any(|s| s.name == name) }
    }
    fn update(&mut self, input: Input<'_, Msg>, cx: &mut Cx<'_, '_, Msg>) {
        if cx.core.reasoning && matches!(&input, Input::Message(Msg::Prompt(_) | Msg::Skills(_) | Msg::Skill(_) | Msg::SkillsAdded(_) | Msg::Synced(_) | Msg::Info(_))) { cx.plain(Plain::Text("\x1b[0m\n".into())); }
        match input {
            Input::Lifecycle(Lifecycle::Mount) if cx.core.interactive => self.request(Request::ListSkills, cx),
            Input::Lifecycle(Lifecycle::History(loaded)) => self.loaded_skills.clone_from(&loaded.skills),
            Input::Lifecycle(Lifecycle::SessionReset) => {
                self.generation += 1; self.loaded_skills.clear(); self.menu = None; self.picker_pending = false; cx.close();
            }
            Input::Lifecycle(Lifecycle::FocusLost) => self.menu = None,
            Input::Slash { name: "/skills", args } => self.skills_cmd(args, cx),
            Input::Slash { name: "/prompt", .. } => self.request(Request::ShowPrompt(cx.core.session), cx),
            Input::Slash { name, args } => self.use_skill(name[1..].to_owned(), args.to_owned(), cx),
            Input::Message(Msg::Use { name, args }) => self.use_skill(name, args, cx),
            Input::Message(Msg::Created { session, request, generation }) => {
                if generation == self.generation {
                    cx.core(CoreAction::SessionCreated(session)); cx.post(Msg::Continue { request, generation });
                } else {
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    cx.plain(Plain::Line(format!("session {session}")));
                }
            }
            Input::Message(Msg::Continue { request, generation }) if generation == self.generation => self.request(request, cx),
            Input::Message(Msg::Used { session, generation }) => {
                if generation == self.generation { cx.core(CoreAction::SessionCreated(session)); }
                else {
                    if cx.core.reasoning { cx.plain(Plain::Text("\x1b[0m\n".into())); }
                    cx.plain(Plain::Line(format!("session {session}")));
                }
            }
            Input::Message(Msg::Loaded { session, skills }) if Some(session) == cx.core.session => self.loaded_skills = skills,
            Input::Message(Msg::Prompt(text)) => {
                cx.plain(Plain::Line(text.clone()));
                Self::card("§", crate::ui::SAND, "prompt".into(), &format!("```\n{text}\n```"), cx);
            }
            Input::Message(Msg::Skills(items)) => {
                for s in &items { cx.plain(Plain::Line(format!("{}  {}{}{}", s.name, s.description, s.repo.as_deref().map_or_else(String::new, |r| format!("  [{r}]")), if s.always { "  always" } else { "" }))); }
                if !std::mem::take(&mut self.picker_pending) { self.skills = items; return; }
                self.skills.clone_from(&items);
                if items.is_empty() { cx.core(CoreAction::Notice("no skills".into())); }
                else { self.menu = Some((items, 0)); cx.focus(FocusClass::Modal); }
            }
            Input::Message(Msg::Skill(s)) => {
                cx.plain(Plain::Line(format!("◇ {}\n{}\n{}", s.name, s.body, s.files.join("\n"))));
                let text = if s.files.is_empty() { s.body } else { format!("{}\n\n---\nfiles: {}", s.body, s.files.join(", ")) };
                Self::card("◇", crate::ui::OCHRE, s.name, &text, cx);
            }
            Input::Message(Msg::SkillsAdded(items)) => {
                for s in &items { cx.plain(Plain::Line(format!("{}  {}{}{}", s.name, s.description, s.repo.as_deref().map_or_else(String::new, |r| format!("  [{r}]")), if s.always { "  always" } else { "" }))); }
                cx.core(CoreAction::Notice(if items.is_empty() { "cloned; no skills found".into() } else { format!("added {} skills: {}", items.len(), items.iter().map(|s| s.name.as_str()).collect::<Vec<_>>().join(", ")) }));
            }
            Input::Message(Msg::Synced(results)) => {
                if results.is_empty() { cx.core(CoreAction::Notice("no repos".into())); }
                for r in results { let line = format!("{}  {}", r.name, r.status); cx.plain(Plain::Line(line.clone())); cx.core(CoreAction::Notice(line)); }
            }
            Input::Message(Msg::Info(line)) => { cx.plain(Plain::Line(line.clone())); cx.core(CoreAction::Notice(line)); }
            Input::Message(Msg::Failed(error)) => cx.error(error),
            Input::Key(key) if self.menu.is_some() => { self.key(key.code, cx); cx.consume_key(); }
            _ => {}
        }
    }
    fn view(&self, at: Surface<'_>, cx: &ViewCx<'_>) -> Option<View> {
        match at {
            Surface::Status(StatusSlot::BeforeCatalog) => {
                let commands = cx.core.harness_commands;
                let skills: Vec<_> = commands.iter().filter_map(|c| c.name.strip_prefix("skill:")).collect();
                Some(View::Text(match (commands.len(), self.loaded_skills.len()) {
                    (0, 0) => String::new(),
                    (0, _) => format!("  ·  skills: {}", self.loaded_skills.join(", ")),
                    (n, _) if skills.is_empty() => format!("  ·  harness commands: {n}"),
                    _ => format!("  ·  harness skills: {}", skills.join(", ")),
                }))
            }
            Surface::Modal => {
                let (items, cursor) = self.menu.as_ref()?;
                let rows = items.iter().map(|s| {
                    // ponytail: fixed 48-char description; pass the terminal width if it grates.
                    let desc: String = s.description.chars().take(48).collect();
                    format!("{} {:<20} {:<48} {:<16} {}", if self.loaded_skills.contains(&s.name) { "●" } else { " " }, s.name, desc, s.repo.as_deref().map_or_else(String::new, |r| format!("[{r}]")), if s.always { "always" } else { "" })
                }).collect();
                Some(View::Element { element: crate::ui::menu_view("skills  ⏎ use  s show  esc close   ● in context".into(), rows, *cursor, cx.width as u16, cx.height as u16, false), rows: 0, replace: true })
            }
            _ => None,
        }
    }
}
