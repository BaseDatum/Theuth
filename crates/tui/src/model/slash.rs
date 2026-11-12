use super::*;

/// Slash commands: name, argument hint (`<x>` required, `[x]` optional, `""` none), description.
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("/model", "[id]", "switch model (picker without id)"),
    (
        "/title",
        "[model|off]",
        "model that names sessions in the history list (off = the session's model)",
    ),
    (
        "/vision",
        "[model|off]",
        "model that inspects images when the main model cannot (off = images are omitted)",
    ),
    (
        "/effort",
        "[level|clear]",
        "reasoning effort (picker without arg)",
    ),
    (
        "/advisor",
        "[off|on|status|consults <n>|<provider/model>]",
        "a second model that reviews this session and answers its consults (picker without argument)",
    ),
    (
        "/aws",
        "[profile|clear]",
        "AWS profile for Bedrock, harness-wide (picker without arg)",
    ),
    (
        "/acp",
        "[add <registry-id>|remove <name>]",
        "External harnesses (ACP) and the registry",
    ),
    (
        "/skills",
        "[add <url>|rm <name>|sync]",
        "skills: picker (⏎ hands one to the model, s shows it); `/<skill> [words]` does the same",
    ),
    (
        "/login",
        "[name]",
        "log in to a provider or MCP server (picker without name)",
    ),
    ("/logout", "<name>", "forget a login"),
    ("/prompt", "", "show the assembled system prompt"),
    (
        "/agents",
        "",
        "subagents of this session (picker: ⏎ opens one)",
    ),
    (
        "/talk",
        "",
        "talk with the session aloud; Esc cancels the reply or leaves the view, Ctrl+T stops",
    ),
    (
        "/project",
        "<label|off>",
        "label this session's root for grouping in /sessions and /artifacts (off clears)",
    ),
    ("/parent", "", "open the parent session (from a subagent)"),
    (
        "/sessions",
        "",
        "switch session (picker; owner chip on sessions owned elsewhere)",
    ),
    ("/take", "", "continue the open session on this node (68)"),
    (
        "/resume",
        "[id]",
        "open a session; on a stopped ACP child, restart its harness in the same context (75 §10)",
    ),
    (
        "/new",
        "[private]",
        "start a new session (private: memory-only tree, closed when you leave it)",
    ),
    (
        "/clear",
        "",
        "drop the conversation, keep the session and its settings",
    ),
    (
        "/approvals",
        "[on|off] [always]",
        "tool approval prompts; `always` persists to config.yaml",
    ),
    (
        "/cancel",
        "",
        "abort the in-flight turn (Esc; Esc x3 within 1 s stops its subagents too)",
    ),
    (
        "/compact",
        "[summary|cut|snap|style …]",
        "fold context; style sets the default",
    ),
    (
        "/endless",
        "[on|off|fold|pause|resume|model <id>|model clear]",
        "endless memory: switch it, fold the unsummarized backlog, pause/resume settling, pick this chat's compactor, or show the view state",
    ),
    ("/undo", "", "drop the last turn (redo restores)"),
    ("/redo", "", "restore the last undone turn"),
    (
        "/edit",
        "[n]",
        "edit turn n's message and regenerate from it (picker without n)",
    ),
    (
        "/regen",
        "[n]",
        "regenerate the reply to turn n (picker without n)",
    ),
    (
        "/fork",
        "[n]",
        "new session with the conversation through turn n (picker without n)",
    ),
    ("/quit", "", "exit"),
    (
        "/provider",
        "[add [name [url [key]]] | refresh <name> | rm <name> | limits <provider> <model> <context> [output] | find]",
        "providers: built-in opencode (Zen) / openrouter keyed via /secrets add; add prompts for a custom provider's name, url, key (no key for a local server); refresh lists a provider's models again; limits sets a model's context/output (0 = unknown); find probes local model servers",
    ),
    (
        "/artifacts",
        "",
        "files the agent showed (picker: ⏎ opens, s shows inline, e exports to ~/Downloads)",
    ),
    (
        "/apps",
        "",
        "interactive apps the agent built, run on desktop/web (picker: ⏎ opens, s shows source, e exports to ~/Downloads)",
    ),
    ("/exit", "", "same as /quit"),
];

/// Commands whose name starts with the typed word (`/mo` → `/model`);
/// `/apps` only while the app plugin runs.
pub fn slash_matches(
    word: &str,
    app_enabled: bool,
) -> Vec<&'static (&'static str, &'static str, &'static str)> {
    COMMANDS
        .iter()
        .filter(|c| c.0.starts_with(word) && (app_enabled || c.0 != "/apps"))
        .collect()
}

