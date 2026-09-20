//! `message_user` on Slack (design doc 24 §5): the [`UserSink`] the plugin
//! registers with the agent crate's `"user"` actor. Bound sessions get a
//! post in their thread; the owner DM is the fallback.

use agent::{Outbound, UserSink};

use crate::api::{MARKDOWN_TEXT_MAX, UploadFile};
use crate::channel::{Handle, clip, split_key};

/// Where one delivery goes: a channel and, in a thread, its root `ts`.
struct Target {
    channel: String,
    thread_ts: Option<String>,
    /// The tool result's `where` (24 §5): `#<channel> thread` / `owner DM`.
    label: String,
}

#[async_trait::async_trait]
impl UserSink for Handle {
    /// `"slack"` (24 §3).
    fn name(&self) -> &'static str {
        "slack"
    }

    /// Bound pass: the first session of `m.sessions` with a thread (24 §2
    /// step 1); `Ok(None)` when none has one. Fallback: the owner DM with
    /// `_<origin>_` as the first line (24 §2 step 2). Files go up in one
    /// `upload_file` with the text as `initial_comment`; without files the
    /// text is one `post_markdown` (24 §5).
    async fn deliver(&self, m: &Outbound, fallback: bool) -> Result<Option<String>, String> {
        let chan = &self.chan;
        let (target, text) = if fallback {
            let dm = chan
                .identity
                .get()
                .and_then(|i| i.owner_dm.clone())
                .ok_or_else(|| "slack: owner DM not opened yet".to_owned())?;
            let target = Target {
                channel: dm,
                thread_ts: None,
                label: "owner DM".to_owned(),
            };
            (target, format!("_{}_\n{}", m.origin, m.text))
        } else {
            let Some((key, _)) = m.sessions.iter().find_map(|s| chan.bound(*s)) else {
                return Ok(None);
            };
            let (channel, ts) = split_key(&key);
            let target = Target {
                channel: channel.to_owned(),
                thread_ts: Some(ts.to_owned()),
                label: format!("#{channel} thread"),
            };
            (target, m.text.clone())
        };
        let text = clip(&text, MARKDOWN_TEXT_MAX);
        let thread_ts = target.thread_ts.as_deref();
        let sent = if m.files.is_empty() {
            chan.api
                .post_markdown(&target.channel, thread_ts, &text)
                .await
                .map(|_| ())
        } else {
            let mut files = Vec::with_capacity(m.files.len());
            for f in &m.files {
                let bytes = tokio::fs::read(&f.path)
                    .await
                    .map_err(|e| format!("cannot read {}: {e}", f.path.display()))?;
                files.push(UploadFile {
                    name: f.name.clone(),
                    mime: f.mime.clone(),
                    bytes,
                });
            }
            // ponytail: `initial_comment` is mrkdwn, not `markdown_text`; the crate
            // has no md->mrkdwn renderer (21 §1 dropped it), so the text goes as is.
            let comment = (!text.trim().is_empty()).then_some(text.as_str());
            chan.api
                .upload_file(&target.channel, thread_ts, &files, comment)
                .await
                .map(|_| ())
        };
        sent.map_err(|e| format!("slack: {e}"))?;
        Ok(Some(format!("slack: {}", target.label)))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use agent::OutboundFile;
    use runtime::SessionId;

    use super::*;
    use crate::channel::tests::{DM, OWNER, OWNER_DM, fixture};

    fn outbound(sessions: Vec<SessionId>, files: Vec<OutboundFile>) -> Outbound {
        Outbound {
            sessions,
            origin: "session deadbeef · untitled".into(),
            text: "hello *there*".into(),
            files,
        }
    }

    fn handle(f: &crate::channel::tests::Fixture) -> Handle {
        Handle {
            chan: f.chan.clone(),
        }
    }

    #[tokio::test]
    async fn bound_session_posts_in_its_thread() {
        let f = fixture();
        f.bind(OWNER);
        let got = handle(&f)
            .deliver(&outbound(vec![f.stub.session], vec![]), false)
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("slack: #D0001 thread"));
        assert_eq!(
            f.api.calls(),
            vec![format!("post_markdown {DM} 100.1 hello *there*")]
        );
    }

    #[tokio::test]
    async fn child_lands_in_the_bound_parents_thread() {
        let f = fixture();
        f.bind(OWNER);
        let child = SessionId::new();
        let got = handle(&f)
            .deliver(&outbound(vec![child, f.stub.session], vec![]), false)
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("slack: #D0001 thread"));
        assert_eq!(
            f.api.calls()[0],
            format!("post_markdown {DM} 100.1 hello *there*")
        );
    }

    #[tokio::test]
    async fn unbound_chain_is_not_ours_on_the_bound_pass() {
        let f = fixture();
        let got = handle(&f)
            .deliver(&outbound(vec![SessionId::new()], vec![]), false)
            .await
            .unwrap();
        assert_eq!(got, None);
        assert!(f.api.calls().is_empty());
    }

    #[tokio::test]
    async fn fallback_goes_to_the_owner_dm_with_the_origin_line() {
        let f = fixture();
        let got = handle(&f)
            .deliver(&outbound(vec![SessionId::new()], vec![]), true)
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("slack: owner DM"));
        assert_eq!(
            f.api.calls(),
            vec![format!(
                "post_markdown {OWNER_DM} - _session deadbeef · untitled_\nhello *there*"
            )]
        );
    }

    #[tokio::test]
    async fn files_go_up_in_one_upload_with_the_text_as_comment() {
        let f = fixture();
        f.bind(OWNER);
        let dir = tempfile::tempdir().unwrap();
        let mut files = Vec::new();
        for (name, mime) in [("a.png", "image/png"), ("b.pdf", "application/pdf")] {
            let path = dir.path().join(name);
            std::fs::File::create(&path)
                .unwrap()
                .write_all(b"x")
                .unwrap();
            files.push(OutboundFile {
                path,
                name: name.into(),
                mime: mime.into(),
                bytes: 1,
            });
        }
        let got = handle(&f)
            .deliver(&outbound(vec![f.stub.session], files), false)
            .await
            .unwrap();
        assert_eq!(got.as_deref(), Some("slack: #D0001 thread"));
        assert_eq!(
            f.api.calls(),
            vec![format!(
                "upload_file {DM} 100.1 [a.png,b.pdf] hello *there*"
            )]
        );
    }

    #[tokio::test]
    async fn unreadable_file_is_an_error_before_any_call() {
        let f = fixture();
        f.bind(OWNER);
        let missing = OutboundFile {
            path: "/nonexistent/theuth/file.bin".into(),
            name: "file.bin".into(),
            mime: "application/octet-stream".into(),
            bytes: 1,
        };
        let err = handle(&f)
            .deliver(&outbound(vec![f.stub.session], vec![missing]), false)
            .await
            .unwrap_err();
        assert!(
            err.starts_with("cannot read /nonexistent/theuth/file.bin"),
            "{err}"
        );
        assert!(f.api.calls().is_empty());
    }
}
