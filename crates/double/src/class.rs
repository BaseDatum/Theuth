//! Decision classes (126 §3): the deterministic `kind:shape:requester` key of a
//! card, computed from the payload alone so the same card lands in the same
//! class on every engine and after every restart. Classes are discovered —
//! the first card of a new key creates the `classes` row at L0 — and v1 has
//! no merge/split (Revision 1 ruling 1).

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use tools::Risk;

/// What kind of card a class keys (126 §2 "decision card").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export))]
#[serde(rename_all = "snake_case")]
pub enum CardKind {
    /// A tool-call approval raised by the registry.
    Approval,
    /// An `ask_user` question set.
    Ask,
    /// An attention-inbox item that is not an approval or ask (104).
    Attention,
    /// A decision / review packet (101 / 103).
    Packet,
    /// A task / agent / routine draft card set.
    Draft,
}

impl CardKind {
    /// The key prefix, also the `classes.kind` column value.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Approval => "approval",
            Self::Ask => "ask",
            Self::Attention => "attention",
            Self::Packet => "packet",
            Self::Draft => "draft",
        }
    }
}

/// The three draft tools (crates/app/src/{draft,agent_draft,routine_draft}.rs).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DraftKind {
    /// A task draft (`task_draft`).
    Task,
    /// An agent draft (`agent_draft`).
    Agent,
    /// A routine draft (`routine_draft`).
    Routine,
}

impl DraftKind {
    /// The key spelling.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Task => "task",
            Self::Agent => "agent",
            Self::Routine => "routine",
        }
    }
}

/// Who raised the card (126 §3). Part of the key: the same `git push` means
/// different things from the owner's own session and from a nightly routine.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind", content = "name")]
pub enum Requester {
    /// A session the owner opened.
    Root,
    /// A child of a root session.
    Subagent,
    /// A desk session; the roster's session→agent map names the agent.
    Desk(String),
    /// A schedule run, by routine name.
    Routine(String),
}

impl fmt::Display for Requester {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Root => f.write_str("root"),
            Self::Subagent => f.write_str("subagent"),
            Self::Desk(a) => write!(f, "desk:{a}"),
            Self::Routine(n) => write!(f, "routine:{n}"),
        }
    }
}

/// The card payload the keyer reads — only what the shape rule needs.
#[derive(Clone, Debug)]
pub enum Card<'a> {
    /// `ApprovalRequested { tool, args, risk }`. `mcp` is true for a tool the
    /// MCP bridge registered (`<server>__<tool>`, or a bare remote name): the
    /// registry knows, the payload does not, so the subscriber passes it.
    Approval {
        /// Tool name as the registry knows it.
        tool: &'a str,
        /// Raw arguments (`shell` reads `command`; `edit` / `write_file` read `path`).
        args: &'a serde_json::Value,
        /// Risk class that triggered the ask.
        risk: Risk,
        /// The tool came from the MCP bridge.
        mcp: bool,
    },
    /// An `ask_user` set.
    Ask {
        /// Every option label of the set, in card order; empty = freeform.
        options: &'a [String],
    },
    /// An attention-inbox item. `approval` / `ask` items are the registry's
    /// own cards and are keyed by those rules instead; `handoff` / `secret`
    /// are never predicted — callers do not key them.
    Attention {
        /// `AttentionKind` in snake_case (the `serde` name).
        kind: &'a str,
    },
    /// A decision / review packet.
    Packet {
        /// `PacketKind::as_str`: `decision` | `review`.
        kind: &'a str,
    },
    /// A draft card set.
    Draft(DraftKind),
}

/// The key of a freeform ask: never auto-answered.
pub const ASK_FREEFORM: &str = "ask:freeform";

/// `kind:shape:requester` (126 §3).
pub fn class_key(card: &Card<'_>, requester: &Requester) -> String {
    format!("{}:{requester}", shape(card))
}

