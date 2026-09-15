//! `message_user` outbound (design doc 24 §6): the channel as an
//! [`agent::UserSink`]. Text goes through the 09 §8 plain-text chunker, then
//! one attachment message per file with its file name.

use agent::{Outbound, UserSink};

use crate::channel::Handle;
use crate::text;

#[async_trait::async_trait]
impl UserSink for Handle {
    /// `"signal"`: the `via` value and the word in the tool result (24 §3).
    fn name(&self) -> &'static str {
        "signal"
    }

    /// Bound pass: the first session of `m.sessions` with a thread
    /// (`threads.recipient_for`, 09 §6). Fallback: `signal.owner`, text
    /// prefixed with `m.origin` on its own line (24 §2, §6).
    async fn deliver(&self, m: &Outbound, fallback: bool) -> Result<Option<String>, String> {
        let chan = &self.chan;
        let (to, text) = if fallback {
            if chan.owner.is_empty() {
                return Err("signal: no owner configured".into());
            }
            if !chan.transport.is_registered().await.unwrap_or(false) {
                return Err("signal: not linked".into());
            }
            let to = chan.lock().threads.recipient(&chan.owner);
            (to, format!("{}\n{}", m.origin, m.text))
        } else {
            let to = {
                let s = chan.lock();
                m.sessions
                    .iter()
                    .find_map(|id| s.threads.recipient_for(*id))
            };
            let Some(to) = to else {
                return Ok(None);
            };
            (to, m.text.clone())
        };
        let plain = text::markdown_to_plain(&text);
        if !plain.is_empty() && chan.send_chunks(&to, &plain).await.is_none() {
            return Err("signal: send failed".into());
        }
        for f in &m.files {
            let bytes = tokio::fs::read(&f.path)
                .await
                .map_err(|e| format!("cannot read {}: {e}", f.path.display()))?;
            if !chan
                .send_attachment(&to, &bytes, &f.mime, Some(&f.name))
                .await
            {
                return Err(format!("signal: attachment {} failed", f.name));
            }
        }
        Ok(Some(format!("signal: {}", to.number)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent::OutboundFile;
    use runtime::{Runtime, SessionId};

    use super::*;
    use crate::SignalConfig;
    use crate::channel;
    use crate::mock::{MockTransport, SentItem};
    use crate::transport::Recipient;

    const OWNER: &str = "+15550001111";
    const FRIEND: &str = "+15550002222";

    fn fixture() -> (tempfile::TempDir, MockTransport, Handle) {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let transport = MockTransport::new();
        let cfg = SignalConfig {
            owner: OWNER.into(),
            ..SignalConfig::default()
        };
        let handle = channel::unstarted(Arc::new(transport.clone()), cfg, rt);
        (dir, transport, handle)
    }

    fn outbound(sessions: Vec<SessionId>, text: &str, files: Vec<OutboundFile>) -> Outbound {
        Outbound {
            sessions,
            origin: "session deadbeef · untitled".into(),
            text: text.into(),
            files,
        }
    }

    #[tokio::test]
    async fn bound_session_gets_text_then_one_attachment_per_file() {
        let (dir, transport, handle) = fixture();
        let (root, child) = (SessionId::new(), SessionId::new());
        handle
            .chan
            .lock()
            .threads
            .bind(&Recipient::new(FRIEND), root);
        let path = dir.path().join("report.pdf");
        std::fs::write(&path, b"%PDF").unwrap();
        let m = outbound(
            vec![child, root],
            "**done**",
            vec![OutboundFile {
                path,
                name: "report.pdf".into(),
                mime: "application/pdf".into(),
                bytes: 4,
            }],
        );
        let r = handle.deliver(&m, false).await;
        assert_eq!(r, Ok(Some(format!("signal: {FRIEND}"))));
        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        assert!(
            matches!(&sent[0], SentItem::Text { to, text, .. } if to == FRIEND && text == "done")
        );
        assert_eq!(
            sent[1],
            SentItem::Attachment {
                to: FRIEND.into(),
                mime: "application/pdf".into(),
                bytes: b"%PDF".to_vec(),
                file_name: Some("report.pdf".into()),
                caption: None,
            }
        );
    }

    #[tokio::test]
    async fn fallback_goes_to_owner_with_origin_line() {
        let (_dir, transport, handle) = fixture();
        let m = outbound(vec![SessionId::new()], "hi", Vec::new());
        assert_eq!(handle.deliver(&m, false).await, Ok(None));
        assert!(transport.sent().is_empty());
        let r = handle.deliver(&m, true).await;
        assert_eq!(r, Ok(Some(format!("signal: {OWNER}"))));
        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert!(matches!(&sent[0], SentItem::Text { to, text, .. }
            if to == OWNER && text == "session deadbeef · untitled\nhi"));
    }

    #[tokio::test]
    async fn missing_file_is_an_error_after_text() {
        let (dir, transport, handle) = fixture();
        let m = outbound(
            Vec::new(),
            "see file",
            vec![OutboundFile {
                path: dir.path().join("nope.bin"),
                name: "nope.bin".into(),
                mime: "application/octet-stream".into(),
                bytes: 0,
            }],
        );
        let r = handle.deliver(&m, true).await;
        assert!(
            r.as_ref().is_err_and(|e| e.starts_with("cannot read ")),
            "{r:?}"
        );
        assert_eq!(transport.sent().len(), 1);
    }
}
