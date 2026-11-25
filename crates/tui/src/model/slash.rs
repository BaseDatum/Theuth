use super::*;

/// Slash commands: name, argument hint (`<x>` required, `[x]` optional, `""` none), description.
pub const COMMANDS: &[(&str, &str, &str)] = &[
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
        "/always-ask",
        "[<tool> [on|off] [always]]",
        "a tool always needs your approval, even with approvals off; `always` = every chat (list without args)",
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
        "/fractal",
        "[on|off|fold|pause|resume|model <id>|model clear]",
        "fractal context: switch it, fold the unsummarized backlog, pause/resume settling, pick this chat's compactor, or show the view state",
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

    ("/exit", "", "same as /quit"),
];

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

    /// Open or close the slash popup to match what the input box holds.
    pub fn sync_slash(&mut self, input: &str) {
        if self.ask().is_some() || self.components.editor().is_some() {
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
            .filter(|spec| spec.0.starts_with(word))
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
        // 216: plugin `/commands` after the built-ins, which win a clash.
        if let Some(plugins) = self.components.get::<crate::components::plugins::Plugins>() {
            for (name, hint, desc) in plugins.commands() {
                if name.starts_with(word) && !rows.iter().any(|r| r.name == name) {
                    rows.push(SlashRow { name, hint, desc, raw: false });
                }
            }
        }
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
        "/approvals" => 36, "/always-ask" => 37, "/communication" => 38, "/peers" => 39,
        "/a2a" => 40, "/double" => 41, "/cancel" => 42, "/compact" => 43,
        "/fractal" => 44, "/shake" => 45, "/undo" => 46, "/redo" => 47,
        "/edit" => 48, "/regen" => 49, "/fork" => 50, "/quit" => 51,
        "/provider" => 52, "/secrets" => 53, "/link" => 54, "/mesh" => 55,
        "/nodes" => 56, "/sandbox" => 57, "/artifacts" => 58, "/apps" => 59,
        "/tools" => 60, "/exit" => 61, _ => usize::MAX,
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
