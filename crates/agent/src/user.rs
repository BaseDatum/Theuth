//! `message_user`: the agent reaches the user out of band (design doc 24).
//! The Slack and Signal plugins register a [`UserSink`] with the
//! [`UserChannels`] actor (`"user"`); [`MessageUserTool`] validates the
//! files, builds the calling session's ancestor chain and asks the actor to
//! deliver in two passes: bound thread first, owner fallback second (24 §2).

use std::path::PathBuf;
use std::sync::Arc;

use kameo::actor::ActorRef;
use kameo::error::SendError;
use kameo::message::{Context, Message};
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tools::{Risk, Tool, ToolCx, ToolOutput};

use crate::Ancestors;
use crate::peers::supervisor;

/// Registry name of the [`UserChannels`] actor (24 §3).
pub const USER_ACTOR: &str = "user";

/// Largest file the tool forwards: Signal's ceiling, one cap for both (24 §3).
pub const MAX_FILE: u64 = 100 * 1024 * 1024;

/// Tool error when no sink is registered (24 §2).
const NO_CHANNEL: &str = "no user channel: slack and signal are not running";

/// What one frontend can deliver; registered by the Slack and Signal plugins at start (24 §3).
#[async_trait::async_trait]
pub trait UserSink: Send + Sync {
    /// `"slack"` / `"signal"`: the `via` value and the word in the tool result.
    fn name(&self) -> &'static str;
    /// Deliver `m` to the first session of `m.sessions` this sink renders
    /// (`fallback == false`), or to the owner (`fallback == true`).
    /// `Ok(None)`: not mine (bound pass only). `Ok(Some(where))`: delivered,
    /// human description. `Err(why)`: mine, but the send failed.
    async fn deliver(&self, m: &Outbound, fallback: bool) -> Result<Option<String>, String>;
}

/// One message for the user (24 §3).
#[derive(Debug, Clone)]
pub struct Outbound {
    /// Calling session first, root last (24 §2).
    pub sessions: Vec<SessionId>,
    /// `session <last 8> · <title>` for the fallback pass; the sink decides how to show it.
    pub origin: String,
    /// Markdown; Signal receives it as plain text.
    pub text: String,
    /// Validated by the tool: regular files, at most [`MAX_FILE`] each, with their guessed MIME.
    pub files: Vec<OutboundFile>,
}

/// One validated attachment (24 §3); the sink reads `path` itself.
#[derive(Debug, Clone)]
pub struct OutboundFile {
    /// Absolute path on this machine.
    pub path: PathBuf,
    /// File name shown to the user.
    pub name: String,
    /// MIME by extension, `application/octet-stream` when unknown.
    pub mime: String,
    /// Size on disk.
    pub bytes: u64,
}

/// The sink registry (24 §3). Look up with `ActorRef::<UserChannels>::lookup(USER_ACTOR)`.
#[derive(Default, kameo::Actor)]
pub struct UserChannels {
    sinks: Vec<Arc<dyn UserSink>>,
}

/// `ask` → `()`. Replaces an existing sink with the same name (24 §3).
pub struct RegisterUserSink(pub Arc<dyn UserSink>);

impl Message<RegisterUserSink> for UserChannels {
    type Reply = ();

    async fn handle(&mut self, m: RegisterUserSink, _: &mut Context<Self, Self::Reply>) {
        self.sinks.retain(|s| s.name() != m.0.name());
        self.sinks.push(m.0);
    }
}

/// `ask` → `Result<Vec<String>, String>`: run the two passes of 24 §2 and
/// return one `where` per sink that delivered (a failed sink is appended as
/// `<name> failed: <why>`). `Err` when nothing delivered.
pub struct Deliver {
    /// The message.
    pub m: Outbound,
    /// Restrict both passes to the sink with this name.
    pub via: Option<String>,
}

impl Message<Deliver> for UserChannels {
    type Reply = Result<Vec<String>, String>;