/// The key without its requester suffix.
fn shape(card: &Card<'_>) -> String {
    match card {
        Card::Approval {
            tool,
            args,
            risk,
            mcp,
        } => {
            let risk = risk.as_str();
            match *tool {
                "shell" => {
                    let cmd = args.get("command").and_then(|c| c.as_str()).unwrap_or("");
                    format!("approval:shell:{risk}:{}", shell_shape(cmd))
                }
                "edit" | "write_file" | "move_text" | "copy_text" | "indent" => {
                    // One class per touched-path shape set (move_text: source + target,
                    // so approving a move inside one tree does not cover a move across trees).
                    let mut shapes: Vec<String> = Vec::new();
                    for p in touched_paths(tool, args) {
                        let s = path_shape(p);
                        if !shapes.contains(&s) {
                            shapes.push(s);
                        }
                    }
                    if shapes.is_empty() {
                        shapes.push(path_shape(""));
                    }
                    format!("approval:{tool}:{risk}:{}", shapes.join("+"))
                }
                _ if *mcp || tool.contains("__") => {
                    let mut keys: Vec<&str> = args
                        .as_object()
                        .map(|o| o.keys().map(String::as_str).collect())
                        .unwrap_or_default();
                    keys.sort_unstable();
                    format!("approval:{tool}:{risk}:{}", keys.join(","))
                }
                _ => format!("approval:{tool}:{risk}"),
            }
        }
        Card::Ask { options } => match ask_hash(options) {
            Some(h) => format!("ask:{h}"),
            None => ASK_FREEFORM.to_owned(),
        },
        Card::Attention { kind } => format!("attention:{kind}"),
        Card::Packet { kind } => format!("packet:{kind}"),
        Card::Draft(k) => format!("draft:{}", k.as_str()),
    }
}

/// The first two command tokens after stripping leading `cd … &&` segments,
/// `VAR=value` assignments and `nohup` / `nice [-n N]` / `env` prefixes; the
/// first token keeps only its basename (`/usr/bin/git` → `git`). Empty when
/// nothing is left.
pub fn shell_shape(command: &str) -> String {
    let mut rest = command.trim();
    // `cd /a/b && cd c && git push` → `git push`.
    while rest.starts_with("cd ") || rest == "cd" {
        match rest.find("&&") {
            Some(i) => rest = rest[i + 2..].trim_start(),
            None => return String::new(),
        }
    }
    let mut tokens = rest.split_whitespace().peekable();
    while let Some(t) = tokens.peek().copied() {
        if is_assignment(t) || t == "nohup" || t == "env" {
            tokens.next();
        } else if t == "nice" {
            tokens.next();
            match tokens.peek().copied() {
                Some("-n") => {
                    tokens.next();
                    tokens.next();
                }
                Some(a) if a.starts_with('-') && a[1..].bytes().all(|b| b.is_ascii_digit()) => {
                    tokens.next();
                }
                _ => {}
            }
        } else {
            break;
        }
    }
    let mut out = String::new();
    if let Some(first) = tokens.next() {
        out.push_str(first.rsplit('/').next().unwrap_or(first));
        if let Some(second) = tokens.next() {
            out.push(' ');
            out.push_str(second);
        }
    }
    out
}

/// `VAR=value` (shell identifier before the `=`).
fn is_assignment(t: &str) -> bool {
    let Some(i) = t.find('=') else {
        return false;
    };
    let name = &t[..i];
    !name.is_empty()
        && !name.starts_with(|c: char| c.is_ascii_digit())
        && name.bytes().all(|b| b == b'_' || b.is_ascii_alphanumeric())
}

/// Paths a path-shaped tool call touches. Refs split at the last `#`; `clip:` refs have none.
fn touched_paths<'a>(tool: &str, args: &'a serde_json::Value) -> Vec<&'a str> {
    let get = |k: &str| args.get(k).and_then(|v| v.as_str());
    let ref_path =
        |s: &'a str| (!s.starts_with("clip:")).then(|| s.rsplit_once('#').map_or(s, |(p, _)| p));
    let target = || {
        ["before", "after", "at"]
            .into_iter()
            .find_map(get)
            .and_then(ref_path)
    };
    match tool {
        "move_text" => get("src")
            .and_then(ref_path)
            .into_iter()
            .chain(target())
            .collect(),
        "copy_text" => target().into_iter().collect(),
        _ => get("path").into_iter().collect(),
    }
}

