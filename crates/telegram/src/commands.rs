//! NEEDS(COMMANDS): native bot commands (design doc 98 §10.3). Stage 1 lands
//! the command table, [`owner_only`], [`scopes`] and [`help`] for real;
//! [`handle`] / [`continue_prompt`] answer the help panel until COMMANDS
//! replaces them.

use runtime::SessionId;

use crate::api::{BotCommandSpec, CommandScope, Keyboard};
use crate::port::Port;
use crate::{TelegramStatus, ui};

/// Name without slash, argument hint, one-line description (09 §9 table
/// minus `approve` / `deny` / `signal`, plus `start` and `telegram`).
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("start", "[code]", "pair with a code, or this list"),
    ("help", "", "this list"),
    ("status", "", "polling state, session, model, turn"),
    ("new", "", "start a new thread"),
    ("sessions", "", "10 newest sessions"),
    (
        "resume",
        "<id>",
        "bind this thread to a session (prefix ok)",
    ),
    (
        "cancel",
        "[name]",
        "abort the turn, or a subagent's by name",
    ),
    ("compact", "[summary|cut|snap|style …]", "fold the context"),
    (
        "clear",
        "",
        "drop the conversation, keep the thread's settings",
    ),
    ("undo", "", "drop the last turn (redo restores)"),
    ("redo", "", "restore the last undone turn"),
    ("model", "[id]", "show or set the model"),
    (
        "effort",
        "[level|clear]",
        "show or set the reasoning effort",
    ),
    (
        "agents",
        "",
        "this session's subagents: name · state · last · age",
    ),
    ("jobs", "[kill <id>]", "list background jobs"),
    ("prompt", "", "the assembled system prompt"),
    ("skills", "[add <url>|rm <name>|sync]", "skills"),
    ("mcp", "[add <url|json>|sync]", "MCP servers"),
    ("login", "[name]", "list logins or start one"),
    ("logout", "<name>", "log out"),
    (
        "provider",
        "[add <name> <url> <key>|rm <name>]",
        "OpenAI-compatible providers",
    ),
    (
        "approvals",
        "on|off [always]",
        "tool approval prompts (owner)",
    ),
    (
        "telegram",
        "[status|link|allow <id>|rm <id>|group add <id>|group rm <id>]",
        "this channel (owner)",
    ),
    (
        "communication",
        "[on|off [always]]",
        "peer messaging for this session or the default (owner)",
    ),
    (
        "peers",
        "[<query> | scope owner|isolated [alias]]",
        "this session's peer directory or root scope (owner)",
    ),
    (
        "requests",
        "[<id> | answer <id> <text>]",
        "pending peer requests: inspect or answer one (owner)",
    ),
    (
        "broadcast",
        "[--next] running|roots|<id>[,<id>…] <text>",
        "one message to many sessions; --next lands after the turn (owner)",
    ),
    ("a2a", "[reload]", "network status or config reload (owner)"),
];

const OWNER_ONLY: &[&str] = &[
    "approvals",
    "telegram",
    "communication",
    "peers",
    "requests",
    "a2a",
    "broadcast",
];

/// Commands in the group scope (98 §10.3), each `is_ephemeral`.
const GROUP: &[&str] = &[
    "help", "status", "new", "sessions", "resume", "cancel", "compact", "model", "agents",
];

/// Owner-only names (09 §9 with `telegram` in place of `signal`).
#[must_use]
pub fn owner_only(name: &str) -> bool {
    OWNER_ONLY.contains(&name)
}

fn spec(name: &str, description: &str, ephemeral: bool) -> BotCommandSpec {
    BotCommandSpec {
        name: name.to_owned(),
        description: description.to_owned(),
        ephemeral,
    }
}

/// The three `setMyCommands` scopes (98 §10.3): default = public set; the
/// owner's chat = everything; all groups = [`GROUP`] flagged ephemeral.
#[must_use]
pub fn scopes(owner: Option<i64>) -> Vec<(CommandScope, Vec<BotCommandSpec>)> {
    let public: Vec<BotCommandSpec> = COMMANDS
        .iter()
        .filter(|(n, _, _)| !owner_only(n))
        .map(|(n, _, d)| spec(n, d, false))
        .collect();
    let groups: Vec<BotCommandSpec> = COMMANDS
        .iter()
        .filter(|(n, _, _)| GROUP.contains(n))
        .map(|(n, _, d)| spec(n, d, true))
        .collect();
    let mut out = vec![
        (CommandScope::Default, public),
        (CommandScope::AllGroupChats, groups),
    ];
    if let Some(chat) = owner {
        out.push((
            CommandScope::Chat(chat),
            COMMANDS.iter().map(|(n, _, d)| spec(n, d, false)).collect(),
        ));
    }
    out
}