    async fn handle(
        &mut self,
        m: Deliver,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<Vec<String>, String> {
        let sinks: Vec<&Arc<dyn UserSink>> = self
            .sinks
            .iter()
            .filter(|s| m.via.as_deref().is_none_or(|v| v == s.name()))
            .collect();
        if sinks.is_empty() {
            return Err(match m.via {
                Some(v) => format!("no user channel named {v}"),
                None => NO_CHANNEL.to_owned(),
            });
        }
        let mut delivered = Vec::new();
        let mut failed = Vec::new();
        for fallback in [false, true] {
            for s in &sinks {
                match s.deliver(&m.m, fallback).await {
                    Ok(Some(w)) => delivered.push(w),
                    Ok(None) => {}
                    Err(why) => failed.push(format!("{} failed: {why}", s.name())),
                }
            }
            if !delivered.is_empty() {
                break;
            }
        }
        if delivered.is_empty() {
            return Err(failed.join("; "));
        }
        delivered.extend(failed);
        Ok(delivered)
    }
}

/// Arguments for [`MessageUserTool`] (24 §3).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct MessageUserArgs {
    /// What to say. Markdown; Signal receives it as plain text.
    pub text: String,
    /// Absolute paths of files to attach. Images preview inline on both channels.
    #[serde(default)]
    pub files: Vec<PathBuf>,
    /// Restrict to one channel. Default: the channel this session is bound to,
    /// else every channel's owner.
    pub via: Option<String>,
}

/// Out-of-band message to the user on Slack and/or Signal (24 §3).
pub struct MessageUserTool;

#[async_trait::async_trait]
impl Tool for MessageUserTool {
    const NAME: &'static str = "message_user";
    // Recipient is always the user; see 24 §4.
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = true;
    type Args = MessageUserArgs;

    fn description(&self) -> String {
        "Send a message, optionally with files or images, to the user on their chat channel \
         (Slack or Signal). Arrives immediately and out of band. Your normal reply already \
         reaches the user — use this only to hand over files or images, or when asked to report \
         progress while working. `files` are absolute paths of existing files."
            .into()
    }

    async fn call(&self, args: MessageUserArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if args.text.trim().is_empty() && args.files.is_empty() {
            return Ok(ToolOutput::err(
                "nothing to send: give text, files, or both",
            ));
        }
        let files = match check_files(&args.files, MAX_FILE) {
            Ok(f) => f,
            Err(e) => return Ok(ToolOutput::err(e)),
        };
        let Some(channels) = ActorRef::<UserChannels>::lookup(USER_ACTOR).ok().flatten() else {
            return Ok(ToolOutput::err(NO_CHANNEL));
        };
        let mut chain = match supervisor() {
            Ok(agents) => agents
                .ask(Ancestors {
                    session: cx.session,
                })
                .await
                .unwrap_or_default(),
            Err(_) => Vec::new(),
        };
        if chain.is_empty() {
            chain.push(crate::AncestorInfo {
                id: cx.session,
                title: None,
                name: None,
            });
        }
        let root = chain.last().expect("non-empty chain");
        let id = root.id.to_string();
        let origin = format!(
            "session {} · {}",
            &id[id.len().saturating_sub(8)..],
            root.title.as_deref().unwrap_or("untitled")
        );
        let m = Outbound {
            sessions: chain.iter().map(|a| a.id).collect(),
            origin,
            // 30 §9: json-render fences as their text rendering on chat channels.
            text: surface::flatten(&args.text, 60),
            files,
        };
        Ok(match channels.ask(Deliver { m, via: args.via }).await {
            Ok(wheres) => ToolOutput::ok(format!("delivered via {}", wheres.join(", "))),
            Err(SendError::HandlerError(why)) => ToolOutput::err(why),
            Err(e) => ToolOutput::err(format!("user channels: {e}")),
        })
    }
}