/// One slash popup row (`Model::slash_rows`): a theuth command, or a harness
/// command (`raw`) that is sent as typed when picked (141 D15 c).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlashRow {
    /// `/name` as completed into the editor.
    pub name: String,
    /// Argument hint shown after the name; `<…>` means one is required.
    pub hint: String,
    /// Description; harness rows are prefixed `[<harness>]`.
    pub desc: String,
    /// Picked → `Command::Send`/`Steer` with the raw line, not `submit`.
    pub raw: bool,
}

impl Model {

    /// Does `/name` hand over a skill: a known one; before the index has
    /// arrived, any plain word (the agent names an unknown skill) — but not
    /// one holding another `/` or a `.` (`/etc/hosts`, `/x.txt`), nor a bare `/`.
    pub(super) fn skill_word(&self, name: &str) -> bool {
        if self.skills.is_empty() {
            !name.is_empty() && !name.contains(['/', '.'])
        } else {
            self.skills.iter().any(|s| s.name == name)
        }
    }
}

impl Model {

    /// `/skills add <url>` | `rm <name>` | `sync`.
    pub(super) fn skills_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        let (sub, rest) = arg
            .split_once(' ')
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        match (sub, rest) {
            ("", _) => {
                self.skills_picker_pending = true;
                send(tx, Command::ListSkills);
            }
            ("sync", _) => send(tx, Command::SyncSkills),
            ("add", url) if !url.is_empty() => send(tx, Command::AddSkillRepo(url.to_owned())),
            ("rm", name) if !name.is_empty() => send(tx, Command::RemoveSkill(name.to_owned())),
            _ => self.entries.push(Entry::Error(
                "usage: /skills [add <url> | rm <name> | sync]".into(),
            )),
        }
    }
}

impl Model {

    /// Open or close the slash popup to match what the input box holds.
    pub fn sync_slash(&mut self, input: &str) {
        if self.prompt.is_some() || self.ask().is_some() || self.components.editor().is_some() {
            return;
        }
        // 141 §10.1: the watch-only editor lets go once the slash command is gone.
        self.watch_slash &= input.starts_with('/');
        match (&self.menu, input.starts_with('/')) {
            (None, true) => self.menu = Some(Menu::Slash { cursor: 0 }),
            (Some(Menu::Slash { .. }), false) => self.menu = None,
            _ => {}
        }
    }

    /// Builtin vocabulary in its original presentation order, before harness rows.
    pub(crate) fn command_specs(&self, word: &str) -> Vec<&'static crate::components::SlashSpec> {
        let mut specs: Vec<_> = COMMANDS.iter().chain(self.components.slash_specs())
            .filter(|spec| spec.0.starts_with(word) && (self.app_enabled || spec.0 != "/apps"))
            .collect();
        specs.sort_unstable_by_key(|spec| (command_rank(spec.0), spec.0));
        specs
    }

    /// Slash popup rows for the typed `word`: theuth's commands, then the
    /// harness's own (141 D15 c) labelled with the harness name — those are
    /// sent raw when picked (`raw`), never through `use_skill`.
    pub fn slash_rows(&self, word: &str) -> Vec<SlashRow> {
        let mut rows: Vec<SlashRow> = self.command_specs(word)
            .into_iter()
            .map(|(name, hint, desc)| SlashRow {
                name: (*name).to_owned(),
                hint: (*hint).to_owned(),
                desc: (*desc).to_owned(),
                raw: false,
            })
            .collect();
        let label = self.agent.as_deref().unwrap_or("harness");
        rows.extend(
            self.harness_commands
                .iter()
                .map(|c| SlashRow {
                    name: match c.name.strip_prefix('/') {
                        Some(_) => c.name.clone(),
                        None => format!("/{}", c.name),
                    },
                    hint: c.hint.clone().unwrap_or_default(),
                    desc: format!("[{label}] {}", c.description),
                    raw: true,
                })
                .filter(|r| r.name.starts_with(word)),
        );
        rows
    }
}

impl Model {