/// What a command runs against.
pub struct CommandCx<'a> {
    /// Invoker.
    pub user: u64,
    /// `telegram.owner`.
    pub owner: Option<u64>,
    /// Chat the command was typed in.
    pub chat: i64,
    /// Topic, if any.
    pub thread: Option<i64>,
    /// A group chat (replies ephemeral where possible).
    pub in_group: bool,
    /// The thread's bound session.
    pub session: Option<SessionId>,
    /// Snapshot for `/status` and `/telegram status`.
    pub status: TelegramStatus,
    /// Agent asks.
    pub port: &'a dyn Port,
    /// Panel page (`cmd:page:…`).
    pub page: usize,
}

/// A rendered reply.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Reply {
    /// Rich Markdown.
    pub markdown: String,
    /// Inline keyboard, if any.
    pub keyboard: Option<Keyboard>,
}

/// What the channel does after a command.
#[derive(Debug)]
pub enum Outcome {
    /// Send the reply.
    Reply(Reply),
    /// Unbind this thread, then reply.
    NewThread(Reply),
    /// Bind this thread to `session`, then reply.
    Resume {
        /// Session.
        session: SessionId,
        /// Its title (for the topic name).
        title: String,
        /// Reply.
        reply: Reply,
    },
    /// The turn was cancelled; reply.
    Cancelled {
        /// Session.
        session: SessionId,
        /// Reply.
        reply: Reply,
    },
    /// Owner-only: the channel applies and replies.
    Allow(u64),
    /// Owner-only: the channel applies and replies.
    RemoveAllowed(u64),
    /// Owner-only: the channel applies and replies.
    AllowGroup(i64),
    /// Owner-only: the channel applies and replies.
    RemoveGroup(i64),
    /// The channel mints a code and replies with link + QR.
    Link,
    /// A `force_reply` prompt; the next reply to that message is `expect`.
    Prompt {
        /// The prompt.
        reply: Reply,
        /// What the answer is.
        expect: Expect,
    },
}

/// What a prompted reply is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expect {
    /// `/skills add …` URL.
    SkillsAdd,
    /// `/mcp add …` URL or JSON.
    McpAdd,
}

/// Runs `name` with `arg`.
// NEEDS(COMMANDS): the dispatcher. Stage 1 answers the help panel.
pub async fn handle(_cx: CommandCx<'_>, _name: &str, _arg: &str) -> Outcome {
    Outcome::Reply(help())
}

/// Finishes a [`Outcome::Prompt`] with the user's reply.
// NEEDS(COMMANDS): `skills add` / `mcp add`. Stage 1 answers the help panel.
pub async fn continue_prompt(_cx: CommandCx<'_>, _expect: Expect, _text: &str) -> Outcome {
    Outcome::Reply(help())
}

/// The help panel: the command table.
#[must_use]
pub fn help() -> Reply {
    let rows = std::iter::once(vec![
        "command".to_owned(),
        "args".to_owned(),
        "what".to_owned(),
    ])
    .chain(
        COMMANDS
            .iter()
            .map(|(n, a, d)| vec![format!("/{n}"), (*a).to_owned(), (*d).to_owned()]),
    );
    Reply {
        markdown: ui::table(rows),
        keyboard: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_split_public_owner_and_group() {
        let s = scopes(Some(7));
        assert_eq!(s.len(), 3);
        let (scope, public) = &s[0];
        assert_eq!(*scope, CommandScope::Default);
        assert!(public.iter().all(|c| !owner_only(&c.name) && !c.ephemeral));
        assert!(public.iter().any(|c| c.name == "start"));
        let (scope, groups) = &s[1];
        assert_eq!(*scope, CommandScope::AllGroupChats);
        assert_eq!(groups.len(), GROUP.len());
        assert!(groups.iter().all(|c| c.ephemeral));
        let (scope, all) = &s[2];
        assert_eq!(*scope, CommandScope::Chat(7));
        assert_eq!(all.len(), COMMANDS.len());
        assert!(all.iter().any(|c| c.name == "telegram"));
        assert_eq!(scopes(None).len(), 2);
    }

    #[test]
    fn table_is_well_formed_for_the_api() {
        for (n, _, d) in COMMANDS {
            assert!(
                n.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_'),
                "{n}"
            );
            assert!((3..=256).contains(&d.len()), "{n}");
        }
        assert!(
            !COMMANDS
                .iter()
                .any(|(n, _, _)| *n == "approve" || *n == "deny" || *n == "signal")
        );
        assert!(owner_only("telegram"));
        assert!(!owner_only("help"));
        assert!(help().markdown.contains("| /help |"));
    }
}