/// Refuse what a sink could not send (24 §3): a missing or non-regular file,
/// or one over `max` bytes. The error names the path.
fn check_files(paths: &[PathBuf], max: u64) -> Result<Vec<OutboundFile>, String> {
    paths
        .iter()
        .map(|p| {
            let meta = match std::fs::metadata(p) {
                Ok(m) if m.is_file() => m,
                Ok(_) => return Err(format!("{}: not a regular file", p.display())),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(format!("{}: not found", p.display()));
                }
                Err(e) => return Err(format!("{}: {e}", p.display())),
            };
            if meta.len() > max {
                return Err(format!(
                    "{}: {:.1} MB exceeds {} MB",
                    p.display(),
                    meta.len() as f64 / (1024.0 * 1024.0),
                    max >> 20
                ));
            }
            Ok(OutboundFile {
                path: p.clone(),
                name: p
                    .file_name()
                    .unwrap_or(p.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
                mime: mime_guess::from_path(p).first_or_octet_stream().to_string(),
                bytes: meta.len(),
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use kameo::actor::Spawn;

    use super::*;

    /// Records every `(fallback)` call and the text delivered; delivers on
    /// the bound pass when `bound`.
    struct Fake {
        name: &'static str,
        bound: bool,
        fail: bool,
        calls: Mutex<Vec<bool>>,
        texts: Mutex<Vec<String>>,
    }

    impl Fake {
        fn new(name: &'static str, bound: bool, fail: bool) -> Arc<Self> {
            Arc::new(Self {
                name,
                bound,
                fail,
                calls: Mutex::new(Vec::new()),
                texts: Mutex::new(Vec::new()),
            })
        }
        fn calls(&self) -> Vec<bool> {
            self.calls.lock().unwrap_or_else(|p| p.into_inner()).clone()
        }
    }

    #[async_trait::async_trait]
    impl UserSink for Fake {
        fn name(&self) -> &'static str {
            self.name
        }
        async fn deliver(&self, m: &Outbound, fallback: bool) -> Result<Option<String>, String> {
            self.calls
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(fallback);
            self.texts
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .push(m.text.clone());
            if self.fail {
                return Err("down".into());
            }
            match (fallback, self.bound) {
                (true, _) => Ok(Some(format!("{}: owner", self.name))),
                (false, true) => Ok(Some(format!("{}: thread", self.name))),
                (false, false) => Ok(None),
            }
        }
    }

    fn outbound() -> Outbound {
        Outbound {
            sessions: vec![SessionId::new()],
            origin: "session 0 · t".into(),
            text: "hi".into(),
            files: Vec::new(),
        }
    }

    async fn channels(sinks: &[Arc<Fake>]) -> ActorRef<UserChannels> {
        let ch = UserChannels::spawn(UserChannels::default());
        for s in sinks {
            ch.ask(RegisterUserSink(s.clone())).await.unwrap();
        }
        ch
    }

    async fn deliver(
        ch: &ActorRef<UserChannels>,
        via: Option<&str>,
    ) -> Result<Vec<String>, String> {
        match ch
            .ask(Deliver {
                m: outbound(),
                via: via.map(str::to_owned),
            })
            .await
        {
            Ok(v) => Ok(v),
            Err(SendError::HandlerError(e)) => Err(e),
            Err(e) => panic!("{e}"),
        }
    }

    #[tokio::test]
    async fn bound_pass_stops_when_any_sink_delivered() {
        let a = Fake::new("slack", true, false);
        let b = Fake::new("signal", false, false);
        let ch = channels(&[a.clone(), b.clone()]).await;
        assert_eq!(deliver(&ch, None).await.unwrap(), vec!["slack: thread"]);
        assert_eq!(a.calls(), vec![false]);
        assert_eq!(b.calls(), vec![false], "no fallback once something bound");
    }

    #[tokio::test]
    async fn fallback_fans_out_to_every_sink() {
        let a = Fake::new("slack", false, false);
        let b = Fake::new("signal", false, false);
        let ch = channels(&[a.clone(), b.clone()]).await;
        assert_eq!(
            deliver(&ch, None).await.unwrap(),
            vec!["slack: owner", "signal: owner"]
        );
        assert_eq!(a.calls(), vec![false, true]);
        assert_eq!(b.calls(), vec![false, true]);
    }

    #[tokio::test]
    async fn via_restricts_both_passes() {
        let a = Fake::new("slack", true, false);
        let b = Fake::new("signal", false, false);
        let ch = channels(&[a.clone(), b.clone()]).await;
        assert_eq!(
            deliver(&ch, Some("signal")).await.unwrap(),
            vec!["signal: owner"]
        );
        assert!(a.calls().is_empty());
        assert_eq!(b.calls(), vec![false, true]);
        assert_eq!(
            deliver(&ch, Some("pigeon")).await.unwrap_err(),
            "no user channel named pigeon"
        );
    }

    #[tokio::test]
    async fn no_sinks_and_failures() {
        let ch = channels(&[]).await;
        assert_eq!(deliver(&ch, None).await.unwrap_err(), NO_CHANNEL);

        let a = Fake::new("slack", false, true);
        let b = Fake::new("signal", false, false);
        let ch = channels(&[a.clone(), b.clone()]).await;
        assert_eq!(
            deliver(&ch, None).await.unwrap(),
            vec!["signal: owner", "slack failed: down", "slack failed: down"]
        );
        let ch = channels(std::slice::from_ref(&a)).await;
        assert_eq!(
            deliver(&ch, None).await.unwrap_err(),
            "slack failed: down; slack failed: down"
        );
    }

    #[tokio::test]
    async fn validation_refuses_bad_input() {
        let dir = std::env::temp_dir().join(format!("theuth-user-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = dir.join("shot.png");
        std::fs::write(&png, b"12").unwrap();

        let ok = check_files(std::slice::from_ref(&png), MAX_FILE).unwrap();
        assert_eq!(ok[0].name, "shot.png");
        assert_eq!(ok[0].mime, "image/png");
        assert_eq!(ok[0].bytes, 2);
        assert!(
            check_files(std::slice::from_ref(&dir), MAX_FILE)
                .unwrap_err()
                .contains("not a regular file")
        );
        assert!(
            check_files(&[dir.join("nope.txt")], MAX_FILE)
                .unwrap_err()
                .contains("not found")
        );
        assert!(
            check_files(std::slice::from_ref(&png), 1)
                .unwrap_err()
                .contains("exceeds")
        );

        let out = MessageUserTool
            .call(
                MessageUserArgs {
                    text: "  ".into(),
                    files: Vec::new(),
                    via: None,
                },
                ToolCx {
                    session: SessionId::new(),
                    call_id: "c".into(),
                    quiet: false,
                    resources: tools::SessionResources::durable_arc(&std::env::temp_dir()),
                },
            )
            .await
            .unwrap();
        assert!(out.is_error);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// 30 §9: the delivered body carries a `json-render` fence as its
    /// rendering, not its JSON.
    #[tokio::test]
    async fn message_user_text_is_flattened() {
        let a = Fake::new("slack", true, false);
        channels(std::slice::from_ref(&a))
            .await
            .register(USER_ACTOR)
            .unwrap();
        let out = MessageUserTool
            .call(
                MessageUserArgs {
                    text: "Status:\n\n```json-render\n{\"root\":\"c\",\"elements\":{\
                        \"c\":{\"type\":\"Card\",\"props\":{\"title\":\"Rollout\"},\"children\":[\"l\"]},\
                        \"l\":{\"type\":\"Checklist\",\"props\":{\"items\":[{\"text\":\"schema migration\",\"done\":true}]}}}}\n```\n"
                        .into(),
                    files: Vec::new(),
                    via: None,
                },
                ToolCx {
                    session: SessionId::new(),
                    call_id: "c".into(),
                    quiet: false,
                    resources: tools::SessionResources::durable_arc(&std::env::temp_dir()),
                },
            )
            .await
            .unwrap();
        assert!(!out.is_error, "{}", out.content);
        let texts = a.texts.lock().unwrap_or_else(|p| p.into_inner()).clone();
        assert!(texts[0].starts_with("Status:\n\n```\n╭"), "{}", texts[0]);
        assert!(texts[0].contains("[x] schema migration"), "{}", texts[0]);
        assert!(!texts[0].contains("\"type\""), "{}", texts[0]);
    }
}