/// The first path component under the home directory, or the first two when
/// the first is not a dot-directory (`Develop/theuth`, `.theuth`); a path
/// outside home or relative → its first component.
// ponytail: the repo root is not in the payload; "two components under home
// unless dotted" is the heuristic. A projects dir nested deeper than one level
// keys on the projects dir; refine with the session's repo root if that bites.
pub fn path_shape(path: &str) -> String {
    let p = Path::new(path);
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
    let rel = match home.as_deref().and_then(|h| p.strip_prefix(h).ok()) {
        Some(rel) => Some((rel, true)),
        None if p.is_absolute() => p.strip_prefix("/").ok().map(|r| (r, false)),
        None => Some((p, false)),
    };
    let Some((rel, under_home)) = rel else {
        return String::new();
    };
    let mut parts = rel.components().filter_map(|c| match c {
        std::path::Component::Normal(s) => s.to_str(),
        _ => None,
    });
    let Some(first) = parts.next() else {
        return String::new();
    };
    if under_home
        && !first.starts_with('.')
        && let Some(second) = parts.next()
    {
        return format!("{first}/{second}");
    }
    first.to_owned()
}

/// First 12 hex of sha256 over the normalised option labels (lower-cased,
/// whitespace collapsed, sorted, `\n`-joined); `None` when there are none.
pub fn ask_hash(options: &[String]) -> Option<String> {
    if options.is_empty() {
        return None;
    }
    let mut labels: Vec<String> = options
        .iter()
        .map(|l| {
            l.split_whitespace()
                .collect::<Vec<_>>()
                .join(" ")
                .to_lowercase()
        })
        .collect();
    labels.sort_unstable();
    Some(platform::sha256::hex(labels.join("\n").as_bytes())[..12].to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn approval(
        tool: &str,
        args: serde_json::Value,
        risk: Risk,
        mcp: bool,
        req: Requester,
    ) -> String {
        class_key(
            &Card::Approval {
                tool,
                args: &args,
                risk,
                mcp,
            },
            &req,
        )
    }

    /// 126 §3: the ten example keys. Key 9's hex is the real sha256 of the
    /// task-draft trio (the design's `3f9a1c2b7d0e` was written without a shell).
    #[test]
    fn ten_example_keys() {
        let sh =
            |cmd: &str, risk, req| approval("shell", json!({ "command": cmd }), risk, false, req);
        assert_eq!(
            sh("git push origin main", Risk::Mutating, Requester::Root),
            "approval:shell:mutating:git push:root"
        );
        assert_eq!(
            sh(
                "git worktree add ../x feat/x",
                Risk::Mutating,
                Requester::Root
            ),
            "approval:shell:mutating:git worktree:root"
        );
        assert_eq!(
            sh(
                "cd /Users/x/Develop/theuth && CARGO_BUILD_JOBS=4 cargo test -p tools",
                Risk::Mutating,
                Requester::Subagent
            ),
            "approval:shell:mutating:cargo test:subagent"
        );
        assert_eq!(
            sh("rm -rf target", Risk::Destructive, Requester::Root),
            "approval:shell:destructive:rm -rf:root"
        );
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            approval(
                "write_file",
                json!({ "path": format!("{home}/Develop/theuth/crates/x/src/lib.rs"), "content": "" }),
                Risk::Mutating,
                false,
                Requester::Subagent
            ),
            "approval:write_file:mutating:Develop/theuth:subagent"
        );
        assert_eq!(
            approval(
                "edit",
                json!({ "path": format!("{home}/.theuth/config.yaml") }),
                Risk::Mutating,
                false,
                Requester::Root
            ),
            "approval:edit:mutating:.theuth:root"
        );
        assert_eq!(
            approval(
                "move_text",
                json!({ "src": format!("{home}/Develop/theuth/a.rs#L1-L3@1a2b3c4d"), "after": format!("{home}/.theuth/b.md#Sym") }),
                Risk::Mutating,
                false,
                Requester::Root
            ),
            "approval:move_text:mutating:Develop/theuth+.theuth:root"
        );
        assert_eq!(
            approval(
                "copy_text",
                json!({ "src": "clip:x", "at": format!("{home}/.theuth/b.md#end") }),
                Risk::Mutating,
                false,
                Requester::Root
            ),
            "approval:copy_text:mutating:.theuth:root"
        );
        assert_eq!(
            approval(
                "create_pull_request",
                json!({ "title": "t", "owner": "o", "repo": "r", "head": "h", "base": "b", "body": "" }),
                Risk::Mutating,
                true,
                Requester::Root
            ),
            "approval:create_pull_request:mutating:base,body,head,owner,repo,title:root"
        );
        assert_eq!(
            approval(
                "knowledge_put",
                json!({ "id": "x", "body": "y" }),
                Risk::Mutating,
                false,
                Requester::Root
            ),
            "approval:knowledge_put:mutating:root"
        );
        let trio = [
            "Add to backlog".to_owned(),
            "Start now".to_owned(),
            "Discard".to_owned(),
        ];
        assert_eq!(
            class_key(&Card::Ask { options: &trio }, &Requester::Root),
            "ask:b9474647c6a4:root"
        );
        assert_eq!(
            class_key(&Card::Draft(DraftKind::Agent), &Requester::Root),
            "draft:agent:root"
        );
    }

    /// The ask hash ignores case, whitespace and order, and the question text.
    #[test]
    fn ask_hash_normalises() {
        let a = [
            "Start now".to_owned(),
            "Discard".to_owned(),
            "Add to backlog".to_owned(),
        ];
        let b = [
            "discard".to_owned(),
            "  add  to\tbacklog ".to_owned(),
            "START NOW".to_owned(),
        ];
        assert_eq!(ask_hash(&a), ask_hash(&b));
        assert_ne!(ask_hash(&a), ask_hash(&a[..2]));
        assert_eq!(
            class_key(&Card::Ask { options: &[] }, &Requester::Root),
            "ask:freeform:root"
        );
    }

    #[test]
    fn shell_prefixes_are_stripped() {
        assert_eq!(
            shell_shape("nohup nice -n 15 env FOO=1 /usr/bin/git push"),
            "git push"
        );
        assert_eq!(shell_shape("nice -5 cargo build --release"), "cargo build");
        assert_eq!(shell_shape("cd \"/a b\" && cd c && ls"), "ls");
        assert_eq!(shell_shape("cd /tmp"), "");
        assert_eq!(shell_shape("A=1 B=2"), "");
        assert_eq!(shell_shape("git push && curl evil"), "git push");
    }

    #[test]
    fn other_kinds_and_requesters() {
        let desk = Requester::Desk("reviewer".into());
        assert_eq!(
            class_key(&Card::Attention { kind: "review" }, &desk),
            "attention:review:desk:reviewer"
        );
        assert_eq!(
            class_key(
                &Card::Packet { kind: "decision" },
                &Requester::Routine("nightly".into())
            ),
            "packet:decision:routine:nightly"
        );
        // MCP by name shape, without the flag.
        assert_eq!(
            approval(
                "gh__list_issues",
                json!({ "repo": "r", "owner": "o" }),
                Risk::ReadOnly,
                false,
                Requester::Root
            ),
            "approval:gh__list_issues:read_only:owner,repo:root"
        );
        assert_eq!(path_shape("/tmp/x/y"), "tmp");
        assert_eq!(path_shape("crates/double/src/lib.rs"), "crates");
    }
}