    pub(super) fn slash_key(&mut self, code: KeyCode, input: &str, tx: &UnboundedSender<Command>) -> MenuKey {
        let (word, arg) = input
            .split_once(' ')
            .map_or((input, ""), |(w, a)| (w, a.trim()));
        let rows = self.slash_rows(word);
        let Some(Menu::Slash { cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        *cursor = (*cursor).min(rows.len().saturating_sub(1));
        let picked = rows.get(*cursor);
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(rows.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Tab => {
                if let Some(r) = picked {
                    let tail = if arg.is_empty() && r.hint.is_empty() {
                        ""
                    } else {
                        " "
                    };
                    return MenuKey::Input(format!("{}{tail}{arg}", r.name));
                }
            }
            KeyCode::Enter => {
                let (line, raw) = match picked {
                    Some(r) if r.hint.starts_with('<') && arg.is_empty() => {
                        return MenuKey::Input(format!("{} ", r.name));
                    }
                    Some(r) => (format!("{} {arg}", r.name), r.raw),
                    None => (input.to_owned(), false),
                };
                self.menu = None;
                if raw {
                    // 141 D15 c: the harness's own command, as typed.
                    self.send_text(line.trim(), false, tx);
                } else {
                    self.submit(&line, tx);
                }
                return MenuKey::Input(String::new());
            }
            _ => {}
        }
        MenuKey::Consumed
    }
}

impl Model {

    /// `/skills`: Enter hands the highlighted skill to the model (04 §2.4);
    /// `s` shows its body as a card to review first.
    pub(super) fn skills_key(&mut self, code: KeyCode, tx: &UnboundedSender<Command>) -> MenuKey {
        let Some(Menu::Skills { items, cursor }) = &mut self.menu else {
            return MenuKey::Consumed;
        };
        match code {
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Esc => self.menu = None,
            KeyCode::Char('s') => {
                if let Some(s) = items.get(*cursor) {
                    send(tx, Command::LoadSkill(s.name.clone()));
                }
                self.menu = None;
            }
            KeyCode::Enter => {
                let name = items.get(*cursor).map(|s| s.name.clone());
                self.menu = None;
                if let Some(name) = name {
                    self.use_skill(name, String::new(), tx);
                }
            }
            _ => {}
        }
        MenuKey::Consumed
    }

    /// Hand a skill to the model: the agent composes the input and delivers
    /// it like typed text (turn when idle, steer when running); the cards
    /// arrive with `TurnStarted` / `InputInjected`.
    pub(super) fn use_skill(&mut self, name: String, args: String, tx: &UnboundedSender<Command>) {
        self.awaiting_session = self.session.is_none();
        send(
            tx,
            Command::UseSkill {
                session: self.session,
                name,
                args,
                draft: self.draft_options(),
            },
        );
    }
}

// Presentation metadata only: routing and command declarations remain with owners.
pub(crate) fn command_rank(name: &str) -> usize {
    match name {
        "/model" => 0, "/title" => 1, "/vision" => 2, "/effort" => 3,
        "/advisor" => 4, "/aws" => 5, "/mcp" => 6, "/acp" => 7,
        "/skills" => 8, "/knowledge" => 9, "/login" => 10, "/logout" => 11,
        "/prompt" => 12, "/agents" => 13, "/todos" => 14, "/dictate" => 15,
        "/voice" => 16, "/talk" => 17, "/jobs" => 18, "/terminals" => 19,
        "/cost" => 20, "/project" => 21, "/schedules" => 22, "/autoresearch" => 23,
        "/tasks" => 24, "/roster" => 25, "/computer" => 26, "/parent" => 27,
        "/sessions" => 28, "/take" => 29, "/broadcast" => 30, "/search" => 31,
        "/attach" => 32, "/resume" => 33, "/new" => 34, "/clear" => 35,
        "/approvals" => 36, "/communication" => 37, "/peers" => 38, "/a2a" => 39,
        "/double" => 40, "/cancel" => 41, "/compact" => 42, "/endless" => 43,
        "/shake" => 44, "/undo" => 45, "/redo" => 46, "/edit" => 47,
        "/regen" => 48, "/fork" => 49, "/quit" => 50, "/provider" => 51,
        "/secrets" => 52, "/link" => 53, "/mesh" => 54, "/nodes" => 55,
        "/sandbox" => 56, "/artifacts" => 57, "/apps" => 58, "/tools" => 59,
        "/exit" => 60, _ => usize::MAX,
    }
}

#[test]
fn completion_preserves_builtin_order_across_component_owners() {
    let model = Model::new(String::new(), true);
    let selected = ["/mcp", "/jobs", "/computer", "/broadcast", "/sandbox", "/tools"];
    let names: Vec<_> = model.command_specs("/").into_iter()
        .map(|spec| spec.0).filter(|name| selected.contains(name)).collect();
    assert_eq!(names, selected);
}
